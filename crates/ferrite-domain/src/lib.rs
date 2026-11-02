//! Pure domain types shared by every FerriteCMS storage backend.

mod auth;
mod comment;
mod content;
mod event;
mod hex;
pub mod html;
mod ids;
mod import;
mod job;
mod menu;
mod metrics;
mod node;
mod page;
mod plugin;
mod search;
mod secret;
mod setting;
mod site;
mod taxonomy;
mod time;
mod url;
mod user;

pub use auth::{
    ApiToken, AuditEntry, AuditQuery, ClientKind, IDEMPOTENCY_RETENTION, IDEMPOTENCY_STALE,
    IdemBegin, IdempotencyKey, NewAudit, OAuthClient, OAuthClientId, OAuthCode, OAuthGrant,
    OAuthPruned, OAuthRequest, RateHit, SESSION_ABSOLUTE, SESSION_IDLE, SESSION_TOUCH_EVERY,
    Session,
};
pub use comment::{Comment, CommentQuery, CommentStatus};
pub use content::{
    Autosave, ContentItem, ContentOrder, ContentQuery, ContentStatus, ContentType, MEDIA_KIND,
    MediaKind, NewContent, ParentFilter, ReadableBy, Revision,
};
pub use event::{Event, NewEvent};
pub use hex::to_hex;
pub use ids::{
    ApiTokenId, ArtifactHash, AuditId, CommentId, ContentId, ImportRunId, JobId, MenuItemId,
    NodeId, OAuthGrantId, OAuthRequestId, RedirectId, RevisionId, TermId, UserId,
};
pub use import::{ImportMapKind, ImportMapRow, ImportRun, ImportRunState};
pub use job::{DEFAULT_MAX_ATTEMPTS, Job, JobLease, JobQuery, JobStatus, NewJob};
pub use menu::{Menu, MenuItem, MenuItemKind};
pub use metrics::{
    MINUTE_MICROS, MetricCounts, NODE_METRICS_RETENTION, PAGE_LATENCY_BOUNDS_MS,
    PAGE_LATENCY_SLOTS, SeriesBucket, SeriesStep, page_p95_ms,
};
pub use node::{NewNode, Node};
pub use page::{Cursor, Page};
pub use plugin::{
    Artifact, ArtifactKind, FailureBucket, InstalledPlugin, InstalledTheme, MAX_PLUGIN_NAME, Media,
    valid_plugin_name,
};
pub use search::SearchQuery;
pub use secret::Secret;
pub use setting::{
    MAX_SETTING_SECRET, MAX_SETTING_TEXT, MAX_SETTING_TEXTAREA, SettingType, check_setting_value,
    empty_setting_value, is_color,
};
pub use site::{OptionEntry, REDIRECT_CODES, Redirect, RedirectOrigin, RedirectQuery};
pub use taxonomy::{Taxonomy, Term};
pub use time::Timestamp;
pub use url::{MAX_WEB_URL, is_web_url};
pub use user::{NewUser, Role, User, UserQuery, UserStatus};

/// Maximum page size accepted by list queries.
pub const MAX_PAGE: u32 = 200;

/// Errors produced while constructing or parsing domain values.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("invalid id: {0}")]
    InvalidId(String),
    #[error("invalid artifact hash: {0}")]
    InvalidHash(String),
    #[error("timestamp out of range: {0} microseconds")]
    TimestampOutOfRange(i64),
    #[error("invalid {kind} value: {value}")]
    InvalidEnum { kind: &'static str, value: String },
    #[error("invalid cursor")]
    InvalidCursor,
}

/// Defines a string-backed enum with `as_str`, `FromStr` and `Display`.
macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::DomainError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok($name::$variant),)+
                    other => Err($crate::DomainError::InvalidEnum {
                        kind: stringify!($name),
                        value: other.to_owned(),
                    }),
                }
            }
        }
    };
}
pub(crate) use string_enum;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use uuid::Uuid;

    #[test]
    fn ids_are_v7_and_roundtrip_text() {
        let id = ContentId::new();
        assert_eq!(id.as_uuid().get_version_num(), 7);
        let parsed: ContentId = id.to_string().parse().unwrap();
        assert_eq!(parsed, id);
        assert_eq!(ContentId::from_uuid(id.as_uuid()), id);
        assert!("not-a-uuid".parse::<UserId>().is_err());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
    }

    #[test]
    fn timestamp_micros_roundtrip() {
        let now = Timestamp::now();
        assert_eq!(Timestamp::from_micros(now.as_micros()).unwrap(), now);
        assert!(Timestamp::from_micros(i64::MAX).is_err());
        let json = serde_json::to_string(&now).unwrap();
        let back: Timestamp = serde_json::from_str(&json).unwrap();
        assert_eq!(back, now);
        assert_eq!(
            Timestamp::from_micros(0).unwrap().to_rfc3339(),
            "1970-01-01T00:00:00+00:00"
        );
    }

    #[test]
    fn status_str_roundtrip() {
        for s in ContentStatus::ALL {
            assert_eq!(s.as_str().parse::<ContentStatus>().unwrap(), *s);
        }
        assert_eq!(ContentStatus::ALL.len(), 6);
        assert!("Draft".parse::<ContentStatus>().is_err());
        assert_eq!(RedirectOrigin::SlugChange.as_str(), "slug-change");
        for o in RedirectOrigin::ALL {
            assert_eq!(o.as_str().parse::<RedirectOrigin>().unwrap(), *o);
        }
        let names: Vec<_> = CommentStatus::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, ["pending", "approved", "spam", "trash"]);
        for s in CommentStatus::ALL {
            assert_eq!(s.as_str().parse::<CommentStatus>().unwrap(), *s);
        }
        assert!("Approved".parse::<CommentStatus>().is_err());
        assert_eq!(IDEMPOTENCY_RETENTION.as_secs(), 24 * 3600);
        assert_eq!(IDEMPOTENCY_STALE.as_secs(), 3600);
    }

    #[test]
    fn job_status_and_lease() {
        for s in JobStatus::ALL {
            assert_eq!(s.as_str().parse::<JobStatus>().unwrap(), *s);
        }
        assert!("failed".parse::<JobStatus>().is_err());
        assert!(JobStatus::Queued < JobStatus::Dead);
        let node = NodeId::new();
        let mut job = Job {
            id: JobId::new(),
            kind: "k".into(),
            payload: serde_json::json!({}),
            unique_key: None,
            run_at: Timestamp::now(),
            priority: 0,
            status: JobStatus::Running,
            attempts: 3,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            locked_by: Some(node),
            locked_until: None,
            last_error: None,
            created_at: Timestamp::now(),
            finished_at: None,
        };
        assert_eq!(
            job.lease(),
            Some(JobLease {
                id: job.id,
                node,
                attempt: 3
            })
        );
        job.status = JobStatus::Done;
        assert_eq!(job.lease(), None);
        job.status = JobStatus::Running;
        job.locked_by = None;
        assert_eq!(job.lease(), None);
        let new = NewJob::new("k", serde_json::json!({}));
        assert_eq!((new.priority, new.max_attempts, new.run_at), (0, 5, None));
    }

    #[test]
    fn cursor_roundtrip() {
        let id = Uuid::now_v7();
        let key = serde_json::json!(["2026-01-01", 5]);
        let encoded = Cursor::encode("t:x", &key, id);
        assert!(!encoded.contains('=') && !encoded.contains('+') && !encoded.contains('/'));
        assert_eq!(Cursor::decode(&encoded, "t:x").unwrap(), (key, id));
        assert!(Cursor::decode(&encoded, "t:y").is_err());
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(Cursor::decode("!!!", "t").is_err());
        assert!(Cursor::decode("", "t").is_err());
        // valid base64, not valid JSON triple
        assert!(Cursor::decode("aGVsbG8", "t").is_err());
        let enc = |v: serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
        let id = Uuid::now_v7();
        assert!(Cursor::decode(&enc(serde_json::json!(["t", 1, id, 3])), "t").is_err());
        assert!(Cursor::decode(&enc(serde_json::json!(["t", 1, "nope"])), "t").is_err());
        // The untagged two-element form is rejected.
        assert!(Cursor::decode(&enc(serde_json::json!([1, id])), "t").is_err());
        assert!(Cursor::decode(&enc(serde_json::json!({"k": 1, "id": id})), "t").is_err());
    }

    #[test]
    fn artifact_hash_validation() {
        let ok = "a".repeat(64);
        assert_eq!(ArtifactHash::parse(&ok).unwrap().as_str(), ok);
        assert!(ArtifactHash::parse(&"a".repeat(63)).is_err());
        assert!(ArtifactHash::parse(&"A".repeat(64)).is_err());
        assert!(ArtifactHash::parse(&"g".repeat(64)).is_err());
        assert!(serde_json::from_str::<ArtifactHash>("\"abc\"").is_err());
    }

    #[test]
    fn timestamp_edge_cases() {
        for m in [-1, -86_400_000_000_123, 0] {
            assert_eq!(Timestamp::from_micros(m).unwrap().as_micros(), m);
        }
        assert!(Timestamp::from_micros(i64::MIN).is_err());
        assert_eq!(
            Timestamp::from_micros(i64::MIN),
            Err(DomainError::TimestampOutOfRange(i64::MIN))
        );
    }

    #[test]
    fn timestamp_deserialize_truncates() {
        let ts: Timestamp = serde_json::from_str("\"2026-01-01T00:00:00.123456789Z\"").unwrap();
        assert_eq!(ts, Timestamp::from_micros(1_767_225_600_123_456).unwrap());
    }

    #[test]
    fn user_debug_redacts_secrets() {
        let user = User {
            id: UserId::new(),
            login: "a".into(),
            email: "a@example.com".into(),
            display_name: "A".into(),
            password_hash: Some(Secret::new("$argon2id$topsecret")),
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
            totp_secret: Some(Secret::new("JBSWY3DPEHPK3PXP")),
            totp_last_step: Some(1),
            created_at: Timestamp::now(),
            last_login_at: None,
        };
        let dbg = format!("{user:?}");
        assert!(!dbg.contains("topsecret") && !dbg.contains("JBSWY3DP"));
        assert!(dbg.contains("[redacted]"));
        assert_eq!(
            user.password_hash.as_ref().map(Secret::expose),
            Some("$argon2id$topsecret")
        );
    }
}
