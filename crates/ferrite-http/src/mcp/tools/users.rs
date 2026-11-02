//! User tools (design 12 §5.2): `users_list` and `user_get`. Read-only on purpose:
//! creating, changing or deleting accounts, roles and passwords, and everything about
//! tokens, sessions, TOTP and connected apps, is a person's job in the admin (design 12
//! §5.4).
//!
//! Adapters over [`UserService`](ferrite_core::UserService) and the REST list function
//! (`api::users::find_users`); core checks `manage_users` with `users:read` (a user may
//! read themselves). The answer is its own [`UserOut`], not the REST `UserDto`: it has
//! no password data (`must_reset`) and no TOTP data (`totp_enabled`), so a tool result
//! can never carry them, whatever the DTO grows later.

use chrono::{DateTime, Utc};
use ferrite_domain::{User, UserId};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::per_page;
use crate::{
    api::{
        dto::users::{RoleDto, UserListParams, UserStatusDto},
        users::find_users,
    },
    mcp::{Caller, FerriteMcp, results},
};

/// The input of `users_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Only users whose login, display name or e-mail holds this text, ignoring case (at
    /// most 100 characters).
    #[serde(default)]
    pub q: Option<String>,
    /// Only users with this role.
    #[serde(default)]
    pub role: Option<RoleDto>,
    /// Only users with this status.
    #[serde(default)]
    pub status: Option<UserStatusDto>,
    /// Users per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same query, to get the next page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// The input of `user_get`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    /// The user's id (from users_list).
    pub id: Uuid,
}

/// A user as the tools show them: who they are and what they may do, nothing about how
/// they sign in.
#[derive(Debug, Serialize)]
pub struct UserOut {
    pub id: Uuid,
    /// The login name (lowercase; never changes).
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub role: RoleDto,
    pub status: UserStatusDto,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl From<User> for UserOut {
    fn from(u: User) -> Self {
        Self {
            id: u.id.as_uuid(),
            login: u.login,
            email: u.email,
            display_name: u.display_name,
            role: u.role.into(),
            status: u.status.into(),
            created_at: u.created_at.as_datetime(),
            last_login_at: u.last_login_at.map(|t| t.as_datetime()),
        }
    }
}

/// One page of `users_list`.
#[derive(Debug, Serialize)]
pub struct ListOut {
    pub items: Vec<UserOut>,
    /// Pass as `cursor` for the next page; `null` on the last one.
    pub next_cursor: Option<String>,
}

#[tool_router(router = user_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists user accounts by login: id, login, e-mail, display name, role (`admin`,
    /// `editor`, `author`, `contributor`), status (`active` or `disabled`), when the
    /// account was created and when the user last signed in. Filter with `q`, `role` or
    /// `status`; page with `cursor`. Needs an administrator. Accounts are managed in the
    /// admin.
    #[tool(annotations(title = "List users", read_only_hint = true, open_world_hint = false))]
    pub async fn users_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ListInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let params = UserListParams {
            q: input.q,
            role: input.role,
            status: input.status,
            limit: None,
            cursor: input.cursor,
        };
        match find_users(&self.api, caller.actor(), params, limit).await {
            Ok(page) => {
                let out = ListOut {
                    items: page.items.into_iter().map(UserOut::from).collect(),
                    next_cursor: page.next_cursor,
                };
                let n = out.items.len();
                let summary = format!(
                    "{}.",
                    results::page_summary(n, "user", out.next_cursor.is_some())
                );
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Reads one user account (same fields as users_list). Anyone may read their own
    /// account; reading another needs an administrator.
    #[tool(annotations(title = "Get user", read_only_hint = true, open_world_hint = false))]
    pub async fn user_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<IdInput>,
    ) -> CallToolResult {
        match self
            .api
            .users
            .get(caller.actor(), UserId::from_uuid(input.id))
            .await
        {
            Ok(user) => {
                let summary = format!("{} ({}).", user.display_name, user.role.as_str());
                results::ok(UserOut::from(user), summary)
            }
            Err(e) => results::from_core(e),
        }
    }
}
