//! `Idempotency-Key` records (`0006_media_comments`) on Postgres. Every time the store
//! picks (`created_at`, the staleness check) is the database clock (`now()`).

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{IdemBegin, IdempotencyKey, Timestamp, UserId};
use ferrite_store::{
    IdempotencyRepo, StoreError, check_idempotency, check_idempotency_outcome, check_key_hash,
    check_prune_limit, idempotency_status,
};
use sqlx::{Row, types::Json};

use super::PgRepos;
use crate::exec::{map_db_err, run};

/// Attempts of the insert / read / take-over cycle before `begin` gives up with
/// `InProgress` (each lost race means another request holds or just freed the key).
const BEGIN_ROUNDS: usize = 3;

/// `IdempotencyRepo::prune`. The outer predicate repeats the subquery's: a row picked here
/// and touched meanwhile (READ COMMITTED re-checks only the outer `WHERE`) is kept.
/// Bind 1: created before; bind 2: the limit. Public so the plan tests can `EXPLAIN` it.
pub const PRUNE_IDEMPOTENCY_SQL: &str = "DELETE FROM idempotency_keys WHERE (user_id, key_hash) IN \
     (SELECT user_id, key_hash FROM idempotency_keys WHERE created_at < $1 LIMIT $2) \
     AND created_at < $1";

#[async_trait]
impl IdempotencyRepo for PgRepos {
    async fn begin(
        &self,
        key: &IdempotencyKey,
        stale_after: Duration,
    ) -> Result<IdemBegin, StoreError> {
        let stale = check_idempotency(key, stale_after)?;
        for _ in 0..BEGIN_ROUNDS {
            let inserted: Option<DateTime<Utc>> = run!(&self.exec, |e| sqlx::query_scalar(
                "INSERT INTO idempotency_keys (user_id, key_hash, method, path, created_at) \
                 VALUES ($1, $2, $3, $4, now()) ON CONFLICT DO NOTHING RETURNING created_at"
            )
            .bind(key.user.as_uuid())
            .bind(&key.key_hash)
            .bind(&key.method)
            .bind(&key.path)
            .fetch_optional(e))?;
            if let Some(token) = inserted {
                return Ok(IdemBegin::Started {
                    token: token.into(),
                });
            }
            let row = run!(&self.exec, |e| sqlx::query(
                "SELECT method, path, status_code, response, created_at, now() AS now \
                 FROM idempotency_keys WHERE user_id = $1 AND key_hash = $2"
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
            let status: Option<i32> = row.try_get("status_code").map_err(map_db_err)?;
            if let Some(status) = status {
                let response: Option<Json<serde_json::Value>> =
                    row.try_get("response").map_err(map_db_err)?;
                let response = response.ok_or_else(|| {
                    StoreError::Database("finished idempotency row without response".into())
                })?;
                return Ok(IdemBegin::Replay {
                    status: idempotency_status(i64::from(status))?,
                    response: response.0,
                });
            }
            let created: DateTime<Utc> = row.try_get("created_at").map_err(map_db_err)?;
            let now: DateTime<Utc> = row.try_get("now").map_err(map_db_err)?;
            let created_us = Timestamp::from(created).as_micros();
            if created_us >= Timestamp::from(now).as_micros().saturating_sub(stale) {
                return Ok(IdemBegin::InProgress);
            }
            // Stale: take it over, but only if nobody else did since the read (READ
            // COMMITTED re-checks the WHERE on the row a concurrent taker wrote). The new
            // `created_at` always differs from the old one, so the old token stops working.
            let taken: Option<DateTime<Utc>> = run!(&self.exec, |e| sqlx::query_scalar(
                "UPDATE idempotency_keys \
                 SET created_at = GREATEST(now(), created_at + interval '1 microsecond') \
                 WHERE user_id = $1 AND key_hash = $2 AND status_code IS NULL \
                 AND created_at = $3 RETURNING created_at"
            )
            .bind(key.user.as_uuid())
            .bind(&key.key_hash)
            .bind(created)
            .fetch_optional(e))?;
            if let Some(token) = taken {
                return Ok(IdemBegin::Started {
                    token: token.into(),
                });
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
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE idempotency_keys SET status_code = $1, response = $2::jsonb \
             WHERE user_id = $3 AND key_hash = $4 AND status_code IS NULL AND created_at = $5"
        )
        .bind(status)
        .bind(response)
        .bind(user.as_uuid())
        .bind(key_hash)
        .bind(DateTime::<Utc>::from(token))
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
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM idempotency_keys \
             WHERE user_id = $1 AND key_hash = $2 AND status_code IS NULL AND created_at = $3"
        )
        .bind(user.as_uuid())
        .bind(key_hash)
        .bind(DateTime::<Utc>::from(token))
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn prune(&self, created_before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |e| sqlx::query(PRUNE_IDEMPOTENCY_SQL)
            .bind(DateTime::<Utc>::from(created_before))
            .bind(limit)
            .execute(e))?;
        Ok(done.rows_affected())
    }
}
