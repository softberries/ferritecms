//! The install pipeline for plugins and themes (plan ruling P8), shared by the admin
//! API and the CLI. [`Installer`] checks an upload and stores its bytes; recording the
//! rows is the caller's: `ExtensionAdmin` (API, with an actor and audit) or
//! `ExtensionService` (CLI, the operator path).
//!
//! **Plugins (P3, P5, R1).** [`Installer::inspect_plugin`] bounds the component
//! ([`MAX_PLUGIN_BYTES`]) and the manifest (64 KiB, [`Manifest::parse`]), builds the
//! permission report against the installed version's grant ([`Grant::legacy`] of its
//! manifest for a null `granted`, pre-flight F9), and compiles the component on a scratch
//! host with the requested limits treated as approved, so a component that cannot load
//! is refused now. Nothing is written. [`Installer::stage_plugin`] needs the report's
//! fingerprint, so an approval of one upload never approves another (including a
//! widening update, R1); it uploads the component and returns the rows to record, with
//! `granted` = [`Grant::of`] the manifest (always an object).
//!
//! **Themes (P11, P12).** [`Installer::stage_theme`] bounds the bundle
//! ([`MAX_BUNDLE_BYTES`]) before reading it, unpacks it in memory (ustar regular files
//! only: no links, absolute paths or `..`, file count and total size capped; see
//! `ferrite_theme`'s bundle docs) and compiles its templates, then uploads it.
//! [`load_theme_artifact`] is the reverse, used at startup, on a hot swap and on
//! activation: bounded fetch, SHA-256 check, unpack, name check.
//!
//! **Bounds.** Component compiles and theme loads of uploads run on the blocking pool,
//! at most [`COMPILE_SLOTS`] at once per installer (one per node). The scratch host has
//! in-memory services and never commits a plugin.
//!
//! **Failures leave nothing behind.** Every check runs before the upload. Artifacts are
//! content-addressed: bytes uploaded for a stage whose rows are then not recorded are
//! harmless (nothing names them) and stay until the artifact GC (M4); they are not
//! deleted here, because another row may name the same bytes.

use std::sync::Arc;

use axum::http::StatusCode;
use bytes::Bytes;
use ferrite_core::CoreError;
use ferrite_domain::{Artifact, ArtifactKind, InstalledPlugin, InstalledTheme, Timestamp};
use ferrite_media::{MediaError, MediaStore};
use ferrite_plugin_host::{
    ApprovedLimits, Grant, HostConfig, InMemoryServices, Manifest, ManifestError, ManifestProblem,
    PermissionReport, PluginError, PluginHost, PreparedPlugin,
};
use ferrite_store::Store;
use ferrite_theme::{
    MAX_BUNDLE_BYTES, Theme, ThemeError, ThemeManifest, ThemeSource, pack_embedded,
};
use futures::StreamExt;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::api::Problem;

/// Largest plugin component accepted (plan "Exact values"): 32 MiB.
pub const MAX_PLUGIN_BYTES: u64 = 32 * 1024 * 1024;

/// Component compiles (and theme loads of uploads) at once per installer (P8).
pub const COMPILE_SLOTS: usize = 2;

/// MIME type recorded for theme bundle artifacts (M1.5 P17).
pub const THEME_MIME: &str = "application/x-tar";

/// MIME type recorded for plugin component artifacts.
pub const PLUGIN_MIME: &str = "application/wasm";

/// Why an install step failed.
#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("{0}")]
    Manifest(#[from] ManifestError),
    /// The component does not load (inspection, link, or a limit above its ceiling).
    #[error("the plugin does not load: {0}")]
    Component(PluginError),
    /// The theme bundle is malformed or does not compile, or a stored one is too large.
    #[error("the theme does not load: {0}")]
    Theme(#[from] ThemeError),
    /// An upload larger than `limit` bytes, refused before it is parsed.
    #[error("the upload is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("artifact storage: {0}")]
    Media(#[from] MediaError),
    /// The bundle holds another theme than the row names.
    #[error("the bundle holds theme {bundle:?}, the row names {row:?}")]
    NameMismatch { bundle: String, row: String },
    /// The stored bytes are not the artifact the row names (tampered or corrupted).
    #[error("content hash mismatch: the stored bytes are not the artifact the row names")]
    HashMismatch,
    /// The approval is not the fingerprint of this upload's report.
    #[error("{0}")]
    Approval(String),
    #[error(transparent)]
    Core(#[from] CoreError),
    /// A blocking task panicked or was cancelled (runtime shutdown).
    #[error("internal error: {0}")]
    Task(String),
}

impl From<ExtensionError> for Problem {
    fn from(e: ExtensionError) -> Self {
        match e {
            ExtensionError::Manifest(m) => {
                let mut p = Self::validation(format!(
                    "invalid plugin manifest ({} problem(s))",
                    m.problems.len()
                ));
                for problem in m.problems {
                    let field = (!problem.path.is_empty()).then_some(problem.path.as_str());
                    p = p.with_error(field, problem.message.clone());
                }
                p
            }
            ExtensionError::Component(_) => {
                let detail = e.to_string();
                Self::validation(detail.clone()).with_error(Some("component"), detail)
            }
            // Pre-flight F11: bytes sent or stored that do not load are an actionable
            // refusal, not an internal error.
            ExtensionError::Theme(_)
            | ExtensionError::NameMismatch { .. }
            | ExtensionError::HashMismatch
            | ExtensionError::Media(MediaError::NotFound) => Self::validation(e.to_string()),
            ExtensionError::Approval(message) => {
                Self::validation(message.clone()).with_error(Some("approve"), message)
            }
            ExtensionError::TooLarge { limit } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too-large",
                "Request too large",
                format!("the upload is larger than {limit} bytes"),
            ),
            ExtensionError::Core(c) => Self::from(c),
            ExtensionError::Media(_) | ExtensionError::Task(_) => {
                tracing::error!(error = %e, "extension install failed");
                Self::internal()
            }
        }
    }
}

/// An inspected plugin upload: what [`Installer::stage_plugin`] stores once approved.
#[derive(Debug, Clone)]
pub struct Inspected {
    pub manifest: Manifest,
    /// The `ferrite-plugin.toml` table in JSON form (the `plugins.manifest` column).
    pub manifest_json: Value,
    pub report: PermissionReport,
    pub wasm: Bytes,
}

type PrepareFn = Arc<
    dyn Fn(&PluginHost, Manifest, &[u8], ApprovedLimits) -> Result<PreparedPlugin, PluginError>
        + Send
        + Sync,
>;

/// The install pipeline (see the module docs).
pub struct Installer {
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
    host: Arc<PluginHost>,
    slots: Arc<Semaphore>,
    prepare: PrepareFn,
}

impl Installer {
    /// An installer whose scratch host has `host_config` (the site's `[plugins]`
    /// defaults, so what passes here loads on the site's nodes).
    pub fn new(
        store: Arc<dyn Store>,
        media: Arc<MediaStore>,
        host_config: HostConfig,
    ) -> Result<Installer, PluginError> {
        let host = PluginHost::new(host_config, Arc::new(InMemoryServices::new()))?;
        Ok(Installer {
            store,
            media,
            host: Arc::new(host),
            slots: Arc::new(Semaphore::new(COMPILE_SLOTS)),
            prepare: Arc::new(|host, manifest, wasm, approved| {
                host.prepare_with(manifest, wasm, approved)
            }),
        })
    }

    /// Test seam: compile through `prepare` instead of [`PluginHost::prepare_with`] (for
    /// example a wrapper that counts compiles in flight).
    #[doc(hidden)]
    pub fn with_prepare(
        mut self,
        prepare: impl Fn(
            &PluginHost,
            Manifest,
            &[u8],
            ApprovedLimits,
        ) -> Result<PreparedPlugin, PluginError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.prepare = Arc::new(prepare);
        self
    }

    /// Checks a plugin upload and builds its permission report (see the module docs).
    /// Nothing is written.
    pub async fn inspect_plugin(
        &self,
        manifest_toml: &str,
        wasm: Bytes,
    ) -> Result<Inspected, ExtensionError> {
        if wasm.len() as u64 > MAX_PLUGIN_BYTES {
            return Err(ExtensionError::TooLarge {
                limit: MAX_PLUGIN_BYTES,
            });
        }
        let manifest = Manifest::parse(manifest_toml)?;
        let manifest_json = manifest_json(manifest_toml)?;
        let current = self.current_grant(&manifest.plugin.name).await?;
        let permit = self.slot().await?;
        let host = self.host.clone();
        let prepare = self.prepare.clone();
        let (m, component) = (manifest.clone(), wasm.clone());
        let report = tokio::task::spawn_blocking(move || {
            // Held until the compile ends, even if the caller stops waiting.
            let _permit = permit;
            let artifact = sha256_hex(&component);
            let report = m.permission_report(&artifact, current.as_ref(), host.config())?;
            let approved = Grant::of(&m).limits;
            prepare(&host, m, &component, approved)?;
            Ok::<_, PluginError>(report)
        })
        .await
        .map_err(task_error)?
        .map_err(ExtensionError::Component)?;
        Ok(Inspected {
            manifest,
            manifest_json,
            report,
            wasm,
        })
    }

    /// Uploads an inspected plugin approved with `approve` (its report's fingerprint)
    /// and returns the artifact and the row to record (disabled, `granted` =
    /// [`Grant::of`] the manifest). Nothing is uploaded without the fingerprint. The
    /// fingerprint is recomputed here from `inspected.manifest` and the hash of
    /// `inspected.wasm`, so an approval binds what is stored even if the caller changed
    /// the [`Inspected`] it got.
    pub async fn stage_plugin(
        &self,
        inspected: Inspected,
        approve: &str,
    ) -> Result<(Artifact, InstalledPlugin), ExtensionError> {
        let (m, component) = (inspected.manifest.clone(), inspected.wasm.clone());
        let cfg = self.host.config().clone();
        let (artifact_hex, fingerprint) = tokio::task::spawn_blocking(move || {
            let artifact = sha256_hex(&component);
            let report = m.permission_report(&artifact, None, &cfg)?;
            Ok::<_, PluginError>((artifact, report.fingerprint))
        })
        .await
        .map_err(task_error)?
        .map_err(ExtensionError::Component)?;
        if approve != fingerprint || approve != inspected.report.fingerprint {
            return Err(ExtensionError::Approval(
                "the approval does not match this upload; inspect it again".to_owned(),
            ));
        }
        let stored = self.media.put_artifact(inspected.wasm).await?;
        if stored.hash.as_str() != artifact_hex {
            return Err(ExtensionError::HashMismatch);
        }
        let now = Timestamp::now();
        let manifest = &inspected.manifest;
        let artifact = Artifact {
            hash: stored.hash.clone(),
            kind: ArtifactKind::Plugin,
            bytes: i64::try_from(stored.bytes).unwrap_or(i64::MAX),
            mime: PLUGIN_MIME.to_owned(),
            created_at: now,
        };
        let row = InstalledPlugin {
            name: manifest.plugin.name.clone(),
            version: manifest.plugin.version.to_string(),
            artifact_hash: stored.hash,
            manifest: inspected.manifest_json,
            granted: Grant::of(manifest).to_json(),
            enabled: false,
            disabled_reason: None,
            installed_at: now,
            updated_at: now,
        };
        Ok((artifact, row))
    }

    /// Checks a theme bundle (size before parsing, unpack, compile) and uploads it;
    /// returns the artifact and the row to record (not active).
    pub async fn stage_theme(
        &self,
        bundle: Bytes,
    ) -> Result<(Artifact, InstalledTheme), ExtensionError> {
        if bundle.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(ExtensionError::TooLarge {
                limit: MAX_BUNDLE_BYTES,
            });
        }
        let permit = self.slot().await?;
        let source = ThemeSource::Bundle(bundle.clone());
        let theme = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            Theme::load(&source)
        })
        .await
        .map_err(task_error)??;
        let stored = self.media.put_artifact(bundle).await?;
        let now = Timestamp::now();
        let artifact = Artifact {
            hash: stored.hash.clone(),
            kind: ArtifactKind::Theme,
            bytes: i64::try_from(stored.bytes).unwrap_or(i64::MAX),
            mime: THEME_MIME.to_owned(),
            created_at: now,
        };
        let m = theme.manifest();
        let row = InstalledTheme {
            name: m.name.clone(),
            version: m.version.to_string(),
            artifact_hash: stored.hash,
            manifest: theme_row_manifest(m),
            active: false,
            installed_at: now,
        };
        Ok((artifact, row))
    }

    /// [`stage_theme`](Self::stage_theme) of the embedded starter theme `name`
    /// (unknown: `Theme(NotFound)`).
    pub async fn stage_embedded(
        &self,
        name: &str,
    ) -> Result<(Artifact, InstalledTheme), ExtensionError> {
        let bundle = pack_embedded(name)?;
        self.stage_theme(Bytes::from(bundle)).await
    }

    /// [`load_theme_artifact`] from this installer's media store.
    pub async fn theme_from_artifact(&self, row: &InstalledTheme) -> Result<Theme, ExtensionError> {
        load_theme_artifact(&self.media, row).await
    }

    /// The grant of the installed plugin `name`, if any. A row whose manifest or grant
    /// no longer parses has no usable grant: the report then lists everything as new.
    async fn current_grant(&self, name: &str) -> Result<Option<Grant>, ExtensionError> {
        let Some(row) = self
            .store
            .read()
            .plugins()
            .get(name)
            .await
            .map_err(CoreError::from)?
        else {
            return Ok(None);
        };
        let grant = if row.granted.is_null() {
            Manifest::from_json(&row.manifest)
                .map(|m| Grant::legacy(&m))
                .map_err(|e| e.to_string())
        } else {
            Grant::from_json(&row.manifest["plugin"], &row.granted)
        };
        match grant {
            Ok(grant) => Ok(Some(grant)),
            Err(error) => {
                tracing::warn!(
                    plugin = %name,
                    %error,
                    "installed plugin row has no usable grant; reporting every capability as new"
                );
                Ok(None)
            }
        }
    }

    async fn slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ExtensionError> {
        self.slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ExtensionError::Task(e.to_string()))
    }
}

/// Fetches the row's bundle in one request, checks its SHA-256 against the row and
/// loads it. The declared size is checked before any body is read and the running
/// total while it streams, so at most [`MAX_BUNDLE_BYTES`] are ever buffered. Hashing
/// and unpacking run on the blocking pool. The bundle must hold the theme the row
/// names.
pub async fn load_theme_artifact(
    media: &MediaStore,
    row: &InstalledTheme,
) -> Result<Theme, ExtensionError> {
    let too_large = |what: String| {
        ExtensionError::Theme(ThemeError::Invalid(vec![format!(
            "{what}, more than a theme bundle may be ({MAX_BUNDLE_BYTES} bytes)"
        )]))
    };
    let hash = row.artifact_hash.clone();
    let read = media.open_read(&hash, None).await?;
    if read.size > MAX_BUNDLE_BYTES {
        return Err(too_large(format!("the artifact is {} bytes", read.size)));
    }
    let mut chunks = Vec::new();
    let mut total = 0u64;
    let mut stream = read.stream;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        total += chunk.len() as u64;
        if total > MAX_BUNDLE_BYTES {
            return Err(too_large("the artifact is larger".to_owned()));
        }
        chunks.push(chunk);
    }
    let theme = tokio::task::spawn_blocking(move || -> Result<Theme, ExtensionError> {
        let mut hasher = Sha256::new();
        let mut bytes = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
        for chunk in &chunks {
            hasher.update(chunk);
            bytes.extend_from_slice(chunk);
        }
        let actual = hex(&hasher.finalize());
        if actual != hash.as_str() {
            tracing::warn!(artifact = %hash, %actual, "theme artifact hash mismatch");
            return Err(ExtensionError::HashMismatch);
        }
        Ok(Theme::load(&ThemeSource::Bundle(Bytes::from(bytes)))?)
    })
    .await
    .map_err(task_error)??;
    if theme.name() != row.name {
        return Err(ExtensionError::NameMismatch {
            bundle: theme.name().to_owned(),
            row: row.name.clone(),
        });
    }
    Ok(theme)
}

/// The `themes.manifest` column: the M1.5 object plus `images` and `settings` (an
/// array, the form of the settings schema; pre-flight F10).
fn theme_row_manifest(m: &ThemeManifest) -> Value {
    let images: Map<String, Value> = m
        .images
        .iter()
        .map(|(name, size)| {
            (
                name.clone(),
                json!({ "width": size.width, "height": size.height, "crop": size.crop }),
            )
        })
        .collect();
    json!({
        "name": m.name,
        "version": m.version.to_string(),
        "author": m.author,
        "license": m.license,
        "description": m.description,
        "menus": m.menus,
        "images": images,
        "settings": [],
    })
}

/// The manifest TOML (already validated by [`Manifest::parse`]) as the JSON the row
/// stores; it must read back through [`Manifest::from_json`], as every reload does.
fn manifest_json(manifest_toml: &str) -> Result<Value, ExtensionError> {
    let problem = |message: String| {
        ExtensionError::Manifest(ManifestError {
            problems: vec![ManifestProblem {
                path: String::new(),
                message,
            }],
        })
    };
    let table: toml::Table = manifest_toml
        .parse()
        .map_err(|e: toml::de::Error| problem(e.to_string()))?;
    let json = serde_json::to_value(table)
        .map_err(|e| problem(format!("cannot be stored as JSON: {e}")))?;
    Manifest::from_json(&json)?;
    Ok(json)
}

fn task_error(e: tokio::task::JoinError) -> ExtensionError {
    ExtensionError::Task(e.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_map_to_slugs() {
        let cases: Vec<(ExtensionError, StatusCode, &str)> = vec![
            (
                ExtensionError::TooLarge { limit: 1 },
                StatusCode::PAYLOAD_TOO_LARGE,
                "too-large",
            ),
            (
                ExtensionError::HashMismatch,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
            ),
            (
                ExtensionError::Media(MediaError::NotFound),
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
            ),
            (
                ExtensionError::Media(MediaError::Backend("down".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
            ),
            (
                ExtensionError::Task("panicked".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
            ),
            (
                ExtensionError::Core(CoreError::NotFound),
                StatusCode::NOT_FOUND,
                "not-found",
            ),
            (
                ExtensionError::NameMismatch {
                    bundle: "a".into(),
                    row: "b".into(),
                },
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
            ),
        ];
        for (e, status, slug) in cases {
            let p = Problem::from(e);
            assert_eq!((p.status, p.slug), (status, slug));
        }
        let p = Problem::from(ExtensionError::Approval("x".into()));
        assert_eq!(p.errors[0].field.as_deref(), Some("approve"));
        let p = Problem::from(ExtensionError::Component(PluginError::Engine("e".into())));
        assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(p.errors[0].field.as_deref(), Some("component"));
    }

    #[test]
    fn manifest_json_reads_back() {
        let toml = "[plugin]\nname = \"seo\"\nversion = \"1.0.0\"\npublisher = \"x.dev\"\n\
                    world = \"ferrite:plugin/plugin@0.1.0\"\n[capabilities]\nrender.head = true\n";
        let json = manifest_json(toml).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["capabilities"]["render"]["head"], true);
    }
}
