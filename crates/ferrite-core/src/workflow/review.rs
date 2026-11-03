//! Reviews (design 15 §6.2): submit, decide, assign, withdraw; staleness on change.

use std::sync::Arc;

use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, Decision, Page, ReleaseId, Review, ReviewDecision,
    ReviewId, ReviewQuery, ReviewStatus, ReviewTarget, Timestamp, UserId, UserStatus, WorkflowStep,
};
use ferrite_store::{Repos, Store, StoreError};
use serde_json::json;
use uuid::Uuid;

use super::gate::workflow_of;
use crate::{
    Actor, CoreError,
    actor::{Area, Capability, can, require, require_read},
    events,
    jobs::editorial_notify_job,
    tx::finish,
    webhooks::{self, NewWebhookEvent},
};

/// Longest note or decision comment.
pub const MAX_REVIEW_TEXT: usize = 2000;
/// Most assignees per step.
pub const MAX_ASSIGNEES: usize = 10;

/// What a submit for review carries (design 15 §6.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmitInput {
    /// Assignees of the first step (eligible approvers, at most [`MAX_ASSIGNEES`]).
    pub reviewers: Vec<UserId>,
    pub note: Option<String>,
}

/// A review with its decisions, assignees and item.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewDetail {
    pub review: Review,
    pub decisions: Vec<ReviewDecision>,
    pub assignees: Vec<(u32, UserId)>,
    pub item: ContentItem,
}

fn check_text(field: &'static str, text: Option<String>) -> Result<Option<String>, CoreError> {
    let Some(t) = text.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if t.chars().count() > MAX_REVIEW_TEXT {
        return Err(CoreError::invalid_field(
            field,
            format!("at most {MAX_REVIEW_TEXT} characters"),
        ));
    }
    if t.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(CoreError::invalid_field(field, "no control characters"));
    }
    Ok(Some(t))
}

/// Whether `user` (role `role`) may approve `step` (D17: in its roles or users, with
/// `edit_others`).
fn eligible(step: &WorkflowStep, user: UserId, role: ferrite_domain::Role) -> bool {
    can(role, Capability::EditOthers)
        && (step.approvers.roles.contains(&role) || step.approvers.users.contains(&user))
}

/// Checks `users` as assignees of `step`: at most [`MAX_ASSIGNEES`], each active and
/// eligible. Returns them without duplicates.
async fn check_assignees(
    repos: &dyn Repos,
    step: &WorkflowStep,
    users: &[UserId],
) -> Result<Vec<UserId>, CoreError> {
    let mut out: Vec<UserId> = Vec::new();
    for u in users {
        if !out.contains(u) {
            out.push(*u);
        }
    }
    if out.len() > MAX_ASSIGNEES {
        return Err(CoreError::invalid_field(
            "reviewers",
            format!("at most {MAX_ASSIGNEES} reviewers"),
        ));
    }
    let found = repos.users().get_many(&out).await?;
    for id in &out {
        let ok = found
            .iter()
            .any(|u| u.id == *id && u.status == UserStatus::Active && eligible(step, u.id, u.role));
        if !ok {
            return Err(CoreError::invalid_field(
                "reviewers",
                "reviewers must be able to approve this step",
            ));
        }
    }
    Ok(out)
}

/// Closes the open item review of `item` as stale (a save changed what it approved).
pub(crate) async fn mark_stale_item(repos: &dyn Repos, item: ContentId) -> Result<(), CoreError> {
    repos
        .reviews()
        .mark_stale_item(item, Timestamp::now())
        .await?;
    Ok(())
}

fn review_event(
    kind: &'static str,
    review: &Review,
    item: &ContentItem,
    by: UserId,
) -> NewWebhookEvent {
    NewWebhookEvent {
        kind,
        content_type: None,
        data: json!({
            "review": {"id": review.id.to_string(), "target": review.target.key(), "step": review.step},
            "item": {"id": item.id.to_string(), "type": item.kind, "locale": item.locale},
            "by": by.to_string(),
        }),
    }
}

/// Starts a review of `target` (item `item`) at step 0 when its type has a workflow,
/// inside the caller's `repos`: assignees, note, the `editorial.notify` job and the
/// `review.submitted` event. `Ok(None)` without a workflow (D25). A second open review
/// is `Invalid("already in review")`.
pub(crate) async fn start_in(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
    target: ReviewTarget,
    entry_version: Option<i64>,
    input: SubmitInput,
) -> Result<Option<Review>, CoreError> {
    let Some(wf) = workflow_of(repos, &item.kind).await? else {
        return Ok(None);
    };
    let note = check_text("note", input.note)?;
    let first = wf
        .steps
        .first()
        .ok_or(CoreError::Internal("workflow without steps"))?;
    let reviewers = check_assignees(repos, first, &input.reviewers).await?;
    let revision_id = match target {
        ReviewTarget::Item(id) => repos.revisions().list(id, 1).await?.first().map(|r| r.id),
        ReviewTarget::Entry(..) => None,
    };
    let review = Review {
        id: ReviewId::new(),
        target,
        revision_id,
        entry_version: match target {
            ReviewTarget::Item(_) => None,
            ReviewTarget::Entry(..) => entry_version,
        },
        status: ReviewStatus::InReview,
        step: 0,
        steps: wf.steps.clone(),
        note,
        submitted_by: Some(actor.user),
        submitted_at: Timestamp::now(),
        decided_at: None,
        version: 1,
    };
    match repos.reviews().insert(&review).await {
        Err(StoreError::UniqueViolation { .. }) => {
            return Err(CoreError::Invalid("already in review".into()));
        }
        other => other?,
    }
    if !reviewers.is_empty() {
        repos
            .reviews()
            .set_assignees(review.id, 0, &reviewers)
            .await?;
    }
    repos
        .jobs()
        .enqueue(&editorial_notify_job(
            "review.submitted",
            Some(review.id),
            target.release(),
        ))
        .await?;
    webhooks::emit(
        repos,
        review_event("review.submitted", &review, item, actor.user),
    )
    .await?;
    Ok(Some(review))
}

/// A pending item goes back to draft (request changes, withdraw: D22).
async fn back_to_draft(repos: &dyn Repos, item: &ContentItem) -> Result<(), CoreError> {
    if item.status != ContentStatus::Pending {
        return Ok(());
    }
    let mut next = item.clone();
    next.status = ContentStatus::Draft;
    repos.content().update(&next).await?;
    repos
        .events()
        .append(&events::invalidate([
            events::post_key(item.id),
            events::archive_key(&item.kind),
        ]))
        .await?;
    Ok(())
}

/// Reviews: decisions, assignees, withdrawal, the queue.
pub struct ReviewService {
    store: Arc<dyn Store>,
}

impl ReviewService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Whether `actor` may see `review` of `item`: editors and admins, the submitter,
    /// the item's author.
    fn may_see(actor: &Actor, review: &Review, item: &ContentItem) -> bool {
        can(actor.role, Capability::EditOthers)
            || review.submitted_by == Some(actor.user)
            || item.author_id == actor.user
    }

    async fn load(
        repos: &dyn Repos,
        actor: &Actor,
        id: ReviewId,
    ) -> Result<(Review, ContentItem), CoreError> {
        let review = repos.reviews().get(id).await?.ok_or(CoreError::NotFound)?;
        let item = repos
            .content()
            .get(review.target.item())
            .await?
            .ok_or(CoreError::NotFound)?;
        if !Self::may_see(actor, &review, &item) {
            return Err(CoreError::NotFound);
        }
        Ok((review, item))
    }

    /// A review with its decisions and assignees.
    pub async fn get_for(&self, actor: &Actor, id: ReviewId) -> Result<ReviewDetail, CoreError> {
        require_read(actor, Area::Content)?;
        let repos = self.store.read();
        let (review, item) = Self::load(repos, actor, id).await?;
        Ok(ReviewDetail {
            decisions: repos.reviews().decisions(id).await?,
            assignees: repos.reviews().assignees(id).await?,
            review,
            item,
        })
    }

    /// The review queue. Without `edit_others` only the actor's own submissions.
    pub async fn list(
        &self,
        actor: &Actor,
        mut query: ReviewQuery,
    ) -> Result<Page<Review>, CoreError> {
        require_read(actor, Area::Content)?;
        if !can(actor.role, Capability::EditOthers) {
            query.submitted_by = Some(actor.user);
        }
        query.limit = query.limit.clamp(1, 100);
        Ok(self.store.read().reviews().list(&query).await?)
    }

    /// The number of in-review reviews assigned to `actor` at their current step.
    pub async fn assigned_count(&self, actor: &Actor) -> Result<u64, CoreError> {
        require_read(actor, Area::Content)?;
        Ok(self.store.read().reviews().counts_for(actor.user).await?)
    }

    /// Records `decision` of `actor` on the review's current step (D17: an eligible
    /// approver, not the submitter, not the item's author; once per step). Enough
    /// approvals move it to the next step or to `approved`; `request_changes` closes it
    /// and sends a pending item back to draft (D22).
    pub async fn decide(
        &self,
        actor: &Actor,
        id: ReviewId,
        version: i64,
        decision: Decision,
        comment: Option<String>,
    ) -> Result<Review, CoreError> {
        require(actor, Capability::EditOthers)?;
        let comment = check_text("comment", comment)?;
        let tx = self.store.begin().await?;
        let done = async {
            let (mut review, item) = Self::load(tx.as_ref(), actor, id).await?;
            if review.version != version {
                return Err(CoreError::Conflict {
                    current_version: review.version,
                });
            }
            if review.status != ReviewStatus::InReview {
                return Err(CoreError::Invalid(
                    "the review is not open for decisions".into(),
                ));
            }
            let step = review
                .steps
                .get(review.step as usize)
                .cloned()
                .ok_or(CoreError::Internal("review step out of range"))?;
            if !eligible(&step, actor.user, actor.role) {
                return Err(CoreError::Forbidden("approver of this step"));
            }
            if review.submitted_by == Some(actor.user) || item.author_id == actor.user {
                return Err(CoreError::Forbidden("another approver"));
            }
            // Nobody approves a change they staged themselves (review finding 8).
            if let ReviewTarget::Entry(release, it) = review.target
                && tx
                    .releases()
                    .entry(release, it)
                    .await?
                    .is_some_and(|e| e.added_by == Some(actor.user))
            {
                return Err(CoreError::Forbidden("another approver"));
            }
            let now = Timestamp::now();
            match tx
                .reviews()
                .add_decision(&ReviewDecision {
                    id: Uuid::now_v7(),
                    review_id: id,
                    step: review.step,
                    user_id: Some(actor.user),
                    decision,
                    comment,
                    at: now,
                })
                .await
            {
                Err(StoreError::UniqueViolation { .. }) => {
                    return Err(CoreError::Invalid("you already decided this step".into()));
                }
                other => other?,
            }
            let (event, webhook) = match decision {
                Decision::RequestChanges => {
                    review.status = ReviewStatus::ChangesRequested;
                    review.decided_at = Some(now);
                    if matches!(review.target, ReviewTarget::Item(_)) {
                        back_to_draft(tx.as_ref(), &item).await?;
                    }
                    ("review.changes_requested", Some("review.changes_requested"))
                }
                Decision::Approve => {
                    let approvals = tx
                        .reviews()
                        .decisions(id)
                        .await?
                        .into_iter()
                        .filter(|d| d.step == review.step && d.decision == Decision::Approve)
                        .count();
                    if (approvals as u64) < u64::from(step.required) {
                        ("", None)
                    } else if (review.step as usize) + 1 < review.steps.len() {
                        review.step += 1;
                        ("review.step", None)
                    } else {
                        review.status = ReviewStatus::Approved;
                        review.decided_at = Some(now);
                        ("review.approved", Some("review.approved"))
                    }
                }
            };
            let saved = tx.reviews().update(&review).await?;
            if !event.is_empty() {
                tx.jobs()
                    .enqueue(&editorial_notify_job(
                        event,
                        Some(id),
                        review.target.release(),
                    ))
                    .await?;
            }
            if let Some(kind) = webhook {
                webhooks::emit(tx.as_ref(), review_event(kind, &saved, &item, actor.user)).await?;
            }
            Ok(saved)
        }
        .await;
        finish(tx, done).await
    }

    /// Replaces the assignees of `step` (the submitter or `edit_others`).
    pub async fn assign(
        &self,
        actor: &Actor,
        id: ReviewId,
        step: u32,
        users: Vec<UserId>,
    ) -> Result<Vec<(u32, UserId)>, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let (review, _) = Self::load(tx.as_ref(), actor, id).await?;
            if review.submitted_by != Some(actor.user) && !can(actor.role, Capability::EditOthers) {
                return Err(CoreError::Forbidden(Capability::EditOthers.as_str()));
            }
            if review.status != ReviewStatus::InReview {
                return Err(CoreError::Invalid("the review is closed".into()));
            }
            let s = review
                .steps
                .get(step as usize)
                .ok_or_else(|| CoreError::invalid_field("step", "no such step"))?;
            let users = check_assignees(tx.as_ref(), s, &users).await?;
            tx.reviews().set_assignees(id, step, &users).await?;
            Ok(tx.reviews().assignees(id).await?)
        }
        .await;
        finish(tx, done).await
    }

    /// Withdraws an open review (the submitter or `edit_others`); a pending item goes
    /// back to draft.
    pub async fn withdraw(
        &self,
        actor: &Actor,
        id: ReviewId,
        version: i64,
    ) -> Result<Review, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let (mut review, item) = Self::load(tx.as_ref(), actor, id).await?;
            if review.submitted_by != Some(actor.user) && !can(actor.role, Capability::EditOthers) {
                return Err(CoreError::Forbidden(Capability::EditOthers.as_str()));
            }
            if review.version != version {
                return Err(CoreError::Conflict {
                    current_version: review.version,
                });
            }
            if !review.status.is_open() {
                return Err(CoreError::Invalid("the review is closed".into()));
            }
            review.status = ReviewStatus::Withdrawn;
            review.decided_at = Some(Timestamp::now());
            if matches!(review.target, ReviewTarget::Item(_)) {
                back_to_draft(tx.as_ref(), &item).await?;
            }
            Ok(tx.reviews().update(&review).await?)
        }
        .await;
        finish(tx, done).await
    }

    /// Submits the staged change of `item` in `release` for review (an entry review);
    /// the actor must see the release and edit the item.
    pub async fn submit_entry(
        &self,
        actor: &Actor,
        release: ReleaseId,
        item: ContentId,
        input: SubmitInput,
    ) -> Result<Review, CoreError> {
        require(actor, Capability::EditOwn)?;
        let tx = self.store.begin().await?;
        let done = async {
            let r = tx
                .releases()
                .get(release)
                .await?
                .ok_or(CoreError::NotFound)?;
            if !crate::release::may_see(actor, &r) {
                return Err(CoreError::NotFound);
            }
            let entry = tx
                .releases()
                .entry(release, item)
                .await?
                .ok_or(CoreError::NotFound)?;
            let it = tx.content().get(item).await?.ok_or(CoreError::NotFound)?;
            crate::actor::can_edit(actor, &it)?;
            start_in(
                tx.as_ref(),
                actor,
                &it,
                ReviewTarget::Entry(release, item),
                Some(entry.entry_version),
                input,
            )
            .await?
            .ok_or_else(|| CoreError::Invalid("the type has no workflow".into()))
        }
        .await;
        finish(tx, done).await
    }
}
