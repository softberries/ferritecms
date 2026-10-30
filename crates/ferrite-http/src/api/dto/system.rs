//! Bodies of the `/system` endpoints (M2a-2b P15, P18). Node error texts in `meta` are
//! scrubbed (no paths, endpoints, addresses or credentials); audit `details` never hold
//! secrets (M2a P22); job payloads are never returned (P19).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ferrite_cache::CacheStats;
use ferrite_core::MetricsSeries;
use ferrite_domain::{AuditEntry, Job, Node, SeriesBucket, SeriesStep};
use ferrite_store::Compat;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// How the database's schema relates to this binary (02 §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchemaVerdict {
    /// Every migration this binary knows is applied, and no newer one.
    UpToDate,
    /// Migrations are pending (`ferrite migrate up`).
    NeedsUp,
    /// The database is newer, but its expand-only migrations let this binary run.
    NewerSchemaOk,
    /// The database needs a newer binary.
    Incompatible,
}

impl From<&Compat> for SchemaVerdict {
    fn from(c: &Compat) -> Self {
        match c {
            Compat::UpToDate => Self::UpToDate,
            Compat::NeedsUp => Self::NeedsUp,
            Compat::NewerSchemaOk => Self::NewerSchemaOk,
            Compat::Incompatible { .. } => Self::Incompatible,
        }
    }
}

/// The schema part of [`SystemStatusDto`].
#[derive(Debug, Serialize, ToSchema)]
pub struct SchemaStatusDto {
    /// The highest applied migration.
    pub db_version: i64,
    /// The highest migration this binary embeds.
    pub binary_version: i64,
    /// Embedded migrations not applied yet, oldest first.
    pub pending: Vec<i64>,
    pub verdict: SchemaVerdict,
}

/// The theme part of [`SystemStatusDto`].
#[derive(Debug, Serialize, ToSchema)]
pub struct ThemeStatusDto {
    /// The active theme row; `null`: none is active and nodes serve the embedded
    /// default.
    pub active: Option<String>,
    /// Names of the live nodes that do not serve the active row's artifact
    /// (`nodes.meta.theme.fallback`, 06 §1).
    pub fallback_nodes: Vec<String>,
}

/// Jobs per status.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobCountsDto {
    pub queued: u64,
    pub running: u64,
    pub done: u64,
    pub dead: u64,
}

/// Registered nodes by heartbeat age (live: within 30 s of the database clock).
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeCountsDto {
    pub live: u32,
    pub stale: u32,
}

/// The node that answered.
#[derive(Debug, Serialize, ToSchema)]
pub struct ThisNodeDto {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
}

/// `GET /system/status`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SystemStatusDto {
    /// The FerriteCMS version of the node that answered.
    pub version: String,
    /// `site.url`.
    pub site_url: String,
    /// `solo` or `cluster`.
    pub mode: String,
    /// `sqlite` or `postgres`.
    pub database: String,
    /// `local` or `s3`.
    pub media: String,
    pub schema: SchemaStatusDto,
    pub theme: ThemeStatusDto,
    pub jobs: JobCountsDto,
    pub nodes: NodeCountsDto,
    pub node: ThisNodeDto,
    pub mail: MailStatusDto,
}

/// Outgoing mail (P49): `[mail]` in `ferrite.toml` is the only place to set it; the SMTP
/// URL (it can hold credentials) is never shown.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailStatusDto {
    /// `[mail]` has `smtp_url` and `from`: password resets, plugin `notify-admins` and
    /// the test mail can send.
    pub configured: bool,
    /// `mail.from`; `null` when not configured.
    pub from: Option<String>,
}

/// `POST /system/mail/test`: the mail is queued.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailTestDto {
    /// The actor's own address, where the mail goes.
    pub to: String,
}

/// A registered node (`nodes` row).
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeDto {
    pub id: Uuid,
    /// `cluster.node_name` or the host name.
    pub name: String,
    /// The binary version it runs.
    pub version: String,
    /// The highest migration its binary knows.
    pub schema_version: i64,
    pub started_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    /// The heartbeat is within 30 s of the database clock.
    pub live: bool,
    /// What the node reports (04 §8); error texts scrubbed of URLs, paths, addresses and
    /// credentials, at most 1 KiB each.
    pub meta: NodeMetaDto,
}

/// A node's `meta`, read leniently: every member is optional (a node of another version
/// in a rolling deploy may lack or add members), unknown members are dropped, and a
/// member of the wrong type is `null` rather than failing the list.
#[derive(Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct NodeMetaDto {
    /// The theme this node serves.
    pub theme: Option<NodeThemeDto>,
    /// The node's plugins by name, from its last reload.
    pub plugins: Option<BTreeMap<String, NodePluginDto>>,
    /// Why the plugin rows could not be read at the last reload.
    pub plugins_error: Option<String>,
    /// Why the current permalink options are not in effect.
    pub permalinks_error: Option<String>,
    /// The job kinds this node's runner handles.
    pub job_kinds: Option<Vec<String>>,
    /// The site the node joined (cluster registry).
    pub site_id: Option<String>,
    /// The node's `site.url`.
    pub site_url: Option<String>,
    /// `solo` or `cluster`.
    pub mode: Option<String>,
    /// The node clock minus the database clock at its last heartbeat, in milliseconds.
    pub clock_skew_ms: Option<i64>,
}

/// `meta.theme` of a node.
#[derive(Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct NodeThemeDto {
    pub name: Option<String>,
    /// The theme's content hash.
    pub hash: Option<String>,
    pub origin: Option<NodeThemeOrigin>,
    /// Why the active theme is not served (scrubbed).
    pub error: Option<String>,
    /// There is an active row and this node does not serve its artifact.
    pub fallback: Option<bool>,
}

/// Where a node's theme came from: `"embedded"`, `"folder"` or `{"artifact": <hash>}`.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum NodeThemeOrigin {
    /// `embedded` or `folder`.
    Named(String),
    /// An installed theme's artifact.
    Artifact { artifact: String },
}

/// `meta.plugins.<name>` of a node.
#[derive(Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct NodePluginDto {
    pub version: Option<String>,
    /// The component's content hash.
    pub hash: Option<String>,
    /// `loaded`, `failed` or `disabled`.
    pub status: Option<String>,
    /// Why it is not loaded (scrubbed).
    pub error: Option<String>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl NodeMetaDto {
    /// Reads `meta` (already scrubbed by core) member by member; see the type docs.
    pub fn from_value(meta: &Value) -> Self {
        let theme = meta
            .get("theme")
            .filter(|t| t.is_object())
            .map(|t| NodeThemeDto {
                name: text(t, "name"),
                hash: text(t, "hash"),
                origin: t.get("origin").and_then(|o| match o {
                    Value::String(s) => Some(NodeThemeOrigin::Named(s.clone())),
                    other => text(other, "artifact")
                        .map(|artifact| NodeThemeOrigin::Artifact { artifact }),
                }),
                error: text(t, "error"),
                fallback: t.get("fallback").and_then(Value::as_bool),
            });
        let plugins = meta.get("plugins").and_then(Value::as_object).map(|map| {
            map.iter()
                .filter(|(_, p)| p.is_object())
                .map(|(name, p)| {
                    (
                        name.clone(),
                        NodePluginDto {
                            version: text(p, "version"),
                            hash: text(p, "hash"),
                            status: text(p, "status"),
                            error: text(p, "error"),
                        },
                    )
                })
                .collect()
        });
        let job_kinds = meta
            .get("job_kinds")
            .and_then(Value::as_array)
            .map(|kinds| {
                kinds
                    .iter()
                    .filter_map(|k| k.as_str().map(str::to_owned))
                    .collect()
            });
        Self {
            theme,
            plugins,
            plugins_error: text(meta, "plugins_error"),
            permalinks_error: text(meta, "permalinks_error"),
            job_kinds,
            site_id: text(meta, "site_id"),
            site_url: text(meta, "site_url"),
            mode: text(meta, "mode"),
            clock_skew_ms: meta.get("clock_skew_ms").and_then(Value::as_i64),
        }
    }
}

impl NodeDto {
    pub fn new(node: Node, live: bool) -> Self {
        Self {
            id: node.id.as_uuid(),
            name: node.name,
            version: node.version,
            schema_version: node.schema_version,
            started_at: node.started_at.as_datetime(),
            heartbeat_at: node.heartbeat_at.as_datetime(),
            live,
            meta: NodeMetaDto::from_value(&node.meta),
        }
    }
}

/// `GET /system/nodes`: every registered node, live and stale, by id.
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeList {
    pub items: Vec<NodeDto>,
    /// Always `null`: one page holds every node.
    pub next_cursor: Option<String>,
}

/// Who wrote an audit row.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditActorDto {
    pub id: Uuid,
    pub login: String,
}

/// An audit-log row.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditDto {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    /// `null`: the operator path (CLI), an unauthenticated flow (password reset), or a
    /// deleted user.
    pub actor: Option<AuditActorDto>,
    /// `<target>.<verb>`, e.g. `settings.update`, `plugin.install`.
    pub action: String,
    pub target_kind: String,
    pub target_id: Option<String>,
    /// What changed (never secrets or setting values).
    #[schema(value_type = Object)]
    pub details: Value,
}

impl AuditDto {
    /// `entry` with its actor's login (`login` = `None`: the user is gone).
    pub fn new(entry: AuditEntry, login: Option<String>) -> Self {
        let actor = entry.actor_id.zip(login).map(|(id, login)| AuditActorDto {
            id: id.as_uuid(),
            login,
        });
        Self {
            id: entry.id.as_uuid(),
            at: entry.at.as_datetime(),
            actor,
            action: entry.action,
            target_kind: entry.target_kind,
            target_id: entry.target_id,
            details: entry.details,
        }
    }
}

/// `GET /system/audit`: one page, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditList {
    pub items: Vec<AuditDto>,
    pub next_cursor: Option<String>,
}

/// A job, without its payload (P19): a handler's input can hold anything, and an
/// operator needs the state, not the data.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobDto {
    pub id: Uuid,
    /// `[a-z0-9._-]{1,64}`, e.g. `media.variants`.
    pub kind: String,
    /// `queued`, `running`, `done` or `dead`.
    pub status: String,
    /// Claims so far (including the current one while `running`).
    pub attempts: i32,
    pub max_attempts: i32,
    /// Higher runs first.
    pub priority: i32,
    /// When it is due (the next attempt, while `queued`).
    pub run_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Why the last attempt failed, scrubbed of URLs, paths, addresses and credentials
    /// (the node's log has the full text); kept by a retry.
    pub last_error: Option<String>,
    /// The node running it.
    pub locked_by: Option<Uuid>,
    /// Some live node runs jobs of this kind (its `meta.job_kinds`). `false`: the job
    /// waits until a node that knows the kind joins (e.g. one running a newer
    /// version, or with the plugin's handler). `null`: unknown; only a retry answers
    /// it, when the node list could not be read after the retry committed.
    pub handled: Option<bool>,
}

impl JobDto {
    pub fn new(job: Job, handled: Option<bool>) -> Self {
        Self {
            id: job.id.as_uuid(),
            kind: job.kind,
            status: job.status.as_str().to_owned(),
            attempts: job.attempts,
            max_attempts: job.max_attempts,
            priority: job.priority,
            run_at: job.run_at.as_datetime(),
            created_at: job.created_at.as_datetime(),
            finished_at: job.finished_at.map(|t| t.as_datetime()),
            last_error: job.last_error,
            locked_by: job.locked_by.map(|n| n.as_uuid()),
            handled,
        }
    }
}

/// `GET /system/jobs`: one page, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobList {
    pub items: Vec<JobDto>,
    pub next_cursor: Option<String>,
}

/// The page cache of the answering node.
#[derive(Debug, Serialize, ToSchema)]
pub struct CacheStatsDto {
    /// Pages held.
    pub entries: u64,
    /// Their weight in bytes.
    pub bytes: u64,
    /// Lookups that found a page since the node started.
    pub hits: u64,
    /// Lookups that did not.
    pub misses: u64,
}

impl From<CacheStats> for CacheStatsDto {
    fn from(s: CacheStats) -> Self {
        Self {
            entries: s.entries,
            bytes: s.bytes,
            hits: s.hits,
            misses: s.misses,
        }
    }
}

/// Plugin calls on the answering node since it started.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginTotalsDto {
    /// Calls that ran.
    pub calls: u64,
    /// Calls that failed (counted toward auto-disable).
    pub failures: u64,
}

/// `GET /system/metrics/summary`: a snapshot of the answering node (its counters since
/// it started) plus the site's job counts (P20); time series: `MetricsSeriesDto`.
#[derive(Debug, Serialize, ToSchema)]
pub struct MetricsSummaryDto {
    pub node: ThisNodeDto,
    /// Template renders (pages and error pages) since the node started.
    pub renders: u64,
    pub page_cache: CacheStatsDto,
    pub plugins: PluginTotalsDto,
    /// Jobs per status, every node's.
    pub jobs: JobCountsDto,
}

/// The bucket width of a metrics series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeriesStepDto {
    Minute,
    Hour,
    Day,
}

impl From<SeriesStep> for SeriesStepDto {
    fn from(s: SeriesStep) -> Self {
        match s {
            SeriesStep::Minute => Self::Minute,
            SeriesStep::Hour => Self::Hour,
            SeriesStep::Day => Self::Day,
        }
    }
}

/// One bucket of `GET /system/metrics/series`: the minutes starting in
/// `[start, start + step)`, summed over the nodes (zeros where none wrote a row).
#[derive(Debug, Serialize, ToSchema)]
pub struct SeriesBucketDto {
    pub start: DateTime<Utc>,
    /// Responses of every route class but `other` (health checks).
    pub requests: u64,
    /// Those of `requests` with a 5xx status.
    pub errors_5xx: u64,
    /// HTML page views: `200` answers and `304` revalidations of public pages (no bots,
    /// previews or logged-in requests).
    pub page_views: u64,
    /// Page cache lookups that found a page.
    pub cache_hits: u64,
    /// Page cache lookups that did not.
    pub cache_misses: u64,
    /// Plugin calls that ran.
    pub plugin_calls: u64,
    /// Plugin calls that failed.
    pub plugin_failures: u64,
    /// Jobs completed.
    pub jobs_done: u64,
    /// Job attempts that failed (retried or dead).
    pub jobs_failed: u64,
    /// The upper bound (ms) of the latency bucket holding the 95th percentile of the
    /// bucket's page responses (every minute's and node's together, from the summed
    /// `page_latency`); 0 without page responses. Bounds: 5, 10, 25, 50, 100, 250, 500,
    /// 1000, 2500, 5000 and 10000 (also for anything slower).
    pub p95_ms: u32,
    /// Page responses per latency bucket, 12 counts (not cumulative): at most 5, 10, 25,
    /// 50, 100, 250, 500, 1000, 2500, 5000 and 10000 ms, then over 10 s. Summed over
    /// several buckets they give the p95 of a longer range.
    #[schema(min_items = 12, max_items = 12)]
    pub page_latency: Vec<u64>,
}

impl From<SeriesBucket> for SeriesBucketDto {
    fn from(b: SeriesBucket) -> Self {
        let c = b.counts;
        Self {
            start: b.start.as_datetime(),
            requests: c.requests,
            errors_5xx: c.errors_5xx,
            page_views: c.page_views,
            cache_hits: c.cache_hits,
            cache_misses: c.cache_misses,
            plugin_calls: c.plugin_calls,
            plugin_failures: c.plugin_failures,
            jobs_done: c.jobs_done,
            jobs_failed: c.jobs_failed,
            p95_ms: c.p95_ms(),
            page_latency: c.page_latency.to_vec(),
        }
    }
}

/// `GET /system/metrics/series`: every bucket of the range, ascending (dense).
#[derive(Debug, Serialize, ToSchema)]
pub struct MetricsSeriesDto {
    /// The requested `from` rounded down to a bucket start (UTC, epoch-aligned).
    pub from: DateTime<Utc>,
    /// The requested `to` rounded up to a bucket end (exclusive).
    pub to: DateTime<Utc>,
    pub step: SeriesStepDto,
    /// The node filter, if one was given.
    pub node: Option<Uuid>,
    pub buckets: Vec<SeriesBucketDto>,
}

impl From<MetricsSeries> for MetricsSeriesDto {
    fn from(s: MetricsSeries) -> Self {
        Self {
            from: s.from.as_datetime(),
            to: s.to.as_datetime(),
            step: s.step.into(),
            node: s.node.map(|n| n.as_uuid()),
            buckets: s.buckets.into_iter().map(SeriesBucketDto::from).collect(),
        }
    }
}
