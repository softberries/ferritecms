//! Users. Only `insert` and `get` exist so far; the rest arrives with M1a Task 7.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{NewUser, Page, Timestamp, User, UserId};
use ferrite_store::{StoreError, UserRepo};

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

    async fn get_by_login(&self, _login: &str) -> Result<Option<User>, StoreError> {
        Err(StoreError::Unsupported("UserRepo::get_by_login"))
    }

    async fn get_by_email(&self, _email: &str) -> Result<Option<User>, StoreError> {
        Err(StoreError::Unsupported("UserRepo::get_by_email"))
    }

    async fn update(&self, _user: &User) -> Result<(), StoreError> {
        Err(StoreError::Unsupported("UserRepo::update"))
    }

    async fn list(&self, _limit: u32, _cursor: Option<&str>) -> Result<Page<User>, StoreError> {
        Err(StoreError::Unsupported("UserRepo::list"))
    }
}
