use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactHash, ContentId, Timestamp, string_enum};

string_enum!(
    ArtifactKind {
        Media => "media",
        Variant => "variant",
        Plugin => "plugin",
        Theme => "theme",
        Export => "export",
    }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub hash: ArtifactHash,
    pub kind: ArtifactKind,
    pub bytes: i64,
    pub mime: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub item_id: ContentId,
    pub artifact_hash: ArtifactHash,
    pub mime: String,
    pub bytes: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub alt: String,
    pub original_name: String,
    /// name -> {hash, w, h, mime} (mime is needed for serving)
    pub variants: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: String,
    pub artifact_hash: ArtifactHash,
    pub manifest: Value,
    /// Capabilities the admin approved.
    pub granted: Value,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub installed_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledTheme {
    pub name: String,
    pub version: String,
    pub artifact_hash: ArtifactHash,
    pub manifest: Value,
    pub active: bool,
    pub installed_at: Timestamp,
}
