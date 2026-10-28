//! Sessions, API tokens, rate-limit counters and the audit log (`0005_auth`) on Postgres.
//! Every "now" the store picks (rate-limit windows, audit `at`) is the database clock
//! (`now()`, which inside a `Tx` is the transaction's start).

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    ApiToken, ApiTokenId, AuditEntry, AuditId, AuditQuery, NewAudit, Page, RateHit, Session,
    Timestamp, UserId,
};
use ferrite_store::{
    ApiTokenRepo, AuditRepo, MAX_AUTH_LIST, RateLimitRepo, SessionRepo, StoreError,
    check_audit_query, check_new_audit, check_prune_limit, check_rate_bucket, check_rate_window,
    encode_audit_cursor, rate_hit, validate_text,
};
use sqlx::{AssertSqlSafe, Row, types::Json};

use super::PgRepos;
use crate::{
    codec::{self, API_TOKEN_COLUMNS, AUDIT_COLUMNS, SESSION_COLUMNS},
    exec::{map_db_err, run},
};

fn check_session(s: &Session) -> Result<(), StoreError> {
    validate_text("session id_hash", &s.id_hash)?;
    validate_text("session csrf_secret", s.csrf_secret.expose())?;
    if let Some(ip) = &s.ip_hash {
        validate_text("session ip_hash", ip)?;
    }
    if let Some(ua) = &s.user_agent {
        validate_text("session user_agent", ua)?;
    }
    Ok(())
}

fn check_token(t: &ApiToken) -> Result<(), StoreError> {
    validate_text("api token name", &t.name)?;
    validate_text("api token hash", &t.token_hash)?;
    t.scopes
        .iter()
        .try_for_each(|s| validate_text("api token scope", s))
}

fn pg_ts(t: Timestamp) -> DateTime<Utc> {
    t.into()
}

#[async_trait]
impl SessionRepo for PgRepos {
    async fn insert(&self, session: &Session) -> Result<(), StoreError> {
        check_session(session)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO sessions (id_hash, user_id, csrf_secret, created_at, last_seen_at, \
             expires_at, ip_hash, user_agent) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(&session.id_hash)
        .bind(session.user_id.as_uuid())
        .bind(session.csrf_secret.expose())
        .bind(pg_ts(session.created_at))
        .bind(pg_ts(session.last_seen_at))
        .bind(pg_ts(session.expires_at))
        .bind(session.ip_hash.as_deref())
        .bind(session.user_agent.as_deref())
        .execute(e))?;
        Ok(())
    }

    async fn get(&self, id_hash: &str) -> Result<Option<Session>, StoreError> {
        validate_text("session id_hash", id_hash)?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE id_hash = $1"
        )))
        .bind(id_hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::session).transpose()
    }

    async fn touch(&self, id_hash: &str, at: Timestamp) -> Result<bool, StoreError> {
        validate_text("session id_hash", id_hash)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE sessions SET last_seen_at = GREATEST(last_seen_at, $1) WHERE id_hash = $2"
        )
        .bind(pg_ts(at))
        .bind(id_hash)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete(&self, id_hash: &str) -> Result<(), StoreError> {
        validate_text("session id_hash", id_hash)?;
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM sessions WHERE id_hash = $1"
        )
        .bind(id_hash)
        .execute(e))?;
        Ok(())
    }

    async fn delete_for_user(&self, user: UserId, keep: Option<&str>) -> Result<u64, StoreError> {
        if let Some(keep) = keep {
            validate_text("session id_hash", keep)?;
        }
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM sessions WHERE user_id = $1 AND ($2::text IS NULL OR id_hash <> $2)"
        )
        .bind(user.as_uuid())
        .bind(keep)
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn list_for_user(&self, user: UserId) -> Result<Vec<Session>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE user_id = $1 \
             ORDER BY created_at DESC, id_hash DESC LIMIT $2"
        )))
        .bind(user.as_uuid())
        .bind(i64::from(MAX_AUTH_LIST))
        .fetch_all(e))?;
        rows.iter().map(codec::session).collect()
    }

    async fn prune(
        &self,
        now: Timestamp,
        idle_before: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        // The outer predicate repeats the subquery's: a row picked here and touched
        // meanwhile (READ COMMITTED re-checks only the outer `WHERE`) is kept.
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM sessions WHERE id_hash IN \
             (SELECT id_hash FROM sessions WHERE expires_at < $1 OR last_seen_at < $2 \
              LIMIT $3) \
             AND (expires_at < $1 OR last_seen_at < $2)"
        )
        .bind(pg_ts(now))
        .bind(pg_ts(idle_before))
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}

#[async_trait]
impl ApiTokenRepo for PgRepos {
    async fn insert(&self, token: &ApiToken) -> Result<(), StoreError> {
        check_token(token)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO api_tokens (id, user_id, name, token_hash, scopes, created_at, \
             last_used_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(token.id.as_uuid())
        .bind(token.user_id.as_uuid())
        .bind(&token.name)
        .bind(&token.token_hash)
        .bind(Json(&token.scopes))
        .bind(pg_ts(token.created_at))
        .bind(token.last_used_at.map(pg_ts))
        .bind(token.expires_at.map(pg_ts))
        .execute(e))?;
        Ok(())
    }

    async fn get_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, StoreError> {
        validate_text("api token hash", token_hash)?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {API_TOKEN_COLUMNS} FROM api_tokens WHERE token_hash = $1"
        )))
        .bind(token_hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::api_token).transpose()
    }

    async fn list_for_user(&self, user: UserId) -> Result<Vec<ApiToken>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {API_TOKEN_COLUMNS} FROM api_tokens WHERE user_id = $1 \
             ORDER BY created_at, id LIMIT $2"
        )))
        .bind(user.as_uuid())
        .bind(i64::from(MAX_AUTH_LIST))
        .fetch_all(e))?;
        rows.iter().map(codec::api_token).collect()
    }

    async fn count_for_user(&self, user: UserId) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT COUNT(*) FROM api_tokens WHERE user_id = $1"
        )
        .bind(user.as_uuid())
        .fetch_one(e))?;
        u64::try_from(n).map_err(|_| StoreError::Database(format!("corrupt token count {n}")))
    }

    async fn touch(&self, id: ApiTokenId, at: Timestamp) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE api_tokens SET last_used_at = CASE \
             WHEN last_used_at IS NULL OR last_used_at < $1 THEN $1 ELSE last_used_at END \
             WHERE id = $2"
        )
        .bind(pg_ts(at))
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete(&self, user: UserId, id: ApiTokenId) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM api_tokens WHERE id = $1 AND user_id = $2"
        )
        .bind(id.as_uuid())
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete_for_user(&self, user: UserId) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM api_tokens WHERE user_id = $1"
        )
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn prune(&self, now: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM api_tokens WHERE id IN \
             (SELECT id FROM api_tokens WHERE expires_at < $1 LIMIT $2)"
        )
        .bind(pg_ts(now))
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}

#[async_trait]
impl RateLimitRepo for PgRepos {
    async fn hit(&self, bucket: &str, window: Duration) -> Result<RateHit, StoreError> {
        check_rate_bucket(bucket)?;
        let w = check_rate_window(window)?;
        // `date_bin` floors `now()` to the window in integer microseconds since the epoch.
        let row = run!(&self.exec, |e| sqlx::query(
            "INSERT INTO rate_limits (bucket, window_start, count) VALUES ($1, \
             date_bin($2::bigint * interval '1 microsecond', now(), \
             timestamptz '1970-01-01 00:00:00+00'), 1) \
             ON CONFLICT (bucket) DO UPDATE SET count = CASE \
             WHEN excluded.window_start > rate_limits.window_start \
             THEN 1 ELSE rate_limits.count + 1 END, \
             window_start = GREATEST(rate_limits.window_start, excluded.window_start) \
             RETURNING count, window_start, now() AS now"
        )
        .bind(bucket)
        .bind(w)
        .fetch_one(e))?;
        let count: i32 = row.try_get("count").map_err(map_db_err)?;
        let window_start: DateTime<Utc> = row.try_get("window_start").map_err(map_db_err)?;
        let now: DateTime<Utc> = row.try_get("now").map_err(map_db_err)?;
        let window_start = Timestamp::from(window_start).as_micros();
        rate_hit(i64::from(count), window_start, now.into(), w)
    }

    async fn prune(&self, window_before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        // The outer predicate repeats the subquery's: a row picked here and touched
        // meanwhile (READ COMMITTED re-checks only the outer `WHERE`) is kept.
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM rate_limits WHERE bucket IN \
             (SELECT bucket FROM rate_limits WHERE window_start < $1 LIMIT $2) \
             AND window_start < $1"
        )
        .bind(pg_ts(window_before))
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}

#[async_trait]
impl AuditRepo for PgRepos {
    async fn append(&self, entry: &NewAudit) -> Result<AuditId, StoreError> {
        check_new_audit(entry)?;
        let id = AuditId::new();
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO audit_log (id, at, actor_id, action, target_kind, target_id, \
             details, node) VALUES ($1, now(), $2, $3, $4, $5, $6, NULL)"
        )
        .bind(id.as_uuid())
        .bind(entry.actor_id.map(|a| a.as_uuid()))
        .bind(&entry.action)
        .bind(&entry.target_kind)
        .bind(entry.target_id.as_deref())
        .bind(Json(&entry.details))
        .execute(e))?;
        Ok(id)
    }

    async fn list(&self, query: &AuditQuery) -> Result<Page<AuditEntry>, StoreError> {
        let plan = check_audit_query(query)?;
        let limit = plan.limit;
        let after = plan
            .after
            .map(|(at, id)| Ok::<_, StoreError>((pg_ts(Timestamp::from_micros(at)?), id)))
            .transpose()?;
        // `starts_with` compares literally and case-sensitively (no LIKE wildcards).
        let mut sql = format!("SELECT {AUDIT_COLUMNS} FROM audit_log WHERE 1 = 1");
        let mut n = 0;
        let mut param = || {
            n += 1;
            format!("${n}")
        };
        if query.actor.is_some() {
            sql.push_str(&format!(" AND actor_id = {}", param()));
        }
        match plan.action {
            Some((_, true)) => sql.push_str(&format!(" AND starts_with(action, {})", param())),
            Some((_, false)) => sql.push_str(&format!(" AND action = {}", param())),
            None => {}
        }
        if query.target_kind.is_some() {
            sql.push_str(&format!(" AND target_kind = {}", param()));
        }
        if query.target_id.is_some() {
            sql.push_str(&format!(" AND target_id = {}", param()));
        }
        if after.is_some() {
            let (at, id) = (param(), param());
            sql.push_str(&format!(" AND (at, id) < ({at}, {id})"));
        }
        sql.push_str(&format!(" ORDER BY at DESC, id DESC LIMIT {}", param()));
        let mut q = sqlx::query(AssertSqlSafe(sql));
        if let Some(actor) = query.actor {
            q = q.bind(actor.as_uuid());
        }
        if let Some((action, _)) = plan.action {
            q = q.bind(action);
        }
        if let Some(kind) = &query.target_kind {
            q = q.bind(kind.as_str());
        }
        if let Some(id) = &query.target_id {
            q = q.bind(id.as_str());
        }
        if let Some((at, id)) = after {
            q = q.bind(at).bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.exec, |e| q.fetch_all(e))?;
        let mut items = rows
            .iter()
            .map(codec::audit_entry)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|a| encode_audit_cursor(a.at, a.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }
}
