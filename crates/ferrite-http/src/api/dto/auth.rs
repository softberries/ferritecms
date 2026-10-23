//! Bodies of the `/auth` endpoints.

use ferrite_core::{AuthService, Authenticated, Capability, Via, auth::TotpSetup, require};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// `POST /auth/login`.
#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    /// Login or e-mail address.
    pub login: String,
    pub password: String,
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
    pub current: String,
    /// 12 to 1024 bytes, at least 12 characters.
    pub new: String,
}

/// `POST /auth/totp/confirm`.
#[derive(Deserialize, ToSchema)]
pub struct TotpConfirm {
    /// `enrollment` from `POST /auth/totp/setup` (valid 10 minutes).
    pub enrollment: String,
    pub code: String,
}

/// `POST /auth/totp/disable`.
#[derive(Deserialize, ToSchema)]
pub struct TotpDisable {
    /// The account's current password.
    pub password: String,
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
