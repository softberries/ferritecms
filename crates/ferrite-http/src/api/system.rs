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
//! * `GET /system/audit?limit&cursor`: the audit log, newest first, with each actor's
//!   login.

use std::{collections::BTreeMap, sync::Arc};

use axum::{Json, extract::State};
use ferrite_core::Actor;
use ferrite_domain::{JobStatus, Node, NodeId, Timestamp, UserId};

use super::{
    ApiState, Authed, PageParams,
    dto::system::{
        AuditDto, AuditList, JobCountsDto, NodeCountsDto, NodeDto, NodeList, SchemaStatusDto,
        SchemaVerdict, SystemStatusDto, ThemeStatusDto, ThisNodeDto,
    },
    extract::{ApiQuery, invalid_cursor},
    plugins::is_live,
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
    let count = |s: JobStatus| counts.get(&s).copied().unwrap_or(0);
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
        jobs: JobCountsDto {
            queued: count(JobStatus::Queued),
            running: count(JobStatus::Running),
            done: count(JobStatus::Done),
            dead: count(JobStatus::Dead),
        },
        nodes: NodeCountsDto {
            live: u32::try_from(live.len()).unwrap_or(u32::MAX),
            stale: u32::try_from(stale.len()).unwrap_or(u32::MAX),
        },
        node: ThisNodeDto {
            id: api.node.id.as_uuid(),
            started_at: api.node.started_at.as_datetime(),
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
    params(PageParams),
    responses(
        (status = 200, description = "One page of the audit log, newest first", body = AuditList),
        (status = 400, description = "`validation`: `limit` or `cursor`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_system`, `system:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_audit(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<AuditList>, Problem> {
    let limit = page.limit()?;
    if let Some(c) = &page.cursor
        && ferrite_store::audit_cursor(c).is_err()
    {
        return Err(invalid_cursor());
    }
    let found = api
        .system
        .audit(&who.actor, limit, page.cursor.as_deref())
        .await?;
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

/// The registered nodes (checked and scrubbed by core), by id, and the database clock
/// in microseconds.
async fn nodes_now(api: &ApiState, actor: &Actor) -> Result<(Vec<Node>, i64), Problem> {
    let mut nodes = api.system.nodes(actor).await?;
    nodes.sort_by_key(|n| n.id.as_uuid());
    let now = api.store.read().nodes().db_now().await?;
    Ok((nodes, now.as_micros()))
}
