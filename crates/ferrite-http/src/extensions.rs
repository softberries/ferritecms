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
//! harmless (nothing names them). The API handlers delete them again with
//! [`Installer::discard_staged`], which keeps them when an artifact row names the same
//! bytes; anything else left behind (the CLI path, a failed cleanup) stays until the
//! artifact GC (M4).

use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use bytes::Bytes;
use ferrite_core::{
    CoreError,
    ext_settings::{SettingDef, SettingType, SettingsSchema},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, InstalledTheme, Timestamp,
};
use ferrite_media::{MediaError, MediaStore};
use ferrite_plugin_host::{
    ApprovedLimits, Grant, HostConfig, InMemoryServices, Manifest, ManifestError, ManifestProblem,
    PermissionReport, PluginError, PluginHost, PreparedPlugin,
};
use ferrite_store::{LOCK_EXTENSIONS, Store, lock};
use ferrite_theme::{
    MAX_BUNDLE_BYTES, SettingKind, Theme, ThemeError, ThemeManifest, ThemeSource, pack_embedded,
};
use futures::StreamExt;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::api::Problem;

/// Largest plugin component accepted (plan "Exact values"): 32 MiB.
pub const MAX_PLUGIN_BYTES: u64 = 32 * 1024 * 1024;

/// Component compiles (and theme loads of uploads) at once per installer (P8).
pub const COMPILE_SLOTS: usize = 2;

/// Uploads a node buffers at once (each up to its route's body limit); a request
/// beyond them is refused `busy` before its body is read (Task 4 review).
pub const UPLOAD_SLOTS: usize = 4;

/// How long [`Installer::discard_staged`] waits for the `extensions` lock.
pub const DISCARD_LOCK_WAIT: Duration = Duration::from_secs(5);

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
            // Host faults, not the upload's (final review M8).
            ExtensionError::Component(PluginError::Engine(_) | PluginError::InvalidConfig(_)) => {
                tracing::error!(error = %e, "plugin host fault during install");
                Self::internal()
            }
            ExtensionError::Component(_) => {
                let detail = e.to_string();
                Self::validation(detail.clone()).with_error(Some("component"), detail)
            }
            // Only a theme folder (`ThemeSource::Dir`, CLI and startup) reads files, so
            // no upload or artifact reaches this; its text holds a local path, which is
            // never sent.
            ExtensionError::Theme(ThemeError::Io { .. }) => {
                tracing::error!(error = %e, "theme files cannot be read");
                Self::validation("the theme does not load: its files cannot be read")
            }
            // One `errors[]` entry per problem the package or its templates have
            // (`templates/<file>:<line>: …`), as the admin fixes them one by one.
            ExtensionError::Theme(ThemeError::Invalid(ref problems))
            | ExtensionError::Theme(ThemeError::Templates(ref problems)) => {
                let mut p = Self::validation(e.to_string());
                for problem in problems {
                    p = p.with_error(None, problem.clone());
                }
                p
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
/// Its fields are private, so what is staged is what was inspected: the manifest, its
/// stored JSON form and the component cannot be changed after the report was made.
#[derive(Debug, Clone)]
pub struct Inspected {
    manifest: Manifest,
    /// The `ferrite-plugin.toml` table in JSON form (the `plugins.manifest` column).
    manifest_json: Value,
    report: PermissionReport,
    wasm: Bytes,
    update: bool,
}

impl Inspected {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The manifest as the row will store it.
    pub fn manifest_json(&self) -> &Value {
        &self.manifest_json
    }

    pub fn report(&self) -> &PermissionReport {
        &self.report
    }

    /// Whether a plugin of this name was installed when it was inspected (the report's
    /// `added` is then against that version's grant).
    pub fn is_update(&self) -> bool {
        self.update
    }
}

/// Test seam: runs after [`Installer::theme_from_artifact`] loaded a row's theme.
type ThemeLoadedFn =
    Arc<dyn Fn(&InstalledTheme) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

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
    uploads: Arc<Semaphore>,
    prepare: PrepareFn,
    theme_loaded: Option<ThemeLoadedFn>,
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
            uploads: Arc::new(Semaphore::new(UPLOAD_SLOTS)),
            prepare: Arc::new(|host, manifest, wasm, approved| {
                host.prepare_with(manifest, wasm, approved)
            }),
            theme_loaded: None,
        })
    }

    /// Test seam: await `hook` after each [`theme_from_artifact`](Self::theme_from_artifact)
    /// that loaded (for example to change the row before the caller records anything).
    #[doc(hidden)]
    pub fn with_theme_loaded(
        mut self,
        hook: impl Fn(&InstalledTheme) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.theme_loaded = Some(Arc::new(hook));
        self
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
        let (update, current) = self.current_grant(&manifest.plugin.name).await?;
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
            update,
        })
    }

    /// Uploads an inspected plugin approved with `approve` (its report's fingerprint)
    /// and returns the artifact and the row to record (disabled, `granted` =
    /// [`Grant::of`] the manifest). Nothing is uploaded without the fingerprint. The
    /// fingerprint is recomputed here from the inspected manifest and the hash of the
    /// component (defence in depth: [`Inspected`] cannot be changed after inspection).
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
        let theme = load_theme_artifact(&self.media, row).await?;
        if let Some(hook) = &self.theme_loaded {
            hook(row).await;
        }
        Ok(theme)
    }

    /// The settings schema of an installed theme (pre-flight F22): the one its row
    /// stores, else (a row stored before rows carried it, F10) the manifest of the
    /// row's artifact, fetched and checked like an activation.
    pub async fn theme_row_schema(
        &self,
        row: &InstalledTheme,
    ) -> Result<SettingsSchema, ExtensionError> {
        if let Some(schema) = SettingsSchema::from_theme_row(&row.manifest) {
            return Ok(schema);
        }
        let theme = load_theme_artifact(&self.media, row).await?;
        Ok(theme_schema(theme.manifest()))
    }

    /// One of the node's [`UPLOAD_SLOTS`], if one is free: handlers hold it while they
    /// read and process an upload, and answer `busy` without one.
    pub fn try_upload_slot(&self) -> Option<OwnedSemaphorePermit> {
        self.uploads.clone().try_acquire_owned().ok()
    }

    /// Deletes the blob of a staged artifact whose rows were then not recorded, unless
    /// an artifact row names it (another plugin, theme or media item uses the same
    /// bytes). Runs under the `extensions` lock (waiting at most
    /// [`DISCARD_LOCK_WAIT`]), so an extension install recording the same artifact
    /// cannot commit between the check and the delete; returns whether the blob was
    /// deleted. Residual race: an install of the same bytes that uploaded them but has
    /// not yet taken the lock to record them loses its blob (its plugin then fails to
    /// load and is reinstalled); without a GC grace period this cannot be closed.
    pub async fn discard_staged(&self, hash: &ArtifactHash) -> Result<bool, ExtensionError> {
        let guard = lock(self.store.as_ref(), LOCK_EXTENSIONS, DISCARD_LOCK_WAIT)
            .await
            .map_err(CoreError::from)?;
        let done = async {
            let referenced = self
                .store
                .read()
                .artifacts()
                .get(hash)
                .await
                .map_err(CoreError::from)?
                .is_some();
            if referenced {
                return Ok(false);
            }
            self.media.delete(hash).await?;
            Ok(true)
        }
        .await;
        if let Err(e) = guard.release().await {
            tracing::warn!(error = %e, "cannot release the extensions lock");
        }
        done
    }

    /// The permission report of an installed row: its manifest and component against
    /// its own stored grant ([`Grant::legacy`] for a null `granted`), so `added` lists
    /// what the grant does not cover (nothing for a row installed with approval) and
    /// `fingerprint` is the one its install was approved with. `None` (logged) when
    /// the stored manifest or grant no longer parses. No component is read.
    pub fn row_report(&self, row: &InstalledPlugin) -> Option<PermissionReport> {
        let built = Manifest::from_json(&row.manifest)
            .map_err(|e| e.to_string())
            .and_then(|m| {
                let grant = row_grant(&m, row)?;
                m.permission_report(row.artifact_hash.as_str(), Some(&grant), self.host.config())
                    .map_err(|e| e.to_string())
            });
        match built {
            Ok(report) => Some(report),
            Err(error) => {
                tracing::warn!(plugin = %row.name, %error, "installed plugin row has no usable report");
                None
            }
        }
    }

    /// Whether the plugin `name` is installed, and its grant. A row whose manifest or
    /// grant no longer parses has no usable grant: the report then lists everything as
    /// new.
    async fn current_grant(&self, name: &str) -> Result<(bool, Option<Grant>), ExtensionError> {
        let Some(row) = self
            .store
            .read()
            .plugins()
            .get(name)
            .await
            .map_err(CoreError::from)?
        else {
            return Ok((false, None));
        };
        let grant = Manifest::from_json(&row.manifest)
            .map_err(|e| e.to_string())
            .and_then(|m| row_grant(&m, &row));
        match grant {
            Ok(grant) => Ok((true, Some(grant))),
            Err(error) => {
                tracing::warn!(
                    plugin = %name,
                    %error,
                    "installed plugin row has no usable grant; reporting every capability as new"
                );
                Ok((true, None))
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

/// The grant a row stores: [`Grant::from_json`], or [`Grant::legacy`] of its manifest
/// for a null `granted` (pre-flight F9, R7).
fn row_grant(manifest: &Manifest, row: &InstalledPlugin) -> Result<Grant, String> {
    if row.granted.is_null() {
        Ok(Grant::legacy(manifest))
    } else {
        Grant::from_json(&row.manifest["plugin"], &row.granted)
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
        let actual = ferrite_domain::to_hex(&hasher.finalize());
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

/// The settings schema `theme.toml` declares, in key order.
pub fn theme_schema(m: &ThemeManifest) -> SettingsSchema {
    SettingsSchema(
        m.settings
            .iter()
            .map(|(key, spec)| SettingDef {
                key: key.clone(),
                kind: match spec.kind {
                    SettingKind::Text => SettingType::Text,
                    SettingKind::Textarea => SettingType::Textarea,
                    SettingKind::Bool => SettingType::Bool,
                    SettingKind::Number => SettingType::Number,
                    SettingKind::Select => SettingType::Select,
                    SettingKind::Color => SettingType::Color,
                    SettingKind::Media => SettingType::Media,
                },
                label: spec.label.clone(),
                default: spec.default.clone(),
                options: spec.options.clone(),
            })
            .collect(),
    )
}

/// The `themes.manifest` column: the M1.5 object plus `images` and `settings` (the
/// settings schema, an array of definitions; pre-flight F10).
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
        "settings": theme_schema(m),
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
    ferrite_domain::to_hex(&Sha256::digest(bytes))
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
        let p = Problem::from(ExtensionError::Theme(ThemeError::Templates(vec![
            "templates/a.html:1: x".into(),
            "templates/b.html:2: y".into(),
        ])));
        assert_eq!(
            (p.status, p.slug),
            (StatusCode::UNPROCESSABLE_ENTITY, "validation")
        );
        let details: Vec<&str> = p.errors.iter().map(|e| e.detail.as_str()).collect();
        assert_eq!(details, ["templates/a.html:1: x", "templates/b.html:2: y"]);
        assert!(p.errors.iter().all(|e| e.field.is_none()));
        let p = Problem::from(ExtensionError::Theme(ThemeError::Io {
            path: "/srv/secret/themes/x".into(),
            source: std::io::Error::other("denied"),
        }));
        assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!p.detail.contains("/srv"), "{}", p.detail);
        let p = Problem::from(ExtensionError::Theme(ThemeError::NotFound("x".into())));
        assert_eq!(
            (p.status, p.errors.len()),
            (StatusCode::UNPROCESSABLE_ENTITY, 0)
        );
        let p = Problem::from(ExtensionError::Approval("x".into()));
        assert_eq!(p.errors[0].field.as_deref(), Some("approve"));
        let p = Problem::from(ExtensionError::Component(PluginError::Inspect(
            ferrite_plugin_host::InspectError::NotAComponent,
        )));
        assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(p.errors[0].field.as_deref(), Some("component"));
        // Host faults are not the upload's (final review M8): 500, nothing echoed.
        for fault in [
            PluginError::Engine("engine detail".into()),
            PluginError::InvalidConfig("config detail".into()),
        ] {
            let p = Problem::from(ExtensionError::Component(fault));
            assert_eq!(
                (p.status, p.slug),
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            );
            assert!(!p.detail.contains("detail"), "{}", p.detail);
        }
    }

    #[test]
    fn manifest_json_reads_back() {
        let toml = "[plugin]\nname = \"seo\"\nversion = \"1.0.0\"\npublisher = \"x.dev\"\n\
                    world = \"ferrite:plugin/plugin@0.1.0\"\n[capabilities]\nrender.head = true\n";
        let json = manifest_json(toml).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["capabilities"]["render"]["head"], true);
    }
}
