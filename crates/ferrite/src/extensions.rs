//! Operator install paths for themes and plugins (rulings P21, P26): thin wrappers over
//! the library pipeline ([`Installer`]) and [`ExtensionService`], which writes the rows
//! and the event in one transaction under the `extensions` lock. Used by the CLI and
//! tests.

use std::{path::Path, sync::Arc};

use anyhow::Context;
use bytes::Bytes;
use ferrite_config::PluginsConfig;
use ferrite_core::ExtensionService;
use ferrite_domain::{InstalledPlugin, InstalledTheme};
use ferrite_http::extensions::{Inspected, Installer};
pub use ferrite_http::extensions::{PLUGIN_MIME, THEME_MIME};
use ferrite_media::MediaStore;
use ferrite_plugin_host::HostConfig;
use ferrite_store::Store;
use ferrite_theme::{ThemeSource, pack};

/// Installs a plugin from its `ferrite-plugin.toml` text and component bytes (P21):
/// [`Installer::inspect_plugin`] with the site's `[plugins]` limits (a plugin that would
/// not load on the site's nodes is refused before anything is written), then
/// [`Installer::stage_plugin`] approving exactly that report: an operator install
/// approves the manifest as written (R21, P26), limits up to the ceilings included, so
/// the row's `granted` is [`Grant::of`](ferrite_plugin_host::Grant::of) the manifest.
/// The rows are recorded through [`ExtensionService::install_plugin`]. A new plugin is
/// installed disabled and a replaced one keeps its state; with `enabled` it is then
/// enabled ([`ExtensionService::set_plugin_enabled`], which also clears its failure
/// counters). Returns the plugin row as stored.
pub async fn install_plugin_file(
    store: &Arc<dyn Store>,
    media: &Arc<MediaStore>,
    limits: &PluginsConfig,
    manifest_toml: &str,
    wasm: &[u8],
    enabled: bool,
) -> anyhow::Result<InstalledPlugin> {
    let installer = Installer::new(
        store.clone(),
        media.clone(),
        crate::serve::host_config(limits),
    )
    .context("cannot start a plugin host")?
    .with_trust(crate::registry::trust_settings(&limits.trust)?);
    let inspected = installer
        .inspect_plugin(manifest_toml, Bytes::copy_from_slice(wasm))
        .await
        .context("cannot install the plugin")?;
    record_plugin(store, &installer, inspected, enabled).await
}

/// Stages and records an inspected plugin, approving exactly its report (the operator
/// has seen it, or approves the manifest as written, P26); the steps after the
/// inspection of [`install_plugin_file`]. With `enabled` the plugin is then enabled.
pub(crate) async fn record_plugin(
    store: &Arc<dyn Store>,
    installer: &Installer,
    inspected: Inspected,
    enabled: bool,
) -> anyhow::Result<InstalledPlugin> {
    let name = inspected.manifest().plugin.name.clone();
    let fingerprint = inspected.report().fingerprint.clone();
    let staged = installer
        .stage_plugin(inspected, &fingerprint)
        .await
        .with_context(|| format!("cannot install plugin {name:?}"))?;
    let service = ExtensionService::new(store.clone());
    service
        .install_plugin_trusted(&staged.artifact, &staged.row, &staged.trust)
        .await
        .with_context(|| format!("cannot record plugin {name:?}"))?;
    if enabled {
        service
            .set_plugin_enabled(&name, true)
            .await
            .with_context(|| format!("cannot enable plugin {name:?}"))?;
    }
    store
        .read()
        .plugins()
        .get(&name)
        .await
        .context("cannot read back the plugin row")?
        .with_context(|| format!("plugin {name:?} vanished after install"))
}

/// Installs the theme folder `dir` as an artifact (P17): packs it into a deterministic
/// bundle, then [`Installer::stage_theme`] (a theme that does not compile is refused
/// before anything is written) and [`ExtensionService::install_theme`]; with `activate`
/// it becomes the active theme. Returns the theme row as stored.
pub async fn install_theme_dir(
    store: &Arc<dyn Store>,
    media: &Arc<MediaStore>,
    dir: &Path,
    activate: bool,
) -> anyhow::Result<InstalledTheme> {
    let source = ThemeSource::Dir(dir.to_path_buf());
    let bundle = tokio::task::spawn_blocking(move || pack(&source))
        .await
        .context("the theme pack task failed")?
        .context("cannot pack the theme")?;
    // Only themes are installed here: the scratch plugin host's limits do not matter.
    let installer = Installer::new(store.clone(), media.clone(), HostConfig::default())
        .context("cannot start the installer")?;
    let (artifact, mut row) = installer
        .stage_theme(Bytes::from(bundle))
        .await
        .context("cannot install the theme")?;
    row.active = activate;
    ExtensionService::new(store.clone())
        .install_theme(&artifact, &row, activate)
        .await
        .with_context(|| format!("cannot record theme {:?}", row.name))?;
    store
        .read()
        .themes()
        .get(&row.name)
        .await
        .context("cannot read back the theme row")?
        .with_context(|| format!("theme {:?} vanished after install", row.name))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use ferrite_config::ClusterMode;
    use ferrite_domain::{Artifact, ArtifactHash, ArtifactKind, Timestamp};
    use ferrite_media::{Backend, MediaConfig, PublicUrlMode};
    use ferrite_store_tests::{TestDb, backend_tests};
    use ferrite_theme::Theme;
    use serde_json::json;

    use super::*;
    use crate::site::{ThemeOrigin, load_theme, reload_theme, sha256_hex};

    struct Site {
        data: tempfile::TempDir,
        media: Arc<MediaStore>,
    }

    fn site() -> Site {
        let data = tempfile::tempdir().unwrap();
        let media = MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: data.path().join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap();
        Site {
            data,
            media: Arc::new(media),
        }
    }

    impl Site {
        /// Writes a theme folder `src/<name>` whose index renders `index`.
        fn theme_dir(&self, name: &str, index: &str) -> std::path::PathBuf {
            let dir = self.data.path().join("src").join(name);
            std::fs::create_dir_all(dir.join("templates")).unwrap();
            std::fs::write(
                dir.join("theme.toml"),
                format!("[theme]\nname = \"{name}\"\nversion = \"1.2.3\"\n"),
            )
            .unwrap();
            std::fs::write(dir.join("templates/index.html"), index).unwrap();
            dir
        }

        /// Replaces the stored blob of `hash` with `bytes` (bypassing the media store).
        fn tamper(&self, hash: &ArtifactHash, bytes: &[u8]) {
            let h = hash.as_str();
            let path = self
                .data
                .path()
                .join("media/artifacts")
                .join(&h[..2])
                .join(h);
            assert!(path.is_file(), "blob at {}", path.display());
            std::fs::write(path, bytes).unwrap();
        }
    }

    async fn install_theme_dir_roundtrip(db: &TestDb) {
        let s = site();
        let dir = s.theme_dir("mine", "mine index");
        let row = install_theme_dir(&db.shared_store(), &s.media, &dir, true)
            .await
            .unwrap();
        assert_eq!(row.name, "mine");
        assert_eq!(row.version, "1.2.3");
        assert!(row.active);
        let bundle = pack(&ThemeSource::Dir(dir.clone())).unwrap();
        assert_eq!(row.artifact_hash.as_str(), sha256_hex(&bundle));
        let artifact = db
            .store()
            .read()
            .artifacts()
            .get(&row.artifact_hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(artifact.kind, ArtifactKind::Theme);
        assert_eq!(artifact.mime, THEME_MIME);
        assert_eq!(artifact.bytes, bundle.len() as i64);
        let active = db.store().read().themes().active().await.unwrap().unwrap();
        assert_eq!(active.name, "mine");
        for mode in [ClusterMode::Solo, ClusterMode::Cluster] {
            let loaded = load_theme(db.store(), &s.media, s.data.path(), mode)
                .await
                .unwrap();
            assert_eq!(
                loaded.origin,
                ThemeOrigin::Artifact(row.artifact_hash.clone())
            );
            assert_eq!(loaded.theme.name(), "mine");
            assert!(loaded.error.is_none(), "{:?}", loaded.error);
            let folder = Theme::load(&ThemeSource::Dir(dir.clone())).unwrap();
            assert_eq!(loaded.theme.hash(), folder.hash());
        }
        // Installing the same folder again is idempotent: same artifact, still active.
        let again = install_theme_dir(&db.shared_store(), &s.media, &dir, false)
            .await
            .unwrap();
        assert_eq!(again.artifact_hash, row.artifact_hash);
        assert!(again.active);
    }

    async fn install_refuses_a_theme_that_does_not_compile(db: &TestDb) {
        let s = site();
        let dir = s.theme_dir("broken", "{% if %}");
        let err = install_theme_dir(&db.shared_store(), &s.media, &dir, true)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("template"), "{err:#}");
        assert!(db.store().read().themes().list().await.unwrap().is_empty());
        assert!(
            !s.data.path().join("media/artifacts").exists(),
            "nothing uploaded"
        );
    }

    async fn artifact_hash_mismatch_falls_back_at_startup(db: &TestDb) {
        let s = site();
        let dir = s.theme_dir("mine", "mine index");
        let row = install_theme_dir(&db.shared_store(), &s.media, &dir, true)
            .await
            .unwrap();
        // Valid bundle bytes, but not the ones the row names.
        let other = pack(&ThemeSource::Dir(s.theme_dir("mine", "evil index"))).unwrap();
        s.tamper(&row.artifact_hash, &other);
        for mode in [ClusterMode::Solo, ClusterMode::Cluster] {
            let loaded = load_theme(db.store(), &s.media, s.data.path(), mode)
                .await
                .unwrap();
            assert_eq!(loaded.origin, ThemeOrigin::Embedded);
            assert_eq!(loaded.theme.name(), "blog");
            assert!(loaded.theme.has_template("single.html"));
            let error = loaded.error.unwrap();
            assert!(error.contains("hash mismatch"), "{error}");
            assert!(error.contains(row.artifact_hash.as_str()), "{error}");
        }
    }

    async fn reload_theme_swaps_and_keeps_current_on_failure(db: &TestDb) {
        let s = site();
        let store = db.shared_store();
        let a = install_theme_dir(&store, &s.media, &s.theme_dir("a", "a"), true)
            .await
            .unwrap();
        let mode = ClusterMode::Cluster;
        let current = load_theme(db.store(), &s.media, s.data.path(), mode)
            .await
            .unwrap();
        assert_eq!(current.origin, ThemeOrigin::Artifact(a.artifact_hash));
        let same = reload_theme(db.store(), &s.media, s.data.path(), mode, &current).await;
        assert!(same.unwrap().is_none(), "unchanged artifact: no swap");

        let b = install_theme_dir(&store, &s.media, &s.theme_dir("b", "b"), true)
            .await
            .unwrap();
        let next = reload_theme(db.store(), &s.media, s.data.path(), mode, &current)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.origin, ThemeOrigin::Artifact(b.artifact_hash));
        assert_eq!(next.theme.name(), "b");

        let c = install_theme_dir(&store, &s.media, &s.theme_dir("c", "c"), true)
            .await
            .unwrap();
        s.tamper(&c.artifact_hash, b"not a tar");
        let err = reload_theme(db.store(), &s.media, s.data.path(), mode, &next)
            .await
            .unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");
        assert!(err.contains("\"c\""), "{err}");
    }

    /// Uploads `bytes` and records them as the active theme `name`, bypassing
    /// `install_theme_dir`'s checks.
    async fn record_raw(db: &TestDb, s: &Site, name: &str, bytes: Vec<u8>) -> ArtifactHash {
        let stored = s.media.put_artifact(Bytes::from(bytes)).await.unwrap();
        let artifact = Artifact {
            hash: stored.hash.clone(),
            kind: ArtifactKind::Theme,
            bytes: stored.bytes as i64,
            mime: THEME_MIME.into(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        };
        let row = InstalledTheme {
            name: name.into(),
            version: "1.0.0".into(),
            artifact_hash: stored.hash.clone(),
            manifest: json!({"name": name}),
            active: true,
            installed_at: Timestamp::now(),
        };
        ExtensionService::new(db.shared_store())
            .install_theme(&artifact, &row, true)
            .await
            .unwrap();
        stored.hash
    }

    async fn bundle_naming_another_theme_is_refused(db: &TestDb) {
        let s = site();
        let other = pack(&ThemeSource::Dir(s.theme_dir("other", "other"))).unwrap();
        let hash = record_raw(db, &s, "mine", other).await;
        let loaded = load_theme(db.store(), &s.media, s.data.path(), ClusterMode::Cluster)
            .await
            .unwrap();
        assert_eq!(loaded.origin, ThemeOrigin::Embedded);
        assert_eq!(loaded.theme.name(), "blog");
        let error = loaded.error.unwrap();
        assert!(error.contains("the row names \"mine\""), "{error}");
        assert!(error.contains(hash.as_str()), "{error}");
    }

    async fn oversized_artifact_is_refused(db: &TestDb) {
        let s = site();
        let huge = vec![0u8; ferrite_theme::MAX_BUNDLE_BYTES as usize + 1];
        record_raw(db, &s, "mine", huge).await;
        let loaded = load_theme(db.store(), &s.media, s.data.path(), ClusterMode::Cluster)
            .await
            .unwrap();
        assert_eq!(loaded.theme.name(), "blog");
        let error = loaded.error.unwrap();
        assert!(error.contains("more than a theme bundle may be"), "{error}");
    }

    async fn cluster_ignores_the_folder_of_a_broken_active_theme(db: &TestDb) {
        let s = site();
        let row = install_theme_dir(
            &db.shared_store(),
            &s.media,
            &s.theme_dir("mine", "a"),
            true,
        )
        .await
        .unwrap();
        s.tamper(&row.artifact_hash, b"broken");
        let folder = s.data.path().join("themes/mine");
        std::fs::create_dir_all(folder.join("templates")).unwrap();
        std::fs::write(
            folder.join("theme.toml"),
            "[theme]\nname = \"mine\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(folder.join("templates/index.html"), "folder").unwrap();
        let cluster = load_theme(db.store(), &s.media, s.data.path(), ClusterMode::Cluster)
            .await
            .unwrap();
        assert_eq!(cluster.origin, ThemeOrigin::Embedded);
        assert_eq!(cluster.theme.name(), "blog");
        assert!(cluster.error.unwrap().contains("hash mismatch"));
        // `Auto` (unresolved) fails closed like cluster.
        let auto = load_theme(db.store(), &s.media, s.data.path(), ClusterMode::Auto)
            .await
            .unwrap();
        assert_eq!(auto.origin, ThemeOrigin::Embedded);
        // Solo falls back to the folder.
        let solo = load_theme(db.store(), &s.media, s.data.path(), ClusterMode::Solo)
            .await
            .unwrap();
        assert_eq!(solo.origin, ThemeOrigin::Folder);
        assert_eq!(solo.theme.name(), "mine");
    }

    backend_tests!(
        bundle_naming_another_theme_is_refused,
        oversized_artifact_is_refused,
        cluster_ignores_the_folder_of_a_broken_active_theme,
        install_theme_dir_roundtrip,
        install_refuses_a_theme_that_does_not_compile,
        artifact_hash_mismatch_falls_back_at_startup,
        reload_theme_swaps_and_keeps_current_on_failure,
    );
}
