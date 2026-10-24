//! `Idempotency-Key` records (`0006_media_comments`) on SQLite. The process clock is the
//! database clock (one process owns the file), and the single writer serialises every
//! insert and takeover.

use std::time::Duration;

use async_trait::async_trait;
use ferrite_domain::{IdemBegin, IdempotencyKey, Timestamp, UserId};
use ferrite_store::{
    IdempotencyRepo, StoreError, check_idempotency, check_idempotency_outcome, check_key_hash,
    check_prune_limit, idempotency_status,
};
use sqlx::Row;

use super::SqliteRepos;
use crate::{
    codec,
    exec::{map_db_err, run},
};

/// Attempts of the insert / read / take-over cycle before `begin` gives up with
/// `InProgress` (each lost race means another request holds or just freed the key).
const BEGIN_ROUNDS: usize = 3;

fn ts(micros: i64) -> Result<Timestamp, StoreError> {
    Ok(Timestamp::from_micros(micros)?)
}

#[async_trait]
impl IdempotencyRepo for SqliteRepos {
    async fn begin(
        &self,
        key: &IdempotencyKey,
        stale_after: Duration,
    ) -> Result<IdemBegin, StoreError> {
        let stale = check_idempotency(key, stale_after)?;
        for _ in 0..BEGIN_ROUNDS {
            let now = Timestamp::now().as_micros();
            let inserted: Option<i64> = run!(&self.writer, |e| sqlx::query_scalar(
                "INSERT INTO idempotency_keys (user_id, key_hash, method, path, created_at) \
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING RETURNING created_at"
            )
            .bind(key.user.as_uuid())
            .bind(&key.key_hash)
            .bind(&key.method)
            .bind(&key.path)
            .bind(now)
            .fetch_optional(e))?;
            if let Some(token) = inserted {
                return Ok(IdemBegin::Started { token: ts(token)? });
            }
            let row = run!(&self.reader, |e| sqlx::query(
                "SELECT method, path, status_code, response, created_at FROM idempotency_keys \
                 WHERE user_id = ? AND key_hash = ?"
            )
            .bind(key.user.as_uuid())
            .bind(&key.key_hash)
            .fetch_optional(e))?;
            // Released between the insert and the read: try the insert again.
            let Some(row) = row else { continue };
            let method: String = row.try_get("method").map_err(map_db_err)?;
            let path: String = row.try_get("path").map_err(map_db_err)?;
            if method != key.method || path != key.path {
                return Ok(IdemBegin::Mismatch);
            }
            let status: Option<i64> = row.try_get("status_code").map_err(map_db_err)?;
            if let Some(status) = status {
                let response: Option<String> = row.try_get("response").map_err(map_db_err)?;
                let response = response.ok_or_else(|| {
                    StoreError::Database("finished idempotency row without response".into())
                })?;
                return Ok(IdemBegin::Replay {
                    status: idempotency_status(status)?,
                    response: codec::json_from_db(&response)?,
                });
            }
            let created: i64 = row.try_get("created_at").map_err(map_db_err)?;
            if created >= now.saturating_sub(stale) {
                return Ok(IdemBegin::InProgress);
            }
            // Stale: take it over, but only if nobody else did since the read. The new
            // `created_at` always differs from the old one, so the old token stops working.
            let taken: Option<i64> = run!(&self.writer, |e| sqlx::query_scalar(
                "UPDATE idempotency_keys SET created_at = max(?1, created_at + 1) \
                 WHERE user_id = ?2 AND key_hash = ?3 AND status_code IS NULL \
                 AND created_at = ?4 RETURNING created_at"
            )
            .bind(now)
            .bind(key.user.as_uuid())
            .bind(&key.key_hash)
            .bind(created)
            .fetch_optional(e))?;
            if let Some(token) = taken {
                return Ok(IdemBegin::Started { token: ts(token)? });
            }
        }
        Ok(IdemBegin::InProgress)
    }

    async fn finish(
        &self,
        user: UserId,
        key_hash: &str,
        token: Timestamp,
        status: u16,
        response: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let (response, status) = check_idempotency_outcome(key_hash, status, response)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE idempotency_keys SET status_code = ?, response = ? \
             WHERE user_id = ? AND key_hash = ? AND status_code IS NULL AND created_at = ?"
        )
        .bind(status)
        .bind(response)
        .bind(user.as_uuid())
        .bind(key_hash)
        .bind(token.as_micros())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn release(
        &self,
        user: UserId,
        key_hash: &str,
        token: Timestamp,
    ) -> Result<bool, StoreError> {
        check_key_hash(key_hash)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM idempotency_keys \
             WHERE user_id = ? AND key_hash = ? AND status_code IS NULL AND created_at = ?"
        )
        .bind(user.as_uuid())
        .bind(key_hash)
        .bind(token.as_micros())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn prune(&self, created_before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        // As on Postgres the outer predicate repeats the subquery's (moot under the single
        // writer); `NOT INDEXED` keeps the outer delete on rowid lookups.
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM idempotency_keys NOT INDEXED WHERE rowid IN \
             (SELECT rowid FROM idempotency_keys WHERE created_at < ?1 LIMIT ?2) \
             AND created_at < ?1"
        )
        .bind(created_before.as_micros())
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
