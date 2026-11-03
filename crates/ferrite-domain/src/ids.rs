use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DomainError;

macro_rules! uuid_id {
    ($($(#[$m:meta])* $name:ident),+ $(,)?) => {$(
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            /// Creates a fresh UUIDv7 id.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.hyphenated().fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|_| DomainError::InvalidId(s.to_owned()))
            }
        }
    )+};
}

uuid_id!(
    ContentId,
    RevisionId,
    TermId,
    UserId,
    MenuItemId,
    RedirectId,
    JobId,
    NodeId,
    ApiTokenId,
    AuditId,
    CommentId,
    ImportRunId,
    OAuthRequestId,
    OAuthGrantId,
    WebhookId,
    WebhookEventId,
    DeliveryId,
    ChangeSetId
);

uuid_id!(
    /// A content health run (M6-4, `health_runs`).
    HealthRunId
);

/// SHA-256 content hash: exactly 64 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ArtifactHash(String);

impl ArtifactHash {
    pub fn parse(s: &str) -> Result<Self, DomainError> {
        let valid = s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(DomainError::InvalidHash(s.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ArtifactHash {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for ArtifactHash {
    type Error = DomainError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<ArtifactHash> for String {
    fn from(h: ArtifactHash) -> Self {
        h.0
    }
}
