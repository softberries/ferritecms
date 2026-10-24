//! Bodies of the `/plugins` and `/themes` endpoints. Responses never carry artifact
//! bytes or secrets: a plugin is described by its row (name, version, state, times), the
//! permission report of its stored grant, and what live nodes report about it; a theme
//! by its row or embedded manifest and what live nodes serve. Settings carry the
//! schema and the values, and for a plugin `secret` only whether it is set.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ferrite_core::{
    diagnostics::scrub,
    ext_settings::{PluginSettingsView, SettingDef, SettingsSchema},
};
use ferrite_domain::{InstalledPlugin, InstalledTheme, Node};
use ferrite_plugin_host::{LimitItem, PermissionItem, PermissionReport};
use serde::Serialize;
use serde_json::{Map, Value};
use utoipa::ToSchema;
use uuid::Uuid;

/// One capability a manifest asks for, in plain language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PermissionItemDto {
    /// `content.read`, `content.meta.read`, `content.meta.write`, `options.read`, `kv`,
    /// `http` (one item per host), `render.head`, `render.text`, `render.html`,
    /// `routes` (one item per route), `mail`, `hooks` (one item per action hook),
    /// `limits.memory`, `limits.fuel` (only when above the default).
    pub key: String,
    /// e.g. "read published posts and pages".
    pub text: String,
    /// The patterns, types, host, methods or hook name the item covers.
    pub detail: Vec<String>,
    /// `low`, `medium` (site-wide traffic metadata: `request.served`) or `high`.
    pub risk: String,
    /// Whether `risk` is `high`.
    pub high_risk: bool,
}

impl From<&PermissionItem> for PermissionItemDto {
    fn from(i: &PermissionItem) -> Self {
        Self {
            key: i.key.to_owned(),
            text: i.text.clone(),
            detail: i.detail.clone(),
            risk: i.risk.as_str().to_owned(),
            high_risk: i.high_risk(),
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
    /// The node's diagnostic, scrubbed of URLs, paths, addresses and credentials (also
    /// when an older node stored it unscrubbed), at most 1 KiB.
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
            error: text("error").map(|e| scrub(&e)),
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

/// Where a listed theme comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ThemeOrigin {
    /// A `themes` row (uploaded, or an embedded theme installed on activation).
    Installed,
    /// A starter theme compiled into the binary, without a row.
    Embedded,
}

/// What one live node serves (`nodes.meta.theme`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ThemeNodeState {
    /// The node's id.
    pub node: Uuid,
    /// The node's name (`cluster.node_name` or the host name).
    pub node_name: String,
    /// The binary version the node runs.
    pub node_version: String,
    /// The theme the node serves.
    pub name: String,
    /// The served files' content hash (as in `/_theme/<hash>/` asset URLs).
    pub hash: String,
    /// Where the node's files came from: `artifact` (the row's bundle), `folder`
    /// (`<data>/themes/<name>/`, solo only) or `embedded`.
    pub origin: String,
    /// Why the node could not serve the active theme, if it could not (scrubbed as
    /// [`PluginNodeState::error`]).
    pub error: Option<String>,
    /// The node does not serve the active row's artifact: the startup fallback, or the
    /// previous theme after a failed swap (pre-flight F12). `false` when no theme is
    /// active and the embedded default serves.
    pub fallback: bool,
}

impl ThemeNodeState {
    /// What `node` reports about its theme, if anything.
    pub fn of(node: &Node) -> Option<Self> {
        let theme = node.meta.get("theme")?;
        let text = |key: &str| theme.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        let origin = match theme.get("origin") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Object(o)) if o.contains_key("artifact") => "artifact".into(),
            _ => return None,
        };
        Some(Self {
            node: node.id.as_uuid(),
            node_name: node.name.clone(),
            node_version: node.version.clone(),
            name: text("name")?,
            hash: text("hash").unwrap_or_default(),
            origin,
            error: text("error").map(|e| scrub(&e)),
            fallback: theme
                .get("fallback")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// A theme: installed (a row) or embedded (no row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ThemeDto {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub origin: ThemeOrigin,
    /// The theme nodes should serve: the active row, or with no row active the embedded
    /// default (`blog`) while it has no row. With no row active and an inactive `blog`
    /// row (uploaded without `activate`), no entry is active: nodes then serve the
    /// embedded `blog` files, which no entry describes; activating a theme ends that.
    pub active: bool,
    /// When the row was written; `null` for an embedded theme without a row.
    pub installed_at: Option<DateTime<Utc>>,
    /// What each live node (heartbeat within 30 s) serves, by node; only on the active
    /// theme (empty on the others).
    pub nodes: Vec<ThemeNodeState>,
}

impl ThemeDto {
    /// The entry of an installed row.
    pub fn installed(row: &InstalledTheme) -> Self {
        let description = row
            .manifest
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        Self {
            name: row.name.clone(),
            version: row.version.clone(),
            description,
            origin: ThemeOrigin::Installed,
            active: row.active,
            installed_at: Some(row.installed_at.as_datetime()),
            nodes: Vec::new(),
        }
    }

    /// The entry of an embedded theme without a row.
    pub fn embedded(name: &str, version: &str, description: Option<&str>) -> Self {
        Self {
            name: name.to_owned(),
            version: version.to_owned(),
            description: description.map(str::to_owned),
            origin: ThemeOrigin::Embedded,
            active: false,
            installed_at: None,
            nodes: Vec::new(),
        }
    }
}

/// `GET /themes`: every installed theme and every embedded theme without a row, by
/// name.
#[derive(Debug, Serialize, ToSchema)]
pub struct ThemeList {
    pub items: Vec<ThemeDto>,
    /// Always `null`: one page holds every theme.
    pub next_cursor: Option<String>,
}

/// `POST /themes` (`application/x-tar`; the schema only, the handler reads the body
/// itself): a ustar archive of `theme.toml`, `templates/` and `assets/`.
#[derive(ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct ThemeBundle(pub Vec<u8>);

/// One declared setting: what the admin renders a form field from (05 §4c, 06 §1).
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SettingDefDto {
    /// `[a-z0-9_-]`, at most 64 characters.
    pub key: String,
    /// `text` (at most 1 000 characters), `textarea` (65 536), `bool`, `number`,
    /// `select` (one of `options`), `color` (`#rgb` / `#rrggbb`), `media` (a media item
    /// id) or `secret` (plugins only: 1–4 096 characters, write-only).
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    /// The declared default; `null` for none (always for `media` and `secret`).
    pub default: Option<Value>,
    /// The choices of a `select`; empty otherwise.
    pub options: Vec<String>,
}

impl From<&SettingDef> for SettingDefDto {
    fn from(d: &SettingDef) -> Self {
        Self {
            key: d.key.clone(),
            kind: d.kind.as_str().to_owned(),
            label: d.label.clone(),
            default: d.default.clone(),
            options: d.options.clone(),
        }
    }
}

fn schema_dto(schema: &SettingsSchema) -> Vec<SettingDefDto> {
    schema.0.iter().map(SettingDefDto::from).collect()
}

/// `GET /themes/{name}/settings`: the theme's schema and every declared setting's value
/// as its templates see it (the stored value, else the default, else the type's empty
/// value).
#[derive(Debug, Serialize, ToSchema)]
pub struct ThemeSettingsDto {
    pub schema: Vec<SettingDefDto>,
    pub values: Map<String, Value>,
}

impl ThemeSettingsDto {
    pub fn new(schema: &SettingsSchema, values: Map<String, Value>) -> Self {
        Self {
            schema: schema_dto(schema),
            values,
        }
    }
}

/// `GET /plugins/{name}/settings`: the plugin's schema, the value of every non-secret
/// setting as the plugin reads it (the stored value if it fits the definition, else the
/// default, else `null`), for each `secret` only whether it is set, and the keys whose
/// stored value does not fit. A secret's value is never returned.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginSettingsDto {
    pub schema: Vec<SettingDefDto>,
    pub values: Map<String, Value>,
    /// Secret key -> whether a usable value is set (sealed for this plugin and key).
    pub secrets: BTreeMap<String, bool>,
    /// Keys whose stored value does not fit the current definition (its type changed in
    /// an update): they read as the default (a secret as unset) until written again.
    pub invalid: Vec<String>,
}

impl From<PluginSettingsView> for PluginSettingsDto {
    fn from(v: PluginSettingsView) -> Self {
        Self {
            schema: schema_dto(&v.schema),
            values: v.values,
            secrets: v.secrets,
            invalid: v.invalid,
        }
    }
}

/// `PATCH /themes/{name}/settings` and `PATCH /plugins/{name}/settings` (the schema
/// only): an object of declared keys to their new values; `null` resets a key to its
/// default (for a secret: deletes it). Keys not in the body stay as they are.
#[derive(ToSchema)]
#[schema(value_type = Object)]
pub struct SettingsPatch(pub Map<String, Value>);
