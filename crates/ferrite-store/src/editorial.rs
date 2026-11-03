//! The editorial repositories (`0017_releases_workflows`, design 15 §8): releases with
//! their entries and locks, workflows, reviews and preview links.

use async_trait::async_trait;
use ferrite_domain::{
    ContentId, Cursor, Page, PreviewLink, PreviewLinkId, PreviewTarget, Release, ReleaseEntry,
    ReleaseId, ReleaseQuery, Review, ReviewDecision, ReviewId, ReviewQuery, ReviewTarget,
    Timestamp, UserId, Workflow, WorkflowStep,
};
use serde_json::json;
use uuid::Uuid;

use crate::{StoreError, validate_text};

/// Cursor tag of the release list (newest first).
pub const RELEASE_CURSOR_TAG: &str = "rl:created";
/// Cursor tag of the review list (oldest first).
pub const REVIEW_CURSOR_TAG: &str = "rv:submitted";
/// At most this many releases are returned by [`ReleaseRepo::releases_of`].
pub const MAX_RELEASES_OF: u32 = 10;

/// Encodes a release-list cursor pointing just after (`created_at`, `id`).
pub fn encode_release_cursor(created_at: Timestamp, id: ReleaseId) -> String {
    Cursor::encode(
        RELEASE_CURSOR_TAG,
        &json!(created_at.as_micros()),
        id.as_uuid(),
    )
}

/// Encodes a review-list cursor pointing just after (`submitted_at`, `id`).
pub fn encode_review_cursor(submitted_at: Timestamp, id: ReviewId) -> String {
    Cursor::encode(
        REVIEW_CURSOR_TAG,
        &json!(submitted_at.as_micros()),
        id.as_uuid(),
    )
}

/// Decodes a cursor of `tag` made by [`encode_release_cursor`] or
/// [`encode_review_cursor`] into (micros, id).
pub fn micros_cursor(cursor: &str, tag: &str) -> Result<(i64, Uuid), StoreError> {
    let bad = || StoreError::Invalid("cursor does not match the query order".into());
    match Cursor::decode(cursor, tag)? {
        (serde_json::Value::Number(n), id) => n
            .as_i64()
            .filter(|at| Timestamp::from_micros(*at).is_ok())
            .map(|at| (at, id))
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// NUL-checks the texts of a release.
pub fn check_release(r: &Release) -> Result<(), StoreError> {
    validate_text("release title", &r.title)?;
    validate_text("release description", &r.description)?;
    if let Some(o) = &r.override_reason {
        validate_text("override reason", o)?;
    }
    Ok(())
}

/// NUL-checks the texts of workflow steps.
pub fn check_steps(steps: &[WorkflowStep]) -> Result<(), StoreError> {
    steps.iter().try_for_each(|s| {
        validate_text("workflow step key", &s.key)?;
        validate_text("workflow step label", &s.label)
    })
}

/// NUL-checks the texts of a review.
pub fn check_review(r: &Review) -> Result<(), StoreError> {
    check_steps(&r.steps)?;
    if let Some(n) = &r.note {
        validate_text("review note", n)?;
    }
    Ok(())
}

/// Releases, their entries and their item locks.
#[async_trait]
pub trait ReleaseRepo: Send + Sync {
    /// Inserts a release; a known id is `UniqueViolation`.
    async fn insert(&self, release: &Release) -> Result<(), StoreError>;
    async fn get(&self, id: ReleaseId) -> Result<Option<Release>, StoreError>;
    /// Writes every column but `id`, `created_at` and `created_by` WHERE id AND version =
    /// `release.version`, bumping the version; returns the stored row. Another version is
    /// `Conflict { current_version }`, a missing row `NotFound`.
    async fn update(&self, release: &Release) -> Result<Release, StoreError>;
    /// Deletes a release with its entries, locks and entry reviews; `true` if it existed.
    async fn delete(&self, id: ReleaseId) -> Result<bool, StoreError>;
    /// A page of releases, newest first; cursor tag [`RELEASE_CURSOR_TAG`].
    async fn list(&self, query: &ReleaseQuery) -> Result<Page<Release>, StoreError>;
    /// The number of entries of each release (missing releases are left out).
    async fn entry_counts(&self, ids: &[ReleaseId]) -> Result<Vec<(ReleaseId, u32)>, StoreError>;
    /// The entries of a release in position order.
    async fn entries(&self, release: ReleaseId) -> Result<Vec<ReleaseEntry>, StoreError>;
    async fn entry(
        &self,
        release: ReleaseId,
        item: ContentId,
    ) -> Result<Option<ReleaseEntry>, StoreError>;
    /// Inserts the entry (`position` after the last, `entry_version` 1) or replaces the
    /// existing one (transition, changes, base version, `updated_at`; position and
    /// `added_by` kept, `entry_version` bumped). Returns the stored entry. An unknown
    /// release or item is `ForeignKey`.
    async fn put_entry(&self, entry: &ReleaseEntry) -> Result<ReleaseEntry, StoreError>;
    /// Removes an entry (and its entry reviews); `true` if it existed.
    async fn delete_entry(&self, release: ReleaseId, item: ContentId) -> Result<bool, StoreError>;
    /// The releases that are not applied holding `item`, newest first, at most
    /// [`MAX_RELEASES_OF`].
    async fn releases_of(&self, item: ContentId) -> Result<Vec<Release>, StoreError>;
    /// Locks `item` for `release`; an item already locked is `UniqueViolation {
    /// constraint: "release_locks_pkey" }`.
    async fn lock(&self, item: ContentId, release: ReleaseId) -> Result<(), StoreError>;
    /// Removes every lock of `release`; returns the count.
    async fn unlock_all(&self, release: ReleaseId) -> Result<u64, StoreError>;
    /// The release locking `item`, if any.
    async fn lock_of(&self, item: ContentId) -> Result<Option<ReleaseId>, StoreError>;
    /// Scheduled releases whose `scheduled_at` is at or before `now`, oldest first, at
    /// most `limit` (1..=200).
    async fn due(&self, now: Timestamp, limit: u32) -> Result<Vec<Release>, StoreError>;
}

/// Approval workflows, one per content type.
#[async_trait]
pub trait WorkflowRepo: Send + Sync {
    async fn get(&self, type_name: &str) -> Result<Option<Workflow>, StoreError>;
    /// `expected` `None`: inserts (`UniqueViolation { constraint: "workflows_pkey" }` when
    /// one exists; an unknown type is `ForeignKey`). `Some(v)`: replaces the steps WHERE
    /// version = v, bumping it (`Conflict` / `NotFound`). Returns the stored row.
    async fn put(&self, workflow: &Workflow, expected: Option<i64>)
    -> Result<Workflow, StoreError>;
    /// Deletes the workflow of a type; `true` if it existed.
    async fn delete(&self, type_name: &str) -> Result<bool, StoreError>;
    /// Every workflow by type name.
    async fn list(&self) -> Result<Vec<Workflow>, StoreError>;
}

/// Reviews with their decisions and assignees.
#[async_trait]
pub trait ReviewRepo: Send + Sync {
    /// Inserts a review; a second open review of the same target is `UniqueViolation {
    /// constraint: "reviews_open_target_uq" }`; an entry review of a missing entry is
    /// `ForeignKey`.
    async fn insert(&self, review: &Review) -> Result<(), StoreError>;
    async fn get(&self, id: ReviewId) -> Result<Option<Review>, StoreError>;
    /// Writes status, step, note and `decided_at` WHERE id AND version = `review.version`,
    /// bumping the version; `Conflict` / `NotFound` otherwise.
    async fn update(&self, review: &Review) -> Result<Review, StoreError>;
    /// The open (`in_review` or `approved`) review of a target.
    async fn open_for(&self, target: &ReviewTarget) -> Result<Option<Review>, StoreError>;
    /// A page of reviews, oldest first; cursor tag [`REVIEW_CURSOR_TAG`].
    async fn list(&self, query: &ReviewQuery) -> Result<Page<Review>, StoreError>;
    /// Closes the open item review of `item` as `stale`; returns the count.
    async fn mark_stale_item(&self, item: ContentId, at: Timestamp) -> Result<u64, StoreError>;
    /// Closes the open review of an entry as `stale`; returns the count.
    async fn mark_stale_entry(
        &self,
        release: ReleaseId,
        item: ContentId,
        at: Timestamp,
    ) -> Result<u64, StoreError>;
    /// Closes every open review of items of `type_name` as `withdrawn`; returns the count.
    async fn withdraw_for_type(&self, type_name: &str, at: Timestamp) -> Result<u64, StoreError>;
    /// The decisions of a review, oldest first.
    async fn decisions(&self, review: ReviewId) -> Result<Vec<ReviewDecision>, StoreError>;
    /// Records a decision; a second one of the same user on the same step is
    /// `UniqueViolation { constraint: "review_decisions_step_user_uq" }`.
    async fn add_decision(&self, decision: &ReviewDecision) -> Result<(), StoreError>;
    /// The assignees of a review as (step, user), by step.
    async fn assignees(&self, review: ReviewId) -> Result<Vec<(u32, UserId)>, StoreError>;
    /// Replaces the assignees of one step.
    async fn set_assignees(
        &self,
        review: ReviewId,
        step: u32,
        users: &[UserId],
    ) -> Result<(), StoreError>;
    /// The `in_review` reviews whose current step has `user` as an assignee.
    async fn counts_for(&self, user: UserId) -> Result<u64, StoreError>;
}

/// Hashed, expiring preview links.
#[async_trait]
pub trait PreviewLinkRepo: Send + Sync {
    /// Inserts a link with the sha-256 hex of its token; a known hash is
    /// `UniqueViolation`.
    async fn insert(&self, link: &PreviewLink, token_hash: &str) -> Result<(), StoreError>;
    async fn get(&self, id: PreviewLinkId) -> Result<Option<PreviewLink>, StoreError>;
    async fn get_by_hash(&self, token_hash: &str) -> Result<Option<PreviewLink>, StoreError>;
    /// Sets `last_used_at`.
    async fn touch(&self, id: PreviewLinkId, at: Timestamp) -> Result<(), StoreError>;
    /// The links of a target, newest first.
    async fn list_for(&self, target: &PreviewTarget) -> Result<Vec<PreviewLink>, StoreError>;
    /// `true` if it existed.
    async fn delete(&self, id: PreviewLinkId) -> Result<bool, StoreError>;
    /// Deletes every link of a target; returns the count.
    async fn delete_for_target(&self, target: &PreviewTarget) -> Result<u64, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) links that
    /// expired before `now`; returns the count.
    async fn prune(&self, now: Timestamp, limit: u32) -> Result<u64, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_roundtrip_and_refuse_other_tags() {
        let at = Timestamp::now();
        let r = ReleaseId::new();
        let c = encode_release_cursor(at, r);
        assert_eq!(
            micros_cursor(&c, RELEASE_CURSOR_TAG).unwrap(),
            (at.as_micros(), r.as_uuid())
        );
        assert!(micros_cursor(&c, REVIEW_CURSOR_TAG).is_err());
    }
}
