//! User accounts (07 §1–2, P14–P17): creation, profile and role changes, passwords and
//! deletion, with the rules every caller (API, CLI) shares.
//!
//! * Logins match `^[a-z0-9][a-z0-9._-]{0,59}$` after trimming and ASCII lowercasing, and
//!   never change after creation (R3). E-mails are unique by the store's ASCII
//!   `email_key`; display names are 1..=100 characters. Errors about one input
//!   (`login`, `email`, `display_name`, `password`, a taken login or e-mail) are
//!   `InvalidField` naming it; constraint names never leave core.
//! * The last active admin cannot be demoted, disabled or deleted. The check counts
//!   active admins inside the writing `Tx`, after locking the target row. Under READ
//!   COMMITTED (Postgres default) two concurrent writes on two *different* admins can each
//!   see the other still active and both succeed, leaving none (P16, accepted).
//! * Nobody changes their own role or status, or deletes themselves.
//! * A role change, disable or delete removes the user's sessions; disable and delete
//!   also remove their API tokens (delete through the `ON DELETE CASCADE`), so the next
//!   request on any node is refused.
//! * Every write appends an audit row (`user.create`, `user.update`, `user.password`,
//!   `user.delete`, `user.totp_disable`) in the same `Tx`, without secrets. A display-name
//!   or status change and a delete emit `invalidate [user:<id>]` (a delete with
//!   `reassign_to` also names the new author, whose pages now list the items).
//! * Passwords are hashed before the `Tx` opens (argon2 must not run inside one,
//!   M1c-1 P1); every read-modify-write reads the row with `get_for_update` inside the
//!   `Tx` that writes it, because `UserRepo::update` is unversioned (F5).

use std::sync::Arc;

use ferrite_domain::{NewAudit, NewUser, Page, Role, User, UserId, UserStatus};
use ferrite_store::{Repos, Store, StoreError};
use serde_json::{Value, json};

use crate::{
    Actor, CoreError,
    actor::{Area, Capability, Scope, can, require, require_scope},
    auth::password::{self, Verified},
    events,
    tx::finish,
};

/// Longest login, in characters.
pub const MAX_LOGIN: usize = 60;
/// Longest e-mail address, in bytes.
pub const MAX_EMAIL: usize = 254;
/// Longest display name, in characters.
pub const MAX_DISPLAY_NAME: usize = 100;

const LAST_ADMIN: &str = "the site needs at least one active admin";
const OWN_ROLE: &str = "cannot change your own role or status";
const LOGIN_FIXED: &str = "a login cannot be changed";
const LOGIN_TAKEN: &str = "login taken";
const EMAIL_TAKEN: &str = "e-mail taken";

/// Input for a new account.
#[derive(Debug, Clone)]
pub struct NewAccount {
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub role: Role,
    /// `None` creates an account that cannot log in until a password is set.
    pub password: Option<String>,
    /// Force a password change at the next login.
    pub must_reset: bool,
}

/// A partial user update; `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct UserPatch {
    /// Logins never change (R3): the stored login (after normalisation) is accepted and
    /// ignored, so clients can send whole objects; any other value is `InvalidField`.
    pub login: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: Option<Role>,
    pub status: Option<UserStatus>,
}

/// Manages user accounts. Methods taking an [`Actor`] check capabilities and scopes;
/// [`operator`](Self::operator) is the system path for the CLI.
pub struct UserService {
    store: Arc<dyn Store>,
}

impl UserService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Lists users by login (`manage_users` role, `users:read` scope).
    pub async fn list(
        &self,
        actor: &Actor,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<User>, CoreError> {
        require_manage_read(actor)?;
        Ok(self.store.read().users().list(limit, cursor).await?)
    }

    /// One user: anyone may read themselves; others need `manage_users` (and
    /// `users:read`).
    pub async fn get(&self, actor: &Actor, id: UserId) -> Result<User, CoreError> {
        if id != actor.user {
            require_manage_read(actor)?;
        }
        self.store
            .read()
            .users()
            .get(id)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Creates an active account (`manage_users`).
    pub async fn create(&self, actor: &Actor, account: NewAccount) -> Result<User, CoreError> {
        require(actor, Capability::ManageUsers)?;
        self.create_as(Some(actor.user), account).await
    }

    /// Changes e-mail, display name, role or status. Users may change their own e-mail
    /// and display name (with the `users:write` scope); everything else, and any change
    /// to another user, needs `manage_users`. Role and status never change on oneself: a
    /// self-patch carrying the current role or status ignores it (admin forms send whole
    /// objects), a different value is `Invalid`.
    pub async fn update(
        &self,
        actor: &Actor,
        id: UserId,
        patch: UserPatch,
    ) -> Result<User, CoreError> {
        let is_self = id == actor.user;
        if is_self {
            require_scope(actor, Some(Scope::write(Area::Users)))?;
        } else {
            require(actor, Capability::ManageUsers)?;
        }
        let login = patch.login.as_deref().map(normalize_login).transpose()?;
        let email = patch.email.as_deref().map(normalize_email).transpose()?;
        let display_name = patch
            .display_name
            .as_deref()
            .map(normalize_display_name)
            .transpose()?;
        let tx = self.store.begin().await?;
        let done = async {
            let old = locked(tx.as_ref(), id).await?;
            if login.as_ref().is_some_and(|l| *l != old.login) {
                return Err(CoreError::invalid_field("login", LOGIN_FIXED));
            }
            if is_self
                && (patch.role.is_some_and(|r| r != old.role)
                    || patch.status.is_some_and(|st| st != old.status))
            {
                return Err(CoreError::Invalid(OWN_ROLE.into()));
            }
            let mut new = old.clone();
            if let Some(email) = email {
                new.email = email;
            }
            if let Some(name) = display_name {
                new.display_name = name;
            }
            if let Some(role) = patch.role {
                new.role = role;
            }
            if let Some(status) = patch.status {
                new.status = status;
            }
            let mut changed = Vec::new();
            if new.email != old.email {
                changed.push("email");
            }
            if new.display_name != old.display_name {
                changed.push("display_name");
            }
            if new.role != old.role {
                changed.push("role");
            }
            if new.status != old.status {
                changed.push("status");
            }
            if changed.is_empty() {
                return Ok(old);
            }
            if is_active_admin(&old) && !is_active_admin(&new) {
                check_not_last_admin(tx.as_ref()).await?;
            }
            tx.users().update(&new).await.map_err(map_unique)?;
            let disabled = new.status == UserStatus::Disabled && old.status != new.status;
            if new.role != old.role || disabled {
                tx.sessions().delete_for_user(id, None).await?;
            }
            if disabled {
                tx.api_tokens().delete_for_user(id).await?;
            }
            if new.display_name != old.display_name || new.status != old.status {
                tx.events()
                    .append(&events::invalidate([events::user_key(id)]))
                    .await?;
            }
            let mut details = json!({ "changed": changed });
            if new.role != old.role {
                details["role"] = json!(new.role.as_str());
            }
            if new.status != old.status {
                details["status"] = json!(new.status.as_str());
            }
            audit(tx.as_ref(), Some(actor.user), "user.update", id, details).await?;
            Ok(new)
        }
        .await;
        finish(tx, done).await
    }

    /// Sets a password. For oneself it needs the right `current` password (else
    /// `Forbidden("current password")`) and clears `must_reset`; for another user it needs
    /// `manage_users` and sets `must_reset`. Every session of the user is deleted except
    /// the one whose `id_hash` is `keep_session`.
    ///
    /// A self change made by a request goes through
    /// [`AuthService::change_password`](crate::AuthService::change_password), which rate
    /// limits the current-password check (R11) first.
    pub async fn set_password(
        &self,
        actor: &Actor,
        id: UserId,
        current: Option<&str>,
        new: &str,
        keep_session: Option<&str>,
    ) -> Result<(), CoreError> {
        let is_self = id == actor.user;
        if is_self {
            require_scope(actor, Some(Scope::write(Area::Users)))?;
        } else {
            require(actor, Capability::ManageUsers)?;
        }
        password::check_policy(new)?;
        // The hash the current password was checked against, to detect a concurrent change.
        let checked = if is_self {
            let row = self
                .store
                .read()
                .users()
                .get(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            let current = current.ok_or(CoreError::Forbidden("current password"))?;
            match password::verify(current, row.password_hash.as_ref()).await {
                Verified::Match { .. } => row.password_hash,
                Verified::Mismatch => return Err(CoreError::Forbidden("current password")),
            }
        } else {
            None
        };
        let hashed = password::hash(new).await?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut user = locked(tx.as_ref(), id).await?;
            if is_self && user.password_hash != checked {
                return Err(CoreError::Forbidden("current password"));
            }
            user.password_hash = Some(hashed);
            user.must_reset = !is_self;
            tx.users().update(&user).await?;
            tx.sessions().delete_for_user(id, keep_session).await?;
            let details = json!({ "must_reset": user.must_reset, "self": is_self });
            audit(tx.as_ref(), Some(actor.user), "user.password", id, details).await
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes a user (`manage_users`; not oneself). Authored content and revisions move
    /// to `reassign_to` (an existing active user) in the same `Tx`; a user with content
    /// and no `reassign_to` is `Invalid("reassign the user's content first")`.
    ///
    /// The target and the heir are row-locked in ascending id order, so two concurrent
    /// deletes reassigning to each other serialize instead of deadlocking: the second
    /// one then finds its target or its heir gone (`NotFound` / `Invalid`).
    pub async fn delete(
        &self,
        actor: &Actor,
        id: UserId,
        reassign_to: Option<UserId>,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageUsers)?;
        if id == actor.user {
            return Err(CoreError::Invalid("cannot delete yourself".into()));
        }
        let tx = self.store.begin().await?;
        let done = async {
            if reassign_to == Some(id) {
                return Err(CoreError::Invalid(
                    "cannot reassign content to the deleted user".into(),
                ));
            }
            // Lock both rows in id order (R10) before any check.
            let (user, target) = match reassign_to {
                Some(heir) if heir < id => {
                    let target = tx.users().get_for_update(heir).await?;
                    (locked(tx.as_ref(), id).await?, target)
                }
                Some(heir) => {
                    let user = locked(tx.as_ref(), id).await?;
                    (user, tx.users().get_for_update(heir).await?)
                }
                None => (locked(tx.as_ref(), id).await?, None),
            };
            if is_active_admin(&user) {
                check_not_last_admin(tx.as_ref()).await?;
            }
            let mut keys = vec![events::user_key(id)];
            let mut moved = 0;
            if let Some(heir) = reassign_to {
                match target {
                    None => {
                        return Err(CoreError::Invalid("reassign target not found".into()));
                    }
                    Some(t) if t.status != UserStatus::Active => {
                        return Err(CoreError::Invalid(
                            "reassign target must be an active user".into(),
                        ));
                    }
                    Some(_) => {}
                }
                moved = tx.content().reassign_author(id, heir).await?;
                tx.revisions().reassign_author(id, heir).await?;
                keys.push(events::user_key(heir));
            }
            tx.users().delete(id).await.map_err(|e| match e {
                StoreError::ForeignKey { .. } => {
                    CoreError::Invalid("reassign the user's content first".into())
                }
                other => other.into(),
            })?;
            tx.events().append(&events::invalidate(keys)).await?;
            let details = json!({
                "login": user.login,
                "reassign_to": reassign_to.map(|h| h.to_string()),
                "reassigned_items": moved,
            });
            audit(tx.as_ref(), Some(actor.user), "user.delete", id, details).await
        }
        .await;
        finish(tx, done).await
    }

    /// The system path for the `ferrite user` CLI (P17): no actor, no capability checks.
    /// Only the binary's operator commands may call it; audit rows have no actor.
    pub fn operator(&self) -> OperatorUsers<'_> {
        OperatorUsers { users: self }
    }

    async fn create_as(
        &self,
        actor: Option<UserId>,
        account: NewAccount,
    ) -> Result<User, CoreError> {
        let login = normalize_login(&account.login)?;
        let email = normalize_email(&account.email)?;
        let display_name = normalize_display_name(&account.display_name)?;
        let password_hash = match &account.password {
            Some(pw) => {
                password::check_policy(pw).map_err(|e| match e {
                    CoreError::Invalid(msg) => CoreError::invalid_field("password", msg),
                    other => other,
                })?;
                Some(password::hash(pw).await?)
            }
            None => None,
        };
        let has_password = password_hash.is_some();
        let tx = self.store.begin().await?;
        let done = async {
            // Both unique indexes may fail at once, and the backends report different ones;
            // checking the login first gives the same message on both. The unique
            // constraints still decide races.
            if tx.users().get_by_login(&login).await?.is_some() {
                return Err(CoreError::invalid_field("login", LOGIN_TAKEN));
            }
            let user = tx
                .users()
                .insert(&NewUser {
                    id: UserId::new(),
                    login,
                    email,
                    display_name,
                    password_hash,
                    role: account.role,
                    status: UserStatus::Active,
                    must_reset: account.must_reset,
                })
                .await
                .map_err(map_unique)?;
            let details = json!({
                "login": user.login,
                "role": user.role.as_str(),
                "must_reset": user.must_reset,
                "password": has_password,
            });
            audit(tx.as_ref(), actor, "user.create", user.id, details).await?;
            Ok(user)
        }
        .await;
        finish(tx, done).await
    }
}

/// The CLI's system path ([`UserService::operator`]): acts without an actor.
pub struct OperatorUsers<'a> {
    users: &'a UserService,
}

impl OperatorUsers<'_> {
    /// Creates an account (same rules as [`UserService::create`]).
    pub async fn create(&self, account: NewAccount) -> Result<User, CoreError> {
        self.users.create_as(None, account).await
    }

    /// Sets the password of `login`, sets `must_reset` as given and deletes every session
    /// of the user. An unknown login is `NotFound`.
    pub async fn set_password(
        &self,
        login: &str,
        new: &str,
        must_reset: bool,
    ) -> Result<(), CoreError> {
        password::check_policy(new)?;
        let id = self.id_of(login).await?;
        let hashed = password::hash(new).await?;
        let tx = self.users.store.begin().await?;
        let done = async {
            let mut user = locked(tx.as_ref(), id).await?;
            user.password_hash = Some(hashed);
            user.must_reset = must_reset;
            tx.users().update(&user).await?;
            tx.sessions().delete_for_user(id, None).await?;
            let details = json!({ "must_reset": must_reset, "self": false });
            audit(tx.as_ref(), None, "user.password", id, details).await
        }
        .await;
        finish(tx, done).await
    }

    /// Turns TOTP off for `login` (clears the secret and the replay step), the recovery
    /// path for a lost authenticator (P9). An unknown login is `NotFound`.
    pub async fn disable_totp(&self, login: &str) -> Result<(), CoreError> {
        let id = self.id_of(login).await?;
        let tx = self.users.store.begin().await?;
        let done = async {
            let user = locked(tx.as_ref(), id).await?;
            tx.users().set_totp(user.id, None).await?;
            let details = json!({ "had_totp": user.totp_secret.is_some() });
            audit(tx.as_ref(), None, "user.totp_disable", id, details).await
        }
        .await;
        finish(tx, done).await
    }

    /// Lists users by login.
    pub async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<User>, CoreError> {
        Ok(self.users.store.read().users().list(limit, cursor).await?)
    }

    async fn id_of(&self, login: &str) -> Result<UserId, CoreError> {
        let login = normalize_login(login).map_err(|_| CoreError::NotFound)?;
        self.users
            .store
            .read()
            .users()
            .get_by_login(&login)
            .await?
            .map(|u| u.id)
            .ok_or(CoreError::NotFound)
    }
}

/// `manage_users` by role and `users:read` by scope: reading other accounts.
fn require_manage_read(actor: &Actor) -> Result<(), CoreError> {
    if !can(actor.role, Capability::ManageUsers) {
        return Err(CoreError::Forbidden(Capability::ManageUsers.as_str()));
    }
    require_scope(actor, Some(Scope::read(Area::Users)))
}

/// The user's row, locked for the rest of the `Tx`; `NotFound` if missing.
async fn locked(repos: &dyn Repos, id: UserId) -> Result<User, CoreError> {
    repos
        .users()
        .get_for_update(id)
        .await?
        .ok_or(CoreError::NotFound)
}

fn is_active_admin(user: &User) -> bool {
    user.role == Role::Admin && user.status == UserStatus::Active
}

/// `Invalid` unless another active admin remains once the current one stops being one.
async fn check_not_last_admin(repos: &dyn Repos) -> Result<(), CoreError> {
    if repos.users().count_active_admins().await? <= 1 {
        return Err(CoreError::Invalid(LAST_ADMIN.into()));
    }
    Ok(())
}

async fn audit(
    repos: &dyn Repos,
    actor: Option<UserId>,
    action: &str,
    target: UserId,
    details: Value,
) -> Result<(), CoreError> {
    repos
        .audit()
        .append(&NewAudit {
            actor_id: actor,
            action: action.to_owned(),
            target_kind: "user".to_owned(),
            target_id: Some(target.to_string()),
            details,
        })
        .await?;
    Ok(())
}

fn map_unique(e: StoreError) -> CoreError {
    match e {
        StoreError::UniqueViolation { constraint } if constraint == "users_login_uq" => {
            CoreError::invalid_field("login", LOGIN_TAKEN)
        }
        StoreError::UniqueViolation { constraint } if constraint == "users_email_uq" => {
            CoreError::invalid_field("email", EMAIL_TAKEN)
        }
        other => other.into(),
    }
}

/// Trims and ASCII-lowercases a login and checks `^[a-z0-9][a-z0-9._-]{0,59}$`.
pub fn normalize_login(login: &str) -> Result<String, CoreError> {
    let login = login.trim().to_ascii_lowercase();
    let mut chars = login.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
    if !first_ok || !rest_ok || login.len() > MAX_LOGIN {
        return Err(CoreError::invalid_field(
            "login",
            format!(
                "login must be 1 to {MAX_LOGIN} of a-z, 0-9, '.', '_', '-', starting with a letter or digit"
            ),
        ));
    }
    Ok(login)
}

/// Trims an e-mail address and checks its shape: one `@` with text on both sides, at
/// most 254 bytes, no whitespace or control characters. Case is kept (uniqueness uses
/// the store's ASCII `email_key`).
pub fn normalize_email(email: &str) -> Result<String, CoreError> {
    let email = email.trim();
    let shape_ok = match email.split_once('@') {
        Some((local, domain)) => !local.is_empty() && !domain.is_empty() && !domain.contains('@'),
        None => false,
    };
    if !shape_ok
        || email.len() > MAX_EMAIL
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(CoreError::invalid_field(
            "email",
            format!("e-mail must look like name@example.com (at most {MAX_EMAIL} bytes)"),
        ));
    }
    Ok(email.to_owned())
}

/// Trims a display name and checks it is 1..=100 characters without control characters.
pub fn normalize_display_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_DISPLAY_NAME || name.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "display_name",
            format!(
                "display name must be 1 to {MAX_DISPLAY_NAME} characters without control characters"
            ),
        ));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_rules() {
        assert_eq!(normalize_login("  Alice ").unwrap(), "alice");
        assert_eq!(normalize_login("0.a_b-c").unwrap(), "0.a_b-c");
        assert!(normalize_login(&"a".repeat(60)).is_ok());
        for bad in [
            "",
            " ",
            "a/b",
            "-a",
            ".a",
            "_a",
            "é",
            "a b",
            &"a".repeat(61),
        ] {
            assert!(normalize_login(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn email_rules() {
        assert_eq!(normalize_email(" A@B.c ").unwrap(), "A@B.c");
        assert!(normalize_email(&format!("{}@b.c", "a".repeat(250))).is_ok());
        for bad in [
            "a",
            "a@",
            "@b",
            "a@b@c",
            "a b@c",
            "a@b\u{0}",
            &format!("{}@b.c", "a".repeat(251)),
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn display_name_rules() {
        assert_eq!(normalize_display_name("  Ann  ").unwrap(), "Ann");
        assert!(normalize_display_name(&"é".repeat(100)).is_ok());
        for bad in ["", "  ", "a\nb", &"n".repeat(101)] {
            assert!(normalize_display_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unique_violations_name_the_field() {
        let login = map_unique(StoreError::UniqueViolation {
            constraint: "users_login_uq".into(),
        });
        assert!(matches!(
            login,
            CoreError::InvalidField { field: "login", message } if message == "login taken"
        ));
        let email = map_unique(StoreError::UniqueViolation {
            constraint: "users_email_uq".into(),
        });
        assert!(matches!(
            email,
            CoreError::InvalidField { field: "email", message } if message == "e-mail taken"
        ));
        assert!(matches!(
            map_unique(StoreError::UniqueViolation {
                constraint: "users_pkey".into()
            }),
            CoreError::Store(_)
        ));
    }
}
