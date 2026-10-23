//! Users. Logins are exact (case-sensitive); e-mails are unique and looked up by `email_key`
//! (ASCII-lowercased in Rust, identically on every backend).

use std::collections::HashMap;

use async_trait::async_trait;
use ferrite_domain::Timestamp;
use ferrite_domain::{NewUser, Page, Secret, User, UserId};
use ferrite_store::{
    MAX_ID_FILTER, StoreError, USER_CURSOR_TAG, UserRepo, check_len, check_limit, email_key,
    encode_text_cursor, text_cursor, validate_text,
};

use sqlx::{QueryBuilder, Sqlite};

use super::SqliteRepos;
use crate::{
    codec::{self, USER_COLUMNS},
    exec::run,
};

/// NUL checks shared by insert and update (R13), the password hash included.
fn validate_user_text(
    login: &str,
    email: &str,
    display_name: &str,
    password_hash: Option<&Secret>,
) -> Result<(), StoreError> {
    validate_text("user login", login)?;
    validate_text("user email", email)?;
    validate_text("user display_name", display_name)?;
    if let Some(hash) = password_hash {
        validate_text("user password_hash", hash.expose())?;
    }
    Ok(())
}

#[async_trait]
impl UserRepo for SqliteRepos {
    async fn insert(&self, user: &NewUser) -> Result<User, StoreError> {
        validate_user_text(
            &user.login,
            &user.email,
            &user.display_name,
            user.password_hash.as_ref(),
        )?;
        let row = run!(&self.writer, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO users (id, login, email, email_key, display_name, password_hash, \
             role, status, must_reset, totp_secret, created_at, last_login_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, NULL) \
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
        .bind(Timestamp::now().as_micros())
        .fetch_one(e))?;
        codec::user(&row)
    }

    async fn get(&self, id: UserId) -> Result<Option<User>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    async fn get_by_login(&self, login: &str) -> Result<Option<User>, StoreError> {
        validate_text("user login", login)?;
        // Exact, case-sensitive.
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE login = ?"
        )))
        .bind(login)
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    async fn get_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        validate_text("user email", email)?;
        // Looked up by the Rust-folded key, never by a database collation.
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE email_key = ?"
        )))
        .bind(email_key(email))
        .fetch_optional(e))?;
        row.as_ref().map(codec::user).transpose()
    }

    // `id` and `created_at` are immutable; the TOTP columns have their own methods.
    async fn update(&self, user: &User) -> Result<(), StoreError> {
        validate_user_text(
            &user.login,
            &user.email,
            &user.display_name,
            user.password_hash.as_ref(),
        )?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE users SET login = ?, email = ?, email_key = ?, display_name = ?, \
             password_hash = ?, role = ?, status = ?, must_reset = ?, \
             last_login_at = ? WHERE id = ?"
        )
        .bind(&user.login)
        .bind(&user.email)
        .bind(email_key(&user.email))
        .bind(&user.display_name)
        .bind(user.password_hash.as_ref().map(Secret::expose))
        .bind(user.role.as_str())
        .bind(user.status.as_str())
        .bind(user.must_reset)
        .bind(user.last_login_at.map(|t| t.as_micros()))
        .bind(user.id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn get_for_update(&self, id: UserId) -> Result<Option<User>, StoreError> {
        // Inside a Tx the reader is the Tx's connection, which holds the single writer:
        // no other write can touch the row until the Tx ends.
        UserRepo::get(self, id).await
    }

    async fn set_totp(&self, id: UserId, secret: Option<&Secret>) -> Result<(), StoreError> {
        if let Some(secret) = secret {
            validate_text("user totp_secret", secret.expose())?;
        }
        // One statement: a new (or no) secret always starts a fresh replay guard.
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE users SET totp_secret = ?, totp_last_step = NULL WHERE id = ?"
        )
        .bind(secret.map(Secret::expose))
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn advance_totp_step(&self, id: UserId, step: i64) -> Result<bool, StoreError> {
        // Writes go through the single writer connection, so a concurrent caller with the
        // same step runs after this one and its condition matches nothing.
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE users SET totp_last_step = ?1 \
             WHERE id = ?2 AND (totp_last_step IS NULL OR totp_last_step < ?1)"
        )
        .bind(step)
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn delete(&self, id: UserId) -> Result<(), StoreError> {
        // Sessions and tokens cascade, audit rows are set NULL; content and revisions
        // restrict (ForeignKey).
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM users WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn count_active_admins(&self) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND status = 'active'"
        )
        .fetch_one(e))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<User>, StoreError> {
        let limit = check_limit(limit)?;
        let after = cursor
            .map(|c| text_cursor(USER_CURSOR_TAG, c))
            .transpose()?;
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users \
             WHERE ?1 IS NULL OR (login, id) > (?1, ?2) \
             ORDER BY login, id LIMIT ?3"
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
        let mut qb =
            QueryBuilder::<Sqlite>::new(format!("SELECT {USER_COLUMNS} FROM users WHERE id IN ("));
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
        let rows = run!(&self.reader, |e| qb.build().fetch_all(e))?;
        let mut found = rows
            .iter()
            .map(codec::user)
            .map(|r| r.map(|u| (u.id, u)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }
}
