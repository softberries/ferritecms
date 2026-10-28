//! Sessions, API tokens, rate-limit counters and the audit log (`0005_auth`) on SQLite.
//! The process clock is the database clock (one process owns the file), so `now` is bound
//! from `Timestamp::now()`.

use std::time::Duration;

use async_trait::async_trait;
use ferrite_domain::{
    ApiToken, ApiTokenId, AuditEntry, AuditId, AuditQuery, NewAudit, Page, RateHit, Session,
    Timestamp, UserId,
};
use ferrite_store::{
    ApiTokenRepo, AuditRepo, MAX_AUTH_LIST, RateLimitRepo, SessionRepo, StoreError,
    check_audit_query, check_new_audit, check_prune_limit, check_rate_bucket, check_rate_window,
    encode_audit_cursor, rate_hit, validate_text,
};
use sqlx::{AssertSqlSafe, Row};

use super::SqliteRepos;
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

#[async_trait]
impl SessionRepo for SqliteRepos {
    async fn insert(&self, session: &Session) -> Result<(), StoreError> {
        check_session(session)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO sessions (id_hash, user_id, csrf_secret, created_at, last_seen_at, \
             expires_at, ip_hash, user_agent) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&session.id_hash)
        .bind(session.user_id.as_uuid())
        .bind(session.csrf_secret.expose())
        .bind(session.created_at.as_micros())
        .bind(session.last_seen_at.as_micros())
        .bind(session.expires_at.as_micros())
        .bind(session.ip_hash.as_deref())
        .bind(session.user_agent.as_deref())
        .execute(e))?;
        Ok(())
    }

    async fn get(&self, id_hash: &str) -> Result<Option<Session>, StoreError> {
        validate_text("session id_hash", id_hash)?;
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE id_hash = ?"
        )))
        .bind(id_hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::session).transpose()
    }

    async fn touch(&self, id_hash: &str, at: Timestamp) -> Result<bool, StoreError> {
        validate_text("session id_hash", id_hash)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE sessions SET last_seen_at = max(last_seen_at, ?) WHERE id_hash = ?"
        )
        .bind(at.as_micros())
        .bind(id_hash)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete(&self, id_hash: &str) -> Result<(), StoreError> {
        validate_text("session id_hash", id_hash)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM sessions WHERE id_hash = ?"
        )
        .bind(id_hash)
        .execute(e))?;
        Ok(())
    }

    async fn delete_for_user(&self, user: UserId, keep: Option<&str>) -> Result<u64, StoreError> {
        if let Some(keep) = keep {
            validate_text("session id_hash", keep)?;
        }
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM sessions WHERE user_id = ?1 AND (?2 IS NULL OR id_hash <> ?2)"
        )
        .bind(user.as_uuid())
        .bind(keep)
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn list_for_user(&self, user: UserId) -> Result<Vec<Session>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE user_id = ? \
             ORDER BY created_at DESC, id_hash DESC LIMIT ?"
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
        // As on Postgres the outer predicate repeats the subquery's (moot under the single
        // writer); `NOT INDEXED` keeps the outer delete on rowid lookups.
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM sessions NOT INDEXED WHERE rowid IN \
             (SELECT rowid FROM sessions WHERE expires_at < ?1 OR last_seen_at < ?2 LIMIT ?3) \
             AND (expires_at < ?1 OR last_seen_at < ?2)"
        )
        .bind(now.as_micros())
        .bind(idle_before.as_micros())
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}

#[async_trait]
impl ApiTokenRepo for SqliteRepos {
    async fn insert(&self, token: &ApiToken) -> Result<(), StoreError> {
        check_token(token)?;
        let scopes = codec::json_to_db(&serde_json::json!(token.scopes))?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO api_tokens (id, user_id, name, token_hash, scopes, created_at, \
             last_used_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(token.id.as_uuid())
        .bind(token.user_id.as_uuid())
        .bind(&token.name)
        .bind(&token.token_hash)
        .bind(scopes)
        .bind(token.created_at.as_micros())
        .bind(token.last_used_at.map(|t| t.as_micros()))
        .bind(token.expires_at.map(|t| t.as_micros()))
        .execute(e))?;
        Ok(())
    }

    async fn get_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, StoreError> {
        validate_text("api token hash", token_hash)?;
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {API_TOKEN_COLUMNS} FROM api_tokens WHERE token_hash = ?"
        )))
        .bind(token_hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::api_token).transpose()
    }

    async fn list_for_user(&self, user: UserId) -> Result<Vec<ApiToken>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {API_TOKEN_COLUMNS} FROM api_tokens WHERE user_id = ? \
             ORDER BY created_at, id LIMIT ?"
        )))
        .bind(user.as_uuid())
        .bind(i64::from(MAX_AUTH_LIST))
        .fetch_all(e))?;
        rows.iter().map(codec::api_token).collect()
    }

    async fn count_for_user(&self, user: UserId) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT COUNT(*) FROM api_tokens WHERE user_id = ?"
        )
        .bind(user.as_uuid())
        .fetch_one(e))?;
        u64::try_from(n).map_err(|_| StoreError::Database(format!("corrupt token count {n}")))
    }

    async fn touch(&self, id: ApiTokenId, at: Timestamp) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE api_tokens SET last_used_at = CASE \
             WHEN last_used_at IS NULL OR last_used_at < ?1 THEN ?1 ELSE last_used_at END \
             WHERE id = ?2"
        )
        .bind(at.as_micros())
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete(&self, user: UserId, id: ApiTokenId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM api_tokens WHERE id = ? AND user_id = ?"
        )
        .bind(id.as_uuid())
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete_for_user(&self, user: UserId) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM api_tokens WHERE user_id = ?"
        )
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn prune(&self, now: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM api_tokens WHERE rowid IN \
             (SELECT rowid FROM api_tokens WHERE expires_at < ? LIMIT ?)"
        )
        .bind(now.as_micros())
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}

#[async_trait]
impl RateLimitRepo for SqliteRepos {
    async fn hit(&self, bucket: &str, window: Duration) -> Result<RateHit, StoreError> {
        check_rate_bucket(bucket)?;
        let w = check_rate_window(window)?;
        let now = Timestamp::now();
        let start = now.as_micros() - now.as_micros().rem_euclid(w);
        let row = run!(&self.writer, |e| sqlx::query(
            "INSERT INTO rate_limits (bucket, window_start, count) VALUES (?, ?, 1) \
             ON CONFLICT (bucket) DO UPDATE SET count = CASE \
             WHEN excluded.window_start > rate_limits.window_start \
             THEN 1 ELSE rate_limits.count + 1 END, \
             window_start = max(rate_limits.window_start, excluded.window_start) \
             RETURNING count, window_start"
        )
        .bind(bucket)
        .bind(start)
        .fetch_one(e))?;
        let count: i64 = row.try_get("count").map_err(map_db_err)?;
        let window_start: i64 = row.try_get("window_start").map_err(map_db_err)?;
        rate_hit(count, window_start, now, w)
    }

    async fn prune(&self, window_before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        // As on Postgres the outer predicate repeats the subquery's (moot under the single
        // writer); `NOT INDEXED` keeps the outer delete on rowid lookups.
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM rate_limits NOT INDEXED WHERE rowid IN \
             (SELECT rowid FROM rate_limits WHERE window_start < ?1 LIMIT ?2) \
             AND window_start < ?1"
        )
        .bind(window_before.as_micros())
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}

#[async_trait]
impl AuditRepo for SqliteRepos {
    async fn append(&self, entry: &NewAudit) -> Result<AuditId, StoreError> {
        check_new_audit(entry)?;
        let details = codec::json_to_db(&entry.details)?;
        let id = AuditId::new();
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO audit_log (id, at, actor_id, action, target_kind, target_id, \
             details, node) VALUES (?, ?, ?, ?, ?, ?, ?, NULL)"
        )
        .bind(id.as_uuid())
        .bind(Timestamp::now().as_micros())
        .bind(entry.actor_id.map(|a| a.as_uuid()))
        .bind(&entry.action)
        .bind(&entry.target_kind)
        .bind(entry.target_id.as_deref())
        .bind(details)
        .execute(e))?;
        Ok(id)
    }

    async fn list(&self, query: &AuditQuery) -> Result<Page<AuditEntry>, StoreError> {
        let plan = check_audit_query(query)?;
        let limit = plan.limit;
        // `substr`/`length` count characters, so the prefix is compared literally and
        // case-sensitively (SQLite's LIKE folds ASCII case).
        let mut sql = format!("SELECT {AUDIT_COLUMNS} FROM audit_log WHERE 1 = 1");
        if query.actor.is_some() {
            sql.push_str(" AND actor_id = ?");
        }
        match plan.action {
            Some((_, true)) => sql.push_str(" AND substr(action, 1, length(?)) = ?"),
            Some((_, false)) => sql.push_str(" AND action = ?"),
            None => {}
        }
        if query.target_kind.is_some() {
            sql.push_str(" AND target_kind = ?");
        }
        if query.target_id.is_some() {
            sql.push_str(" AND target_id = ?");
        }
        if plan.after.is_some() {
            sql.push_str(" AND (at, id) < (?, ?)");
        }
        sql.push_str(" ORDER BY at DESC, id DESC LIMIT ?");
        let mut q = sqlx::query(AssertSqlSafe(sql));
        if let Some(actor) = query.actor {
            q = q.bind(actor.as_uuid());
        }
        match plan.action {
            Some((prefix, true)) => q = q.bind(prefix).bind(prefix),
            Some((action, false)) => q = q.bind(action),
            None => {}
        }
        if let Some(kind) = &query.target_kind {
            q = q.bind(kind.as_str());
        }
        if let Some(id) = &query.target_id {
            q = q.bind(id.as_str());
        }
        if let Some((at, id)) = plan.after {
            q = q.bind(at).bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |e| q.fetch_all(e))?;
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
