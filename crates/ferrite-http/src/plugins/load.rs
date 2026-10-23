//! Installing the enabled plugins at startup (plan ruling P3): the first
//! [`PluginReloader::reload`], plus the row checks every reload applies.

use std::sync::Arc;

use ferrite_domain::{ArtifactHash, InstalledPlugin};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{Capabilities, Manifest, PluginHost};
use ferrite_store::{Store, StoreError};
use sha2::{Digest, Sha256};

use super::reload::{PluginReloader, PluginStatus, ReloadReport};

/// What [`load_plugins`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadSummary {
    /// Installed plugin names, in row order (by name).
    pub loaded: Vec<String>,
    /// `(name, reason)` of enabled rows that could not be installed.
    pub skipped: Vec<(String, String)>,
}

impl From<&ReloadReport> for LoadSummary {
    fn from(report: &ReloadReport) -> Self {
        let mut summary = Self::default();
        for (name, state) in &report.states {
            match &state.status {
                PluginStatus::Loaded => summary.loaded.push(name.clone()),
                PluginStatus::Failed(reason) => {
                    summary.skipped.push((name.clone(), reason.clone()));
                }
                PluginStatus::Disabled(_) => {}
            }
        }
        summary
    }
}

/// Install every enabled `plugins` row into `host`: one [`PluginReloader::reload`] by a
/// fresh reloader. For each row: fetch its artifact from `media`, check the bytes
/// against the row's hash, parse the manifest, check the grant and compile. A row that
/// fails any step is logged (`warn`) and skipped; only failing to list the rows is an
/// error. Plugins in `host` without an enabled row are removed.
///
/// **Manifest storage.** The `manifest` column holds the `ferrite-plugin.toml` table in
/// JSON form (every TOML value used by manifests has a JSON equivalent). It is turned
/// back into TOML and validated with [`Manifest::parse`], so unknown keys and invalid
/// values are rejected exactly as at install time. The manifest's `plugin.name` must
/// equal the row's name.
///
/// **Grants (ruling R21).** A non-null `granted` column holds the capabilities the
/// admin approved, in the JSON form of a manifest `[capabilities]` table. The plugin
/// loads only if its manifest's capabilities are a subset of them
/// ([`Capabilities::is_subset_of`](ferrite_plugin_host::Capabilities::is_subset_of));
/// otherwise it is skipped with a warning (approval must be renewed, e.g. after an
/// update asking for more). A null `granted` means the manifest is honoured as written:
/// the only writers of rows are operator paths (`install_plugin_file`, tests); the M2
/// install flow will always set it. The host then runs with the manifest's
/// capabilities, which are within the grant.
pub async fn load_plugins(
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
    host: Arc<PluginHost>,
) -> Result<LoadSummary, StoreError> {
    let report = PluginReloader::new(store, media, host).reload().await?;
    let summary = LoadSummary::from(&report);
    tracing::info!(
        loaded = summary.loaded.len(),
        skipped = summary.skipped.len(),
        "plugins loaded"
    );
    Ok(summary)
}

/// The row's manifest, checked against the row: its name must be the row's and its
/// capabilities within a non-null grant (see [`load_plugins`]).
pub(super) fn check_row(row: &InstalledPlugin) -> Result<Manifest, String> {
    let manifest = manifest_from_json(&row.manifest)?;
    if manifest.plugin.name != row.name {
        return Err(format!(
            "manifest names plugin `{}`, row is `{}`",
            manifest.plugin.name, row.name
        ));
    }
    if !row.granted.is_null() {
        let granted = granted_capabilities(&row.manifest, &row.granted)?;
        if !manifest.capabilities.is_subset_of(&granted) {
            return Err("manifest asks for capabilities beyond the approved grant".to_owned());
        }
    }
    Ok(manifest)
}

/// The manifest stored as JSON (see [`load_plugins`]), validated by [`Manifest::parse`].
pub(crate) fn manifest_from_json(json: &serde_json::Value) -> Result<Manifest, String> {
    let table: toml::Table = serde_json::from_value(json.clone())
        .map_err(|e| format!("manifest is not a TOML table: {e}"))?;
    let src = toml::to_string(&table).map_err(|e| format!("manifest: {e}"))?;
    Manifest::parse(&src).map_err(|e| format!("manifest: {e}"))
}

/// Parse a `granted` value as a `[capabilities]` table, validated by
/// [`Manifest::parse`] (paired with the manifest's own `[plugin]` table).
fn granted_capabilities(
    manifest: &serde_json::Value,
    granted: &serde_json::Value,
) -> Result<Capabilities, String> {
    let doc = serde_json::json!({
        "plugin": manifest.get("plugin").cloned().unwrap_or(serde_json::Value::Null),
        "capabilities": granted,
    });
    manifest_from_json(&doc)
        .map(|m| m.capabilities)
        .map_err(|e| format!("granted: {e}"))
}

pub(super) fn hash_of(bytes: &[u8]) -> Option<ArtifactHash> {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ArtifactHash::parse(&hex).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_round_trips_through_json() {
        let json = json!({
            "plugin": {
                "name": "seo",
                "version": "1.0.0",
                "publisher": "example.dev",
                "world": "ferrite:plugin/plugin@0.1.0",
            },
            // Dotted TOML keys are nested tables.
            "capabilities": { "render": { "head": true }, "content": { "read": ["post"] } },
            "hooks": { "render.head": 10 },
            "limits": { "fuel": 1_000_000 },
        });
        let m = manifest_from_json(&json).unwrap();
        assert_eq!(m.plugin.name, "seo");
        assert_eq!(m.capabilities.content_read, ["post"]);
        assert_eq!(m.hooks.get("render.head"), Some(&10));
        assert_eq!(m.limits.fuel, Some(1_000_000));
    }

    #[test]
    fn bad_manifests_are_errors() {
        for bad in [
            json!("[plugin]\nname = \"x\""),
            json!({"plugin": {"name": null}}),
            json!({"plugin": {"name": "x"}, "unknown": 1}),
        ] {
            assert!(manifest_from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hash_matches_sha256_hex() {
        let h = hash_of(b"abc").unwrap();
        assert_eq!(
            h.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
