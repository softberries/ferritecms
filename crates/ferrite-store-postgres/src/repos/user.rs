//! Users. Logins are exact (case-sensitive); e-mails are unique and looked up by `email_key`
//! (ASCII-lowercased in Rust, identically on every backend).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::Timestamp;
use ferrite_domain::{NewUser, Page, Secret, User, UserId};
use ferrite_store::{
    MAX_ID_FILTER, StoreError, USER_CURSOR_TAG, UserRepo, check_len, check_limit, email_key,
    encode_text_cursor, text_cursor, validate_text,
};

use sqlx::{Postgres, QueryBuilder};

use super::PgRepos;
use crate::{
    codec::{self, USER_COLUMNS},
    exec::run,
};

/// NUL checks shared by insert and update (R13), secrets included.
fn validate_user_text(
    login: &str,
    email: &str,
    display_name: &str,
    password_hash: Option<&Secret>,
    totp_secret: Option<&Secret>,
) -> Result<(), StoreError> {
    validate_text("user login", login)?;
    validate_text("user email", email)?;
    validate_text("user display_name", display_name)?;
    if let Some(hash) = password_hash {
        validate_text("user password_hash", hash.expose())?;
    }
    if let Some(totp) = totp_secret {
        validate_text("user totp_secret", totp.expose())?;
    }
    Ok(())
}

#[async_trait]
impl UserRepo for PgRepos {
    async fn insert(&self, user: &NewUser) -> Result<User, StoreError> {
        validate_user_text(
            &user.login,
            &user.email,
            &user.display_name,
            user.password_hash.as_ref(),
            None,
        )?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO users (id, login, email, email_key, display_name, password_hash, \
             role, status, must_reset, totp_secret, created_at, last_login_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, $10, NULL) \
             RETURNING {USER_COLUMNS}"
        )))
        .bind(user.id.as_uuid())
        .bind(&user.login)
        .bind(&user.email)
        .bind(email_key(&user.email))
        .bind(&user.display_name)
        .bind(user.password_hash.as_ref().map(Secret::expose))
        .bind(user.role.as_str())
        .bind(user.status.as_str())
        .bind(user.must_reset)
        .bind(DateTime::<Utc>::from(Timestamp::now()))
        .fetch_one(e))?;
        codec::user(&row)
    }

    async fn get(&self, id: UserId) -> Result<Option<User>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    async fn get_by_login(&self, login: &str) -> Result<Option<User>, StoreError> {
        validate_text("user login", login)?;
        // Exact, case-sensitive.
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE login = $1"
        )))
        .bind(login)
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    async fn get_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        validate_text("user email", email)?;
        // Looked up by the Rust-folded key, never by a database collation.
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE email_key = $1"
        )))
        .bind(email_key(email))
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    // `id` and `created_at` are immutable; everything else is replaced.
    async fn update(&self, user: &User) -> Result<(), StoreError> {
        validate_user_text(
            &user.login,
            &user.email,
            &user.display_name,
            user.password_hash.as_ref(),
            user.totp_secret.as_ref(),
        )?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE users SET login = $1, email = $2, email_key = $3, display_name = $4, \
             password_hash = $5, role = $6, status = $7, must_reset = $8, \
             totp_secret = $9, last_login_at = $10 WHERE id = $11"
        )
        .bind(&user.login)
        .bind(&user.email)
        .bind(email_key(&user.email))
        .bind(&user.display_name)
        .bind(user.password_hash.as_ref().map(Secret::expose))
        .bind(user.role.as_str())
        .bind(user.status.as_str())
        .bind(user.must_reset)
        .bind(user.totp_secret.as_ref().map(Secret::expose))
        .bind(user.last_login_at.map(DateTime::<Utc>::from))
        .bind(user.id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<User>, StoreError> {
        let limit = check_limit(limit)?;
        let after = cursor
            .map(|c| text_cursor(USER_CURSOR_TAG, c))
            .transpose()?;
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users \
             WHERE $1::text IS NULL OR (login COLLATE \"C\", id) > ($1::text, $2::uuid) \
             ORDER BY login COLLATE \"C\", id LIMIT $3"
        )))
        .bind(after.as_ref().map(|(k, _)| k.as_str()))
        .bind(after.as_ref().map(|(_, id)| *id))
        .bind(i64::from(limit) + 1)
        .fetch_all(e))?;
        let mut items = rows
            .iter()
            .map(codec::user)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|u| encode_text_cursor(USER_CURSOR_TAG, &u.login, u.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn get_many(&self, ids: &[UserId]) -> Result<Vec<User>, StoreError> {
        check_len("ids", ids.len(), MAX_ID_FILTER)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE id IN ("
        ));
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        let mut found = rows
            .iter()
            .map(codec::user)
            .map(|r| r.map(|u| (u.id, u)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }
}
