//! Bodies of the `/users` endpoints (users and their API tokens).
//!
//! Input bodies ignore members they do not know; in a `PATCH` an absent member keeps the
//! stored value and `null` is refused (`400 malformed-json`). No response carries a
//! password hash, a TOTP secret or a token hash; a token's bearer value appears once,
//! in [`CreatedTokenDto`]. Bodies with a password derive no `Debug`.

use chrono::{DateTime, Utc};
use ferrite_domain::{ApiToken, Role, User, UserStatus};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::Password;
use super::content::present;

/// A user's role (07 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RoleDto {
    Admin,
    Editor,
    Author,
    Contributor,
}

impl From<Role> for RoleDto {
    fn from(r: Role) -> Self {
        match r {
            Role::Admin => Self::Admin,
            Role::Editor => Self::Editor,
            Role::Author => Self::Author,
            Role::Contributor => Self::Contributor,
        }
    }
}

impl From<RoleDto> for Role {
    fn from(r: RoleDto) -> Self {
        match r {
            RoleDto::Admin => Self::Admin,
            RoleDto::Editor => Self::Editor,
            RoleDto::Author => Self::Author,
            RoleDto::Contributor => Self::Contributor,
        }
    }
}

/// Whether the account can log in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UserStatusDto {
    Active,
    /// Cannot log in; its sessions and tokens were deleted when it was disabled.
    Disabled,
}

impl From<UserStatus> for UserStatusDto {
    fn from(s: UserStatus) -> Self {
        match s {
            UserStatus::Active => Self::Active,
            UserStatus::Disabled => Self::Disabled,
        }
    }
}

impl From<UserStatusDto> for UserStatus {
    fn from(s: UserStatusDto) -> Self {
        match s {
            UserStatusDto::Active => Self::Active,
            UserStatusDto::Disabled => Self::Disabled,
        }
    }
}

/// A user account. Only the user themselves and accounts with `manage_users` can read
/// one, so `email` is never shown to anyone else.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDto {
    pub id: Uuid,
    /// Lowercase; never changes.
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub role: RoleDto,
    pub status: UserStatusDto,
    /// The password must be changed at the next login.
    pub must_reset: bool,
    pub totp_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl From<User> for UserDto {
    fn from(u: User) -> Self {
        Self {
            id: u.id.as_uuid(),
            login: u.login,
            email: u.email,
            display_name: u.display_name,
            role: u.role.into(),
            status: u.status.into(),
            must_reset: u.must_reset,
            totp_enabled: u.totp_secret.is_some(),
            created_at: u.created_at.as_datetime(),
            last_login_at: u.last_login_at.map(|t| t.as_datetime()),
        }
    }
}

/// `GET /users`: one page, by login.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserList {
    pub items: Vec<UserDto>,
    pub next_cursor: Option<String>,
}

/// `POST /users`.
#[derive(Deserialize, ToSchema)]
pub struct UserInput {
    /// `^[a-z0-9][a-z0-9._-]{0,59}$` after trimming and lowercasing; cannot change later.
    pub login: String,
    /// Unique (ASCII case-insensitive); at most 254 bytes.
    pub email: String,
    /// 1 to 100 characters.
    pub display_name: String,
    pub role: RoleDto,
    /// 12 to 1024 bytes, at least 12 characters. Without it the account cannot log in
    /// until an admin sets one (`POST /users/{id}/password`).
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Password)]
    pub password: Option<Password>,
    /// Force a password change at the next login (default `false`).
    #[serde(default)]
    pub must_reset: bool,
}

/// `PATCH /users/{id}`: merged into the stored account. Role and status cannot change
/// on oneself; the last active admin cannot be demoted or disabled.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UserPatchInput {
    /// Logins never change: the stored login is accepted (and ignored), any other value
    /// is `422` (`errors[0].field` = `login`).
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub login: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub email: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = RoleDto, required = false)]
    pub role: Option<RoleDto>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = UserStatusDto, required = false)]
    pub status: Option<UserStatusDto>,
    /// The request's own current password. Required when a user changes their **own**
    /// `email` to a different address (missing: `422`, `errors[0].field` =
    /// `current_password`; wrong: `403 forbidden`; checked on the same per-login rate
    /// limit as `POST /auth/password`); ignored otherwise, so an admin changing another
    /// user's e-mail sends none.
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Password)]
    pub current_password: Option<Password>,
}

/// The query of `GET /users`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UserListParams {
    /// Only users whose login, display name or e-mail holds this text, ignoring case
    /// (`%` and `_` match only themselves). Trimmed; blank is no search; at most 100
    /// characters (`422`, field `q`).
    pub q: Option<String>,
    /// Only users with this role.
    pub role: Option<RoleDto>,
    /// Only users with this status.
    pub status: Option<UserStatusDto>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page of the same query.
    pub cursor: Option<String>,
}

/// `POST /users/{id}/password`: an admin sets another user's password.
#[derive(Deserialize, ToSchema)]
pub struct PasswordSet {
    /// 12 to 1024 bytes, at least 12 characters.
    #[schema(value_type = String, format = Password)]
    pub password: Password,
}

/// `?reassign=` of `DELETE /users/{id}`.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeleteUserParams {
    /// An active user who becomes the author of the deleted user's content and
    /// revisions; required when the user has any.
    pub reassign: Option<Uuid>,
}

/// An API token's metadata (never its bearer value or hash).
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenDto {
    pub id: Uuid,
    pub name: String,
    /// `<area>:<read|write>`; `write` implies `read`.
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// Updated at most every 5 minutes.
    pub last_used_at: Option<DateTime<Utc>>,
    /// `null`: never expires.
    pub expires_at: Option<DateTime<Utc>>,
}

impl From<ApiToken> for TokenDto {
    fn from(t: ApiToken) -> Self {
        Self {
            id: t.id.as_uuid(),
            name: t.name,
            scopes: t.scopes,
            created_at: t.created_at.as_datetime(),
            last_used_at: t.last_used_at.map(|t| t.as_datetime()),
            expires_at: t.expires_at.map(|t| t.as_datetime()),
        }
    }
}

/// `GET /users/{id}/tokens`: every token of the user, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenList {
    pub items: Vec<TokenDto>,
    /// Always `null`: one page holds every token (at most 50 per user).
    pub next_cursor: Option<String>,
}

/// `POST /users/{id}/tokens`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenInput {
    /// 1 to 100 characters.
    pub name: String,
    /// At least one `<area>:<read|write>`, each held by the request's own credential.
    pub scopes: Vec<String>,
    /// In the future; `null` or absent: never expires.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// The created token. `token` is the bearer value (`Authorization: Bearer <token>`),
/// shown **only in this response**; it cannot be read again.
#[derive(Serialize, ToSchema)]
pub struct CreatedTokenDto {
    pub token: String,
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl CreatedTokenDto {
    pub fn new(token: String, meta: TokenDto) -> Self {
        let TokenDto {
            id,
            name,
            scopes,
            created_at,
            last_used_at,
            expires_at,
        } = meta;
        Self {
            token,
            id,
            name,
            scopes,
            created_at,
            last_used_at,
            expires_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip() {
        for r in Role::ALL {
            assert_eq!(Role::from(RoleDto::from(*r)), *r);
            assert_eq!(serde_json::to_value(RoleDto::from(*r)).unwrap(), r.as_str());
        }
        for s in UserStatus::ALL {
            assert_eq!(UserStatus::from(UserStatusDto::from(*s)), *s);
            assert_eq!(
                serde_json::to_value(UserStatusDto::from(*s)).unwrap(),
                s.as_str()
            );
        }
    }
}
