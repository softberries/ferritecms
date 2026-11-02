//! OAuth clients, pending requests, codes and grants (`0013_oauth`) on Postgres. The
//! caller passes every "now" (the service owns the clock); expired means
//! `expires_at <= now`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    OAuthClient, OAuthClientId, OAuthCode, OAuthGrant, OAuthGrantId, OAuthPruned, OAuthRequest,
    OAuthRequestId, Page, Timestamp, UserId,
};
use ferrite_store::{
    MAX_AUTH_LIST, OAuthRepo, StoreError, check_limit, check_prune_limit, encode_grant_cursor,
    grant_cursor, validate_text,
};
use sqlx::{AssertSqlSafe, types::Json};

use super::PgRepos;
use crate::{
    codec::{
        self, OAUTH_CLIENT_COLUMNS, OAUTH_CODE_COLUMNS, OAUTH_GRANT_COLUMNS, OAUTH_REQUEST_COLUMNS,
    },
    exec::run,
};

fn pg_ts(t: Timestamp) -> DateTime<Utc> {
    t.into()
}

fn check_texts<'a>(
    field: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), StoreError> {
    values.into_iter().try_for_each(|v| validate_text(field, v))
}

fn check_client(c: &OAuthClient) -> Result<(), StoreError> {
    validate_text("oauth client id", c.id.as_str())?;
    validate_text("oauth client name", &c.name)?;
    check_texts(
        "oauth client redirect uri",
        c.redirect_uris.iter().map(String::as_str),
    )?;
    check_texts(
        "oauth client uri",
        c.client_uri.iter().chain(&c.logo_uri).map(String::as_str),
    )
}

fn check_scopes(scopes: &[String]) -> Result<(), StoreError> {
    check_texts("oauth scope", scopes.iter().map(String::as_str))
}

#[async_trait]
impl OAuthRepo for PgRepos {
    async fn upsert_client(&self, client: &OAuthClient) -> Result<(), StoreError> {
        check_client(client)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO oauth_clients (id, kind, name, redirect_uris, client_uri, logo_uri, \
             created_at, fetched_at, last_used_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, name = excluded.name, \
             redirect_uris = excluded.redirect_uris, client_uri = excluded.client_uri, \
             logo_uri = excluded.logo_uri, fetched_at = excluded.fetched_at"
        )
        .bind(client.id.as_str())
        .bind(client.kind.as_str())
        .bind(&client.name)
        .bind(Json(&client.redirect_uris))
        .bind(client.client_uri.as_deref())
        .bind(client.logo_uri.as_deref())
        .bind(pg_ts(client.created_at))
        .bind(client.fetched_at.map(pg_ts))
        .bind(client.last_used_at.map(pg_ts))
        .execute(e))?;
        Ok(())
    }

    async fn get_client(&self, id: &OAuthClientId) -> Result<Option<OAuthClient>, StoreError> {
        validate_text("oauth client id", id.as_str())?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_CLIENT_COLUMNS} FROM oauth_clients WHERE id = $1"
        )))
        .bind(id.as_str())
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_client).transpose()
    }

    async fn touch_client(&self, id: &OAuthClientId, at: Timestamp) -> Result<bool, StoreError> {
        validate_text("oauth client id", id.as_str())?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE oauth_clients SET last_used_at = CASE \
             WHEN last_used_at IS NULL OR last_used_at < $1 THEN $1 ELSE last_used_at END \
             WHERE id = $2"
        )
        .bind(pg_ts(at))
        .bind(id.as_str())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn insert_request(&self, request: &OAuthRequest) -> Result<(), StoreError> {
        validate_text("oauth client id", request.client_id.as_str())?;
        check_texts(
            "oauth request",
            [
                request.redirect_uri.as_str(),
                request.code_challenge.as_str(),
            ]
            .into_iter()
            .chain(request.state.as_deref())
            .chain(request.resource.as_deref()),
        )?;
        check_scopes(&request.scopes)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO oauth_requests (id, client_id, redirect_uri, scopes, state, \
             code_challenge, resource, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        )
        .bind(request.id.as_uuid())
        .bind(request.client_id.as_str())
        .bind(&request.redirect_uri)
        .bind(Json(&request.scopes))
        .bind(request.state.as_deref())
        .bind(&request.code_challenge)
        .bind(request.resource.as_deref())
        .bind(pg_ts(request.created_at))
        .bind(pg_ts(request.expires_at))
        .execute(e))?;
        Ok(())
    }

    async fn get_request(
        &self,
        id: OAuthRequestId,
        now: Timestamp,
    ) -> Result<Option<OAuthRequest>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_REQUEST_COLUMNS} FROM oauth_requests \
             WHERE id = $1 AND expires_at > $2"
        )))
        .bind(id.as_uuid())
        .bind(pg_ts(now))
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_request).transpose()
    }

    async fn take_request(
        &self,
        id: OAuthRequestId,
        now: Timestamp,
    ) -> Result<Option<OAuthRequest>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM oauth_requests WHERE id = $1 AND expires_at > $2 \
             RETURNING {OAUTH_REQUEST_COLUMNS}"
        )))
        .bind(id.as_uuid())
        .bind(pg_ts(now))
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_request).transpose()
    }

    async fn insert_code(&self, code: &OAuthCode) -> Result<(), StoreError> {
        check_texts(
            "oauth code",
            [
                code.code_hash.as_str(),
                code.client_id.as_str(),
                code.redirect_uri.as_str(),
                code.code_challenge.as_str(),
            ]
            .into_iter()
            .chain(code.resource.as_deref()),
        )?;
        check_scopes(&code.scopes)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, scopes, \
             code_challenge, resource, created_at, expires_at, grant_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL)"
        )
        .bind(&code.code_hash)
        .bind(code.client_id.as_str())
        .bind(code.user_id.as_uuid())
        .bind(&code.redirect_uri)
        .bind(Json(&code.scopes))
        .bind(&code.code_challenge)
        .bind(code.resource.as_deref())
        .bind(pg_ts(code.created_at))
        .bind(pg_ts(code.expires_at))
        .execute(e))?;
        Ok(())
    }

    async fn take_code(
        &self,
        code_hash: &str,
        now: Timestamp,
        grant: OAuthGrantId,
    ) -> Result<Option<OAuthCode>, StoreError> {
        validate_text("oauth code hash", code_hash)?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE oauth_codes SET grant_id = $1 \
             WHERE code_hash = $2 AND grant_id IS NULL AND expires_at > $3 \
             RETURNING {OAUTH_CODE_COLUMNS}"
        )))
        .bind(grant.as_uuid())
        .bind(code_hash)
        .bind(pg_ts(now))
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_code).transpose()
    }

    async fn code_by_hash_any(&self, code_hash: &str) -> Result<Option<OAuthCode>, StoreError> {
        validate_text("oauth code hash", code_hash)?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_CODE_COLUMNS} FROM oauth_codes WHERE code_hash = $1"
        )))
        .bind(code_hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_code).transpose()
    }

    // `SKIP LOCKED`: a code another `Tx` is claiming right now is left to it. That exchange
    // then waits on the user's row (locked by the caller) and finds the user disabled, so
    // the two never wait on each other.
    async fn delete_codes_for_user(&self, user: UserId) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM oauth_codes WHERE code_hash IN (SELECT code_hash FROM oauth_codes \
             WHERE user_id = $1 AND grant_id IS NULL FOR UPDATE SKIP LOCKED)"
        )
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn insert_grant(&self, grant: &OAuthGrant, refresh_hash: &str) -> Result<(), StoreError> {
        validate_text("oauth client id", grant.client_id.as_str())?;
        validate_text("oauth refresh hash", refresh_hash)?;
        check_scopes(&grant.scopes)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO oauth_grants (id, client_id, user_id, scopes, refresh_hash, \
             previous_hash, created_at, last_used_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, NULL, $6, $7, $8)"
        )
        .bind(grant.id.as_uuid())
        .bind(grant.client_id.as_str())
        .bind(grant.user_id.as_uuid())
        .bind(Json(&grant.scopes))
        .bind(refresh_hash)
        .bind(pg_ts(grant.created_at))
        .bind(pg_ts(grant.last_used_at))
        .bind(pg_ts(grant.expires_at))
        .execute(e))?;
        Ok(())
    }

    async fn get_grant(&self, id: OAuthGrantId) -> Result<Option<OAuthGrant>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_GRANT_COLUMNS} FROM oauth_grants WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_grant).transpose()
    }

    async fn grant_by_refresh(&self, hash: &str) -> Result<Option<OAuthGrant>, StoreError> {
        validate_text("oauth refresh hash", hash)?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_GRANT_COLUMNS} FROM oauth_grants WHERE refresh_hash = $1"
        )))
        .bind(hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_grant).transpose()
    }

    async fn grant_by_previous(&self, hash: &str) -> Result<Option<OAuthGrant>, StoreError> {
        validate_text("oauth refresh hash", hash)?;
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_GRANT_COLUMNS} FROM oauth_grants WHERE previous_hash = $1"
        )))
        .bind(hash)
        .fetch_optional(e))?;
        row.as_ref().map(codec::oauth_grant).transpose()
    }

    async fn rotate_grant(
        &self,
        id: OAuthGrantId,
        new_hash: &str,
        prev_hash: &str,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        check_texts("oauth refresh hash", [new_hash, prev_hash])?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE oauth_grants SET refresh_hash = $1, previous_hash = $2, expires_at = $3, \
             last_used_at = $4 WHERE id = $5 AND refresh_hash = $2"
        )
        .bind(new_hash)
        .bind(prev_hash)
        .bind(pg_ts(expires_at))
        .bind(pg_ts(now))
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete_grant(&self, id: OAuthGrantId) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM oauth_grants WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn grants_for_user(&self, user: UserId) -> Result<Vec<OAuthGrant>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {OAUTH_GRANT_COLUMNS} FROM oauth_grants WHERE user_id = $1 \
             ORDER BY created_at DESC, id DESC LIMIT $2"
        )))
        .bind(user.as_uuid())
        .bind(i64::from(MAX_AUTH_LIST))
        .fetch_all(e))?;
        rows.iter().map(codec::oauth_grant).collect()
    }

    async fn all_grants(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<OAuthGrant>, StoreError> {
        let limit = check_limit(limit)?;
        let after = cursor
            .map(grant_cursor)
            .transpose()?
            .map(|(at, id)| Ok::<_, StoreError>((pg_ts(Timestamp::from_micros(at)?), id)))
            .transpose()?;
        let mut sql = format!("SELECT {OAUTH_GRANT_COLUMNS} FROM oauth_grants");
        if after.is_some() {
            sql.push_str(
                " WHERE (created_at, id) < ($1, $2) ORDER BY created_at DESC, id DESC LIMIT $3",
            );
        } else {
            sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT $1");
        }
        let mut q = sqlx::query(AssertSqlSafe(sql));
        if let Some((at, id)) = after {
            q = q.bind(at).bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.exec, |e| q.fetch_all(e))?;
        let mut items = rows
            .iter()
            .map(codec::oauth_grant)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|g| encode_grant_cursor(g.created_at, g.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn prune_expired(&self, now: Timestamp, limit: u32) -> Result<OAuthPruned, StoreError> {
        let limit = check_prune_limit(limit)?;
        let mut pruned = OAuthPruned::default();
        for (table, key, count) in [
            ("oauth_requests", "id", &mut pruned.requests),
            ("oauth_codes", "code_hash", &mut pruned.codes),
            ("oauth_grants", "id", &mut pruned.grants),
        ] {
            let done = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
                "DELETE FROM {table} WHERE {key} IN \
                 (SELECT {key} FROM {table} WHERE expires_at <= $1 LIMIT $2)"
            )))
            .bind(pg_ts(now))
            .bind(limit)
            .execute(e))?;
            *count = done.rows_affected();
        }
        Ok(pruned)
    }
}
