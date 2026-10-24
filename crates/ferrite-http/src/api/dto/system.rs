//! Bodies of the `/system` endpoints (M2a-2b P15, P18). Node error texts in `meta` are
//! scrubbed (no paths, endpoints, addresses or credentials); audit `details` never hold
//! secrets (M2a P22); job payloads are never returned (P19).

use chrono::{DateTime, Utc};
use ferrite_cache::CacheStats;
use ferrite_domain::{AuditEntry, Job, Node};
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
    /// What the node reports (04 §8: `theme`, `plugins`, `plugins_error`,
    /// `permalinks_error`, `site_id`, `site_url`, `mode`, `clock_skew_ms`); error texts
    /// scrubbed of URLs, paths, addresses and credentials, at most 1 KiB each.
    #[schema(value_type = Object)]
    pub meta: Value,
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
            meta: node.meta,
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
/// it started) plus the site's job counts; time series are M4 (P20).
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
