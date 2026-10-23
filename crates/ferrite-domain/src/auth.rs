//! Authentication records (design 07 §2, rulings P5, P7, P22, P23): login sessions, API
//! tokens, fixed-window rate-limit counters and the audit log.

use std::time::Duration;

use serde_json::Value;

use crate::{ApiTokenId, AuditId, NodeId, Secret, Timestamp, UserId};

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
