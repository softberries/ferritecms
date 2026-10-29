//! Import users (P9): match an existing account, else create an author who must set a
//! password.

use ferrite_domain::{ImportMapKind, NewUser, Role, UserId, UserStatus};
use ferrite_store::Repos;

use super::ImportBatch;
use crate::{
    CoreError, MAX_LOGIN, normalize_display_name, normalize_email, normalize_login, slug,
    users::insert_account_in,
};

/// Logins tried for one slugified WP login (`base`, `base-2`, …) before giving up.
const LOGIN_ATTEMPTS: usize = 1_000;

/// A WXR `<wp:author>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportUser {
    pub wp_id: i64,
    pub login: String,
    pub email: String,
    pub display_name: String,
}

/// How [`ImportBatch::user`] found the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matched {
    /// The account an earlier run of this source mapped the WP author to (it still
    /// exists), whatever its e-mail and login are now.
    Mapped,
    /// An existing account with the same e-mail (case-insensitively).
    Email,
    /// An existing account with the same login (reported, P9).
    Login,
    /// A new account.
    Created,
}

/// The account an import user maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserOutcome {
    pub id: UserId,
    /// The account's login as stored (a new one's may differ from the WP login).
    pub login: String,
    pub how: Matched,
}

impl ImportBatch<'_> {
    /// Maps a WXR author to an account (P9): the account an earlier run mapped `wp_id` to
    /// (returned as it is, the map row unchanged), else an existing account with the same e-mail,
    /// else one with the same login (the WP login normalised as core does), else a new
    /// active author with no password who must set one, audited as `user.create` with
    /// the importing admin as actor and no `user.created` action (P6). The new login is
    /// the WP login when core accepts it, else its slug with `-N` on a clash; the display
    /// name falls back to the login. An e-mail core refuses matches nothing and makes the
    /// creation `InvalidField { field: "email" }`. The map row (`user`, `wp_id`) is
    /// written in every case.
    pub async fn user(&mut self, u: ImportUser) -> Result<UserOutcome, CoreError> {
        if let Some(mapped) = self.lookup(ImportMapKind::User, u.wp_id).await?
            && let Some(user) = self.tx.users().get(UserId::from_uuid(mapped.id)).await?
        {
            return Ok(UserOutcome {
                id: user.id,
                login: user.login,
                how: Matched::Mapped,
            });
        }
        let repos = self.tx.as_ref();
        let email = normalize_email(&u.email);
        let login = normalize_login(&u.login).ok();
        let found = match &email {
            Ok(email) => repos.users().get_by_email(email).await?,
            Err(_) => None,
        };
        let outcome = if let Some(found) = found {
            UserOutcome {
                id: found.id,
                login: found.login,
                how: Matched::Email,
            }
        } else if let Some(found) = match &login {
            Some(login) => repos.users().get_by_login(login).await?,
            None => None,
        } {
            UserOutcome {
                id: found.id,
                login: found.login,
                how: Matched::Login,
            }
        } else {
            let email = email?;
            let login = match login {
                Some(login) => login,
                None => free_login(repos, &u.login).await?,
            };
            let display_name =
                normalize_display_name(&u.display_name).unwrap_or_else(|_| login.clone());
            let user = insert_account_in(
                repos,
                Some(self.handle.actor.user),
                NewUser {
                    id: UserId::new(),
                    login,
                    email,
                    display_name,
                    password_hash: None,
                    role: Role::Author,
                    status: UserStatus::Active,
                    must_reset: true,
                },
            )
            .await?;
            self.items += 1;
            UserOutcome {
                id: user.id,
                login: user.login,
                how: Matched::Created,
            }
        };
        self.remember(ImportMapKind::User, u.wp_id, outcome.id.as_uuid(), "", 0)
            .await?;
        Ok(outcome)
    }
}

/// A free login derived from a WP login core refuses: its slug cut to [`MAX_LOGIN`],
/// then `-2`, `-3`, … while taken.
async fn free_login(repos: &dyn Repos, wp_login: &str) -> Result<String, CoreError> {
    let base = slug::slugify(wp_login);
    for n in 1..=LOGIN_ATTEMPTS {
        let suffix = if n == 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        let keep = MAX_LOGIN - suffix.len();
        let head = base.get(..keep).unwrap_or(&base).trim_end_matches('-');
        let candidate = normalize_login(&format!("{head}{suffix}"))?;
        if repos.users().get_by_login(&candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(CoreError::invalid_field(
        "login",
        format!("no free login for {wp_login:?}"),
    ))
}
