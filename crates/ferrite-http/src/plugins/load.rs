//! Installing the enabled plugins at startup (plan ruling P3).

use ferrite_domain::{ArtifactHash, InstalledPlugin};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{Manifest, PluginHost};
use ferrite_store::{Store, StoreError};
use sha2::{Digest, Sha256};

/// What [`load_plugins`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadSummary {
    /// Installed plugin names, in row order (by name).
    pub loaded: Vec<String>,
    /// `(name, reason)` of enabled rows that could not be installed.
    pub skipped: Vec<(String, String)>,
}

/// Install every enabled `plugins` row into `host`: fetch its artifact from `media`,
/// check the bytes against the row's hash, parse the manifest and install. A row that
/// fails any step is logged (`warn`) and skipped; only failing to list the rows is an
/// error. Rows are installed in name order.
///
/// **Manifest storage.** The `manifest` column holds the `ferrite-plugin.toml` table in
/// JSON form (every TOML value used by manifests has a JSON equivalent). It is turned
/// back into TOML and validated with [`Manifest::parse`], so unknown keys and invalid
/// values are rejected exactly as at install time. The manifest's `plugin.name` must
/// equal the row's name. Grants are the manifest's capabilities (as in M0); the row's
/// `granted` column is not consulted until admin approval exists (M2).
pub async fn load_plugins(
    store: &dyn Store,
    media: &MediaStore,
    host: &PluginHost,
) -> Result<LoadSummary, StoreError> {
    let mut rows = store.read().plugins().list().await?;
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut summary = LoadSummary::default();
    for row in rows.into_iter().filter(|r| r.enabled) {
        match install(&row, media, host).await {
            Ok(()) => summary.loaded.push(row.name),
            Err(reason) => {
                tracing::warn!(plugin = %row.name, %reason, "plugin not loaded");
                summary.skipped.push((row.name, reason));
            }
        }
    }
    tracing::info!(
        loaded = summary.loaded.len(),
        skipped = summary.skipped.len(),
        "plugins loaded"
    );
    Ok(summary)
}

async fn install(
    row: &InstalledPlugin,
    media: &MediaStore,
    host: &PluginHost,
) -> Result<(), String> {
    let manifest = manifest_from_json(&row.manifest)?;
    if manifest.plugin.name != row.name {
        return Err(format!(
            "manifest names plugin `{}`, row is `{}`",
            manifest.plugin.name, row.name
        ));
    }
    let wasm = media
        .get(&row.artifact_hash)
        .await
        .map_err(|e| format!("artifact {}: {e}", row.artifact_hash))?;
    if hash_of(&wasm).as_ref() != Some(&row.artifact_hash) {
        return Err(format!(
            "artifact {} does not match its hash",
            row.artifact_hash
        ));
    }
    host.install(manifest, &wasm).map_err(|e| e.to_string())
}

/// The manifest stored as JSON (see [`load_plugins`]), validated by [`Manifest::parse`].
pub(crate) fn manifest_from_json(json: &serde_json::Value) -> Result<Manifest, String> {
    let table: toml::Table = serde_json::from_value(json.clone())
        .map_err(|e| format!("manifest is not a TOML table: {e}"))?;
    let src = toml::to_string(&table).map_err(|e| format!("manifest: {e}"))?;
    Manifest::parse(&src).map_err(|e| format!("manifest: {e}"))
}

fn hash_of(bytes: &[u8]) -> Option<ArtifactHash> {
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
