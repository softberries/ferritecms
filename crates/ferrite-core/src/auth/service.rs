//! `AuthService` (07 §2, P5–P9): password login with an optional TOTP step, cookie
//! sessions, CSRF tokens, personal API tokens and the DB-backed rate limits around them.
//!
//! * No per-node cache: every request resolves its session or token from the database,
//!   so a revocation on any node applies to the next request everywhere.
//! * Session cookies and bearer tokens are 32 random bytes; only their SHA-256 hex is
//!   stored. Nothing here logs a token, secret, password or code.
//! * Failed logins look the same whatever failed (unknown login or e-mail, wrong
//!   password, no password, disabled account): `Unauthenticated`, after one full password
//!   verify in every case (P15).
//! * Rate limits are counted outside any `Tx` (R6), before the password is checked, so a
//!   limited client cannot learn whether a password is right (see [`super::limits`]).
//! * The 50-token cap is checked inside the creating `Tx` after locking the owner's row
//!   (`get_for_update`), so concurrent creates for one user queue and the cap holds (F31
//!   had accepted a soft cap; no concurrency test covers the lock). It is housekeeping,
//!   not a security boundary.
//! * The clock (session times, challenge and enrollment expiry, TOTP steps) is
//!   injectable for tests; rate-limit windows use the database clock.

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ferrite_domain::{
    ApiToken, ApiTokenId, NewAudit, SESSION_ABSOLUTE, SESSION_IDLE, SESSION_TOUCH_EVERY, Secret,
    Session, Timestamp, User, UserId, UserStatus,
};
use ferrite_store::{Repos, Store};
use hmac::Mac;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::key::{
    COMMENT_IP_LABEL, PLUGIN_IP_LABEL, SiteKey, hex, ip_hash, keyed_id, keyed_mac, random_bytes,
    sha256_hex,
};
use super::password::{self, Verified};
use super::seal::{open, seal};
use super::{limits, totp};
use crate::{
    Actor, CoreError, Scopes, UserService,
    actor::{Area, Capability, Scope, can, require, require_scope},
    normalize_login,
    tx::finish,
};

/// The service's clock.
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// Longest login or e-mail input accepted at login, in bytes (F7).
pub(super) const MAX_LOGIN_INPUT: usize = 254;
/// Longest bearer value looked up (ours are 47 characters; cookies must be exactly
/// [`SESSION_TOKEN_LEN`]).
const MAX_TOKEN_INPUT: usize = 256;
/// Length of a session token: 32 random bytes as base64url without padding.
const SESSION_TOKEN_LEN: usize = 43;
/// Longest stored user agent, in characters.
const MAX_USER_AGENT: usize = 256;
/// API tokens per user.
const MAX_TOKENS: u64 = 50;
/// Longest API token name, in characters.
const MAX_TOKEN_NAME: usize = 100;
/// Bearer token prefix.
const TOKEN_PREFIX: &str = "fer_";
/// A login challenge (password checked, TOTP pending) is valid this long.
const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);
/// A TOTP enrollment must be confirmed within this long.
const ENROLL_TTL: Duration = Duration::from_secs(10 * 60);
/// `last_used_at` of an API token is written at most this often.
const TOKEN_TOUCH_EVERY: Duration = Duration::from_secs(5 * 60);
/// Subkey label for the password-hash fingerprint in login challenges.
const CHALLENGE_PW_LABEL: &str = "ferrite:challenge-pw:v1";
/// The CSRF MAC's message prefix (the session's `id_hash` follows).
const CSRF_CONTEXT: &str = "ferrite-csrf:v1:";

const CHALLENGE_PURPOSE: &str = "login-challenge";
const ENROLL_PURPOSE: &str = "totp-enroll";

/// One login attempt.
#[derive(Clone, Copy)]
pub struct LoginAttempt<'a> {
    /// Login or e-mail address (an input containing `@` is looked up as an e-mail).
    pub login: &'a str,
    pub password: &'a str,
    pub ip: Option<IpAddr>,
    pub user_agent: Option<&'a str>,
}

impl fmt::Debug for LoginAttempt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginAttempt")
            .field("login", &self.login)
            .field("password", &"[redacted]")
            .field("ip", &self.ip)
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

/// What a correct password leads to. `Debug` redacts the challenge.
pub enum LoginOutcome {
    /// Logged in.
    Session(Box<IssuedSession>),
    /// The account has TOTP: send the code with this challenge to
    /// [`AuthService::login_totp`] within 5 minutes. The challenge proves the password
    /// was right; treat it like a credential.
    TotpRequired { challenge: String },
}

impl fmt::Debug for LoginOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(issued) => f.debug_tuple("Session").field(issued).finish(),
            Self::TotpRequired { .. } => f
                .debug_struct("TotpRequired")
                .field("challenge", &"[redacted]")
                .finish(),
        }
    }
}

/// A new session: `token` is the cookie value (shown once, never stored).
#[derive(Debug, Clone)]
pub struct IssuedSession {
    pub token: Secret,
    pub session: Session,
    pub user: User,
}

/// A request's resolved credentials.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub actor: Actor,
    pub user: User,
    pub via: Via,
}

/// The credential a request used.
#[derive(Debug, Clone)]
pub enum Via {
    Session(Session),
    Token(ApiTokenId),
}

/// A started TOTP enrollment: show `otpauth_url` (or `secret_base32`), then confirm a
/// code with `enrollment` via [`AuthService::totp_confirm`] within 10 minutes.
pub struct TotpSetup {
    pub secret_base32: String,
    pub otpauth_url: String,
    pub enrollment: String,
}

impl fmt::Debug for TotpSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TotpSetup([redacted])")
    }
}

#[derive(Serialize, Deserialize)]
struct Challenge {
    user: UserId,
    /// Expiry, microseconds since the epoch.
    exp: i64,
    /// Keyed fingerprint of the password hash the password was checked against: a
    /// password change or reset invalidates outstanding challenges.
    pw: String,
}

#[derive(Serialize, Deserialize)]
struct Enrollment {
    user: UserId,
    /// The TOTP secret, base64url.
    secret: String,
    exp: i64,
}

/// Logins, sessions, API tokens and TOTP.
pub struct AuthService {
    pub(super) store: Arc<dyn Store>,
    pub(super) key: SiteKey,
    clock: Clock,
    issuer: String,
}

impl AuthService {
    pub fn new(store: Arc<dyn Store>, key: SiteKey) -> Self {
        Self {
            store,
            key,
            clock: Arc::new(Timestamp::now),
            issuer: totp::DEFAULT_ISSUER.to_owned(),
        }
    }

    /// Replaces the clock (tests).
    pub fn with_clock(self, clock: Clock) -> Self {
        Self { clock, ..self }
    }

    /// The issuer shown in authenticator apps, normally the site title: `:` is removed
    /// and an empty result falls back to `FerriteCMS` (F11). Fixed for the service's
    /// lifetime.
    pub fn with_issuer_name(self, name: String) -> Self {
        Self {
            issuer: totp::issuer_name(&name),
            ..self
        }
    }

    /// The keyed hash of a full client address, as stored on sessions.
    pub fn ip_hash(&self, ip: IpAddr) -> String {
        ip_hash(&self.key, ip)
    }

    /// The client-address hash a plugin route sees (`route-request.client-ip-hash`,
    /// M2a-2 P15): the first 32 hex characters of HMAC-SHA256 under the
    /// [`PLUGIN_IP_LABEL`] subkey over `plugin`, a zero byte and the canonical address,
    /// so two plugins cannot join a visitor across each other, and neither can join it
    /// with the session hash ([`ip_hash`](Self::ip_hash)).
    pub fn plugin_ip_hash(&self, plugin: &str, ip: IpAddr) -> String {
        let mut msg = Vec::with_capacity(plugin.len() + 48);
        msg.extend_from_slice(plugin.as_bytes());
        msg.push(0);
        msg.extend_from_slice(ip.to_canonical().to_string().as_bytes());
        keyed_id(&self.key, PLUGIN_IP_LABEL, &msg)
    }

    /// The client-address hash stored on a comment (M2a-2b P13): the first 32 hex
    /// characters of HMAC-SHA256 under the [`COMMENT_IP_LABEL`] subkey over the
    /// canonical address. Moderators see it to tell commenters apart; it cannot be
    /// reversed without the site key, nor joined with the session or plugin hashes.
    pub fn comment_ip_hash(&self, ip: IpAddr) -> String {
        keyed_id(
            &self.key,
            COMMENT_IP_LABEL,
            ip.to_canonical().to_string().as_bytes(),
        )
    }

    /// Counts one hit (outside any `Tx`, R6) on bucket `<scope>:ip:<limit hash>` (IPv6
    /// per /64; `<scope>:ip:none` without an address) in fixed windows of `window`
    /// shared by every node, and refuses it with `RateLimited` (`Retry-After` clamped to
    /// 1 s..=`window`) once the window's count is over `max`. `scope` is the caller's
    /// bucket prefix and must not hold caller text (M2a-1 F7); with the address part it
    /// must fit [`MAX_RATE_BUCKET`](ferrite_store::MAX_RATE_BUCKET), else `Store`
    /// (`Invalid`).
    pub async fn hit_limit(
        &self,
        scope: &str,
        ip: Option<IpAddr>,
        max: u32,
        window: Duration,
    ) -> Result<(), CoreError> {
        let bucket = limits::scoped(&self.key, scope, ip);
        limits::hit(self.store.as_ref(), &bucket, max, window).await
    }

    // ---- login ---------------------------------------------------------------------

    /// Checks a login or e-mail and password. Rate limits first (`login:ip:<hash>` 10/min
    /// when the IP is known, then `login:user:<keyed id>` 5/min; exceeded →
    /// `RateLimited`), then the lookup and one password verify. Any failure is
    /// `Unauthenticated`. On success the hash is upgraded when its parameters are old and
    /// `last_login_at` is set; an account with TOTP gets a challenge, others a session.
    pub async fn login(&self, attempt: LoginAttempt<'_>) -> Result<LoginOutcome, CoreError> {
        let store = self.store.as_ref();
        if let Some(ip) = attempt.ip {
            limits::hit(
                store,
                &limits::login_ip(&self.key, ip),
                limits::LOGIN_PER_IP,
                limits::WINDOW,
            )
            .await?;
        }
        if attempt.login.len() > MAX_LOGIN_INPUT {
            return Err(CoreError::Unauthenticated);
        }
        limits::hit(
            store,
            &limits::login_user(&self.key, attempt.login),
            limits::LOGIN_PER_USER,
            limits::WINDOW,
        )
        .await?;

        let user = self.find_user(attempt.login).await?;
        // Oversized passwords cannot match (policy) but still cost one verify.
        let candidate = if attempt.password.len() > password::MAX_BYTES {
            ""
        } else {
            attempt.password
        };
        let stored = user
            .as_ref()
            .filter(|_| candidate.len() == attempt.password.len())
            .and_then(|u| u.password_hash.as_ref());
        let verified = password::verify(candidate, stored).await;
        let (user, rehash) = match (user, verified) {
            (Some(user), Verified::Match { rehash }) if user.status == UserStatus::Active => {
                (user, rehash)
            }
            _ => return Err(CoreError::Unauthenticated),
        };
        let new_hash = if rehash {
            match password::hash(attempt.password).await {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::warn!(user = %user.id, error = %e, "password rehash failed");
                    None
                }
            }
        } else {
            None
        };
        // A failed or abandoned second factor is not a login (R12): `last_login_at` is
        // written here only without TOTP, else by `login_totp`.
        let full = user.totp_secret.is_none();
        let user = if full || new_hash.is_some() {
            self.record_login(&user, new_hash, full).await?
        } else {
            user
        };

        if !full {
            let exp = plus(self.now(), CHALLENGE_TTL)?.as_micros();
            let challenge = self.seal_json(
                CHALLENGE_PURPOSE,
                &Challenge {
                    user: user.id,
                    exp,
                    pw: self.password_fingerprint(&user),
                },
            )?;
            return Ok(LoginOutcome::TotpRequired { challenge });
        }
        let issued = self
            .issue_session(&user, attempt.ip, attempt.user_agent)
            .await?;
        Ok(LoginOutcome::Session(Box::new(issued)))
    }

    /// The second login step: the challenge from [`login`](Self::login) must be valid,
    /// unexpired and made against the current password hash; `totp:user:<id>` allows 5
    /// codes a minute; the code must match (± one step) and its step must be newer than
    /// the last one accepted (no replay). Then `last_login_at` is set under the row lock
    /// (status and password hash re-checked). Every failure but the rate limit is
    /// `Unauthenticated`. A challenge may be used again within its 5 minutes; each use
    /// needs a fresh code.
    pub async fn login_totp(
        &self,
        challenge: &str,
        code: &str,
        ip: Option<IpAddr>,
        user_agent: Option<&str>,
    ) -> Result<IssuedSession, CoreError> {
        let challenge: Challenge = self
            .open_json(CHALLENGE_PURPOSE, challenge)
            .ok_or(CoreError::Unauthenticated)?;
        let now = self.now();
        if now.as_micros() >= challenge.exp {
            return Err(CoreError::Unauthenticated);
        }
        limits::hit(
            self.store.as_ref(),
            &limits::totp_user(challenge.user),
            limits::TOTP_PER_USER,
            limits::WINDOW,
        )
        .await?;
        let user = self
            .store
            .read()
            .users()
            .get(challenge.user)
            .await?
            .filter(|u| u.status == UserStatus::Active)
            .ok_or(CoreError::Unauthenticated)?;
        if !ct_eq(
            self.password_fingerprint(&user).as_bytes(),
            challenge.pw.as_bytes(),
        ) {
            return Err(CoreError::Unauthenticated);
        }
        let secret = self.totp_secret_of(&user)?;
        let totp = totp::build(&secret, &self.issuer, &user.login)?;
        let step = totp::check(&totp, code, now).ok_or(CoreError::Unauthenticated)?;
        if !self
            .store
            .read()
            .users()
            .advance_totp_step(user.id, step)
            .await?
        {
            return Err(CoreError::Unauthenticated);
        }
        let user = self.record_login(&user, None, true).await?;
        self.issue_session(&user, ip, user_agent).await
    }

    /// Issues a session for an active user without checking credentials: the caller has
    /// (login, TOTP login, a password change that just re-checked the password, F12).
    /// The session lasts 30 days at most and 7 idle days.
    pub async fn issue_session(
        &self,
        user: &User,
        ip: Option<IpAddr>,
        user_agent: Option<&str>,
    ) -> Result<IssuedSession, CoreError> {
        if user.status != UserStatus::Active {
            return Err(CoreError::Unauthenticated);
        }
        let token = URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
        let csrf_secret = Secret::new(hex(&random_bytes::<32>()?));
        let now = self.now();
        let session = Session {
            id_hash: sha256_hex(token.as_bytes()),
            user_id: user.id,
            csrf_secret,
            created_at: now,
            last_seen_at: now,
            expires_at: plus(now, SESSION_ABSOLUTE)?,
            ip_hash: ip.map(|ip| self.ip_hash(ip)),
            user_agent: user_agent.map(clean_user_agent),
        };
        self.store.read().sessions().insert(&session).await?;
        Ok(IssuedSession {
            token: Secret::new(token),
            session,
            user: user.clone(),
        })
    }

    // ---- sessions ------------------------------------------------------------------

    /// The session behind a cookie value. Unknown, past `expires_at`, idle for more than
    /// 7 days, or of a missing or disabled user → `None`; expired rows are deleted on
    /// sight. `last_seen_at` is written when it is more than 5 minutes old.
    ///
    /// A value that cannot be one of our tokens (not 43 base64url characters) is `None`
    /// without a store read, so junk cookies cost nothing on cached public pages.
    pub async fn resolve_session(&self, token: &str) -> Result<Option<Authenticated>, CoreError> {
        if !is_session_token_shape(token) {
            return Ok(None);
        }
        let repos = self.store.read();
        let id_hash = sha256_hex(token.as_bytes());
        let Some(mut session) = repos.sessions().get(&id_hash).await? else {
            return Ok(None);
        };
        let now = self.now();
        if now >= session.expires_at || older_than(session.last_seen_at, now, SESSION_IDLE) {
            repos.sessions().delete(&id_hash).await?;
            return Ok(None);
        }
        let Some(user) = active_user(repos, session.user_id).await? else {
            return Ok(None);
        };
        if older_than(session.last_seen_at, now, SESSION_TOUCH_EVERY) {
            repos.sessions().touch(&id_hash, now).await?;
            session.last_seen_at = now;
        }
        Ok(Some(Authenticated {
            actor: Actor::new(user.id, user.role),
            user,
            via: Via::Session(session),
        }))
    }

    /// Deletes the session behind a cookie value (unknown: no-op).
    pub async fn logout(&self, token: &str) -> Result<(), CoreError> {
        if !is_session_token_shape(token) {
            return Ok(());
        }
        let id_hash = sha256_hex(token.as_bytes());
        Ok(self.store.read().sessions().delete(&id_hash).await?)
    }

    /// The session's CSRF token: base64url(HMAC-SHA256(key = `csrf_secret`,
    /// `"ferrite-csrf:v1:" + id_hash`)).
    pub fn csrf_token(&self, session: &Session) -> String {
        URL_SAFE_NO_PAD.encode(csrf_mac(session).finalize().into_bytes())
    }

    /// Whether `header` is the session's CSRF token (constant-time comparison).
    pub fn check_csrf(&self, session: &Session, header: &str) -> bool {
        match URL_SAFE_NO_PAD.decode(header.trim()) {
            Ok(tag) => csrf_mac(session).verify_slice(&tag).is_ok(),
            Err(_) => false,
        }
    }

    /// A user changes their own password (`POST /auth/password`). The current-password
    /// check is a password guess like a login, so it first counts on the login's
    /// `login:user` bucket (5/min, exceeded → `RateLimited`, R11); then
    /// [`UserService::set_password`] checks it, stores the new hash, clears `must_reset`
    /// and deletes **every** session of the user. A request made with a session gets a
    /// new one (F12: issued only after the check passed), returned here for the caller's
    /// cookie; a token request gets `None` and its token stays valid.
    ///
    /// The `users:write` scope and the new-password policy are checked before the bucket
    /// is hit, so a refused request of that kind uses up no slot.
    ///
    /// This is the only path for a self change made by a request: calling
    /// `UserService::set_password` directly for oneself skips the rate limit.
    pub async fn change_password(
        &self,
        users: &UserService,
        who: &Authenticated,
        current: &str,
        new: &str,
        ip: Option<IpAddr>,
        user_agent: Option<&str>,
    ) -> Result<Option<IssuedSession>, CoreError> {
        // The cheap refusals first, so only a real current-password check takes a slot.
        require_scope(&who.actor, Some(Scope::write(Area::Users)))?;
        password::check_policy(new)?;
        self.password_attempt(&who.user.login).await?;
        users
            .set_password(&who.actor, who.user.id, Some(current), new, None)
            .await?;
        if !matches!(who.via, Via::Session(_)) {
            return Ok(None);
        }
        let user = active_user(self.store.read(), who.user.id)
            .await?
            .ok_or(CoreError::Unauthenticated)?;
        self.issue_session(&user, ip, user_agent).await.map(Some)
    }

    // ---- API tokens ----------------------------------------------------------------

    /// Creates an API token for `user`: oneself with the `users:write` scope, anyone with
    /// `manage_users`. `scopes` must be non-empty and within the actor's own scopes, the
    /// name 1..=100 characters (trimmed), `expires_at` in the future, and the user may
    /// hold at most 50 tokens (`Invalid`). Returns the bearer value (shown once).
    pub async fn create_token(
        &self,
        actor: &Actor,
        user: UserId,
        name: &str,
        scopes: Scopes,
        expires_at: Option<Timestamp>,
    ) -> Result<(Secret, ApiToken), CoreError> {
        require_token_write(actor, user)?;
        for name in scopes.to_strings() {
            let scope = Scope::parse(&name)?;
            if !actor.scopes.allows(scope) {
                return Err(CoreError::Forbidden(scope.as_str()));
            }
        }
        let name = name.trim();
        let len = name.chars().count();
        if len == 0 || len > MAX_TOKEN_NAME || name.chars().any(char::is_control) {
            return Err(CoreError::Invalid(format!(
                "token name must be 1 to {MAX_TOKEN_NAME} characters without control characters"
            )));
        }
        if scopes == Scopes::NONE {
            return Err(CoreError::Invalid(
                "a token needs at least one scope".into(),
            ));
        }
        let now = self.now();
        if expires_at.is_some_and(|at| at <= now) {
            return Err(CoreError::Invalid(
                "expires_at must be in the future".into(),
            ));
        }
        let bearer = format!(
            "{TOKEN_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(random_bytes::<32>()?)
        );
        let token = ApiToken {
            id: ApiTokenId::new(),
            user_id: user,
            name: name.to_owned(),
            token_hash: sha256_hex(bearer.as_bytes()),
            scopes: scopes.to_strings(),
            created_at: now,
            last_used_at: None,
            expires_at,
        };
        let tx = self.store.begin().await?;
        let done = async {
            tx.users()
                .get_for_update(user)
                .await?
                .ok_or(CoreError::NotFound)?;
            if tx.api_tokens().count_for_user(user).await? >= MAX_TOKENS {
                return Err(CoreError::Invalid(format!(
                    "a user can hold at most {MAX_TOKENS} API tokens"
                )));
            }
            tx.api_tokens().insert(&token).await?;
            let details = json!({
                "user": user.to_string(),
                "name": token.name,
                "scopes": token.scopes,
                "expires_at": token.expires_at.map(|t| t.to_rfc3339()),
            });
            token_audit(tx.as_ref(), actor, "token.create", token.id, details).await
        }
        .await;
        finish(tx, done).await?;
        Ok((Secret::new(bearer), token))
    }

    /// The user's API tokens, oldest first: oneself with `users:read`, others with
    /// `manage_users` and `users:read`.
    pub async fn list_tokens(
        &self,
        actor: &Actor,
        user: UserId,
    ) -> Result<Vec<ApiToken>, CoreError> {
        if user == actor.user {
            require_scope(actor, Some(Scope::read(Area::Users)))?;
        } else {
            if !can(actor.role, Capability::ManageUsers) {
                return Err(CoreError::Forbidden(Capability::ManageUsers.as_str()));
            }
            require_scope(actor, Some(Scope::read(Area::Users)))?;
        }
        Ok(self.store.read().api_tokens().list_for_user(user).await?)
    }

    /// Deletes one of the user's tokens (same rights as [`create_token`](Self::create_token)
    /// minus the scope check); a token the user does not hold is `NotFound`.
    pub async fn revoke_token(
        &self,
        actor: &Actor,
        user: UserId,
        id: ApiTokenId,
    ) -> Result<(), CoreError> {
        require_token_write(actor, user)?;
        let tx = self.store.begin().await?;
        let done = async {
            if !tx.api_tokens().delete(user, id).await? {
                return Err(CoreError::NotFound);
            }
            let details = json!({ "user": user.to_string() });
            token_audit(tx.as_ref(), actor, "token.revoke", id, details).await
        }
        .await;
        finish(tx, done).await
    }

    /// The token behind a bearer value. Unknown or expired: one hit on `token:ip:<hash>`
    /// (10/min when the IP is known; exceeded → `RateLimited`) and `None`. A missing or
    /// disabled owner → `None`. The actor has the owner's current role narrowed to the
    /// token's scopes; `last_used_at` is written at most every 5 minutes.
    pub async fn resolve_token(
        &self,
        bearer: &str,
        ip: Option<IpAddr>,
    ) -> Result<Option<Authenticated>, CoreError> {
        let repos = self.store.read();
        let now = self.now();
        let found = if bearer.starts_with(TOKEN_PREFIX) && bearer.len() <= MAX_TOKEN_INPUT {
            repos
                .api_tokens()
                .get_by_hash(&sha256_hex(bearer.as_bytes()))
                .await?
        } else {
            None
        };
        let Some(token) = found.filter(|t| t.expires_at.is_none_or(|at| now < at)) else {
            if let Some(ip) = ip {
                limits::hit(
                    self.store.as_ref(),
                    &limits::token_ip(&self.key, ip),
                    limits::BAD_TOKEN_PER_IP,
                    limits::WINDOW,
                )
                .await?;
            }
            return Ok(None);
        };
        let Some(user) = active_user(repos, token.user_id).await? else {
            return Ok(None);
        };
        let scopes = match Scopes::from_strs(&token.scopes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(token = %token.id, error = %e, "stored token scopes unreadable");
                return Ok(None);
            }
        };
        if token
            .last_used_at
            .is_none_or(|at| older_than(at, now, TOKEN_TOUCH_EVERY))
        {
            repos.api_tokens().touch(token.id, now).await?;
        }
        Ok(Some(Authenticated {
            actor: Actor::new(user.id, user.role).with_scopes(scopes),
            user,
            via: Via::Token(token.id),
        }))
    }

    // ---- TOTP ----------------------------------------------------------------------

    /// Starts TOTP enrollment for the actor (`users:write`). Refused with
    /// `Invalid("disable TOTP first")` while TOTP is on, so a working factor is never
    /// silently replaced (F11). Nothing is stored until [`totp_confirm`](Self::totp_confirm).
    pub async fn totp_setup(&self, actor: &Actor) -> Result<TotpSetup, CoreError> {
        require_scope(actor, Some(Scope::write(Area::Users)))?;
        let user = self
            .store
            .read()
            .users()
            .get(actor.user)
            .await?
            .ok_or(CoreError::NotFound)?;
        if user.totp_secret.is_some() {
            return Err(CoreError::Invalid("disable TOTP first".into()));
        }
        let secret = random_bytes::<{ totp::SECRET_LEN }>()?;
        let otp = totp::build(&secret, &self.issuer, &user.login)?;
        let otpauth_url = otp.to_url().map_err(|e| {
            tracing::error!(error = %e, "otpauth URL refused");
            CoreError::Invalid("TOTP parameters refused".into())
        })?;
        let exp = plus(self.now(), ENROLL_TTL)?.as_micros();
        let enrollment = self.seal_json(
            ENROLL_PURPOSE,
            &Enrollment {
                user: user.id,
                secret: URL_SAFE_NO_PAD.encode(secret),
                exp,
            },
        )?;
        Ok(TotpSetup {
            secret_base32: otp.secret().to_base32(),
            otpauth_url,
            enrollment,
        })
    }

    /// Finishes enrollment with a code from the authenticator and the account's current
    /// password: the secret is stored sealed under `totp-secret:<user id>`, the code's
    /// step is recorded (it cannot log in again) and `totp.enable` is audited.
    ///
    /// * A bad or expired enrollment, or someone else's, is `Invalid` (no slot used).
    /// * The password re-check counts on the login's `login:user` bucket (5/min, as
    ///   R11), so a stolen session cannot enroll an attacker's authenticator without
    ///   the password; wrong → `Forbidden("current password")`.
    /// * Then each checked code counts on `totp:user:<id>` (5/min, shared with
    ///   `login_totp`); a wrong code is `Invalid`. Either limit exceeded →
    ///   `RateLimited`.
    /// * On success the user's other sessions are deleted in the same `Tx` (R26): a user
    ///   who enrolls because a session may be stolen ends it. `keep_session` is the
    ///   `id_hash` of the caller's session (`None` through an API token: every session
    ///   ends). API tokens are kept.
    pub async fn totp_confirm(
        &self,
        actor: &Actor,
        keep_session: Option<&str>,
        enrollment: &str,
        code: &str,
        password: &str,
    ) -> Result<(), CoreError> {
        require_scope(actor, Some(Scope::write(Area::Users)))?;
        let invalid = || CoreError::Invalid("enrollment is not valid".into());
        let enrollment: Enrollment = self
            .open_json(ENROLL_PURPOSE, enrollment)
            .ok_or_else(invalid)?;
        if enrollment.user != actor.user {
            return Err(invalid());
        }
        let now = self.now();
        if now.as_micros() >= enrollment.exp {
            return Err(CoreError::Invalid("enrollment expired".into()));
        }
        let secret = URL_SAFE_NO_PAD
            .decode(&enrollment.secret)
            .map_err(|_| invalid())?;
        let user = self
            .store
            .read()
            .users()
            .get(actor.user)
            .await?
            .ok_or(CoreError::NotFound)?;
        self.password_attempt(&user.login).await?;
        if password::verify(password, user.password_hash.as_ref()).await == Verified::Mismatch {
            return Err(CoreError::Forbidden("current password"));
        }
        limits::hit(
            self.store.as_ref(),
            &limits::totp_user(user.id),
            limits::TOTP_PER_USER,
            limits::WINDOW,
        )
        .await?;
        let otp = totp::build(&secret, &self.issuer, &user.login)?;
        let wrong = || CoreError::Invalid("wrong code".into());
        let step = totp::check(&otp, code, now).ok_or_else(wrong)?;
        let sealed = Secret::new(seal(&self.key, &secret_purpose(user.id), &secret)?);
        let tx = self.store.begin().await?;
        let done = async {
            let locked = tx
                .users()
                .get_for_update(user.id)
                .await?
                .ok_or(CoreError::NotFound)?;
            if locked.password_hash != user.password_hash {
                return Err(CoreError::Forbidden("current password"));
            }
            if locked.totp_secret.is_some() {
                return Err(CoreError::Invalid("disable TOTP first".into()));
            }
            tx.users().set_totp(user.id, Some(&sealed)).await?;
            if !tx.users().advance_totp_step(user.id, step).await? {
                return Err(wrong());
            }
            tx.sessions().delete_for_user(user.id, keep_session).await?;
            user_audit(tx.as_ref(), actor, "totp.enable", user.id).await
        }
        .await;
        finish(tx, done).await
    }

    /// Turns TOTP off for the actor (`users:write`) after re-checking the password
    /// (wrong → `Forbidden("current password")`); audits `totp.disable`. Not enabled →
    /// `Invalid`. The re-check counts on the login's `login:user` bucket (5/min,
    /// exceeded → `RateLimited`) before any password work.
    pub async fn totp_disable(&self, actor: &Actor, password: &str) -> Result<(), CoreError> {
        require_scope(actor, Some(Scope::write(Area::Users)))?;
        let user = self
            .store
            .read()
            .users()
            .get(actor.user)
            .await?
            .ok_or(CoreError::NotFound)?;
        if user.totp_secret.is_none() {
            return Err(CoreError::Invalid("TOTP is not enabled".into()));
        }
        // The re-check is a password guess like a login: same per-login bucket (R12).
        self.password_attempt(&user.login).await?;
        if password::verify(password, user.password_hash.as_ref()).await == Verified::Mismatch {
            return Err(CoreError::Forbidden("current password"));
        }
        let tx = self.store.begin().await?;
        let done = async {
            let locked = tx
                .users()
                .get_for_update(user.id)
                .await?
                .ok_or(CoreError::NotFound)?;
            if locked.password_hash != user.password_hash {
                return Err(CoreError::Forbidden("current password"));
            }
            tx.users().set_totp(user.id, None).await?;
            user_audit(tx.as_ref(), actor, "totp.disable", user.id).await
        }
        .await;
        finish(tx, done).await
    }

    // ---- helpers -------------------------------------------------------------------

    /// Counts one password re-check of `login` on its login bucket (R11): a re-check is
    /// a guess like a login.
    async fn password_attempt(&self, login: &str) -> Result<(), CoreError> {
        limits::hit(
            self.store.as_ref(),
            &limits::login_user(&self.key, login),
            limits::LOGIN_PER_USER,
            limits::WINDOW,
        )
        .await
    }

    pub(super) fn now(&self) -> Timestamp {
        (self.clock)()
    }

    /// The account a login input names: an e-mail when it contains `@`, else a login.
    /// Inputs that cannot name an account are `None` (never an error).
    pub(super) async fn find_user(&self, input: &str) -> Result<Option<User>, CoreError> {
        let input = input.trim();
        if input.is_empty() || input.contains('\0') {
            return Ok(None);
        }
        let users = self.store.read().users();
        if input.contains('@') {
            return Ok(users.get_by_email(input).await?);
        }
        match normalize_login(input) {
            Ok(login) => Ok(users.get_by_login(&login).await?),
            Err(_) => Ok(None),
        }
    }

    /// Login bookkeeping in one short `Tx` on the locked row: an upgraded hash and, when
    /// `logged_in`, `last_login_at`. A password, status or deletion change since `user`
    /// was read makes the login fail.
    async fn record_login(
        &self,
        user: &User,
        new_hash: Option<Secret>,
        logged_in: bool,
    ) -> Result<User, CoreError> {
        let now = self.now();
        let tx = self.store.begin().await?;
        let done = async {
            let mut row = tx
                .users()
                .get_for_update(user.id)
                .await?
                .ok_or(CoreError::Unauthenticated)?;
            if row.status != UserStatus::Active || row.password_hash != user.password_hash {
                return Err(CoreError::Unauthenticated);
            }
            if logged_in {
                row.last_login_at = Some(now);
            }
            if let Some(hash) = new_hash {
                row.password_hash = Some(hash);
            }
            tx.users().update(&row).await?;
            Ok(row)
        }
        .await;
        finish(tx, done).await
    }

    /// Keyed fingerprint of the user's stored password hash (login challenges).
    fn password_fingerprint(&self, user: &User) -> String {
        let hash = user.password_hash.as_ref().map_or("", |h| h.expose());
        keyed_id(&self.key, CHALLENGE_PW_LABEL, hash.as_bytes())
    }

    fn totp_secret_of(&self, user: &User) -> Result<Vec<u8>, CoreError> {
        let sealed = user
            .totp_secret
            .as_ref()
            .ok_or(CoreError::Unauthenticated)?;
        open(&self.key, &secret_purpose(user.id), sealed.expose()).map_err(|_| {
            tracing::error!(user = %user.id, "stored TOTP secret does not open (wrong site key?)");
            CoreError::Unauthenticated
        })
    }

    pub(super) fn seal_json<T: Serialize>(
        &self,
        purpose: &str,
        value: &T,
    ) -> Result<String, CoreError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| CoreError::Invalid(format!("cannot encode {purpose}: {e}")))?;
        seal(&self.key, purpose, &bytes)
    }

    pub(super) fn open_json<T: DeserializeOwned>(&self, purpose: &str, sealed: &str) -> Option<T> {
        let bytes = open(&self.key, purpose, sealed).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Self with the `users:write` scope, others with `manage_users`.
fn require_token_write(actor: &Actor, user: UserId) -> Result<(), CoreError> {
    if user == actor.user {
        require_scope(actor, Some(Scope::write(Area::Users)))
    } else {
        require(actor, Capability::ManageUsers)
    }
}

async fn active_user(repos: &dyn Repos, id: UserId) -> Result<Option<User>, CoreError> {
    Ok(repos
        .users()
        .get(id)
        .await?
        .filter(|u| u.status == UserStatus::Active))
}

fn secret_purpose(user: UserId) -> String {
    format!("totp-secret:{user}")
}

fn csrf_mac(session: &Session) -> hmac::Hmac<sha2::Sha256> {
    let mut mac = keyed_mac(session.csrf_secret.expose().as_bytes());
    mac.update(CSRF_CONTEXT.as_bytes());
    mac.update(session.id_hash.as_bytes());
    mac
}

/// At most 256 characters (cut on a character boundary), without NUL.
fn clean_user_agent(ua: &str) -> String {
    ua.chars()
        .filter(|&c| c != '\0')
        .take(MAX_USER_AGENT)
        .collect()
}

fn micros(d: Duration) -> i64 {
    i64::try_from(d.as_micros()).unwrap_or(i64::MAX)
}

pub(super) fn plus(at: Timestamp, d: Duration) -> Result<Timestamp, CoreError> {
    Timestamp::from_micros(at.as_micros().saturating_add(micros(d)))
        .map_err(|_| CoreError::Invalid("timestamp out of range".into()))
}

/// Whether more than `d` passed from `then` to `now`.
fn older_than(then: Timestamp, now: Timestamp, d: Duration) -> bool {
    now.as_micros().saturating_sub(then.as_micros()) > micros(d)
}

async fn token_audit(
    repos: &dyn Repos,
    actor: &Actor,
    action: &str,
    id: ApiTokenId,
    details: Value,
) -> Result<(), CoreError> {
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor.user),
            action: action.to_owned(),
            target_kind: "api_token".to_owned(),
            target_id: Some(id.to_string()),
            details,
        })
        .await?;
    Ok(())
}

async fn user_audit(
    repos: &dyn Repos,
    actor: &Actor,
    action: &str,
    user: UserId,
) -> Result<(), CoreError> {
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor.user),
            action: action.to_owned(),
            target_kind: "user".to_owned(),
            target_id: Some(user.to_string()),
            details: json!({}),
        })
        .await?;
    Ok(())
}

/// Compares two byte strings in time independent of where they differ.
pub(super) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `token` has the shape of a session token we issue (43 base64url characters).
fn is_session_token_shape(token: &str) -> bool {
    token.len() == SESSION_TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_token_shape() {
        let issued = URL_SAFE_NO_PAD.encode([0xfbu8; 32]);
        assert!(is_session_token_shape(&issued), "{issued}");
        for bad in [
            "",
            "bogus",
            &"a".repeat(42),
            &"a".repeat(44),
            &format!("{}=", "a".repeat(42)),
            &format!("{}+", "a".repeat(42)),
            &format!("{}é", "a".repeat(41)),
        ] {
            assert!(!is_session_token_shape(bad), "{bad:?}");
        }
    }

    #[test]
    fn user_agent_is_cut_on_a_char_boundary() {
        assert_eq!(clean_user_agent("curl/8"), "curl/8");
        let long = "é".repeat(300);
        let cut = clean_user_agent(&long);
        assert_eq!(cut.chars().count(), 256);
        assert_eq!(cut.len(), 512);
        assert_eq!(clean_user_agent("a\0b"), "ab");
    }

    #[test]
    fn time_helpers() {
        let at = |m: i64| Timestamp::from_micros(m).unwrap();
        assert!(!older_than(at(0), at(300_000_000), TOKEN_TOUCH_EVERY));
        assert!(older_than(at(0), at(300_000_001), TOKEN_TOUCH_EVERY));
        assert_eq!(plus(at(5), Duration::from_secs(1)).unwrap(), at(1_000_005));
        let last = Timestamp::from(chrono::DateTime::<chrono::Utc>::MAX_UTC);
        assert!(plus(last, Duration::from_secs(1)).is_err());
    }
}
