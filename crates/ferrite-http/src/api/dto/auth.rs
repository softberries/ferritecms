//! Bodies of the `/auth` endpoints.

use chrono::{DateTime, Utc};
use ferrite_core::{
    AuthService, Authenticated, Capability, SessionInfo, Via, auth::TotpSetup, require,
};
use serde::{Deserialize, Serialize};

use super::Password;
use utoipa::ToSchema;

/// `POST /auth/login`.
#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    /// Login or e-mail address.
    pub login: String,
    #[schema(value_type = String, format = Password)]
    pub password: Password,
}

/// `POST /auth/login/totp`.
#[derive(Deserialize, ToSchema)]
pub struct TotpLoginRequest {
    /// `challenge` from the `totp_required` login response (valid 5 minutes).
    pub challenge: String,
    /// The 6-digit code from the authenticator.
    pub code: String,
}

/// `POST /auth/password`.
#[derive(Deserialize, ToSchema)]
pub struct PasswordChange {
    #[schema(value_type = String, format = Password)]
    pub current: Password,
    /// 12 to 1024 bytes, at least 12 characters.
    #[schema(value_type = String, format = Password)]
    pub new: Password,
}

/// `POST /auth/password-reset`.
#[derive(Deserialize, ToSchema)]
pub struct PasswordResetRequest {
    /// Login or e-mail address.
    pub login: String,
}

/// `POST /auth/password-reset/confirm`.
#[derive(Deserialize, ToSchema)]
pub struct PasswordResetConfirm {
    /// The `token` from the fragment of the mailed link (`…#token=<token>`), valid
    /// 1 hour and once.
    pub token: String,
    /// The new password: 12 to 1024 bytes, at least 12 characters.
    #[schema(value_type = String, format = Password)]
    pub password: Password,
}

/// `POST /auth/totp/confirm`.
#[derive(Deserialize, ToSchema)]
pub struct TotpConfirm {
    /// `enrollment` from `POST /auth/totp/setup` (valid 10 minutes).
    pub enrollment: String,
    pub code: String,
    /// The account's current password: a stolen session alone cannot enroll another
    /// authenticator.
    #[schema(value_type = String, format = Password)]
    pub password: Password,
}

/// The result of `POST /auth/password`.
#[derive(Serialize, ToSchema)]
pub struct PasswordChanged {
    /// The new session's `X-CSRF-Token` (its cookie is set with this response); `null`
    /// for a bearer request, which gets no session.
    pub csrf_token: Option<String>,
}

/// `POST /auth/totp/disable`.
#[derive(Deserialize, ToSchema)]
pub struct TotpDisable {
    /// The account's current password.
    #[schema(value_type = String, format = Password)]
    pub password: Password,
}

/// One of the user's sessions (`GET /auth/sessions`). No user agent or address is
/// shown: sessions are told apart by time.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionDto {
    /// An opaque id (16 hex characters) for `DELETE /auth/sessions/{id}`; never the
    /// cookie value.
    pub id: String,
    pub created_at: DateTime<Utc>,
    /// Updated at most every 5 minutes.
    pub last_seen_at: DateTime<Utc>,
    /// The session this request came with (end it with `POST /auth/logout`).
    pub current: bool,
}

impl From<SessionInfo> for SessionDto {
    fn from(s: SessionInfo) -> Self {
        Self {
            id: s.id,
            created_at: s.created_at.as_datetime(),
            last_seen_at: s.last_seen_at.as_datetime(),
            current: s.current,
        }
    }
}

/// `GET /auth/sessions`: the user's live sessions, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionList {
    pub items: Vec<SessionDto>,
}

/// The result of `POST /auth/sessions/revoke-others`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionsRevoked {
    /// How many sessions ended.
    pub revoked: u64,
}

/// The authenticated user and what the request's credential allows.
#[derive(Serialize, ToSchema)]
pub struct MeDto {
    #[schema(format = Uuid)]
    pub id: String,
    pub login: String,
    pub email: String,
    pub display_name: String,
    /// `admin`, `editor`, `author` or `contributor`.
    pub role: String,
    /// Only `GET /auth/me`, `POST /auth/logout` and `POST /auth/password` are open until
    /// the password is changed.
    pub must_reset: bool,
    pub totp_enabled: bool,
    /// Capabilities granted by the role and the credential's scopes (07 §1 names).
    pub capabilities: Vec<String>,
    /// The credential's scopes (a session holds every scope).
    pub scopes: Vec<String>,
    /// The `X-CSRF-Token` for this session's unsafe requests; `null` for a bearer token.
    pub csrf_token: Option<String>,
}

impl MeDto {
    pub fn of(auth: &AuthService, who: &Authenticated) -> Self {
        let user = &who.user;
        let csrf_token = match &who.via {
            Via::Session(session) => Some(auth.csrf_token(session)),
            Via::Token(_) => None,
        };
        Self {
            id: user.id.to_string(),
            login: user.login.clone(),
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            role: user.role.as_str().to_owned(),
            must_reset: user.must_reset,
            totp_enabled: user.totp_secret.is_some(),
            capabilities: Capability::ALL
                .iter()
                .filter(|&&cap| require(&who.actor, cap).is_ok())
                .map(|cap| cap.as_str().to_owned())
                .collect(),
            scopes: who.actor.scopes.to_strings(),
            csrf_token,
        }
    }
}

/// The result of a correct password (`status` tells which).
#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginResponse {
    /// Logged in; the session cookie is set.
    Session {
        user: MeDto,
        /// The session's `X-CSRF-Token` (also in `user.csrf_token`).
        csrf_token: String,
    },
    /// The account uses TOTP: send a code with `challenge` to `POST /auth/login/totp`
    /// within 5 minutes.
    TotpRequired { challenge: String },
}

/// A started TOTP enrollment: show the URL (as a QR code) or the secret, then confirm a
/// code with `enrollment` within 10 minutes.
#[derive(Serialize, ToSchema)]
pub struct TotpSetupDto {
    pub secret_base32: String,
    pub otpauth_url: String,
    pub enrollment: String,
}

impl From<TotpSetup> for TotpSetupDto {
    fn from(s: TotpSetup) -> Self {
        Self {
            secret_base32: s.secret_base32,
            otpauth_url: s.otpauth_url,
            enrollment: s.enrollment,
        }
    }
}
