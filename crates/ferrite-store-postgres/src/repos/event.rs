//! The event outbox (design 04 §4). `keys` is stored as a JSON array.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{Event, NewEvent, Timestamp};
use ferrite_store::{EventRepo, StoreError, check_event_limit, validate_text};
use sqlx::types::Json;

use super::PgRepos;
use crate::{codec, exec::run};

#[async_trait]
impl EventRepo for PgRepos {
    async fn append(&self, event: &NewEvent) -> Result<i64, StoreError> {
        validate_text("event kind", &event.kind)?;
        for key in &event.keys {
            validate_text("event key", key)?;
        }
        if let Some(origin) = &event.origin_node {
            validate_text("event origin_node", origin)?;
        }
        run!(&self.exec, |e| sqlx::query_scalar(
            "INSERT INTO events (kind, keys, origin_node, created_at) VALUES ($1, $2, $3, $4) \
             RETURNING seq"
        )
        .bind(&event.kind)
        .bind(Json(&event.keys))
        .bind(event.origin_node.as_deref())
        .bind(DateTime::<Utc>::from(Timestamp::now()))
        .fetch_one(e))
    }

    async fn since(&self, after_seq: i64, limit: u32) -> Result<Vec<Event>, StoreError> {
        let limit = check_event_limit(limit)?;
        let rows = run!(&self.exec, |e| sqlx::query(
            "SELECT seq, kind, keys, origin_node, created_at FROM events WHERE seq > $1 \
             ORDER BY seq LIMIT $2"
        )
        .bind(after_seq)
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(codec::event).collect()
    }

    async fn max_seq(&self) -> Result<i64, StoreError> {
        run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0)::bigint FROM events"
        )
        .fetch_one(e))
    }

    async fn prune(&self, older_than: Timestamp) -> Result<u64, StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM events WHERE created_at < $1"
        )
        .bind(DateTime::<Utc>::from(older_than))
        .execute(e))
        .map(|done| done.rows_affected())
    }
}
