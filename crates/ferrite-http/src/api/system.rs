//! `/system` (07 §1; M2a-2b P15, P18): status, nodes and the audit log, for admins.
//!
//! Handlers translate; `SystemService` checks `manage_system` (admins) with the
//! `system:read` scope before anything is read, so editors and tokens without the
//! scope learn nothing (not even the version or the migration state). The public
//! liveness probe stays `GET /healthz`, which reveals none of this.
//!
//! * `GET /system/status`: versions, mode, backends, the schema verdict, the active
//!   theme and the live nodes serving a fallback, job counts, live and stale node
//!   counts, and the answering node.
//! * `GET /system/nodes`: every registered node with `meta`; its error texts are
//!   scrubbed again on read ([`ferrite_core::diagnostics::scrub`]), so a row written by
//!   an older node leaks no paths, endpoints or credentials either (Review Focus 5).
//! * `GET /system/audit?actor&action&target_kind&target_id&limit&cursor`: the audit
//!   log, newest first, with each actor's login; `action` ending in `.` is a prefix
//!   (`user.`), otherwise exact; `target_id` needs `target_kind` (`422`).
//! * `GET /system/jobs?status&kind&limit&cursor`: the jobs, newest first, without their
//!   payloads (P19), `last_error` scrubbed (F16), and whether a live node handles the
//!   kind (`handled`, from the nodes' `meta.job_kinds`).
//! * `POST /system/jobs/{id}/retry` (`system:write`): a `dead` job back to `queued`,
//!   due now with its attempts reset; audited `job.retry`. Anything but a dead job is
//!   `422` (field `status`).
//! * `GET /system/metrics/summary`: the answering node's renders, page cache and plugin
//!   call totals since it started, and the site's job counts (P20).
//! * `GET /system/metrics/series?from&to&step=minute|hour|day[&node]`: the nodes'
//!   per-minute figures (`node_metrics`, M4-2 P35) summed into dense, epoch-aligned
//!   buckets with the page p95 the largest; minute series span at most 24 h, hour and
//!   day series 7 days (`422`, field `step`).
//! * `POST /system/mail/test` (`system:write`): one test mail to the actor's own address
//!   through the node's queue (P49); 3 per 10 minutes per user; audited `mail.test`.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use axum::{Json, extract::State, http::StatusCode};
use ferrite_core::Actor;
use ferrite_domain::{
    AuditQuery, JobId, JobQuery, JobStatus, MAX_PAGE, Node, NodeId, SeriesStep, Timestamp, UserId,
};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed,
    dto::system::{
        AuditDto, AuditList, CacheStatsDto, JobCountsDto, JobDto, JobList, MailStatusDto,
        MailTestDto, MetricsSeriesDto, MetricsSummaryDto, NodeCountsDto, NodeDto, NodeList,
        PluginTotalsDto, SchemaStatusDto, SchemaVerdict, SeriesStepDto, SystemStatusDto,
        ThemeStatusDto, ThisNodeDto,
    },
    extract::{ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit, invalid_cursor, invalid_param},
    plugins::{is_live, live_nodes},
    problem::{Problem, ProblemDocument},
};

/// What the answering node is (set once at boot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: NodeId,
    pub started_at: Timestamp,
    /// `solo` or `cluster`.
    pub mode: &'static str,
    /// `local` or `s3`.
    pub media_backend: &'static str,
}

#[utoipa::path(
    get,
    path = "/system/status",
    tag = "system",
    operation_id = "get_system_status",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "The site's versions, backends, schema, theme, jobs and nodes", body = SystemStatusDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn system_status(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<SystemStatusDto>, Problem> {
    let binary = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| {
        tracing::error!(error = %e, "the crate version is not semver");
        Problem::internal()
    })?;
    let migration = api.system.migration_status(&who.actor, &binary).await?;
    let counts = api.system.job_counts(&who.actor).await?;
    let active = api.system.active_theme(&who.actor).await?;
    let (nodes, now) = nodes_now(&api, &who.actor).await?;
    let (live, stale): (Vec<&Node>, Vec<&Node>) = nodes.iter().partition(|n| is_live(n, now));
    let fallback_nodes = live
        .iter()
        .filter(|n| n.meta["theme"]["fallback"].as_bool() == Some(true))
        .map(|n| n.name.clone())
        .collect();
    Ok(Json(SystemStatusDto {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        site_url: api.site_url.clone(),
        mode: api.node.mode.to_owned(),
        database: match api.store.backend() {
            ferrite_store::Backend::Sqlite => "sqlite",
            ferrite_store::Backend::Postgres => "postgres",
        }
        .to_owned(),
        media: api.node.media_backend.to_owned(),
        schema: SchemaStatusDto {
            db_version: migration.db_version,
            binary_version: migration.binary_version,
            pending: migration.pending.iter().map(|m| m.version).collect(),
            verdict: SchemaVerdict::from(&migration.verdict),
        },
        theme: ThemeStatusDto {
            active,
            fallback_nodes,
        },
        jobs: job_counts(&counts),
        nodes: NodeCountsDto {
            live: u32::try_from(live.len()).unwrap_or(u32::MAX),
            stale: u32::try_from(stale.len()).unwrap_or(u32::MAX),
        },
        node: ThisNodeDto {
            id: api.node.id.as_uuid(),
            started_at: api.node.started_at.as_datetime(),
        },
        mail: MailStatusDto {
            configured: api.mail.is_some(),
            from: api.mail.as_ref().map(|m| m.from.clone()),
        },
    }))
}

#[utoipa::path(
    get,
    path = "/system/nodes",
    tag = "system",
    operation_id = "list_system_nodes",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every registered node (live and stale), by id, with its scrubbed `meta`", body = NodeList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_nodes(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<NodeList>, Problem> {
    let (nodes, now) = nodes_now(&api, &who.actor).await?;
    Ok(Json(NodeList {
        items: nodes
            .into_iter()
            .map(|n| {
                let live = is_live(&n, now);
                NodeDto::new(n, live)
            })
            .collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    get,
    path = "/system/audit",
    tag = "system",
    operation_id = "list_audit",
    security(("session" = []), ("bearer" = [])),
    params(AuditListParams),
    responses(
        (status = 200, description = "One page of the audit log, newest first, filtered", body = AuditList),
        (status = 400, description = "`validation` (`actor`, `action`, `target_kind`, `target_id`, `limit`, `cursor`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `target_id` without `target_kind` (`errors[0].field` = `target_id`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_audit(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<AuditListParams>,
) -> Result<Json<AuditList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    if let Some(c) = &params.cursor
        && ferrite_store::audit_cursor(c).is_err()
    {
        return Err(invalid_cursor());
    }
    let actor = params
        .actor
        .as_deref()
        .map(|a| {
            Uuid::parse_str(a)
                .map(UserId::from_uuid)
                .map_err(|_| invalid_param("actor", "actor must be a user id", "a UUID"))
        })
        .transpose()?;
    for (field, value, max) in [
        ("action", &params.action, MAX_AUDIT_FILTER),
        ("target_kind", &params.target_kind, MAX_AUDIT_FILTER),
        ("target_id", &params.target_id, MAX_AUDIT_TARGET_ID),
    ] {
        if let Some(v) = value
            && (v.is_empty() || v.chars().count() > max || v.chars().any(char::is_control))
        {
            return Err(invalid_param(
                field,
                format!("{field} must be 1 to {max} characters without control characters"),
                format!("1..={max} characters"),
            ));
        }
    }
    let query = AuditQuery {
        actor,
        action: params.action,
        target_kind: params.target_kind,
        target_id: params.target_id,
        limit,
        cursor: params.cursor,
    };
    let found = api.system.audit(&who.actor, &query).await?;
    // At most `limit` (≤ 200 = MAX_ID_FILTER) distinct actors.
    let mut ids: Vec<UserId> = found.items.iter().filter_map(|e| e.actor_id).collect();
    ids.sort();
    ids.dedup();
    let logins: BTreeMap<UserId, String> = if ids.is_empty() {
        BTreeMap::new()
    } else {
        api.store
            .read()
            .users()
            .get_many(&ids)
            .await?
            .into_iter()
            .map(|u| (u.id, u.login))
            .collect()
    };
    let items = found
        .items
        .into_iter()
        .map(|e| {
            let login = e.actor_id.and_then(|id| logins.get(&id).cloned());
            AuditDto::new(e, login)
        })
        .collect();
    Ok(Json(AuditList {
        items,
        next_cursor: found.next_cursor,
    }))
}

/// Longest `action` or `target_kind` filter of `GET /system/audit`, in characters.
const MAX_AUDIT_FILTER: usize = 100;
/// Longest `target_id` filter of `GET /system/audit`, in characters.
const MAX_AUDIT_TARGET_ID: usize = 255;

/// The query of `GET /system/audit`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditListParams {
    /// Rows written by this user (UUID).
    pub actor: Option<String>,
    /// An exact action (`user.update`), or a prefix ending in `.` (`user.`); 1..=100
    /// characters, case-sensitive.
    pub action: Option<String>,
    /// Rows about this kind of target (`user`, `settings`, `plugin`, ...); 1..=100
    /// characters.
    pub target_kind: Option<String>,
    /// Rows about this target (needs `target_kind`); 1..=255 characters.
    pub target_id: Option<String>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page (of the same filters).
    pub cursor: Option<String>,
}

/// The query of `GET /system/jobs`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct JobListParams {
    /// `queued`, `running`, `done` or `dead`; all when absent.
    pub status: Option<String>,
    /// Only jobs of this kind (`[a-z0-9._-]{1,64}`).
    pub kind: Option<String>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/system/jobs",
    tag = "system",
    operation_id = "list_jobs",
    security(("session" = []), ("bearer" = [])),
    params(JobListParams),
    responses(
        (status = 200, description = "One page of jobs, newest first, without payloads", body = JobList),
        (status = 400, description = "`validation` (`status`, `kind`, `limit`, `cursor`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_jobs(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<JobListParams>,
) -> Result<Json<JobList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    let status = params
        .status
        .as_deref()
        .map(|s| {
            s.parse::<JobStatus>().map_err(|_| {
                invalid_param(
                    "status",
                    format!("unknown job status {s:?}"),
                    "queued, running, done or dead",
                )
            })
        })
        .transpose()?;
    if let Some(kind) = &params.kind
        && ferrite_store::check_job_kind(kind).is_err()
    {
        return Err(invalid_param(
            "kind",
            "kind must be 1 to 64 characters from [a-z0-9._-]",
            "[a-z0-9._-]{1,64}",
        ));
    }
    if let Some(c) = &params.cursor
        && ferrite_store::job_cursor(c).is_err()
    {
        return Err(invalid_cursor());
    }
    let page = api
        .system
        .jobs(
            &who.actor,
            JobQuery {
                status,
                kind: params.kind,
                limit,
                cursor: params.cursor,
            },
        )
        .await?;
    let handled = handled_kinds(&api).await?;
    Ok(Json(JobList {
        items: page
            .items
            .into_iter()
            .map(|j| {
                let known = handled.contains(&j.kind);
                JobDto::new(j, Some(known))
            })
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    post,
    path = "/system/jobs/{id}/retry",
    tag = "system",
    operation_id = "retry_job",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The job, `queued` and due now with its attempts reset (`last_error` kept); audited `job.retry`. `handled` is `null` when the nodes could not be read after the retry", body = JobDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the job is not `dead` (`errors[0].field` = `status`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn retry_job(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<JobDto>, Problem> {
    let job = api
        .system
        .retry_job(&who.actor, JobId::from_uuid(id))
        .await?;
    // The retry has committed: a failed node read must not turn it into an error
    // (a repeat would then be `422`), so `handled` is unknown instead.
    let handled = match handled_kinds(&api).await {
        Ok(kinds) => Some(kinds.contains(&job.kind)),
        Err(e) => {
            tracing::warn!(job = %id, error = ?e, "job retried; cannot read the nodes for `handled`");
            None
        }
    };
    Ok(Json(JobDto::new(job, handled)))
}

#[utoipa::path(
    get,
    path = "/system/metrics/summary",
    tag = "system",
    operation_id = "get_metrics_summary",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "A snapshot of the answering node (renders, page cache, plugin calls since it started) and the site's job counts", body = MetricsSummaryDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn metrics_summary(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<MetricsSummaryDto>, Problem> {
    // Checks `manage_system` + `system:read` before anything is read.
    let counts = api.system.job_counts(&who.actor).await?;
    let (calls, failures) = api.site.plugins.totals();
    Ok(Json(MetricsSummaryDto {
        node: ThisNodeDto {
            id: api.node.id.as_uuid(),
            started_at: api.node.started_at.as_datetime(),
        },
        renders: api.site.renders(),
        page_cache: CacheStatsDto::from(api.site.cache.stats().await),
        plugins: PluginTotalsDto { calls, failures },
        jobs: job_counts(&counts),
    }))
}

/// The query of `GET /system/metrics/series`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SeriesParams {
    /// Start of the range (RFC 3339), inclusive; rounded down to a bucket start.
    pub from: String,
    /// End of the range (RFC 3339), exclusive; rounded up to a bucket end. After `from`.
    pub to: String,
    /// `minute` (the range spans at most 24 h), `hour` or `day` (at most 7 days).
    #[param(value_type = SeriesStepDto)]
    pub step: String,
    /// Only this node's rows (UUID); every node's, summed, when absent.
    pub node: Option<String>,
}

/// `value` of query parameter `field` as an RFC 3339 instant.
fn instant(field: &'static str, value: &str) -> Result<Timestamp, Problem> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| Timestamp::from(t.with_timezone(&chrono::Utc)))
        .map_err(|_| {
            invalid_param(
                field,
                format!("{field} must be an RFC 3339 date and time"),
                "RFC 3339, e.g. 2026-10-30T12:00:00Z",
            )
        })
}

#[utoipa::path(
    get,
    path = "/system/metrics/series",
    tag = "system",
    operation_id = "get_metrics_series",
    security(("session" = []), ("bearer" = [])),
    params(SeriesParams),
    responses(
        (status = 200, description = "Every bucket of the range (widened to whole buckets), ascending: the nodes' per-minute figures summed (`p95_ms` the largest), zeros where no node wrote a row", body = MetricsSeriesDto),
        (status = 400, description = "`validation` (`from`, `to`, `step`, `node` malformed) or `malformed-query` (a parameter missing)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `to` not after `from` (`errors[0].field` = `to`), or a span over the step's limit (`step`: minute 24 h, hour and day 7 days)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn metrics_series(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<SeriesParams>,
) -> Result<Json<MetricsSeriesDto>, Problem> {
    let from = instant("from", &params.from)?;
    let to = instant("to", &params.to)?;
    let step = params.step.parse::<SeriesStep>().map_err(|_| {
        invalid_param(
            "step",
            format!("unknown step {:?}", params.step),
            "minute, hour or day",
        )
    })?;
    let node = params
        .node
        .as_deref()
        .map(|n| {
            Uuid::parse_str(n)
                .map(NodeId::from_uuid)
                .map_err(|_| invalid_param("node", "node must be a node id", "a UUID"))
        })
        .transpose()?;
    // Checks `manage_system` + `system:read`, then the range.
    let series = api
        .system
        .metrics_series(&who.actor, from, to, step, node)
        .await?;
    Ok(Json(MetricsSeriesDto::from(series)))
}

#[utoipa::path(
    post,
    path = "/system/mail/test",
    tag = "system",
    operation_id = "send_test_mail",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 202, description = "A test mail to the caller's own address is queued (subject `Test e-mail from <site title>`); audited `mail.test`. Sending runs in the background: an SMTP failure is in the node's log, not here", body = MailTestDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: mail is not configured (`errors[0].field` = `mail`) or the account has no usable address (`email`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: 3 test mails per 10 minutes per user (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: the node's mail queue is full or shutting down", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn send_test_mail(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<(StatusCode, Json<MailTestDto>), Problem> {
    let to = api
        .system
        .send_test_mail(
            &who.actor,
            api.mail.as_ref().map(|m| m.queue.as_ref()),
            &api.site_url,
            &api.site_title,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(MailTestDto { to })))
}

fn job_counts(counts: &BTreeMap<JobStatus, u64>) -> JobCountsDto {
    let count = |s: JobStatus| counts.get(&s).copied().unwrap_or(0);
    JobCountsDto {
        queued: count(JobStatus::Queued),
        running: count(JobStatus::Running),
        done: count(JobStatus::Done),
        dead: count(JobStatus::Dead),
    }
}

/// The job kinds some live node runs (the union of their `meta.job_kinds`; anything
/// but a list of strings counts as none).
async fn handled_kinds(api: &ApiState) -> Result<BTreeSet<String>, Problem> {
    Ok(live_nodes(api)
        .await?
        .iter()
        .filter_map(|n| n.meta.get("job_kinds").and_then(|k| k.as_array()))
        .flatten()
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect())
}

/// The registered nodes (checked and scrubbed by core), by id, and the database clock
/// in microseconds.
async fn nodes_now(api: &ApiState, actor: &Actor) -> Result<(Vec<Node>, i64), Problem> {
    let mut nodes = api.system.nodes(actor).await?;
    nodes.sort_by_key(|n| n.id.as_uuid());
    let now = api.store.read().nodes().db_now().await?;
    Ok((nodes, now.as_micros()))
}
