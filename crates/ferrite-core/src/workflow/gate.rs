//! The publish gate of workflow types (design 15 §6.3–§6.5, ruling P7).

use ferrite_domain::{
    ContentItem, ContentStatus, NewAudit, Review, ReviewStatus, ReviewTarget, Role, Workflow,
};
use ferrite_store::Repos;
use serde_json::json;

use crate::{
    Actor, CoreError,
    actor::{Capability, can},
    webhooks::{self, NewWebhookEvent},
};

/// Longest override reason (D21).
pub const MAX_OVERRIDE_REASON: usize = 500;

/// The review state of an item as the admin shows it (`allowed_actions`, the DTO).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSummary {
    pub id: ferrite_domain::ReviewId,
    pub status: ReviewStatus,
    /// The current step's index and label.
    pub step: u32,
    pub step_label: Option<String>,
    pub steps: u32,
    /// The review approves what is stored now (its fingerprint is current).
    pub current: bool,
    pub version: i64,
}

/// Whether the item's type has a workflow, and its open review.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowState {
    pub workflow: bool,
    pub review: Option<ReviewSummary>,
    /// The actor may approve in this workflow (editors and admins named by a step).
    pub approver: bool,
}

impl WorkflowState {
    /// Publishing passes the gate: no workflow, or an approved, current review.
    pub fn approved(&self) -> bool {
        !self.workflow
            || self
                .review
                .as_ref()
                .is_some_and(|r| r.status == ReviewStatus::Approved && r.current)
    }
}

/// The workflow of content type `kind`, if any.
pub(crate) async fn workflow_of(
    repos: &dyn Repos,
    kind: &str,
) -> Result<Option<Workflow>, CoreError> {
    Ok(repos.workflows().get(kind).await?)
}

/// Whether `actor` may approve some step of `wf` (P7: editors or admins named by a step,
/// by role or by id).
pub(crate) fn may_approve(actor: &Actor, wf: &Workflow) -> bool {
    can(actor.role, Capability::EditOthers)
        && wf.steps.iter().any(|s| {
            s.approvers.roles.contains(&actor.role) || s.approvers.users.contains(&actor.user)
        })
}

/// Whether `review` approves what `item` stores now: its revision fingerprint is the
/// newest revision (item reviews) or its entry version the entry's (entry reviews).
pub(crate) async fn is_current(
    repos: &dyn Repos,
    review: &Review,
    entry_version: Option<i64>,
) -> Result<bool, CoreError> {
    Ok(match review.target {
        ReviewTarget::Item(id) => {
            let newest = repos.revisions().list(id, 1).await?.first().map(|r| r.id);
            newest.is_some() && review.revision_id == newest
        }
        ReviewTarget::Entry(..) => entry_version.is_some() && review.entry_version == entry_version,
    })
}

/// The review summary of `target` (its open review), current against `entry_version`.
pub(crate) async fn summary(
    repos: &dyn Repos,
    target: &ReviewTarget,
    entry_version: Option<i64>,
) -> Result<Option<ReviewSummary>, CoreError> {
    let Some(r) = repos.reviews().open_for(target).await? else {
        return Ok(None);
    };
    let current = is_current(repos, &r, entry_version).await?;
    Ok(Some(ReviewSummary {
        id: r.id,
        status: r.status,
        step: r.step,
        step_label: r.steps.get(r.step as usize).map(|s| s.label.clone()),
        steps: u32::try_from(r.steps.len()).unwrap_or(u32::MAX),
        current,
        version: r.version,
    }))
}

/// The workflow state of `item` for `actor` (item reviews).
pub async fn state_of(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
) -> Result<WorkflowState, CoreError> {
    let Some(wf) = workflow_of(repos, &item.kind).await? else {
        return Ok(WorkflowState::default());
    };
    Ok(WorkflowState {
        workflow: true,
        review: summary(repos, &ReviewTarget::Item(item.id), None).await?,
        approver: may_approve(actor, &wf),
    })
}

/// Checks an override reason: 1–[`MAX_OVERRIDE_REASON`] characters, admins only.
pub(crate) fn check_override(actor: &Actor, reason: &str) -> Result<String, CoreError> {
    if actor.role != Role::Admin {
        return Err(CoreError::Forbidden("admin"));
    }
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_OVERRIDE_REASON {
        return Err(CoreError::invalid_field(
            "override_reason",
            format!("1 to {MAX_OVERRIDE_REASON} characters"),
        ));
    }
    if reason.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "override_reason",
            "no control characters",
        ));
    }
    Ok(reason.to_owned())
}

/// The gate for publishing (or scheduling) `item` of a workflow type (§6.3): `target`'s
/// open review is approved and current (`entry_version` for an entry review). Without
/// one, an admin's `override_reason` lets it through with an audit row
/// `workflow.override` and the event `review.overridden`; otherwise
/// `ApprovalRequired`, naming the step the review waits for. Returns whether an
/// override was used. No workflow: always passes.
pub(crate) async fn require_approval(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
    target: ReviewTarget,
    entry_version: Option<i64>,
    override_reason: Option<&str>,
) -> Result<bool, CoreError> {
    if workflow_of(repos, &item.kind).await?.is_none() {
        return Ok(false);
    }
    let open = repos.reviews().open_for(&target).await?;
    if let Some(r) = &open
        && r.status == ReviewStatus::Approved
        && is_current(repos, r, entry_version).await?
    {
        return Ok(false);
    }
    let Some(reason) = override_reason else {
        let step = open
            .as_ref()
            .filter(|r| r.status == ReviewStatus::InReview)
            .and_then(|r| r.steps.get(r.step as usize))
            .map(|s| s.label.clone());
        return Err(CoreError::ApprovalRequired { step });
    };
    let reason = check_override(actor, reason)?;
    let step = open
        .as_ref()
        .and_then(|r| r.steps.get(r.step as usize))
        .map(|s| s.key.clone());
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor.user),
            action: "workflow.override".into(),
            target_kind: "content".into(),
            target_id: Some(item.id.to_string()),
            details: json!({
                "target": target.key(),
                "review": open.as_ref().map(|r| r.id.to_string()),
                "missing_step": step,
                "reason": reason,
            }),
        })
        .await?;
    webhooks::emit(
        repos,
        NewWebhookEvent {
            kind: "review.overridden",
            content_type: None,
            data: json!({
                "review": open.as_ref().map(|r| json!({
                    "id": r.id.to_string(), "target": target.key(), "step": r.step,
                })),
                "item": {"id": item.id.to_string(), "type": item.kind, "locale": item.locale},
                "by": actor.user.to_string(),
            }),
        },
    )
    .await?;
    Ok(true)
}

/// The rule for changing a public item of a workflow type in place (§6.4, P7 changing
/// D20): approvers (editors or admins named by a step) may, with an audit row
/// `review.bypassed`; anyone else gets `StagingRequired`. No workflow, or an item that
/// is not public: passes.
pub(crate) async fn require_live_edit(
    repos: &dyn Repos,
    actor: &Actor,
    item: &ContentItem,
) -> Result<(), CoreError> {
    if !matches!(
        item.status,
        ContentStatus::Published | ContentStatus::Scheduled | ContentStatus::Private
    ) {
        return Ok(());
    }
    let Some(wf) = workflow_of(repos, &item.kind).await? else {
        return Ok(());
    };
    if !may_approve(actor, &wf) {
        return Err(CoreError::StagingRequired);
    }
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor.user),
            action: "review.bypassed".into(),
            target_kind: "content".into(),
            target_id: Some(item.id.to_string()),
            details: json!({"type": item.kind, "status": item.status.as_str()}),
        })
        .await?;
    Ok(())
}
