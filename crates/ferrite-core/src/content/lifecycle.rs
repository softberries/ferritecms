//! Content status transitions.
//!
//! Allowed: draft/pending/scheduled -> published (publish); draft/pending -> scheduled
//! (schedule); draft -> pending (submit); published/scheduled/private -> draft
//! (unpublish); any non-trash status -> trash; trash -> draft (restore); trash ->
//! deleted (delete). Anything else is `Invalid("illegal transition ...")`.
//!
//! Every transition runs in one transaction, checks permission before writing, checks
//! the expected version (`Conflict`) and emits one `invalidate` event for `post:<id>`,
//! `archive:<kind>`, the item's `term:<id>` keys and `post:<parent>` (see
//! [`events`](crate::events)). Transitions only
//! change status and publish times, so they write **no revision**.
//!
//! `schedule` also enqueues a [`content.publish_scheduled`](crate::jobs) job in the same
//! transaction (ruling P12); its handler calls
//! [`publish_scheduled`](ContentService::publish_scheduled). Every transition to
//! `published` (publish, and the scheduler's publish) enqueues the `post.published`
//! action jobs in its transaction ([`actions`](crate::actions), M2a-2 P20).

use ferrite_domain::{ContentId, ContentItem, ContentStatus, Role, Term, Timestamp};
use ferrite_store::{Repos, StoreError};

use super::ContentService;
use crate::{
    Actor, CoreError, actions,
    actor::{Capability, can_edit, require},
    blocks, events, jobs,
    permalink::{Permalinks, load_input},
    redirects::clear_redirects_from,
    tx::finish,
};

/// Items fetched per round by [`ContentService::publish_due`].
const DUE_BATCH: u32 = 100;

#[derive(Debug, Clone, Copy)]
pub(super) enum Action {
    Publish,
    /// Publish by the scheduler: the item must be scheduled.
    PublishDue,
    Unpublish,
    Schedule(Timestamp),
    Submit,
    Trash,
    Restore,
}

impl Action {
    fn target(self) -> ContentStatus {
        match self {
            Action::Publish | Action::PublishDue => ContentStatus::Published,
            Action::Unpublish | Action::Restore => ContentStatus::Draft,
            Action::Schedule(_) => ContentStatus::Scheduled,
            Action::Submit => ContentStatus::Pending,
            Action::Trash => ContentStatus::Trash,
        }
    }
}

impl ContentService {
    /// Publishes a draft, pending or scheduled item (needs `publish_own`, or
    /// `publish_others` for someone else's item). `published_at` is set to now only if it
    /// was never set, so it survives unpublish and republish. A scheduled item is
    /// published now ("publish now"): `scheduled_at` is cleared, and the queued
    /// `content.publish_scheduled` job finds it no longer scheduled and does nothing.
    /// Redirects that start at the item's permalink (either slash variant) are deleted,
    /// so the item is reachable at its URL. Writes no revision.
    pub async fn publish(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        self.transition(actor, id, expected_version, Action::Publish)
            .await
    }

    /// Moves a published, scheduled or private item back to draft (same permissions as
    /// publish). `published_at` is kept and no redirect is created: the URL becomes 404
    /// once caches are invalidated. Writes no revision.
    pub async fn unpublish(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        self.transition(actor, id, expected_version, Action::Unpublish)
            .await
    }

    /// Schedules a draft or pending item for `at`, which must lie in the future
    /// (`Invalid` otherwise); same permissions as publish. Sets `scheduled_at`;
    /// `published_at` stays as it is. Writes no revision.
    ///
    /// In the same transaction it enqueues [`jobs::publish_scheduled_job`]`(id, at)`,
    /// whose handler publishes the item at `at` through
    /// [`publish_scheduled`](Self::publish_scheduled); [`publish_due`](Self::publish_due)
    /// is the periodic safety net. A job of an earlier schedule stays queued and does
    /// nothing when it runs. If the key is already taken (the same item scheduled for
    /// the same instant before), no job is added: a queued or running holder publishes;
    /// a `done` or `dead` one does not, and `publish_due` (every 30 s) publishes the
    /// item instead.
    pub async fn schedule(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        at: Timestamp,
    ) -> Result<ContentItem, CoreError> {
        self.transition(actor, id, expected_version, Action::Schedule(at))
            .await
    }

    /// Submits a draft for review (draft -> pending). Contributors may submit their own
    /// drafts; others' drafts need `edit_others`. Writes no revision.
    pub async fn submit(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        self.transition(actor, id, expected_version, Action::Submit)
            .await
    }

    /// Moves an item in any non-trash status to trash and removes it from the search
    /// index. Needs the same rights as editing the item (contributors: only while draft
    /// or pending). A scheduled time is cleared. Writes no revision.
    pub async fn trash(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        self.transition(actor, id, expected_version, Action::Trash)
            .await
    }

    /// Restores a trashed item as a draft and indexes it for search again. Needs
    /// `edit_own` for own items, `edit_others` otherwise; contributors are forbidden. Writes no revision.
    pub async fn restore(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        self.transition(actor, id, expected_version, Action::Restore)
            .await
    }

    /// Permanently deletes a trashed item at `expected_version` (a stale version is
    /// `Conflict`, a non-trashed item `Invalid`); same rights as restore. Meta,
    /// revisions, term links and the search entry go with it (cascading foreign keys).
    /// An item that still has children is `Invalid("has children")`.
    ///
    /// The row is first written with its own version check (a touch that bumps the
    /// version), which locks it: a concurrent restore either committed before (our
    /// check fails with `Conflict`) or fails its own version check afterwards.
    pub async fn delete(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<(), CoreError> {
        let tx = self.store.begin().await?;
        let done = delete_in(tx.as_ref(), actor, id, expected_version).await;
        finish(tx, done).await
    }

    /// Publishes every scheduled item whose `scheduled_at` is at or before `now`, and
    /// returns how many it published. This is a system path without an actor, meant for
    /// a periodic job.
    ///
    /// Each item gets its own transaction: it is re-read and published only if it is
    /// still scheduled and due (`published_at` is set to `now` if it was never set,
    /// `scheduled_at` is cleared, redirects from its permalink are deleted as in
    /// [`publish`](Self::publish)). Concurrent calls publish each item exactly once: the
    /// loser of a race fails the optimistic version check (or sees the new status) and
    /// counts nothing. Items that are skipped (including after a store error, which is logged
    /// at warn) stay scheduled and are not retried within the call.
    pub async fn publish_due(&self, now: Timestamp) -> Result<u32, CoreError> {
        let mut seen = std::collections::HashSet::new();
        let mut published = 0;
        loop {
            let batch = self
                .store
                .read()
                .content()
                .due_scheduled(now, DUE_BATCH)
                .await?;
            let fresh: Vec<ContentId> = batch
                .iter()
                .map(|i| i.id)
                .filter(|id| seen.insert(*id))
                .collect();
            if fresh.is_empty() {
                return Ok(published);
            }
            for id in fresh {
                // One set of rules per item's write (R10).
                let rules = self.permalinks.pin();
                let tx = self.store.begin().await?;
                let done = publish_one(tx.as_ref(), rules.as_ref(), id, now, Due::By(now)).await;
                match finish(tx, done).await {
                    Ok(true) => published += 1,
                    Ok(false) | Err(CoreError::Conflict { .. }) => {}
                    Err(e) => {
                        tracing::warn!(item = %id, error = %e, "publish_due: skipping item");
                    }
                }
            }
        }
    }

    /// Publishes `id` if it is still scheduled for exactly `at`; `false` (nothing
    /// written) when it is missing, not scheduled, or scheduled for another time. The
    /// handler of a [`content.publish_scheduled`](crate::jobs::PUBLISH_SCHEDULED_KIND)
    /// job calls it with the job's payload: the job's `run_at` is the schedule, so the
    /// clock is not compared again. A system path without an actor.
    ///
    /// Same transaction and event as one item of [`publish_due`](Self::publish_due),
    /// with `now` = the later of the process clock and `at` (so `published_at`, when it
    /// was never set, is not earlier than the scheduled time on a node whose clock lags
    /// the database's). A version `Conflict` with a concurrent writer is retried once,
    /// then reported as `false` (the periodic `publish_due` still catches the item).
    pub async fn publish_scheduled(&self, id: ContentId, at: Timestamp) -> Result<bool, CoreError> {
        for attempt in 0..2 {
            let now = Timestamp::now().max(at);
            // One set of rules for the write (R10).
            let rules = self.permalinks.pin();
            let tx = self.store.begin().await?;
            let done =
                publish_one(tx.as_ref(), rules.as_ref(), id, now, Due::ScheduledFor(at)).await;
            match finish(tx, done).await {
                Err(CoreError::Conflict { .. }) if attempt == 0 => {}
                Err(CoreError::Conflict { .. }) => return Ok(false),
                other => return other,
            }
        }
        Ok(false)
    }

    /// [`publish`](Self::publish) of `item` (as read or written in the caller's `Tx`,
    /// at its current version) inside the caller's `repos` with the caller's pinned
    /// `rules`: the same checks, redirect cleanup, `invalidate` keys and `post.published`
    /// action jobs.
    pub(crate) async fn publish_in(
        repos: &dyn Repos,
        rules: &dyn Permalinks,
        actor: &Actor,
        item: ContentItem,
    ) -> Result<ContentItem, CoreError> {
        let version = item.version;
        transition_item(repos, rules, actor, item, version, Action::Publish).await
    }

    /// [`trash`](Self::trash) of item `id` inside the caller's `repos` with the caller's
    /// pinned `rules`: the same checks, search removal and `invalidate` event.
    pub(crate) async fn trash_in(
        repos: &dyn Repos,
        rules: &dyn Permalinks,
        actor: &Actor,
        current: ContentItem,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        transition_item(
            repos,
            rules,
            actor,
            current,
            expected_version,
            Action::Trash,
        )
        .await
    }

    /// Whether `actor` may publish `item` ([`publish`](Self::publish)'s rights check
    /// alone: `publish_own` for the author, `publish_others` otherwise).
    pub(crate) fn may_publish(actor: &Actor, item: &ContentItem) -> bool {
        authorize(actor, item, Action::Publish).is_ok()
    }

    /// [`restore`](Self::restore) of item `id` inside the caller's `repos` with the
    /// caller's pinned `rules`: the same checks (R8), search entry and `invalidate` event.
    pub(crate) async fn restore_in(
        repos: &dyn Repos,
        rules: &dyn Permalinks,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        transition_in(repos, rules, actor, id, expected_version, Action::Restore).await
    }

    async fn transition(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        action: Action,
    ) -> Result<ContentItem, CoreError> {
        // One set of rules for the write (R10).
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = transition_in(
            tx.as_ref(),
            rules.as_ref(),
            actor,
            id,
            expected_version,
            action,
        )
        .await;
        finish(tx, done).await
    }
}

async fn transition_in(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    actor: &Actor,
    id: ContentId,
    expected_version: i64,
    action: Action,
) -> Result<ContentItem, CoreError> {
    let current = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
    transition_item(repos, permalinks, actor, current, expected_version, action).await
}

/// [`transition_in`] for `current`, the item as read in this `Tx`.
async fn transition_item(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    actor: &Actor,
    current: ContentItem,
    expected_version: i64,
    action: Action,
) -> Result<ContentItem, CoreError> {
    let id = current.id;
    authorize(actor, &current, action)?;
    if current.version != expected_version {
        return Err(CoreError::Conflict {
            current_version: current.version,
        });
    }
    let next = apply(&current, action, Timestamp::now())?;
    let saved = repos.content().update(&next).await?;
    match action {
        Action::Publish => {
            reclaim_path(repos, permalinks, &saved).await?;
            enqueue_published(repos, &saved).await?;
        }
        Action::Schedule(at) => {
            // `None`: the same (item, instant) was scheduled before and that job row
            // still exists; if it already finished, `publish_due` covers the item.
            repos
                .jobs()
                .enqueue(&jobs::publish_scheduled_job(id, at))
                .await?;
        }
        Action::Trash => repos.search().remove(id).await?,
        Action::Restore => {
            repos
                .search()
                .index(id, &saved.title, &blocks::extract_text(&saved.body))
                .await?;
        }
        _ => {}
    }
    emit(repos, &saved).await?;
    Ok(saved)
}

async fn delete_in(
    repos: &dyn Repos,
    actor: &Actor,
    id: ContentId,
    expected_version: i64,
) -> Result<(), CoreError> {
    let current = repos.content().get(id).await?.ok_or(CoreError::NotFound)?;
    authorize_ownership(actor, &current)?;
    if current.version != expected_version {
        return Err(CoreError::Conflict {
            current_version: current.version,
        });
    }
    if !deletable(current.status) {
        return Err(illegal(current.status, "deleted"));
    }
    // Version-checked touch: see `delete`.
    repos.content().update(&current).await?;
    // Read the terms first: the links go with the item.
    let terms = repos.terms().for_item(id).await?;
    match repos.content().delete(id).await {
        Err(StoreError::ForeignKey { .. }) => {
            return Err(CoreError::Invalid("has children".into()));
        }
        other => other?,
    }
    emit_for(repos, &current, &terms).await
}

/// Which scheduled item [`publish_one`] publishes.
#[derive(Debug, Clone, Copy)]
enum Due {
    /// Scheduled at or before this time (`publish_due`).
    By(Timestamp),
    /// Scheduled for exactly this time (`publish_scheduled`).
    ScheduledFor(Timestamp),
}

/// Publishes `id` with `now` as the publish time if it is still scheduled and matches
/// `due`; `false` if it was skipped.
async fn publish_one(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    id: ContentId,
    now: Timestamp,
    due: Due,
) -> Result<bool, CoreError> {
    let Some(current) = repos.content().get(id).await? else {
        return Ok(false);
    };
    let matches = current.status == ContentStatus::Scheduled
        && current.scheduled_at.is_some_and(|at| match due {
            Due::By(limit) => at <= limit,
            Due::ScheduledFor(exact) => at == exact,
        });
    if !matches {
        return Ok(false);
    }
    let next = apply(&current, Action::PublishDue, now)?;
    // Conflicts with a concurrent publisher surface as `Conflict` from the version check.
    let saved = repos.content().update(&next).await?;
    reclaim_path(repos, permalinks, &saved).await?;
    enqueue_published(repos, &saved).await?;
    emit(repos, &saved).await?;
    Ok(true)
}

/// The `post.published` action jobs for `item` (P20), in the transition's `Tx`.
async fn enqueue_published(repos: &dyn Repos, item: &ContentItem) -> Result<(), CoreError> {
    let id = item.id.to_string();
    actions::enqueue(
        repos,
        actions::POST_PUBLISHED,
        Some(id.clone()),
        serde_json::json!({ "post": id }),
    )
    .await?;
    Ok(())
}

/// Deletes the redirects that start at `item`'s permalink: it is live now.
async fn reclaim_path(
    repos: &dyn Repos,
    permalinks: &dyn Permalinks,
    item: &ContentItem,
) -> Result<(), CoreError> {
    let input = load_input(repos, item.clone()).await?;
    clear_redirects_from(repos, &permalinks.path(&input)).await
}

/// The item as it looks after `action`, or `Invalid("illegal transition ...")`. Pure.
fn apply(current: &ContentItem, action: Action, now: Timestamp) -> Result<ContentItem, CoreError> {
    let from = current.status;
    if !legal(action, from) {
        return Err(illegal(from, action.target().as_str()));
    }
    let mut next = current.clone();
    next.status = action.target();
    match action {
        Action::Publish | Action::PublishDue => {
            next.published_at = current.published_at.or(Some(now));
            next.scheduled_at = None;
        }
        Action::Schedule(at) => {
            if at <= now {
                return Err(CoreError::Invalid(
                    "scheduled time must be in the future".into(),
                ));
            }
            next.scheduled_at = Some(at);
        }
        Action::Unpublish | Action::Trash => next.scheduled_at = None,
        Action::Submit | Action::Restore => {}
    }
    Ok(next)
}

/// Whether `action` may start from status `from` (the transition table in the module
/// docs). [`apply`] and [`allowed_actions`](super::allowed_actions) both use it.
pub(super) fn legal(action: Action, from: ContentStatus) -> bool {
    use ContentStatus::*;
    match action {
        // "Publish now" on a scheduled item; the scheduler's own path is `PublishDue`.
        Action::Publish => matches!(from, Draft | Pending | Scheduled),
        Action::Schedule(_) => matches!(from, Draft | Pending),
        Action::PublishDue => from == Scheduled,
        Action::Submit => from == Draft,
        Action::Unpublish => matches!(from, Published | Scheduled | Private),
        Action::Trash => from != Trash,
        Action::Restore => from == Trash,
    }
}

/// Whether an item in `status` may be permanently deleted (only from trash).
pub(super) fn deletable(status: ContentStatus) -> bool {
    status == ContentStatus::Trash
}

/// Whether an item in `status` may be edited (`update`, revision restore): not while
/// trashed (`Invalid("restore first")`).
pub(super) fn editable(status: ContentStatus) -> bool {
    status != ContentStatus::Trash
}

fn illegal(from: ContentStatus, to: &str) -> CoreError {
    CoreError::Invalid(format!("illegal transition {} -> {to}", from.as_str()))
}

pub(super) fn authorize(
    actor: &Actor,
    item: &ContentItem,
    action: Action,
) -> Result<(), CoreError> {
    match action {
        Action::Publish | Action::Unpublish | Action::Schedule(_) => {
            if item.author_id == actor.user {
                require(actor, Capability::PublishOwn)
            } else {
                require(actor, Capability::PublishOthers)
            }
        }
        // System path: `publish_due` has no actor and never calls this.
        Action::PublishDue => Ok(()),
        Action::Submit | Action::Trash => can_edit(actor, item),
        Action::Restore => authorize_ownership(actor, item),
    }
}

/// `edit_own` for the author, `edit_others` for anyone else, whatever the status.
/// Contributors have no rights on trashed items at all.
pub(super) fn authorize_ownership(actor: &Actor, item: &ContentItem) -> Result<(), CoreError> {
    if actor.role == Role::Contributor {
        return Err(CoreError::Forbidden(if item.author_id == actor.user {
            Capability::EditOwn.as_str()
        } else {
            Capability::EditOthers.as_str()
        }));
    }
    if item.author_id == actor.user {
        require(actor, Capability::EditOwn)
    } else {
        require(actor, Capability::EditOthers)
    }
}

/// One `invalidate` event for a transition of `item`.
async fn emit(repos: &dyn Repos, item: &ContentItem) -> Result<(), CoreError> {
    let terms = repos.terms().for_item(item.id).await?;
    emit_for(repos, item, &terms).await
}

/// `post:<id>`, `archive:<kind>`, `term:<t>` for every term in `terms` and
/// `post:<parent>` if the item has a parent.
async fn emit_for(repos: &dyn Repos, item: &ContentItem, terms: &[Term]) -> Result<(), CoreError> {
    let keys = [events::post_key(item.id), events::archive_key(&item.kind)]
        .into_iter()
        .chain(terms.iter().map(|t| events::term_key(t.id)))
        .chain(item.parent_id.map(events::post_key));
    repos.events().append(&events::invalidate(keys)).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ContentStatus::*;
    use ferrite_domain::UserId;
    use serde_json::json;

    fn item(status: ContentStatus) -> ContentItem {
        let now = Timestamp::now();
        ContentItem {
            id: ContentId::new(),
            kind: "post".into(),
            status,
            slug: "s".into(),
            parent_id: None,
            author_id: UserId::new(),
            title: String::new(),
            excerpt: String::new(),
            body: json!({}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: None,
            scheduled_at: None,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    #[test]
    fn state_machine() {
        let now = Timestamp::now();
        let future = Timestamp::from_micros(now.as_micros() + 1_000_000).unwrap();
        let all = [Draft, Pending, Scheduled, Published, Private, Trash];
        let actions = [
            ("publish", Action::Publish),
            ("publish_due", Action::PublishDue),
            ("unpublish", Action::Unpublish),
            ("schedule", Action::Schedule(future)),
            ("submit", Action::Submit),
            ("trash", Action::Trash),
            ("restore", Action::Restore),
        ];
        let legal: &[(&str, &[ContentStatus])] = &[
            ("publish", &[Draft, Pending, Scheduled]),
            ("publish_due", &[Scheduled]),
            ("unpublish", &[Published, Scheduled, Private]),
            ("schedule", &[Draft, Pending]),
            ("submit", &[Draft]),
            ("trash", &[Draft, Pending, Scheduled, Published, Private]),
            ("restore", &[Trash]),
        ];
        for (name, action) in actions {
            let ok = legal.iter().find(|(n, _)| *n == name).unwrap().1;
            for from in all {
                let r = apply(&item(from), action, now);
                assert_eq!(r.is_ok(), ok.contains(&from), "{name} from {from:?}");
                if let Err(e) = r {
                    assert!(
                        matches!(e, CoreError::Invalid(m) if m.starts_with("illegal transition"))
                    );
                }
            }
        }
    }

    #[test]
    fn publish_keeps_first_published_at_and_schedule_needs_future() {
        let now = Timestamp::now();
        let earlier = Timestamp::from_micros(now.as_micros() - 5_000_000).unwrap();
        let mut it = item(Draft);
        it.published_at = Some(earlier);
        assert_eq!(
            apply(&it, Action::Publish, now).unwrap().published_at,
            Some(earlier)
        );
        assert_eq!(
            apply(&item(Draft), Action::Publish, now)
                .unwrap()
                .published_at,
            Some(now)
        );
        assert!(matches!(
            apply(&item(Draft), Action::Schedule(now), now),
            Err(CoreError::Invalid(_))
        ));
        let scheduled = apply(
            &it,
            Action::Schedule(Timestamp::from_micros(now.as_micros() + 1).unwrap()),
            now,
        )
        .unwrap();
        assert_eq!(scheduled.published_at, Some(earlier));
        assert!(scheduled.scheduled_at.is_some());
        assert_eq!(
            apply(&scheduled, Action::Unpublish, now)
                .unwrap()
                .scheduled_at,
            None
        );
    }
}
