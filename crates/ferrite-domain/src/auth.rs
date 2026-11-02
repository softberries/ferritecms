//! Authentication records (design 07 §2, rulings P5, P7, P22, P23): login sessions, API
//! tokens, fixed-window rate-limit counters and the audit log.

use std::{fmt, time::Duration};

use serde_json::Value;

use crate::{ApiTokenId, AuditId, NodeId, OAuthGrantId, OAuthRequestId, Secret, Timestamp, UserId};

/// A session ends after this long without a request (P7).
pub const SESSION_IDLE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A session ends this long after login, however active it is (P7).
pub const SESSION_ABSOLUTE: Duration = Duration::from_secs(30 * 24 * 3600);
/// `last_seen_at` is written at most this often per session (P7).
pub const SESSION_TOUCH_EVERY: Duration = Duration::from_secs(5 * 60);

/// A login session. The cookie carries a random token; only its SHA-256 (`id_hash`) is
/// stored, so a database dump cannot be replayed as a cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Lowercase hex SHA-256 of the cookie token.
    pub id_hash: String,
    pub user_id: UserId,
    /// Per-session secret the CSRF token is derived from (P8).
    pub csrf_secret: Secret,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    /// The absolute end of the session (`created_at + SESSION_ABSOLUTE`).
    pub expires_at: Timestamp,
    pub ip_hash: Option<String>,
    pub user_agent: Option<String>,
}

/// A personal API token. Only the SHA-256 of the bearer value is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    pub id: ApiTokenId,
    pub user_id: UserId,
    pub name: String,
    /// Lowercase hex SHA-256 of the bearer value.
    pub token_hash: String,
    /// `<area>:<read|write>` scopes (P6).
    pub scopes: Vec<String>,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
    /// The OAuth grant this access token was issued for (design 12 §3.2); `None` for a
    /// personal token. Personal-token rules (the 50-token limit, the Account list) ignore
    /// rows with a grant, and deleting the grant deletes the token.
    pub oauth_grant_id: Option<OAuthGrantId>,
}

/// The result of one rate-limit hit (P5, F6): the bucket's count in the current window,
/// when that window ends, and the database clock the hit was counted at, so callers
/// compute `Retry-After` as `reset_at - now` on any node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateHit {
    pub count: u32,
    pub reset_at: Timestamp,
    pub now: Timestamp,
}

/// Input for an audit-log row; `id`, `at` and `node` are set by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAudit {
    pub actor_id: Option<UserId>,
    pub action: String,
    pub target_kind: String,
    pub target_id: Option<String>,
    /// A JSON object; never secrets (P22).
    pub details: Value,
}

/// A stored audit-log row. `actor_id` becomes `None` when the user is deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: AuditId,
    pub at: Timestamp,
    pub actor_id: Option<UserId>,
    pub action: String,
    pub target_kind: String,
    pub target_id: Option<String>,
    pub details: Value,
    pub node: Option<NodeId>,
}

/// An audit-log listing (`GET /system/audit`): newest first (`at DESC, id DESC`), every
/// filter optional and combined with AND.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditQuery {
    /// Rows written by this user.
    pub actor: Option<UserId>,
    /// An exact action (`user.update`), or a prefix when it ends in `.` (`user.` matches
    /// `user.create` but not `users.purge`); compared byte for byte, case-sensitive.
    pub action: Option<String>,
    pub target_kind: Option<String>,
    /// Only together with `target_kind`.
    pub target_id: Option<String>,
    /// 1..=[`MAX_PAGE`](crate::MAX_PAGE).
    pub limit: u32,
    pub cursor: Option<String>,
}

impl AuditQuery {
    /// The newest `limit` rows, unfiltered.
    pub fn first(limit: u32) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }
}

/// Idempotency rows are kept this long after the request started (M2a-2b P10, F7); the
/// `sessions.prune` job deletes older ones.
pub const IDEMPOTENCY_RETENTION: Duration = Duration::from_secs(24 * 3600);
/// A pending idempotency key older than this may be taken over by a retry (P10, F5).
pub const IDEMPOTENCY_STALE: Duration = Duration::from_secs(3600);

/// One `Idempotency-Key` use: scoped to the user, stored as the SHA-256 hex of the key
/// (`key_hash`), and bound to the request's method and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyKey {
    pub user: UserId,
    /// 64 lowercase hex characters.
    pub key_hash: String,
    pub method: String,
    pub path: String,
}

/// What `IdempotencyRepo::begin` found (P10, F5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdemBegin {
    /// The caller owns the key now (a new row, or a stale pending one taken over).
    /// `token` is the row's `created_at` (database clock); `finish` and `release` act
    /// only while the row still carries it, so a taken-over original cannot touch the
    /// new owner's row.
    Started { token: Timestamp },
    /// The first request finished: answer with its stored status and response.
    Replay { status: u16, response: Value },
    /// Another request holds the key and is not stale yet.
    InProgress,
    /// The key was used for another method or path.
    Mismatch,
}

/// An OAuth client's `client_id`: an `https` URL (a Client ID Metadata Document) or a
/// random `ferc_…` id (dynamic registration, design 12 §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OAuthClientId(String);

impl OAuthClientId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OAuthClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a client became known (design 12 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// Fetched from its Client ID Metadata Document URL.
    Metadata,
    /// Registered with `POST /oauth/register`.
    Registered,
}

impl ClientKind {
    /// The stored text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Registered => "registered",
        }
    }

    /// Parses the stored text.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "metadata" => Some(Self::Metadata),
            "registered" => Some(Self::Registered),
            _ => None,
        }
    }
}

/// A known OAuth client (public clients only: no secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClient {
    pub id: OAuthClientId,
    pub kind: ClientKind,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    pub created_at: Timestamp,
    /// When a metadata document was last fetched; `None` for a registered client.
    pub fetched_at: Option<Timestamp>,
    pub last_used_at: Option<Timestamp>,
}

/// A validated authorization request waiting for the user's consent (10 minutes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthRequest {
    pub id: OAuthRequestId,
    pub client_id: OAuthClientId,
    pub redirect_uri: String,
    /// The requested area scopes.
    pub scopes: Vec<String>,
    /// The client's `state`, kept as given.
    pub state: Option<String>,
    /// The PKCE S256 challenge.
    pub code_challenge: String,
    pub resource: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

/// An authorization code (60 seconds, single use). Only the SHA-256 hex of the code is
/// stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCode {
    /// Lowercase hex SHA-256 of the code.
    pub code_hash: String,
    pub client_id: OAuthClientId,
    pub user_id: UserId,
    pub redirect_uri: String,
    /// The approved scopes.
    pub scopes: Vec<String>,
    pub code_challenge: String,
    pub resource: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    /// The grant made from the code once it was used, so a replay revokes that grant.
    pub grant_id: Option<OAuthGrantId>,
}

/// A user's authorization of a client (the refresh token's record). The refresh token's
/// hashes are never part of this type; the repository takes and compares them by value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrant {
    pub id: OAuthGrantId,
    pub client_id: OAuthClientId,
    pub user_id: UserId,
    /// The granted area scopes.
    pub scopes: Vec<String>,
    pub created_at: Timestamp,
    pub last_used_at: Timestamp,
    /// The end of the refresh token (30 days, sliding).
    pub expires_at: Timestamp,
}

/// What one `OAuthRepo::prune_expired` call deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OAuthPruned {
    pub requests: u64,
    pub codes: u64,
    pub grants: u64,
}
