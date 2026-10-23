//! Site parts shared by `serve` and `seed`: the media store and the active theme.
//! [`load_theme`] / [`reload_theme`] are public for the M1.5 hot swap and its tests.

use std::{path::Path, sync::Arc};

use anyhow::{Context, bail};
use bytes::Bytes;
use ferrite_config::{ClusterMode, Config, MediaSection};
use ferrite_core::PermalinkRules;
use ferrite_domain::{ArtifactHash, InstalledTheme};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};
use ferrite_store::Store;
use ferrite_theme::{MAX_BUNDLE_BYTES, Theme, ThemeSource};
use futures::StreamExt;
use sha2::{Digest, Sha256};

/// Path prefix under which the app serves media (`/_media/{hash}/{name}`).
const MEDIA_PATH: &str = "/_media";
/// The theme used when none is active, and the fallback when the active one fails.
pub(crate) const DEFAULT_THEME: &str = "blog";
/// Longest theme name accepted from the `themes` table.
const MAX_THEME_NAME: usize = 64;

/// The media store settings from `ferrite.toml`. Local media and S3 without
/// `public_base_url` are served by the app under `/_media`; S3 with it links there.
pub(crate) fn media_config(config: &Config) -> MediaConfig {
    let via_app = || PublicUrlMode::ServeViaApp {
        path_prefix: MEDIA_PATH.to_owned(),
    };
    match &config.media {
        MediaSection::Local { root, .. } => MediaConfig {
            backend: Backend::Local { root: root.clone() },
            prefix: String::new(),
            public: via_app(),
        },
        MediaSection::S3 {
            bucket,
            region,
            endpoint,
            access_key_id,
            secret_access_key,
            allow_http,
            virtual_hosted_style,
            prefix,
            public_base_url,
        } => MediaConfig {
            backend: Backend::S3(S3Config {
                bucket: bucket.clone(),
                region: region.clone(),
                endpoint: endpoint.clone(),
                access_key_id: access_key_id.clone(),
                secret_access_key: secret_access_key.clone(),
                allow_http: *allow_http,
                virtual_hosted_style: *virtual_hosted_style,
            }),
            prefix: prefix.clone(),
            public: public_base_url
                .as_ref()
                .map_or_else(via_app, |base| PublicUrlMode::Direct {
                    base_url: base.clone(),
                }),
        },
    }
}

/// Opens the configured media store (no probe; see [`MediaStore::check`]).
pub(crate) fn open_media(config: &Config) -> anyhow::Result<MediaStore> {
    MediaStore::open(media_config(config)).context("cannot open the media store")
}

/// Permalink rules from the options (defaults when unset), checked against the
/// registered content types and taxonomies (rulings R3, R13).
pub(crate) async fn permalink_rules(store: &dyn Store) -> anyhow::Result<PermalinkRules> {
    ferrite_core::permalink::rules_from(store.read(), &[])
        .await
        .context("invalid permalink option")
}

/// Where the running theme's files came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeOrigin {
    /// The active row's bundle artifact (P17).
    Artifact(ArtifactHash),
    /// `<data>/themes/<name>/` (solo mode only).
    Folder,
    /// A starter theme compiled into the binary.
    Embedded,
}

/// The theme a node serves, where it came from and, when it is a fallback, why.
#[derive(Debug, Clone)]
pub struct LoadedTheme {
    pub theme: Arc<Theme>,
    pub origin: ThemeOrigin,
    /// Why the active theme could not be used (set only when this is a fallback).
    pub error: Option<String>,
}

/// The active theme at startup (06 §1, §7, ruling P17). `mode` must be the resolved
/// cluster mode ([`Config::effective_cluster_mode`]); an unresolved `Auto` is treated as
/// `Cluster` (folders not read: fail closed).
///
/// Precedence: the active row's artifact (fetched, SHA-256 checked against the row's
/// hash, unpacked in memory); when that fails, or there is no row (name `blog`), the
/// folder `<data>/themes/<name>/` (**solo only**), then the embedded theme of that
/// name, then the embedded `blog` theme. A broken theme never takes the site down: the
/// failure is logged and returned in [`LoadedTheme::error`]. In cluster mode
/// `<data>/themes/` is never read (every node must run the same theme, C5); a warning
/// says so if the folder exists. Only a store error is an error.
pub async fn load_theme(
    store: &dyn Store,
    media: &MediaStore,
    data_dir: &Path,
    mode: ClusterMode,
) -> anyhow::Result<LoadedTheme> {
    let folders = reads_folders(mode);
    let themes_dir = data_dir.join("themes");
    if !folders && std::fs::symlink_metadata(&themes_dir).is_ok() {
        tracing::warn!(
            dir = %themes_dir.display(),
            "cluster mode: theme folders are ignored; install themes as artifacts"
        );
    }
    let row = store
        .read()
        .themes()
        .active()
        .await
        .context("cannot read the active theme")?;
    let mut errors = Vec::new();
    let name = match row {
        None => DEFAULT_THEME.to_owned(),
        Some(row) => match artifact_theme(media, &row).await {
            Ok(theme) => {
                return Ok(LoadedTheme {
                    theme: Arc::new(theme),
                    origin: ThemeOrigin::Artifact(row.artifact_hash),
                    error: None,
                });
            }
            Err(e) => {
                errors.push(format!("artifact {}: {e:#}", row.artifact_hash));
                row.name
            }
        },
    };
    fallback_theme(&name, data_dir, folders, errors)
}

/// Re-reads the active theme for a hot swap (ruling P18). `Ok(None)`: nothing to do
/// (the active row names the artifact `current` came from, or there is no row and
/// `current` is not an artifact). `Ok(Some)`: the theme to swap in. `Err`: the new
/// theme failed; the caller keeps `current` and reports the message. Unlike
/// [`load_theme`] there is no fallback to `blog` for a broken artifact. `mode` as for
/// [`load_theme`] (resolved; `Auto` counts as `Cluster`).
pub async fn reload_theme(
    store: &dyn Store,
    media: &MediaStore,
    data_dir: &Path,
    mode: ClusterMode,
    current: &LoadedTheme,
) -> Result<Option<LoadedTheme>, String> {
    let row = store
        .read()
        .themes()
        .active()
        .await
        .map_err(|e| format!("cannot read the active theme: {e}"))?;
    match row {
        Some(row) => {
            let origin = ThemeOrigin::Artifact(row.artifact_hash.clone());
            if current.origin == origin {
                return Ok(None);
            }
            let theme = artifact_theme(media, &row).await.map_err(|e| {
                format!(
                    "theme {:?} (artifact {}): {e:#}",
                    row.name, row.artifact_hash
                )
            })?;
            Ok(Some(LoadedTheme {
                theme: Arc::new(theme),
                origin,
                error: None,
            }))
        }
        // The active row went away: back to the default, as at startup.
        None if matches!(current.origin, ThemeOrigin::Artifact(_)) => {
            fallback_theme(DEFAULT_THEME, data_dir, reads_folders(mode), Vec::new())
                .map(Some)
                .map_err(|e| format!("{e:#}"))
        }
        None => Ok(None),
    }
}

/// Theme folders are a solo-mode convenience (theme development, P17). Only an
/// explicit `Solo` reads them: an unresolved `Auto` fails closed, as cluster.
fn reads_folders(mode: ClusterMode) -> bool {
    matches!(mode, ClusterMode::Solo)
}

/// Fetches the row's bundle in one request, checks its SHA-256 against the row and
/// loads it. The declared size is checked before any body is read and the running
/// total while it streams, so at most [`MAX_BUNDLE_BYTES`] are ever buffered. Hashing
/// and unpacking run on the blocking pool. The bundle must hold the theme the row
/// names.
async fn artifact_theme(media: &MediaStore, row: &InstalledTheme) -> anyhow::Result<Theme> {
    let hash = row.artifact_hash.clone();
    let read = media
        .open_read(&hash, None)
        .await
        .context("cannot read the artifact")?;
    if read.size > MAX_BUNDLE_BYTES {
        bail!(
            "the artifact is {} bytes, more than a theme bundle may be ({MAX_BUNDLE_BYTES})",
            read.size
        );
    }
    let mut chunks = Vec::new();
    let mut total = 0u64;
    let mut stream = read.stream;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("cannot read the artifact")?;
        total += chunk.len() as u64;
        if total > MAX_BUNDLE_BYTES {
            bail!("the artifact is more than a theme bundle may be ({MAX_BUNDLE_BYTES} bytes)");
        }
        chunks.push(chunk);
    }
    let theme = tokio::task::spawn_blocking(move || -> anyhow::Result<Theme> {
        let mut hasher = Sha256::new();
        let mut bytes = Vec::with_capacity(usize::try_from(total)?);
        for chunk in &chunks {
            hasher.update(chunk);
            bytes.extend_from_slice(chunk);
        }
        let actual = hex(&hasher.finalize());
        if actual != hash.as_str() {
            bail!("content hash mismatch: the stored bytes hash to {actual}");
        }
        Theme::load(&ThemeSource::Bundle(Bytes::from(bytes))).context("the bundle does not load")
    })
    .await
    .context("the theme load task failed")??;
    if theme.name() != row.name {
        bail!(
            "the bundle holds theme {:?}, the row names {:?}",
            theme.name(),
            row.name
        );
    }
    Ok(theme)
}

#[cfg(test)]
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The startup fallback chain for `name`: folder (if `folders`), embedded `name`,
/// embedded `blog`. `errors` holds what failed before; every failure here is added,
/// logged and returned in [`LoadedTheme::error`].
fn fallback_theme(
    name: &str,
    data_dir: &Path,
    folders: bool,
    mut errors: Vec<String>,
) -> anyhow::Result<LoadedTheme> {
    let found = match try_named(name, data_dir, folders, &mut errors) {
        Some(found) => found,
        None => (
            Theme::load_embedded(DEFAULT_THEME)
                .context("the embedded blog theme failed to load")?,
            ThemeOrigin::Embedded,
        ),
    };
    let error = (!errors.is_empty()).then(|| errors.join("; "));
    if let Some(error) = &error {
        tracing::error!(
            theme = %name,
            error = %error,
            using = %found.0.name(),
            "the active theme failed to load; using a fallback"
        );
    }
    Ok(LoadedTheme {
        theme: Arc::new(found.0),
        origin: found.1,
        error,
    })
}

/// The folder (if `folders`) or embedded theme `name`; failures go to `errors`.
fn try_named(
    name: &str,
    data_dir: &Path,
    folders: bool,
    errors: &mut Vec<String>,
) -> Option<(Theme, ThemeOrigin)> {
    let valid = !name.is_empty()
        && name.len() <= MAX_THEME_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !valid {
        errors.push(format!("invalid theme name {name:?}"));
        return None;
    }
    let dir = data_dir.join("themes").join(name);
    // A symlink counts as present: the loader refuses it rather than us skipping it.
    if folders && std::fs::symlink_metadata(&dir).is_ok() {
        match Theme::load(&ThemeSource::Dir(dir.clone())) {
            Ok(theme) => return Some((theme, ThemeOrigin::Folder)),
            Err(e) => errors.push(format!("theme folder {}: {e}", dir.display())),
        }
    }
    match Theme::load_embedded(name) {
        Ok(theme) => Some((theme, ThemeOrigin::Embedded)),
        Err(e) => {
            errors.push(format!("embedded theme: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_theme(dir: &Path, name: &str, index: &str) {
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::write(
            dir.join("theme.toml"),
            format!("[theme]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("templates/index.html"), index).unwrap();
    }

    fn pick(name: &str, data: &Path) -> LoadedTheme {
        fallback_theme(name, data, true, Vec::new()).unwrap()
    }

    #[test]
    fn folder_wins_over_embedded() {
        let data = tempfile::tempdir().unwrap();
        // A folder named like the embedded theme replaces it.
        write_theme(&data.path().join("themes/blog"), "blog", "custom");
        let t = pick("blog", data.path());
        assert_eq!(t.origin, ThemeOrigin::Folder);
        assert!(t.error.is_none());
        assert!(t.theme.has_template("index.html"));
        assert!(
            !t.theme.has_template("single.html"),
            "the folder theme, not the embedded one"
        );
        write_theme(&data.path().join("themes/mine"), "mine", "mine");
        assert_eq!(pick("mine", data.path()).theme.name(), "mine");
    }

    #[test]
    fn embedded_when_no_folder() {
        let data = tempfile::tempdir().unwrap();
        let t = pick("blog", data.path());
        assert_eq!(t.origin, ThemeOrigin::Embedded);
        assert!(t.error.is_none());
        assert_eq!(t.theme.name(), "blog");
        assert!(t.theme.has_template("single.html"));
    }

    #[test]
    fn broken_unknown_or_invalid_falls_back_to_blog() {
        let data = tempfile::tempdir().unwrap();
        write_theme(&data.path().join("themes/broken"), "broken", "{% if %}");
        for name in ["broken", "missing", "../etc", "", "UPPER", "a/b"] {
            let t = pick(name, data.path());
            assert_eq!(t.theme.name(), "blog", "{name:?}");
            assert!(t.theme.has_template("single.html"), "{name:?}");
            assert!(t.error.is_some(), "{name:?}");
        }
    }

    async fn store_and_media() -> (
        ferrite_store_sqlite::SqliteStore,
        MediaStore,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let (store, db_dir) = ferrite_store_tests::sqlite_store().await;
        let data = tempfile::tempdir().unwrap();
        let media = MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: data.path().join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: MEDIA_PATH.to_owned(),
            },
        })
        .unwrap();
        (store, media, data, db_dir)
    }

    #[tokio::test]
    async fn cluster_ignores_theme_folders() {
        let (store, media, data, _db) = store_and_media().await;
        write_theme(&data.path().join("themes/blog"), "blog", "folder index");
        let solo = load_theme(&store, &media, data.path(), ClusterMode::Solo)
            .await
            .unwrap();
        assert_eq!(solo.origin, ThemeOrigin::Folder);
        assert!(!solo.theme.has_template("single.html"));
        let cluster = load_theme(&store, &media, data.path(), ClusterMode::Cluster)
            .await
            .unwrap();
        assert_eq!(cluster.origin, ThemeOrigin::Embedded);
        assert!(cluster.theme.has_template("single.html"));
        assert!(cluster.error.is_none());
        assert_ne!(cluster.theme.hash(), solo.theme.hash());
    }

    #[tokio::test]
    async fn reload_without_a_row_keeps_a_non_artifact_theme() {
        let (store, media, data, _db) = store_and_media().await;
        let current = load_theme(&store, &media, data.path(), ClusterMode::Solo)
            .await
            .unwrap();
        let next = reload_theme(&store, &media, data.path(), ClusterMode::Solo, &current)
            .await
            .unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn media_config_maps_backends() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(
            data.path().join("ferrite.toml"),
            "[site]\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        let config = ferrite_config::load(data.path(), Vec::new()).unwrap();
        let m = media_config(&config);
        assert!(
            matches!(m.backend, Backend::Local { ref root } if root == &data.path().join("media"))
        );
        assert!(
            matches!(m.public, PublicUrlMode::ServeViaApp { ref path_prefix } if path_prefix == "/_media")
        );
        std::fs::write(
            data.path().join("ferrite.toml"),
            "[site]\nurl = \"https://example.com\"\n[media]\nbackend = \"s3\"\n\
             [media.s3]\nbucket = \"b\"\nregion = \"r\"\nprefix = \"p/\"\n\
             public_base_url = \"https://cdn.example.com\"\n",
        )
        .unwrap();
        let config = ferrite_config::load(data.path(), Vec::new()).unwrap();
        let m = media_config(&config);
        assert!(matches!(m.backend, Backend::S3(ref s) if s.bucket == "b" && s.region == "r"));
        assert_eq!(m.prefix, "p/");
        assert!(
            matches!(m.public, PublicUrlMode::Direct { ref base_url } if base_url.as_str() == "https://cdn.example.com/")
        );
    }
}
