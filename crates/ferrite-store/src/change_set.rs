//! The change-set repository (`0018_change_sets`, design 17 §5) and its list cursor.

use async_trait::async_trait;
use ferrite_domain::{
    CHANGE_SET_CURSOR_TAG, Change, ChangeSet, ChangeSetId, ChangeSetQuery, ChangeSummary, Cursor,
    NewChange, NewChangeSet, Page, Timestamp, UserId,
};
use serde_json::json;
use uuid::Uuid;

use crate::{StoreError, validate_text};

/// At most this many changes are read by [`ChangeSetRepo::changes`] and
/// [`ChangeSetRepo::summaries`] (a set closes at 500; a multi-key write may overshoot).
pub const MAX_CHANGES_READ: u32 = 2_000;

/// Encodes the list cursor pointing just after the set (`last_at`, `id`).
pub fn encode_change_set_cursor(last_at: Timestamp, id: Uuid) -> String {
    Cursor::encode(CHANGE_SET_CURSOR_TAG, &json!(last_at.as_micros()), id)
}

/// Decodes a cursor made by [`encode_change_set_cursor`] into (`last_at` micros, `id`).
pub fn change_set_cursor(cursor: &str) -> Result<(i64, Uuid), StoreError> {
    let bad = || StoreError::Invalid("cursor does not match the query order".into());
    match Cursor::decode(cursor, CHANGE_SET_CURSOR_TAG)? {
        (serde_json::Value::Number(n), id) => n
            .as_i64()
            .filter(|at| Timestamp::from_micros(*at).is_ok())
            .map(|at| (at, id))
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// NUL-checks the texts of a new set.
pub fn check_new_change_set(s: &NewChangeSet) -> Result<(), StoreError> {
    validate_text("change set client", &s.client)?;
    validate_text("change set open key", &s.open_key)
}

/// NUL-checks the texts of a new change and its `seq` (1-based).
pub fn check_new_change(c: &NewChange) -> Result<(), StoreError> {
    if c.seq == 0 {
        return Err(StoreError::Invalid("change seq starts at 1".into()));
    }
    validate_text("change op", &c.op)?;
    validate_text("change object kind", &c.object_kind)?;
    validate_text("change object id", &c.object_id)?;
    validate_text("change label", &c.label)
}

/// Change sets and their changes. Every text bound is NUL-checked (`Invalid`).
#[async_trait]
pub trait ChangeSetRepo: Send + Sync {
    /// Adds `n` changes to the set holding `open_key` when it is not idle (`last_at >=
    /// idle_since`) and not full (`changes < max_changes`): `changes += n`, `last_at =
    /// now`. Returns the set and the first `seq` reserved for the caller.
    async fn extend_open(
        &self,
        open_key: &str,
        now: Timestamp,
        idle_since: Timestamp,
        max_changes: u32,
        n: u32,
    ) -> Result<Option<(ChangeSetId, u32)>, StoreError>;
    /// Closes a stale or full set holding `set.open_key` (per `set.idle_since` and
    /// `set.max_changes`), then inserts the new set with `changes = n`; a concurrent
    /// opener of the same key (whose row now holds it) is joined instead (`changes +=
    /// n`). Returns the set and the first `seq` reserved for the caller.
    async fn open(&self, set: &NewChangeSet, n: u32) -> Result<(ChangeSetId, u32), StoreError>;
    /// Inserts a change; an unknown set is `ForeignKey`, a known `(set, seq)`
    /// `UniqueViolation`.
    async fn insert_change(&self, change: &NewChange) -> Result<(), StoreError>;
    async fn get(&self, id: ChangeSetId) -> Result<Option<ChangeSet>, StoreError>;
    /// Newest `last_at` first, then id; cursor tag
    /// [`CHANGE_SET_CURSOR_TAG`](ferrite_domain::CHANGE_SET_CURSOR_TAG).
    async fn list(&self, query: &ChangeSetQuery) -> Result<Page<ChangeSet>, StoreError>;
    /// The changes of a set without image bytes, by `seq`, at most [`MAX_CHANGES_READ`].
    async fn summaries(&self, id: ChangeSetId) -> Result<Vec<ChangeSummary>, StoreError>;
    async fn change(&self, id: ChangeSetId, seq: u32) -> Result<Option<Change>, StoreError>;
    /// The changes of a set with their images, by `seq`, at most [`MAX_CHANGES_READ`].
    async fn changes(&self, id: ChangeSetId) -> Result<Vec<Change>, StoreError>;
    /// Marks `seqs` reverted by `by` at `at` (`revert_forced` for those in `forced`), then
    /// the set itself when none of its changes is left unreverted.
    async fn mark_reverted(
        &self,
        id: ChangeSetId,
        seqs: &[u32],
        by: UserId,
        at: Timestamp,
        forced: &[u32],
    ) -> Result<(), StoreError>;
    /// Clears the set's `open_key`: later writes of the agent open a new set.
    async fn close(&self, id: ChangeSetId) -> Result<(), StoreError>;
    /// How many changes were recorded at or after `since` in sets of `user` (every
    /// user's when `None`).
    async fn changes_since(
        &self,
        user: Option<UserId>,
        since: Timestamp,
    ) -> Result<u64, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) sets whose
    /// `last_at` is before `before`, oldest first, with their changes. Returns the count.
    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError>;
}
