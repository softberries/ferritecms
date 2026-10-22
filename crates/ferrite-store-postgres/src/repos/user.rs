//! Users. Logins are exact (case-sensitive); e-mails compare case-insensitively.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{NewUser, Page, Timestamp, User, UserId};
use ferrite_store::{StoreError, UserRepo, check_limit, encode_text_cursor, text_cursor};

use super::PgRepos;
use crate::{
    codec::{self, USER_COLUMNS},
    exec::run,
};

#[async_trait]
impl UserRepo for PgRepos {
    async fn insert(&self, user: &NewUser) -> Result<User, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO users (id, login, email, display_name, password_hash, role, status, \
             must_reset, totp_secret, created_at, last_login_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, NULL) RETURNING {USER_COLUMNS}"
        )))
        .bind(user.id.as_uuid())
        .bind(&user.login)
        .bind(&user.email)
        .bind(&user.display_name)
        .bind(user.password_hash.as_ref().map(|s| s.expose().to_owned()))
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
        // Exact, case-sensitive.
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE login = $1"
        )))
        .bind(login)
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    async fn get_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        // Matches the unique index on `lower(email)`.
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE lower(email) = lower($1)"
        )))
        .bind(email)
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    // `id` and `created_at` are immutable; everything else is replaced.
    async fn update(&self, user: &User) -> Result<(), StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE users SET login = $1, email = $2, display_name = $3, password_hash = $4, \
             role = $5, status = $6, must_reset = $7, totp_secret = $8, \
             last_login_at = $9 WHERE id = $10"
        )
        .bind(&user.login)
        .bind(&user.email)
        .bind(&user.display_name)
        .bind(user.password_hash.as_ref().map(|s| s.expose().to_owned()))
        .bind(user.role.as_str())
        .bind(user.status.as_str())
        .bind(user.must_reset)
        .bind(user.totp_secret.as_ref().map(|s| s.expose().to_owned()))
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
        let after = cursor.map(text_cursor).transpose()?;
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
                .map(|u| encode_text_cursor(&u.login, u.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }
}
