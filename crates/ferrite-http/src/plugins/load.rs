//! The row checks every [`PluginReloader::reload`](super::PluginReloader::reload) applies (plan ruling P3), and the
//! startup summary of its report.
//!
//! **Manifest storage.** The `manifest` column holds the `ferrite-plugin.toml` table in
//! JSON form (every TOML value used by manifests has a JSON equivalent). It is read by
//! [`Manifest::from_json`], so unknown keys and invalid values are rejected exactly as
//! at install time. The manifest's `plugin.name` must equal the row's name.
//!
//! **Grants (rulings R21, P3, P5).** A non-null `granted` column holds what the admin
//! approved, read by [`Grant::from_json`]: the v1 object (capabilities plus approved
//! limits) or the M1c-2 bare `[capabilities]` table (no approved limits). The plugin
//! loads only if its manifest's capabilities are a subset of the granted ones
//! ([`Capabilities::is_subset_of`](ferrite_plugin_host::Capabilities::is_subset_of));
//! otherwise it is skipped with a warning (approval must be renewed, e.g. after an
//! update asking for more). Its `[limits]` above the host defaults load only when the
//! grant approved them ([`PluginHost::prepare_with`](ferrite_plugin_host::PluginHost::prepare_with)).
//! A null `granted` (legacy operator rows) counts as [`Grant::of`] the row's manifest
//! (pre-flight F9): the manifest is honoured as written, limits included. The host
//! then runs with the manifest's capabilities, which are within the grant.

use ferrite_domain::{ArtifactHash, InstalledPlugin};
use ferrite_plugin_host::{ApprovedLimits, Grant, Manifest};
use sha2::{Digest, Sha256};

use super::reload::{PluginStatus, ReloadReport};

/// What a [`ReloadReport`] did, as logged at startup.
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

/// The row's manifest, checked against the row, and the limits its grant approved: the
/// name must be the row's and the capabilities within the grant (see the module docs).
pub(super) fn check_row(row: &InstalledPlugin) -> Result<(Manifest, ApprovedLimits), String> {
    let manifest = Manifest::from_json(&row.manifest).map_err(|e| format!("manifest: {e}"))?;
    if manifest.plugin.name != row.name {
        return Err(format!(
            "manifest names plugin `{}`, row is `{}`",
            manifest.plugin.name, row.name
        ));
    }
    let grant = if row.granted.is_null() {
        Grant::of(&manifest)
    } else {
        Grant::from_json(&row.manifest["plugin"], &row.granted)?
    };
    if !manifest.capabilities.is_subset_of(&grant.capabilities) {
        return Err("manifest asks for capabilities beyond the approved grant".to_owned());
    }
    Ok((manifest, grant.limits))
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
        let m = Manifest::from_json(&json).unwrap();
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
            assert!(Manifest::from_json(&bad).is_err(), "{bad}");
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
