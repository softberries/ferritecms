//! Bodies of the `/plugins` endpoints. Responses never carry artifact bytes or secrets:
//! a plugin is described by its row (name, version, state, times), the permission
//! report of its stored grant, and what live nodes report about it.

use chrono::{DateTime, Utc};
use ferrite_domain::{InstalledPlugin, Node};
use ferrite_plugin_host::{LimitItem, PermissionItem, PermissionReport};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// One capability a manifest asks for, in plain language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PermissionItemDto {
    /// `content.read`, `content.meta.read`, `content.meta.write`, `options.read`, `kv`,
    /// `http` (one item per host), `render.head`, `render.text`, `render.html`,
    /// `routes` (one item per route), `mail`, `limits.memory`, `limits.fuel` (only when
    /// above the default).
    pub key: String,
    /// e.g. "read published posts and pages".
    pub text: String,
    /// The patterns, types, host or methods the item covers.
    pub detail: Vec<String>,
    pub high_risk: bool,
}

impl From<&PermissionItem> for PermissionItemDto {
    fn from(i: &PermissionItem) -> Self {
        Self {
            key: i.key.to_owned(),
            text: i.text.clone(),
            detail: i.detail.clone(),
            high_risk: i.high_risk,
        }
    }
}

/// One `[limits]` request against the host default and the approval ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LimitItemDto {
    /// `memory` (bytes) or `fuel`.
    pub name: String,
    pub requested: u64,
    pub default: u64,
    pub ceiling: u64,
    pub above_default: bool,
}

impl From<&LimitItem> for LimitItemDto {
    fn from(l: &LimitItem) -> Self {
        Self {
            name: l.name.to_owned(),
            requested: l.requested,
            default: l.default,
            ceiling: l.ceiling,
            above_default: l.above_default,
        }
    }
}

/// A permission report: what a manifest asks for in plain language, against a grant
/// (P3, P6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct GrantReportDto {
    pub plugin: String,
    pub version: String,
    /// The component's SHA-256 (64 lowercase hex).
    pub artifact: String,
    /// Every capability asked for.
    pub items: Vec<PermissionItemDto>,
    /// What the grant compared against does not cover: for an upload, the installed
    /// version's grant (all of `items` for a new plugin; a non-empty `added` on an
    /// update widens the grant, R1); for an installed plugin, its own stored grant
    /// (empty when it was installed with approval).
    pub added: Vec<PermissionItemDto>,
    /// Every limit the manifest sets.
    pub limits: Vec<LimitItemDto>,
    /// For an upload: what `POST /plugins` must carry as `approve` to install exactly
    /// it (SHA-256 over plugin, version, artifact and grant). For an installed plugin:
    /// the fingerprint it was approved with.
    pub fingerprint: String,
}

impl From<&PermissionReport> for GrantReportDto {
    fn from(r: &PermissionReport) -> Self {
        Self {
            plugin: r.plugin.clone(),
            version: r.version.clone(),
            artifact: r.artifact.clone(),
            items: r.items.iter().map(PermissionItemDto::from).collect(),
            added: r.added.iter().map(PermissionItemDto::from).collect(),
            limits: r.limits.iter().map(LimitItemDto::from).collect(),
            fingerprint: r.fingerprint.clone(),
        }
    }
}

/// The report of a plugin upload, as the admin approves it (`POST /plugins/inspect`,
/// and the `report` of an approval refusal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PermissionReportDto {
    #[serde(flatten)]
    pub report: GrantReportDto,
    /// Whether a plugin of this name is installed: `added` is then against its grant.
    pub update: bool,
}

impl PermissionReportDto {
    pub fn new(r: &PermissionReport, update: bool) -> Self {
        Self {
            report: r.into(),
            update,
        }
    }
}

/// What one live node reports about a plugin (`nodes.meta.plugins.<name>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PluginNodeState {
    /// The node's id.
    pub node: Uuid,
    /// The node's name (`cluster.node_name` or the host name).
    pub name: String,
    /// The version the node last loaded the row as.
    pub version: Option<String>,
    /// `loaded`, `failed` or `disabled`.
    pub status: String,
    pub error: Option<String>,
}

impl PluginNodeState {
    /// What `node` reports about `plugin`, if anything.
    pub fn of(node: &Node, plugin: &str) -> Option<Self> {
        let entry = node.meta.get("plugins")?.get(plugin)?;
        let text = |key: &str| entry.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Some(Self {
            node: node.id.as_uuid(),
            name: node.name.clone(),
            version: text("version"),
            status: text("status")?,
            error: text("error"),
        })
    }
}

/// An installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PluginDto {
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// Why it was disabled automatically (failures), if it was.
    pub disabled_reason: Option<String>,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The report of the stored grant: `added` is what it does not cover (empty for a
    /// plugin installed with approval), `fingerprint` the approved one. It has no
    /// `update`: rows keep no previous version. `null` when the stored manifest or
    /// grant no longer parses (the plugin does not load).
    pub report: Option<GrantReportDto>,
    /// What each live node (heartbeat within 30 s) reports about it, by node; a node
    /// that has not loaded the plugin yet is not listed.
    pub nodes: Vec<PluginNodeState>,
}

impl PluginDto {
    pub fn new(
        row: InstalledPlugin,
        report: Option<GrantReportDto>,
        nodes: Vec<PluginNodeState>,
    ) -> Self {
        Self {
            name: row.name,
            version: row.version,
            enabled: row.enabled,
            disabled_reason: row.disabled_reason,
            installed_at: row.installed_at.as_datetime(),
            updated_at: row.updated_at.as_datetime(),
            report,
            nodes,
        }
    }
}

/// `GET /plugins`: every installed plugin, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginList {
    pub items: Vec<PluginDto>,
    /// Always `null`: one page holds every plugin.
    pub next_cursor: Option<String>,
}

/// `POST /plugins/inspect` (`multipart/form-data`; the schema only, the handler reads
/// the parts itself). Every part is required; an unknown or repeated one is refused.
#[derive(ToSchema)]
pub struct PluginInspectUpload {
    /// `ferrite-plugin.toml` (UTF-8, at most 64 KiB).
    pub manifest: String,
    /// The component (`.wasm`, at most 32 MiB).
    #[schema(value_type = String, format = Binary)]
    pub component: Vec<u8>,
}

/// `POST /plugins` (`multipart/form-data`; the schema only). Every part is required;
/// an unknown or repeated one is refused.
#[derive(ToSchema)]
pub struct PluginUpload {
    /// `ferrite-plugin.toml` (UTF-8, at most 64 KiB).
    pub manifest: String,
    /// The component (`.wasm`, at most 32 MiB).
    #[schema(value_type = String, format = Binary)]
    pub component: Vec<u8>,
    /// The `fingerprint` of the report `POST /plugins/inspect` gave for this upload (at
    /// most 128 bytes).
    pub approve: String,
}
