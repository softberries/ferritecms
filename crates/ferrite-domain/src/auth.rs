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
