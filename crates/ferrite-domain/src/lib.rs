//! Pure domain types shared by every FerriteCMS storage backend.

mod content;
mod event;
mod ids;
mod menu;
mod page;
mod plugin;
mod site;
mod taxonomy;
mod time;
mod user;

pub use content::{
    ContentItem, ContentOrder, ContentQuery, ContentStatus, ContentType, NewContent, ParentFilter,
    Revision,
};
pub use event::{Event, NewEvent};
pub use ids::{ArtifactHash, ContentId, MenuItemId, RedirectId, RevisionId, TermId, UserId};
pub use menu::{Menu, MenuItem, MenuItemKind};
pub use page::{Cursor, Page};
pub use plugin::{Artifact, ArtifactKind, InstalledPlugin, InstalledTheme, Media};
pub use site::{OptionEntry, REDIRECT_CODES, Redirect, RedirectOrigin};
pub use taxonomy::{Taxonomy, Term};
pub use time::Timestamp;
pub use user::{NewUser, Role, User, UserStatus};

/// Maximum page size accepted by list queries.
pub const MAX_PAGE: u32 = 200;

/// Errors produced while constructing or parsing domain values.
#[derive(Debug, thiserror::Error)]
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
    }

    #[test]
    fn cursor_roundtrip() {
        let id = Uuid::now_v7();
        let key = serde_json::json!(["2026-01-01", 5]);
        let encoded = Cursor::encode(&key, id);
        assert!(!encoded.contains('=') && !encoded.contains('+') && !encoded.contains('/'));
        assert_eq!(Cursor::decode(&encoded).unwrap(), (key, id));
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(Cursor::decode("!!!").is_err());
        assert!(Cursor::decode("").is_err());
        // valid base64, not valid JSON pair
        assert!(Cursor::decode("aGVsbG8").is_err());
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
}
