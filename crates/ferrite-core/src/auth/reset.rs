//! Password reset by e-mail (M2a-2 P19, 07 §2): a stateless, single-use sealed token
//! sent as a link, then a new password set with it.
//!
//! * **Requests do not tell accounts apart.** The rate limits come first (`reset:ip`
//!   5/min when the address is known, then `reset:user:<keyed id of the input>` 3/h);
//!   after them every input answers `Ok(())` the same way. Only an active account gets
//!   a mail, handed to the node's [`MailQueue`], which sends it from a background task:
//!   the caller never waits for SMTP, and a send failure is logged, not returned.
//!   Nothing about a request is audited (a row per request would show which inputs are
//!   accounts).
//! * **Tokens** are `seal(key, "password-reset", {user, exp, fp})`, valid one hour, with
//!   `fp` a keyed fingerprint of the password hash the account had when the token was
//!   made: any password change (this reset included) kills every outstanding token, so
//!   a token works once. Tokens are never stored, logged or audited; the link carries
//!   one in its fragment (`#token=`), which browsers send to no server and leave out of
//!   `Referer`.
//! * **Confirming** checks the password policy first, then the token (sealed, unexpired,
//!   active account, fingerprint), hashes the new password outside any `Tx`, and in one
//!   `Tx` on the locked row re-checks status and fingerprint, writes the hash, clears
//!   `must_reset`, ends every session and appends one `user.password_reset` audit row
//!   (F31: two concurrent confirms with one token cannot both pass). API tokens stay
//!   (R22) and so does TOTP: a reset never bypasses the second factor.

use std::net::IpAddr;
use std::time::Duration;

use ferrite_domain::{User, UserId, UserStatus};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::key::keyed_id;
use super::service::{AuthService, MAX_LOGIN_INPUT, ct_eq, plus};
use super::{limits, password};
use crate::{
    CoreError, UserService,
    mail::{MailQueue, Mailer, OutgoingMail, header_safe},
};

/// How long a reset link works.
const RESET_TTL: Duration = Duration::from_secs(3600);
/// Longest token looked at; ours are under 200 characters.
const MAX_TOKEN_INPUT: usize = 512;
/// Longest site title in the subject, in characters.
const MAX_TITLE_CHARS: usize = 100;
const RESET_PURPOSE: &str = "password-reset";
/// Subkey label for the password-hash fingerprint in reset tokens.
const RESET_PW_LABEL: &str = "ferrite:reset-pw:v1";
const INVALID_TOKEN: &str = "the reset link is invalid or has expired";

#[derive(Serialize, Deserialize)]
struct ResetClaims {
    user: UserId,
    /// Expiry, microseconds since the epoch.
    exp: i64,
    /// Keyed fingerprint of the password hash when the token was made.
    fp: String,
}

impl AuthService {
    /// Starts a password reset for a login or e-mail address (an input containing `@`
    /// is looked up as an e-mail). Limits first: `reset:ip:<hash>` 5 a minute when `ip`
    /// is known, then `reset:user:<keyed id of the trimmed, ASCII-lowercased input>` 3 an
    /// hour (exceeded → `RateLimited`). Then an active account gets a mail to its
    /// address with the link `<reset_base>#token=<token>` (valid 1 hour), queued on
    /// `queue`; an unknown or disabled account, or a mail the queue refuses, changes
    /// nothing visible (a refusal is logged). Always `Ok(())` unless limited or the
    /// store fails.
    ///
    /// `reset_base` is `<site.url>/admin/reset-password`; `site_title` names the site in
    /// the mail.
    pub async fn request_password_reset(
        &self,
        queue: &MailQueue,
        input: &str,
        ip: Option<IpAddr>,
        reset_base: &str,
        site_title: &str,
    ) -> Result<(), CoreError> {
        let store = self.store.as_ref();
        if let Some(ip) = ip {
            limits::hit(
                store,
                &limits::reset_ip(&self.key, ip),
                limits::RESET_PER_IP,
                limits::WINDOW,
            )
            .await?;
        }
        if input.len() > MAX_LOGIN_INPUT {
            return Ok(());
        }
        limits::hit(
            store,
            &limits::reset_user(&self.key, input),
            limits::RESET_PER_USER,
            limits::RESET_USER_WINDOW,
        )
        .await?;
        let Some(user) = self
            .find_user(input)
            .await?
            .filter(|u| u.status == UserStatus::Active && !u.email.is_empty())
        else {
            return Ok(());
        };
        let mail = match self.reset_mail(&user, reset_base, site_title) {
            Ok(mail) => mail,
            Err(e) => {
                tracing::error!(user = %user.id, error = %e, "password reset token not made");
                return Ok(());
            }
        };
        if let Err(e) = queue.send(mail).await {
            tracing::warn!(user = %user.id, error = %e, "password reset mail not queued");
        }
        Ok(())
    }

    /// Sets a new password with a token from [`request_password_reset`]
    /// (Self::request_password_reset). The new password must pass the policy
    /// (`InvalidField("password")`); the token must open, be unexpired, belong to an
    /// active account and match its current password hash, else
    /// `InvalidField("token", "the reset link is invalid or has expired")`. Then, in one
    /// `Tx` on the locked row (both re-checked there): the new hash, `must_reset`
    /// cleared, every session of the user deleted and one `user.password_reset` audit
    /// row without an actor. API tokens and TOTP are kept.
    ///
    /// `users` is the site's [`UserService`]; the write goes through its system path.
    pub async fn confirm_password_reset(
        &self,
        users: &UserService,
        token: &str,
        new: &str,
    ) -> Result<(), CoreError> {
        password::check_policy(new).map_err(|e| match e {
            CoreError::Invalid(msg) => CoreError::invalid_field("password", msg),
            other => other,
        })?;
        let invalid = || CoreError::invalid_field("token", INVALID_TOKEN);
        if token.len() > MAX_TOKEN_INPUT {
            return Err(invalid());
        }
        let claims: ResetClaims = self.open_json(RESET_PURPOSE, token).ok_or_else(invalid)?;
        if self.now().as_micros() >= claims.exp {
            return Err(invalid());
        }
        let usable = |user: &User| {
            let fp = self.reset_fingerprint(user);
            if user.status == UserStatus::Active && ct_eq(fp.as_bytes(), claims.fp.as_bytes()) {
                Ok(())
            } else {
                Err(invalid())
            }
        };
        // Checked before hashing too, so a used or stale link costs no password hash.
        let user = self
            .store
            .read()
            .users()
            .get(claims.user)
            .await?
            .ok_or_else(invalid)?;
        usable(&user)?;
        let hashed = password::hash(new).await?;
        users
            .write_password(
                claims.user,
                hashed,
                false,
                &usable,
                "user.password_reset",
                json!({}),
            )
            .await
            .map_err(|e| match e {
                // The account was deleted since the check above.
                CoreError::NotFound => invalid(),
                other => other,
            })
    }

    /// The reset mail for `user`, with a fresh token.
    fn reset_mail(
        &self,
        user: &User,
        reset_base: &str,
        site_title: &str,
    ) -> Result<OutgoingMail, CoreError> {
        let exp = plus(self.now(), RESET_TTL)?.as_micros();
        let token = self.seal_json(
            RESET_PURPOSE,
            &ResetClaims {
                user: user.id,
                exp,
                fp: self.reset_fingerprint(user),
            },
        )?;
        let title = mail_title(site_title);
        let subject = match &title {
            Some(t) => format!("[{t}] Password reset"),
            None => "Password reset".to_owned(),
        };
        let site = title.as_deref().unwrap_or("the site");
        let text = format!(
            "Someone asked to reset the password of the account \"{login}\" on {site}.\n\
             \n\
             To choose a new password, open this link within one hour:\n\
             \n\
             {reset_base}#token={token}\n\
             \n\
             The link works once. If you did not ask for this, ignore this mail: your \
             password stays the same.\n",
            login = user.login,
        );
        Ok(OutgoingMail {
            to: vec![user.email.clone()],
            bcc: Vec::new(),
            subject,
            text,
        })
    }

    /// Keyed fingerprint (32 hex) of the user's stored password hash (`""` without one).
    fn reset_fingerprint(&self, user: &User) -> String {
        let hash = user.password_hash.as_ref().map_or("", |h| h.expose());
        keyed_id(&self.key, RESET_PW_LABEL, hash.as_bytes())
    }
}

/// The site title for the mail: trimmed, at most [`MAX_TITLE_CHARS`] characters; `None`
/// when empty or not header-safe.
fn mail_title(site_title: &str) -> Option<String> {
    let title = site_title.trim();
    if title.is_empty() || !header_safe(title) {
        return None;
    }
    let cut: String = title.chars().take(MAX_TITLE_CHARS).collect();
    Some(cut.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_title_rules() {
        assert_eq!(mail_title(" My Blog ").as_deref(), Some("My Blog"));
        assert_eq!(mail_title("   "), None);
        assert_eq!(mail_title("Evil\r\nBcc: x@evil.test"), None);
        let long = "é".repeat(150);
        assert_eq!(mail_title(&long).unwrap().chars().count(), MAX_TITLE_CHARS);
    }
}
