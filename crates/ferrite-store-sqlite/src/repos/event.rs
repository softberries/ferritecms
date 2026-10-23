//! The event outbox (design 04 §4). `keys` is stored as a JSON array.

use async_trait::async_trait;
use ferrite_domain::{Event, NewEvent, Timestamp};
use ferrite_store::{EventRepo, StoreError, check_event_limit, check_prune_limit, validate_text};

use super::SqliteRepos;
use crate::{codec, exec::run};

#[async_trait]
impl EventRepo for SqliteRepos {
    async fn append(&self, event: &NewEvent) -> Result<i64, StoreError> {
        validate_text("event kind", &event.kind)?;
        for key in &event.keys {
            validate_text("event key", key)?;
        }
        if let Some(origin) = &event.origin_node {
            validate_text("event origin_node", origin)?;
        }
        let keys =
            serde_json::to_string(&event.keys).map_err(|e| StoreError::Invalid(e.to_string()))?;
        run!(&self.writer, |e| sqlx::query_scalar(
            "INSERT INTO events (kind, keys, origin_node, created_at) VALUES (?, ?, ?, ?) \
             RETURNING seq"
        )
        .bind(&event.kind)
        .bind(keys)
        .bind(event.origin_node.as_deref())
        .bind(Timestamp::now().as_micros())
        .fetch_one(e))
    }

    async fn since(&self, after_seq: i64, limit: u32) -> Result<Vec<Event>, StoreError> {
        let limit = check_event_limit(limit)?;
        let rows = run!(&self.reader, |e| sqlx::query(
            "SELECT seq, kind, keys, origin_node, created_at FROM events WHERE seq > ? \
             ORDER BY seq LIMIT ?"
        )
        .bind(after_seq)
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(codec::event).collect()
    }

    async fn max_seq(&self) -> Result<i64, StoreError> {
        run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) FROM events"
        )
        .fetch_one(e))
    }

    async fn min_seq(&self) -> Result<i64, StoreError> {
        run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT COALESCE(MIN(seq), 0) FROM events"
        )
        .fetch_one(e))
    }

    async fn prune(&self, older_than: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM events WHERE seq IN \
             (SELECT seq FROM events WHERE created_at < ? ORDER BY seq LIMIT ?)"
        )
        .bind(older_than.as_micros())
        .bind(limit)
        .execute(e))
        .map(|done| done.rows_affected())
    }
}
