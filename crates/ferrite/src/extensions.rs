//! Operator install paths for themes and plugins (ruling P21): check that what is
//! installed loads, upload the artifact bytes first, then let [`ExtensionService`] write
//! the rows and the event in one transaction. Used by tests now and by the M2 CLI/admin
//! API.

use std::{path::Path, sync::Arc};

use anyhow::Context;
use bytes::Bytes;
use ferrite_config::PluginsConfig;
use ferrite_core::ExtensionService;
use ferrite_domain::{Artifact, ArtifactKind, InstalledPlugin, InstalledTheme, Timestamp};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{Grant, InMemoryServices, Manifest, PluginHost};
use ferrite_store::Store;
use ferrite_theme::{Theme, ThemeSource, pack};
use serde_json::json;

/// MIME type recorded for theme bundle artifacts (P17).
pub const THEME_MIME: &str = "application/x-tar";

/// MIME type recorded for plugin component artifacts.
pub const PLUGIN_MIME: &str = "application/wasm";

/// Installs a plugin from its `ferrite-plugin.toml` text and component bytes (P21):
/// parses the manifest and prepares the component on a scratch host with the site's
/// `[plugins]` limits and the manifest's own `[limits]` approved up to the ceilings (P5;
/// a plugin that would not load on the site's nodes is refused before anything is
/// written),
/// uploads the bytes to the media store, and records the artifact and plugin rows
/// through [`ExtensionService::install_plugin`]. A new plugin is installed disabled and
/// a replaced one keeps its state; with `enabled` it is then enabled
/// ([`ExtensionService::set_plugin_enabled`], which also clears its failure counters).
/// The row's `granted` is [`Grant::of`] the manifest: an operator install approves the
/// manifest as written (R21, P26), limits included. Returns the plugin row as stored.
pub async fn install_plugin_file(
    store: &Arc<dyn Store>,
    media: &MediaStore,
    limits: &PluginsConfig,
    manifest_toml: &str,
    wasm: &[u8],
    enabled: bool,
) -> anyhow::Result<InstalledPlugin> {
    let manifest = Manifest::parse(manifest_toml).context("cannot install the plugin")?;
    let table: toml::Table = manifest_toml.parse().context("invalid plugin manifest")?;
    let name = manifest.plugin.name.clone();
    let version = manifest.plugin.version.to_string();
    let grant = Grant::of(&manifest);
    let approved = grant.limits;
    let bytes = Bytes::copy_from_slice(wasm);
    let component = bytes.clone();
    let host_config = crate::serve::host_config(limits);
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let host = PluginHost::new(host_config, Arc::new(InMemoryServices::new()))
            .context("cannot start a plugin host")?;
        host.prepare_with(manifest, &component, approved)
            .context("the plugin does not load")?;
        Ok(())
    })
    .await
    .context("the plugin check task failed")??;
    let stored = media
        .put_artifact(bytes)
        .await
        .context("cannot upload the plugin component")?;
    let artifact = Artifact {
        hash: stored.hash.clone(),
        kind: ArtifactKind::Plugin,
        bytes: i64::try_from(stored.bytes)?,
        mime: PLUGIN_MIME.to_owned(),
        created_at: Timestamp::now(),
    };
    let now = Timestamp::now();
    let row = InstalledPlugin {
        name: name.clone(),
        version,
        artifact_hash: stored.hash,
        manifest: serde_json::to_value(table).context("cannot store the manifest")?,
        granted: grant.to_json(),
        enabled: false,
        disabled_reason: None,
        installed_at: now,
        updated_at: now,
    };
    let service = ExtensionService::new(store.clone());
    service
        .install_plugin(&artifact, &row)
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
/// bundle, loads the bundle (a theme that does not compile is refused before anything
/// is written), uploads it to the media store and records the artifact and theme rows
/// through [`ExtensionService::install_theme`]; with `activate` it becomes the active
/// theme. Returns the theme row as stored.
pub async fn install_theme_dir(
    store: &Arc<dyn Store>,
    media: &MediaStore,
    dir: &Path,
    activate: bool,
) -> anyhow::Result<InstalledTheme> {
    let source = ThemeSource::Dir(dir.to_path_buf());
    let (bundle, theme) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let bundle = Bytes::from(pack(&source).context("cannot pack the theme")?);
        let theme =
            Theme::load(&ThemeSource::Bundle(bundle.clone())).context("the theme does not load")?;
        Ok((bundle, theme))
    })
    .await
    .context("the theme pack task failed")??;
    let stored = media
        .put_artifact(bundle)
        .await
        .context("cannot upload the theme bundle")?;
    let artifact = Artifact {
        hash: stored.hash.clone(),
        kind: ArtifactKind::Theme,
        bytes: i64::try_from(stored.bytes)?,
        mime: THEME_MIME.to_owned(),
        created_at: Timestamp::now(),
    };
    let m = theme.manifest();
    let row = InstalledTheme {
        name: m.name.clone(),
        version: m.version.to_string(),
        artifact_hash: stored.hash,
        manifest: json!({
            "name": m.name,
            "version": m.version.to_string(),
            "author": m.author,
            "license": m.license,
            "description": m.description,
            "menus": m.menus,
        }),
        active: activate,
        installed_at: Timestamp::now(),
    };
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
    use ferrite_domain::ArtifactHash;
    use ferrite_media::{Backend, MediaConfig, PublicUrlMode};
    use ferrite_store_tests::{TestDb, backend_tests};

    use super::*;
    use crate::site::{ThemeOrigin, load_theme, reload_theme, sha256_hex};

    struct Site {
        data: tempfile::TempDir,
        media: MediaStore,
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
        Site { data, media }
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
