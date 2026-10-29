use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactHash, ContentId, Timestamp, string_enum};

/// Longest plugin name, in bytes.
pub const MAX_PLUGIN_NAME: usize = 48;

/// The plugin name rule, `[a-z0-9-]{2,48}`: the manifest's `plugin.name`, the
/// `/_p/<name>/` route segment and the theme helper `plugin_route` all apply it.
pub fn valid_plugin_name(name: &str) -> bool {
    (2..=MAX_PLUGIN_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Failures of one plugin hook in one hour (`PluginFailureRepo::buckets`): the shared
/// per-minute counters (P20) summed per hour, so the hours are aligned to the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureBucket {
    pub hook: String,
    /// The start of the hour.
    pub bucket_start: Timestamp,
    pub count: u64,
}

string_enum!(
    ArtifactKind {
        Media => "media",
        Variant => "variant",
        Plugin => "plugin",
        Theme => "theme",
        Export => "export",
        Import => "import",
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
    /// The stored bytes are the output of the SVG sanitiser (M2a-2b P7): only such an
    /// SVG is ever served inline. `false` for every row written before `0006`.
    #[serde(default)]
    pub sanitized: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_rule() {
        for ok in ["ab", "seo-pro", "a1", &"a".repeat(48)] {
            assert!(valid_plugin_name(ok), "{ok}");
        }
        for bad in [
            "",
            "a",
            "Seo",
            "seo_pro",
            "seo.pro",
            "s eo",
            "é1",
            &"a".repeat(49),
        ] {
            assert!(!valid_plugin_name(bad), "{bad}");
        }
    }
}
