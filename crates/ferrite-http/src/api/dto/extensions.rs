//! Bodies of the `/plugins` and `/themes` endpoints. Responses never carry artifact
//! bytes or secrets: a plugin is described by its row (name, version, state, times), the
//! permission report of its stored grant, and what live nodes report about it; a theme
//! by its row or embedded manifest and what live nodes serve. Settings carry the
//! schema and the values, and for a plugin `secret` only whether it is set.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ferrite_core::{
    InstallTrust, RetainedPluginData, SignerSource, TrustDecision,
    diagnostics::scrub,
    ext_settings::{PluginSettingsView, SettingDef, SettingsSchema},
    owner_label,
};
use ferrite_domain::plugin_trust::{InstallSource, KeySource, OwnerId};
use ferrite_domain::{FailureBucket, InstalledPlugin, InstalledTheme, Node};
use ferrite_plugin_host::{HookStats, LimitItem, PermissionItem, PermissionReport, PluginMeta};
use serde::{Deserialize, Serialize};
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

/// What a plugin manifest's `[plugin]` table says about the plugin beyond its name
/// and version: text its author wrote (render it as text; the publisher is not
/// verified, M4 "Signed plugins").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginFacts {
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
}

impl PluginFacts {
    /// From a parsed manifest (an upload).
    pub fn of(meta: &PluginMeta) -> Self {
        Self {
            description: meta.description.clone(),
            publisher: Some(meta.publisher.clone()),
            license: meta.license.clone(),
            homepage: web_url(meta.homepage.as_deref()),
        }
    }

    /// From a row's stored manifest (`manifest.plugin`); a key it lacks is `None`.
    pub fn stored(manifest: &Value) -> Self {
        let text = |key: &str| {
            manifest
                .get("plugin")
                .and_then(|p| p.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Self {
            description: text("description"),
            publisher: text("publisher"),
            license: text("license"),
            homepage: web_url(text("homepage").as_deref()),
        }
    }
}

/// `url` if it is an `http(s)://` URL (`ferrite_domain::is_web_url`), else `None`: a row
/// stored before uploads were checked never hands the admin another scheme to link.
fn web_url(url: Option<&str>) -> Option<String> {
    url.filter(|u| ferrite_domain::is_web_url(u))
        .map(str::to_owned)
}

/// The report of a plugin upload, as the admin approves it (`POST /plugins/inspect`,
/// and the `report` of an approval refusal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PermissionReportDto {
    #[serde(flatten)]
    pub report: GrantReportDto,
    /// Whether a plugin of this name is installed: `added` is then against its grant.
    pub update: bool,
    /// The manifest's `plugin.description` (text).
    pub description: Option<String>,
    /// The manifest's `plugin.publisher` (text, unverified).
    pub publisher: Option<String>,
    /// The manifest's `plugin.license` (text).
    pub license: Option<String>,
    /// The manifest's `plugin.homepage`: an `http://` or `https://` URL (else `null`).
    pub homepage: Option<String>,
    /// Who signed it and whose data it gets (M6-2, design 14 §7.2).
    pub trust: TrustDto,
}

impl PermissionReportDto {
    pub fn new(r: &PermissionReport, update: bool, facts: PluginFacts, trust: TrustDto) -> Self {
        Self {
            report: r.into(),
            update,
            description: facts.description,
            publisher: facts.publisher,
            license: facts.license,
            homepage: facts.homepage,
            trust,
        }
    }
}

/// A registry publisher as the index names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PublisherRefDto {
    pub id: String,
    pub name: String,
}

/// The key that signed a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SignerDto {
    /// 16 uppercase hex digits, as minisign prints it.
    pub key_id: String,
    /// `registry` (listed by the configured registry), `approved` (approved at an earlier
    /// install), `operator` (`ferrite plugin keys trust`) or `new` (approving trusts it).
    pub source: String,
    /// The registry publisher that lists the key.
    pub publisher: Option<PublisherRefDto>,
    /// The manifest's `plugin.publisher` (text; verified only when `publisher` is set).
    pub claimed: String,
}

/// What an install does to the owner of the plugin name's data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerChangeDto {
    /// `bind`, `bind_unsigned`, `same`, `claim` or `unsigned_update`.
    pub change: String,
    /// The current owner (`reg:<key>:<publisher>` or `key:<key>`); `null` when unsigned
    /// or none.
    pub current: Option<String>,
    /// The owner after the install.
    pub next: Option<String>,
    /// `next` in words ("FerriteCMS (registry)", "key 5A1C…").
    pub next_label: Option<String>,
}

/// How a plugin arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SourceDto {
    /// `upload`, `folder` or `registry`.
    pub kind: String,
    /// The registry index URL, for `registry`.
    pub url: Option<String>,
    /// The registry version, for `registry`.
    pub version: Option<String>,
}

/// A trust refusal (the install would be refused with this `422`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TrustRefusalDto {
    /// `signature` or `publisher`.
    pub field: String,
    pub message: String,
}

/// The trust part of a permission report (design 14 §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TrustDto {
    /// `[plugins] signatures`: `any`, `signed` or `trusted`.
    pub policy: String,
    /// `verified` (a signature verified) or `unsigned`.
    pub signature: String,
    pub signer: Option<SignerDto>,
    /// `null` when the install is refused (`refusal`).
    pub owner: Option<OwnerChangeDto>,
    pub source: SourceDto,
    /// `unsigned`, `new_key`, `unsigned_data_claimed`, `unsigned_reads_data`.
    pub warnings: Vec<String>,
    /// Set when an install would be refused.
    pub refusal: Option<TrustRefusalDto>,
}

fn signer_source(s: &SignerSource) -> &'static str {
    match s {
        SignerSource::Registry { .. } => "registry",
        SignerSource::Approved => "approved",
        SignerSource::Operator => "operator",
        SignerSource::New => "new",
    }
}

impl TrustDto {
    /// From an inspection: its input, its decision (or refusal) and whether its report
    /// carries a signer.
    pub fn new(
        trust: &InstallTrust,
        decision: Result<&TrustDecision, (&'static str, &str)>,
        signed: bool,
        version: &str,
    ) -> Self {
        let registry = trust.registry.as_ref();
        let source = SourceDto {
            kind: trust.source.as_str().to_owned(),
            url: (trust.source == InstallSource::Registry)
                .then(|| registry.map(|r| r.url.clone()))
                .flatten(),
            version: (trust.source == InstallSource::Registry).then(|| version.to_owned()),
        };
        let signature = if signed { "verified" } else { "unsigned" }.to_owned();
        match decision {
            Ok(d) => Self {
                policy: trust.policy.as_str().to_owned(),
                signature,
                signer: d.signer.as_ref().map(|v| SignerDto {
                    key_id: v.key_id.to_string(),
                    source: signer_source(&v.source).to_owned(),
                    publisher: match &v.source {
                        SignerSource::Registry { publisher_id, name } => Some(PublisherRefDto {
                            id: publisher_id.clone(),
                            name: name.clone(),
                        }),
                        _ => None,
                    },
                    claimed: v.claimed.clone(),
                }),
                owner: Some(OwnerChangeDto {
                    change: d.owner_change.as_str().to_owned(),
                    current: d.current_owner.as_ref().map(ToString::to_string),
                    next: d.next_owner.as_ref().map(ToString::to_string),
                    next_label: d.next_owner.as_ref().map(|o| owner_label(o, registry)),
                }),
                source,
                warnings: d.warnings.iter().map(|w| w.as_str().to_owned()).collect(),
                refusal: None,
            },
            Err((field, message)) => Self {
                policy: trust.policy.as_str().to_owned(),
                signature,
                signer: None,
                owner: None,
                source,
                warnings: Vec::new(),
                refusal: Some(TrustRefusalDto {
                    field: field.to_owned(),
                    message: message.to_owned(),
                }),
            },
        }
    }
}

/// The owner of an installed plugin's data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerDto {
    /// `reg:<registry key>:<publisher>` or `key:<key id>`.
    pub id: String,
    /// In words: "FerriteCMS (registry)" or "key 5A1C…".
    pub label: String,
}

/// An update the cached registry index offers for an installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct UpdateDto {
    pub version: String,
    /// Whether the registry's publisher is this plugin's owner (`false` for an
    /// unsigned-owned plugin: updating binds its data to that publisher).
    pub publisher_matches: bool,
}

/// An installed plugin's trust (design 14 §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct InstalledTrustDto {
    /// `verified` (a registry publisher's key), `signed` (a key of this site),
    /// `unsigned`, or `revoked` (its signing key is revoked).
    pub state: String,
    pub owner: Option<OwnerDto>,
    pub signer: Option<SignerDto>,
    /// How the installed version arrived; `null` for one installed before M6-2.
    pub source: Option<SourceDto>,
    pub update: Option<UpdateDto>,
}

impl InstalledTrustDto {
    /// From the listing's [`TrustContext`](crate::extensions::TrustContext).
    pub fn of(ctx: &crate::extensions::TrustContext, row: &InstalledPlugin) -> Self {
        let name = row.name.as_str();
        let registry = ctx.registry.as_ref();
        let update = registry.and_then(|r| {
            crate::extensions::update_for(&r.index, &r.registry_key, row, ctx.owner(name)).map(
                |(version, publisher_matches)| UpdateDto {
                    version,
                    publisher_matches,
                },
            )
        });
        let Some(o) = ctx.owner(name) else {
            return Self {
                state: "unsigned".into(),
                owner: None,
                signer: None,
                source: None,
                update,
            };
        };
        let listed = |id| registry.and_then(|r| r.index.publisher_of_key(id));
        let signer = o.signer_key.as_ref().map(|k| {
            let by_registry = listed(k);
            SignerDto {
                key_id: k.to_string(),
                source: if by_registry.is_some() {
                    "registry"
                } else {
                    match ctx.key(k).map(|t| t.source) {
                        Some(KeySource::Operator) => "operator",
                        _ => "approved",
                    }
                }
                .to_owned(),
                publisher: by_registry.map(|p| PublisherRefDto {
                    id: p.id.clone(),
                    name: p.name.clone(),
                }),
                claimed: o.publisher.clone(),
            }
        });
        let state = match (&o.signer_key, &o.owner) {
            (None, _) => "unsigned",
            (Some(k), _) if ctx.revoked(k) => "revoked",
            (
                Some(k),
                Some(OwnerId::Registry {
                    registry_key,
                    publisher,
                }),
            ) if registry.is_some_and(|r| r.registry_key == *registry_key)
                && listed(k).is_some_and(|p| p.id == *publisher) =>
            {
                "verified"
            }
            (Some(_), _) => "signed",
        };
        Self {
            state: state.to_owned(),
            owner: o.owner.as_ref().map(|id| OwnerDto {
                id: id.to_string(),
                label: owner_label(id, registry),
            }),
            signer,
            source: Some(SourceDto {
                kind: o.source.as_str().to_owned(),
                url: o.registry_url.clone(),
                version: None,
            }),
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
    /// The stored manifest's `plugin.description` (text).
    pub description: Option<String>,
    /// The stored manifest's `plugin.publisher` (text, unverified).
    pub publisher: Option<String>,
    /// The stored manifest's `plugin.license` (text).
    pub license: Option<String>,
    /// The stored manifest's `plugin.homepage`: an `http://` or `https://` URL (a stored
    /// value of another form is `null`).
    pub homepage: Option<String>,
    /// The report of the stored grant: `added` is what it does not cover (empty for a
    /// plugin installed with approval), `fingerprint` the approved one. It has no
    /// `update`: rows keep no previous version. `null` when the stored manifest or
    /// grant no longer parses (the plugin does not load).
    pub report: Option<GrantReportDto>,
    /// What each live node (heartbeat within 30 s) reports about it, by node; a node
    /// that has not loaded the plugin yet is not listed.
    pub nodes: Vec<PluginNodeState>,
    /// Who signed it, whose data it holds, and an update the registry offers (M6-2).
    pub trust: InstalledTrustDto,
}

impl PluginDto {
    pub fn new(
        row: InstalledPlugin,
        report: Option<GrantReportDto>,
        nodes: Vec<PluginNodeState>,
        trust: InstalledTrustDto,
    ) -> Self {
        let facts = PluginFacts::stored(&row.manifest);
        Self {
            name: row.name,
            version: row.version,
            enabled: row.enabled,
            disabled_reason: row.disabled_reason,
            installed_at: row.installed_at.as_datetime(),
            updated_at: row.updated_at.as_datetime(),
            description: facts.description,
            publisher: facts.publisher,
            license: facts.license,
            homepage: facts.homepage,
            report,
            nodes,
            trust,
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

/// A deleted plugin whose KV and settings are still kept (M2b-3 P47).
#[derive(Debug, Serialize, ToSchema)]
pub struct RetainedPluginDto {
    pub name: String,
    pub deleted_at: DateTime<Utc>,
    /// When the data is erased unless the plugin is installed again first: 30 days
    /// after `deleted_at` (`POST /plugins/{name}/purge` erases it earlier).
    pub purge_after: DateTime<Utc>,
}

impl From<RetainedPluginData> for RetainedPluginDto {
    fn from(r: RetainedPluginData) -> Self {
        Self {
            name: r.name,
            deleted_at: r.deleted_at.as_datetime(),
            purge_after: r.purge_after.as_datetime(),
        }
    }
}

/// `GET /plugins/retained`: every deleted plugin whose data is kept, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetainedPluginList {
    pub items: Vec<RetainedPluginDto>,
}

/// `POST /plugins/inspect` (`multipart/form-data`; the schema only, the handler reads
/// the parts itself). `manifest` and `component` are required; an unknown or repeated
/// part is refused.
#[derive(ToSchema)]
pub struct PluginInspectUpload {
    /// `ferrite-plugin.toml` (UTF-8, at most 64 KiB).
    pub manifest: String,
    /// The component (`.wasm`, at most 32 MiB).
    #[schema(value_type = String, format = Binary)]
    pub component: Vec<u8>,
    /// `ferrite-plugin.minisig`: the publisher's minisign signature over the signing
    /// payload (UTF-8, at most 4 KiB).
    pub signature: Option<String>,
    /// `ferrite-plugin.pub`: the publisher's minisign public key (at most 1 KiB), for a
    /// key this site does not know yet.
    pub publisher_key: Option<String>,
}

/// `POST /plugins` (`multipart/form-data`; the schema only). `manifest`, `component`
/// and `approve` are required; an unknown or repeated part is refused.
#[derive(ToSchema)]
pub struct PluginUpload {
    /// `ferrite-plugin.toml` (UTF-8, at most 64 KiB).
    pub manifest: String,
    /// The component (`.wasm`, at most 32 MiB).
    #[schema(value_type = String, format = Binary)]
    pub component: Vec<u8>,
    /// `ferrite-plugin.minisig` (UTF-8, at most 4 KiB).
    pub signature: Option<String>,
    /// `ferrite-plugin.pub` (at most 1 KiB).
    pub publisher_key: Option<String>,
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

/// A menu location a theme declares (its `[menus]` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ThemeMenuDto {
    /// The location, and the name of the menu the theme shows there.
    pub name: String,
    pub label: String,
}

/// An image variant size a theme wants (its `[images]` table, 03 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ThemeImageDto {
    /// The size's name (`[a-z0-9_-]`), the key of its `[images]` entry.
    pub name: String,
    /// In pixels.
    pub width: u32,
    /// In pixels; `null`: scaled to `width` keeping the aspect ratio, or with `crop` a
    /// `width` square.
    pub height: Option<u32>,
    /// Whether the variant is cropped to `width` x `height`.
    pub crop: bool,
}

/// A theme: installed (a row) or embedded (no row). The manifest facts (`author`,
/// `license`, `homepage`, `description`, `menus`, `images`, `has_settings`) are the
/// row's stored manifest, or the embedded theme's `theme.toml`; each is text the theme
/// author wrote (render it as text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ThemeDto {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    /// An `http://` or `https://` URL (`theme.toml` refuses anything else).
    pub homepage: Option<String>,
    /// Whether `theme.toml` declares settings (`GET /themes/{name}/settings` has a form).
    pub has_settings: bool,
    /// The menu locations, by name.
    pub menus: Vec<ThemeMenuDto>,
    /// The image variant sizes, by name.
    pub images: Vec<ThemeImageDto>,
    /// The row's artifact (SHA-256 of the bundle, 64 lowercase hex); `null` for an
    /// embedded theme without a row.
    pub hash: Option<String>,
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
        Self {
            hash: Some(row.artifact_hash.as_str().to_owned()),
            active: row.active,
            installed_at: Some(row.installed_at.as_datetime()),
            ..Self::from_manifest(
                &row.name,
                &row.version,
                &row.manifest,
                ThemeOrigin::Installed,
            )
        }
    }

    /// The entry of an embedded theme without a row; `manifest` is the object a row
    /// would store for it (`theme_row_manifest`).
    pub fn embedded(name: &str, version: &str, manifest: &Value) -> Self {
        Self::from_manifest(name, version, manifest, ThemeOrigin::Embedded)
    }

    /// The facts of a stored manifest object. A row written before a key existed
    /// lacks it: no author, no menus, no settings.
    fn from_manifest(name: &str, version: &str, manifest: &Value, origin: ThemeOrigin) -> Self {
        let text = |key: &str| manifest.get(key).and_then(Value::as_str).map(str::to_owned);
        let menus = manifest
            .get("menus")
            .and_then(Value::as_object)
            .map(|table| {
                let mut menus: Vec<ThemeMenuDto> = table
                    .iter()
                    .filter_map(|(name, label)| {
                        Some(ThemeMenuDto {
                            name: name.clone(),
                            label: label.as_str()?.to_owned(),
                        })
                    })
                    .collect();
                menus.sort_by(|a, b| a.name.cmp(&b.name));
                menus
            })
            .unwrap_or_default();
        let images = manifest
            .get("images")
            .and_then(Value::as_object)
            .map(|table| {
                let mut images: Vec<ThemeImageDto> = table
                    .iter()
                    .filter_map(|(name, size)| {
                        let px = |key: &str| {
                            size.get(key)
                                .and_then(Value::as_u64)
                                .and_then(|n| u32::try_from(n).ok())
                        };
                        Some(ThemeImageDto {
                            name: name.clone(),
                            width: px("width")?,
                            height: px("height"),
                            crop: size.get("crop").and_then(Value::as_bool).unwrap_or(false),
                        })
                    })
                    .collect();
                images.sort_by(|a, b| a.name.cmp(&b.name));
                images
            })
            .unwrap_or_default();
        let has_settings = manifest
            .get("settings")
            .and_then(Value::as_array)
            .is_some_and(|defs| !defs.is_empty());
        Self {
            name: name.to_owned(),
            version: version.to_owned(),
            description: text("description"),
            author: text("author"),
            license: text("license"),
            homepage: web_url(text("homepage").as_deref()),
            has_settings,
            menus,
            images,
            hash: None,
            origin,
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

/// Failures of one hook in one hour, every node's (the shared counters, P20).
#[derive(Debug, Serialize, ToSchema)]
pub struct FailureBucketDto {
    /// `render.head`, `post.saved`, `route GET /path`, ...
    pub hook: String,
    /// The start of the hour (UTC).
    pub bucket_start: DateTime<Utc>,
    pub count: u64,
}

impl From<FailureBucket> for FailureBucketDto {
    fn from(b: FailureBucket) -> Self {
        Self {
            hook: b.hook,
            bucket_start: b.bucket_start.as_datetime(),
            count: b.count,
        }
    }
}

/// One hook's calls on the answering node since it started.
#[derive(Debug, Serialize, ToSchema)]
pub struct HookStatsDto {
    pub hook: String,
    /// Calls that ran (successes, failures and budget cuts).
    pub calls: u64,
    /// Calls that failed (counted toward auto-disable).
    pub failures: u64,
    /// Calls skipped for want of a call slot (not failures).
    pub busy: u64,
    /// Calls cut short or skipped by the page's render budget (not failures).
    pub budget_cut: u64,
    /// Summed duration of the calls that ran, in microseconds.
    pub total_micros: u64,
    /// The longest call, in microseconds.
    pub max_micros: u64,
    /// Calls that ran by duration: at most 1, 5, 25, 100 and 500 ms, and longer.
    pub buckets: [u64; 6],
}

impl From<HookStats> for HookStatsDto {
    fn from(s: HookStats) -> Self {
        Self {
            hook: s.hook,
            calls: s.calls,
            failures: s.failures,
            busy: s.busy,
            budget_cut: s.budget_cut,
            total_micros: s.total_micros,
            max_micros: s.max_micros,
            buckets: s.buckets,
        }
    }
}

/// The answering node's part of [`PluginMetricsDto`].
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeHookStatsDto {
    pub id: Uuid,
    /// When the node started counting.
    pub since: DateTime<Utc>,
    /// By hook name; at most 64.
    pub hooks: Vec<HookStatsDto>,
}

/// `GET /plugins/{name}/metrics`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginMetricsDto {
    /// The window of `failures`: 86400 (24 hours).
    pub window_secs: u64,
    /// Failures per hook and hour in the window, every node's, newest hour first.
    pub failures: Vec<FailureBucketDto>,
    /// The answering node's call statistics (each node counts its own).
    pub node: NodeHookStatsDto,
}

/// The configured registry and its cached copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RegistryMetaDto {
    /// The index URL.
    pub url: String,
    /// The index's `name`; `null` before the first successful check.
    pub name: Option<String>,
    pub serial: Option<i64>,
    pub generated_at: Option<DateTime<Utc>>,
    /// The last successful check.
    pub checked_at: Option<DateTime<Utc>>,
    /// The last failed check's error, cleared by a successful one.
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
}

/// A registry plugin that is installed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RegistryInstalledDto {
    pub version: String,
    /// A newer version this binary runs is listed (and may be installed over it).
    pub update_available: bool,
    /// The registry publisher owns this plugin's data (`false`: unsigned-owned, an
    /// update binds its data to the registry publisher).
    pub publisher_matches: bool,
}

/// One plugin of the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RegistryPluginDto {
    pub name: String,
    pub publisher: PublisherRefDto,
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// The newest version this binary runs, not yanked; `null` when none.
    pub latest: Option<String>,
    /// Every listed version, newest first (yanked ones included).
    pub versions: Vec<RegistryVersionDto>,
    pub installed: Option<RegistryInstalledDto>,
}

/// One listed version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RegistryVersionDto {
    pub version: String,
    pub world: String,
    pub published_at: Option<DateTime<Utc>>,
    /// Why it was yanked; `null` when it is not.
    pub yanked: Option<String>,
}

/// `GET /plugins/registry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RegistryDto {
    /// `null` when no registry is configured.
    pub registry: Option<RegistryMetaDto>,
    /// The cached index's plugins matching `q`, by name (at most 50).
    pub plugins: Vec<RegistryPluginDto>,
}

impl From<crate::extensions::RegistryView> for RegistryDto {
    fn from(v: crate::extensions::RegistryView) -> Self {
        Self {
            registry: v.registry.map(|m| RegistryMetaDto {
                url: m.url,
                name: m.name,
                serial: m.serial,
                generated_at: m.generated_at.map(|t| t.as_datetime()),
                checked_at: m.checked_at.map(|t| t.as_datetime()),
                last_error: m.last_error,
                last_error_at: m.last_error_at.map(|t| t.as_datetime()),
            }),
            plugins: v
                .plugins
                .into_iter()
                .map(|p| {
                    let mut versions: Vec<RegistryVersionDto> = p
                        .plugin
                        .versions
                        .iter()
                        .map(|v| RegistryVersionDto {
                            version: v.version.clone(),
                            world: v.world.clone(),
                            published_at: v.published_at.map(|t| t.as_datetime()),
                            yanked: v.yanked.clone(),
                        })
                        .collect();
                    versions.sort_by(|a, b| {
                        match (
                            semver::Version::parse(&a.version),
                            semver::Version::parse(&b.version),
                        ) {
                            (Ok(a), Ok(b)) => b.cmp(&a),
                            _ => b.version.cmp(&a.version),
                        }
                    });
                    RegistryPluginDto {
                        name: p.plugin.name,
                        publisher: PublisherRefDto {
                            id: p.plugin.publisher,
                            name: p.publisher_name,
                        },
                        description: p.plugin.description,
                        homepage: p.plugin.homepage,
                        license: p.plugin.license,
                        latest: p.latest,
                        versions,
                        installed: p.installed.map(|i| RegistryInstalledDto {
                            version: i.version,
                            update_available: i.update_available,
                            publisher_matches: i.publisher_matches,
                        }),
                    }
                })
                .collect(),
        }
    }
}

/// `POST /plugins/registry/inspect`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryInspectRequest {
    pub name: String,
    /// Default: the newest version this binary runs, not yanked.
    pub version: Option<String>,
}

/// `POST /plugins/registry/install`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryInstallRequest {
    pub name: String,
    pub version: String,
    /// The `fingerprint` of the report `POST /plugins/registry/inspect` gave.
    pub approve: String,
}

/// A publisher key this site knows: its own (approved, operator), the configured
/// registry's, and every revocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PublisherKeyDto {
    pub key_id: String,
    /// The minisign public key (base64 line).
    pub public_key: String,
    pub label: String,
    /// `approved`, `operator` or `registry`.
    pub source: String,
    /// The registry publisher that lists it.
    pub registry_publisher: Option<PublisherRefDto>,
    /// When this site trusted it; `null` for a registry key with no row here.
    pub trusted_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_reason: Option<String>,
    pub compromised: bool,
    /// The installed or retained plugins whose current version it signed.
    pub plugins: Vec<String>,
}

/// `GET /plugins/publishers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PublisherKeyList {
    pub items: Vec<PublisherKeyDto>,
    /// `[plugins] signatures`.
    pub policy: String,
}

/// `POST /plugins/publishers/{key_id}/revoke`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeKeyRequest {
    /// Why (1..=500 characters; shown with the key and on disabled plugins).
    pub reason: String,
    /// The key was stolen or leaked: every installed plugin it signed is disabled.
    #[serde(default)]
    pub compromised: bool,
}
