//! `/plugins` (07 §1; M2a-2 P3, P4, P8, P9, P10, P11, R1, R2).
//!
//! Handlers translate; `ExtensionAdmin` checks `manage_plugins` with
//! `extensions:write` for writes and `manage_plugins` with `extensions:read` for reads
//! (editors see no plugin list), records the rows under the `extensions` lock and
//! audits every write. The node's one [`Installer`](crate::extensions::Installer)
//! checks and compiles uploads (at most two at once per node). No `If-Match` (P11).
//!
//! * `POST /plugins/inspect` takes `manifest` and `component` and answers the
//!   permission report: what the upload asks for, what is new against the installed
//!   version's grant (`added`), the limits, and the `fingerprint`. Nothing is stored.
//! * `POST /plugins` takes the same parts plus `approve`, that fingerprint. The upload
//!   is inspected again and installed only if `approve` is the fingerprint of *this*
//!   upload (plugin, version, component and grant); otherwise `422 validation` with
//!   `errors[0].field = "approve"` and the upload's `report`, and nothing changes: an
//!   installed version keeps running (R1). A new plugin is installed disabled; a
//!   replaced one keeps its enabled state and gets the new grant.
//! * `DELETE /plugins/{name}` keeps the plugin's KV and settings for 30 days (a
//!   reinstall in that time finds them); `?purge=true` deletes them at once (P9, R2).
//! * `GET /plugins/retained` lists the deleted plugins whose data is still kept, with
//!   the day it goes; `POST /plugins/{name}/purge` erases it now (M2b-3 P47; `422`
//!   while a plugin of that name is installed, `404` when nothing is retained).
//!
//! * `GET /plugins/{name}/settings` answers the schema of the manifest's `[settings]`,
//!   the non-secret values as the plugin reads them, and for each `secret` only whether
//!   it is set; `PATCH` writes the keys of its body (`null` deletes one) and answers the
//!   same. Secrets are sealed with the site key for that plugin and key and never
//!   returned (P13, Review Focus 3); plugins read settings per call (P14), and pages
//!   whose plugin calls read a changed key are flushed through `options.changed`.
//!
//! * `GET /plugins/{name}/metrics` (M2a-2b P20): the plugin's failures per hook and
//!   hour over the last 24 hours, every node's (the shared counters), and the answering
//!   node's call statistics per hook since it started (calls, failures, busy and
//!   budget skips, durations). Each node counts its own; time series are M4.
//!
//! Uploads are `multipart/form-data` ([`multipart`](super::multipart)); the caller is
//! authorized before any of the body is read. Responses never carry the component's
//! bytes or a secret.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use ferrite_domain::{InstalledPlugin, Node, Timestamp};
use ferrite_plugin_host::MAX_MANIFEST_BYTES;
use serde::Deserialize;
use serde_json::{Map, Value};
use utoipa::IntoParams;

use super::{
    ApiState, Authed, BASE,
    dto::extensions::{
        FailureBucketDto, GrantReportDto, HookStatsDto, NodeHookStatsDto, PermissionReportDto,
        PluginDto, PluginFacts, PluginInspectUpload, PluginList, PluginMetricsDto, PluginNodeState,
        PluginSettingsDto, PluginUpload, RetainedPluginDto, RetainedPluginList, SettingsPatch,
    },
    extract::{ApiJson, ApiPath, created_at, parse_query},
    multipart::{open, read_parts},
    problem::{Problem, ProblemDocument},
};
use crate::extensions::{ExtensionError, MAX_PLUGIN_BYTES};

/// Largest `approve` part.
pub const MAX_APPROVE_BYTES: usize = 128;

/// The body limit of the upload routes: the component's 32 MiB plus 1 MiB for the
/// manifest, the approval and the multipart framing.
pub const MAX_UPLOAD_BODY: usize = MAX_PLUGIN_BYTES as usize + 1024 * 1024;

/// A node whose heartbeat is older than this is not listed in `nodes` (pre-flight F20;
/// the value of `ferrite_cluster::STALE_AFTER`, which this crate does not depend on).
pub const NODE_STALE_AFTER: Duration = Duration::from_secs(30);

/// The window of `GET /plugins/{name}/metrics` failures: 24 hours.
pub const PLUGIN_METRICS_WINDOW: Duration = Duration::from_secs(24 * 3600);

const COMPONENT_PART: (&str, usize) = ("component", MAX_PLUGIN_BYTES as usize);
const MANIFEST_PART: (&str, usize) = ("manifest", MAX_MANIFEST_BYTES);

#[utoipa::path(
    get,
    path = "/plugins",
    tag = "plugins",
    operation_id = "list_plugins",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every installed plugin, by name", body = PluginList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_plugins(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<PluginList>, Problem> {
    let rows = api.extensions.list_plugins(&who.actor).await?;
    let nodes = live_nodes(&api).await?;
    Ok(Json(PluginList {
        items: rows.into_iter().map(|r| dto(&api, r, &nodes)).collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    get,
    path = "/plugins/{name}",
    tag = "plugins",
    operation_id = "get_plugin",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin")),
    responses(
        (status = 200, description = "The plugin", body = PluginDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<PluginDto>, Problem> {
    let row = api.extensions.get_plugin(&who.actor, &name).await?;
    Ok(Json(dto(&api, row, &live_nodes(&api).await?)))
}

#[utoipa::path(
    post,
    path = "/plugins/inspect",
    tag = "plugins",
    operation_id = "inspect_plugin",
    security(("session" = []), ("bearer" = [])),
    request_body(content = PluginInspectUpload, content_type = "multipart/form-data", description = "Parts `manifest` (at most 64 KiB) and `component` (at most 32 MiB); body at most 33 MiB"),
    responses(
        (status = 200, description = "The permission report; nothing is stored", body = PermissionReportDto),
        (status = 400, description = "`malformed-body`: an unknown, unnamed or repeated part, or a malformed body", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: a part or the body over its limit", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `multipart/form-data`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: a missing part (`errors[0].field` = its name), a manifest that is not valid (`errors[].field` = the manifest path, or `manifest`), a component that does not load or a limit above its ceiling (`component`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: every upload slot of this node is taken (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn inspect_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    req: Request,
) -> Result<Json<PermissionReportDto>, Problem> {
    api.extensions.authorize_plugins(&who.actor)?;
    let _upload = upload_slot(&api)?;
    let mut parts = read_parts(
        open(req, MAX_UPLOAD_BODY).await?,
        &[MANIFEST_PART, COMPONENT_PART],
    )
    .await?;
    let manifest = manifest_text(&parts)?;
    let component = parts.remove("component").unwrap_or_default();
    let inspected = api.installer.inspect_plugin(&manifest, component).await?;
    Ok(Json(PermissionReportDto::new(
        inspected.report(),
        inspected.is_update(),
        PluginFacts::of(&inspected.manifest().plugin),
    )))
}

#[utoipa::path(
    post,
    path = "/plugins",
    tag = "plugins",
    operation_id = "install_plugin",
    security(("session" = []), ("bearer" = [])),
    request_body(content = PluginUpload, content_type = "multipart/form-data", description = "Parts `manifest` (at most 64 KiB), `component` (at most 32 MiB) and `approve` (the inspection's `fingerprint`, at most 128 bytes); body at most 33 MiB"),
    responses(
        (status = 201, description = "Installed (`Location`): a new plugin disabled, a replaced one keeping its enabled state, with the approved grant", body = PluginDto),
        (status = 400, description = "`malformed-body`: an unknown, unnamed or repeated part, or a malformed body", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: a part or the body over its limit", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `multipart/form-data`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: as for the inspection; or `errors[0].field` = `approve` when `approve` is missing or is not the fingerprint of this upload (the body's `report` is this upload's report; nothing is stored and an installed version keeps running)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: every upload slot of this node is taken, or another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn install_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    req: Request,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    api.extensions.authorize_plugins(actor)?;
    let _upload = upload_slot(&api)?;
    let mut parts = read_parts(
        open(req, MAX_UPLOAD_BODY).await?,
        &[
            MANIFEST_PART,
            COMPONENT_PART,
            ("approve", MAX_APPROVE_BYTES),
        ],
    )
    .await?;
    let manifest = manifest_text(&parts)?;
    // Not UTF-8 cannot be a fingerprint: it simply does not match.
    let approve =
        String::from_utf8(parts.remove("approve").unwrap_or_default().to_vec()).unwrap_or_default();
    let component = parts.remove("component").unwrap_or_default();
    let inspected = api.installer.inspect_plugin(&manifest, component).await?;
    let report = PermissionReportDto::new(
        inspected.report(),
        inspected.is_update(),
        PluginFacts::of(&inspected.manifest().plugin),
    );
    let (artifact, row) = match api.installer.stage_plugin(inspected, &approve).await {
        Ok(staged) => staged,
        Err(e @ ExtensionError::Approval(_)) => {
            let report = serde_json::to_value(&report).map_err(|e| {
                tracing::error!(error = %e, "cannot serialize a permission report");
                Problem::internal()
            })?;
            return Err(Problem::from(e).with_report(report));
        }
        Err(e) => return Err(e.into()),
    };
    if let Err(e) = api.extensions.install_plugin(actor, &artifact, &row).await {
        // No row this request wrote names the staged blob; drop it unless another row
        // names the same bytes.
        if let Err(cleanup) = api.installer.discard_staged(&artifact.hash).await {
            tracing::warn!(artifact = %artifact.hash, error = %cleanup, "cannot discard a staged plugin component");
        }
        return Err(e.into());
    }
    let stored = api.extensions.get_plugin(actor, &row.name).await?;
    let (status, headers) = created_at(&format!("{BASE}/plugins/{}", stored.name));
    let body = dto(&api, stored, &live_nodes(&api).await?);
    Ok((status, headers, Json(body)).into_response())
}

#[utoipa::path(
    post,
    path = "/plugins/{name}/enable",
    tag = "plugins",
    operation_id = "enable_plugin",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin")),
    responses(
        (status = 200, description = "Enabled (its failure counters and any automatic disable reason cleared); nodes load it on `plugins.changed`", body = PluginDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn enable_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<PluginDto>, Problem> {
    set_enabled(&api, &who.actor, &name, true).await
}

#[utoipa::path(
    post,
    path = "/plugins/{name}/disable",
    tag = "plugins",
    operation_id = "disable_plugin",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin")),
    responses(
        (status = 200, description = "Disabled; nodes unload it on `plugins.changed`", body = PluginDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn disable_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<PluginDto>, Problem> {
    set_enabled(&api, &who.actor, &name, false).await
}

/// `DELETE /plugins/{name}` query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeleteParams {
    /// `true`: delete the plugin's KV and settings now; `false` (default): keep them
    /// for 30 days, so a reinstall in that time finds them.
    pub purge: Option<bool>,
}

#[utoipa::path(
    delete,
    path = "/plugins/{name}",
    tag = "plugins",
    operation_id = "delete_plugin",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin"), DeleteParams),
    responses(
        (status = 204, description = "Deleted; its data kept for 30 days unless `purge=true`"),
        (status = 400, description = "`malformed-query`: `purge` is not `true` or `false`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
    uri: axum::http::Uri,
) -> Result<StatusCode, Problem> {
    // Authorized before the query is read, as every extension write (core checks again).
    api.extensions.authorize_plugins(&who.actor)?;
    let params: DeleteParams = parse_query(&uri)?;
    api.extensions
        .delete_plugin(&who.actor, &name, params.purge.unwrap_or(false))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/plugins/retained",
    tag = "plugins",
    operation_id = "list_retained_plugins",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every deleted plugin whose KV and settings are still kept (deleted without `purge`, not installed again), by name", body = RetainedPluginList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_retained_plugins(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<RetainedPluginList>, Problem> {
    let retained = api.extensions.retained_plugins(&who.actor).await?;
    Ok(Json(RetainedPluginList {
        items: retained.into_iter().map(RetainedPluginDto::from).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/plugins/{name}/purge",
    tag = "plugins",
    operation_id = "purge_plugin",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "A deleted plugin")),
    responses(
        (status = 204, description = "The deleted plugin's KV and settings are erased now (audited `plugin.purge`); a later install of that name starts empty"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no data of that name is retained", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: a plugin of that name is installed (`errors[0].field` = `name`); delete it with `purge=true` instead", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn purge_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, Problem> {
    api.extensions.purge_plugin(&who.actor, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/plugins/{name}/settings",
    tag = "plugins",
    operation_id = "get_plugin_settings",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin")),
    responses(
        (status = 200, description = "The settings schema, the non-secret values, and whether each secret is set (never its value)", body = PluginSettingsDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_plugin_settings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<PluginSettingsDto>, Problem> {
    let view = api.extensions.plugin_settings(&who.actor, &name).await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    patch,
    path = "/plugins/{name}/settings",
    tag = "plugins",
    operation_id = "update_plugin_settings",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin")),
    request_body(content = SettingsPatch, description = "Declared keys and their new values; `null` deletes a key's value (the plugin then reads the default). A `secret` is stored sealed and never returned"),
    responses(
        (status = 200, description = "Saved; the schema, the non-secret values and whether each secret is set", body = PluginSettingsDto),
        (status = 400, description = "`malformed-json` (the body must be an object)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: an undeclared key, or a bad value (`errors[0].field` = the key; the value is never echoed); nothing is written", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_plugin_settings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
    ApiJson(patch): ApiJson<Map<String, Value>>,
) -> Result<Json<PluginSettingsDto>, Problem> {
    let view = api
        .extensions
        .set_plugin_settings(&who.actor, &name, patch)
        .await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    get,
    path = "/plugins/{name}/metrics",
    tag = "plugins",
    operation_id = "get_plugin_metrics",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Plugin")),
    responses(
        (status = 200, description = "Failures per hook and hour over 24 hours (every node's) and the answering node's call statistics", body = PluginMetricsDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn plugin_metrics(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<PluginMetricsDto>, Problem> {
    // This node's clock (nothing is read before core's check); a few seconds of skew
    // against the nodes that stamped the buckets do not matter over 24 hours.
    let now = Timestamp::now();
    let window = i64::try_from(PLUGIN_METRICS_WINDOW.as_micros()).unwrap_or(i64::MAX);
    let since = Timestamp::from_micros(now.as_micros().saturating_sub(window))
        .map_err(|_| Problem::internal())?;
    let failures = api
        .extensions
        .failure_buckets(&who.actor, &name, since)
        .await?;
    Ok(Json(PluginMetricsDto {
        window_secs: PLUGIN_METRICS_WINDOW.as_secs(),
        failures: failures.into_iter().map(FailureBucketDto::from).collect(),
        node: NodeHookStatsDto {
            id: api.node.id.as_uuid(),
            since: api.node.started_at.as_datetime(),
            hooks: api
                .site
                .plugins
                .call_stats(&name)
                .into_iter()
                .map(HookStatsDto::from)
                .collect(),
        },
    }))
}

async fn set_enabled(
    api: &ApiState,
    actor: &ferrite_core::Actor,
    name: &str,
    enabled: bool,
) -> Result<Json<PluginDto>, Problem> {
    api.extensions
        .set_plugin_enabled(actor, name, enabled)
        .await?;
    let row = api.extensions.get_plugin(actor, name).await?;
    Ok(Json(dto(api, row, &live_nodes(api).await?)))
}

/// One of the node's upload slots (shared by plugin and theme uploads), or `503 busy`
/// (`Retry-After`) when all are taken.
pub(super) fn upload_slot(api: &ApiState) -> Result<tokio::sync::OwnedSemaphorePermit, Problem> {
    api.installer.try_upload_slot().ok_or_else(|| {
        let mut p = Problem::busy();
        p.detail = "too many uploads on this node; retry".to_owned();
        p
    })
}

/// The `manifest` part as text (`422`, `field = "manifest"`, when not UTF-8).
fn manifest_text(
    parts: &std::collections::BTreeMap<&'static str, Bytes>,
) -> Result<String, Problem> {
    let bytes = parts.get("manifest").cloned().unwrap_or_default();
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        Problem::validation("the manifest is not UTF-8").with_error(Some("manifest"), "not UTF-8")
    })
}

fn dto(api: &ApiState, row: InstalledPlugin, nodes: &[Node]) -> PluginDto {
    let report = api
        .installer
        .row_report(&row)
        .map(|r| GrantReportDto::from(&r));
    let states = nodes
        .iter()
        .filter_map(|n| PluginNodeState::of(n, &row.name))
        .collect();
    PluginDto::new(row, report, states)
}

/// The registered nodes with a heartbeat within [`NODE_STALE_AFTER`] of the database
/// clock, by node id.
pub(super) async fn live_nodes(api: &ApiState) -> Result<Vec<Node>, Problem> {
    let read = api.store.read();
    let nodes = read.nodes().list().await?;
    let now = read.nodes().db_now().await?;
    Ok(live(nodes, now.as_micros()))
}

fn live(mut nodes: Vec<Node>, now_micros: i64) -> Vec<Node> {
    nodes.retain(|n| is_live(n, now_micros));
    nodes.sort_by_key(|n| n.id.as_uuid());
    nodes
}

/// Whether `node`'s heartbeat is within [`NODE_STALE_AFTER`] of `now_micros` (the
/// database clock).
pub(super) fn is_live(node: &Node, now_micros: i64) -> bool {
    let stale = i64::try_from(NODE_STALE_AFTER.as_micros()).unwrap_or(i64::MAX);
    node.heartbeat_at.as_micros() >= now_micros.saturating_sub(stale)
}

#[cfg(test)]
mod tests {
    use ferrite_domain::{NodeId, Timestamp};
    use serde_json::json;

    use super::*;

    fn node(heartbeat_micros: i64) -> Node {
        let at = Timestamp::from_micros(heartbeat_micros).unwrap();
        Node {
            id: NodeId::new(),
            name: "n".into(),
            version: "0".into(),
            schema_version: 1,
            started_at: at,
            heartbeat_at: at,
            meta: json!({}),
        }
    }

    #[test]
    fn stale_nodes_are_not_listed() {
        let now = 1_000_000_000_000;
        let stale = i64::try_from(NODE_STALE_AFTER.as_micros()).unwrap();
        let fresh = node(now - stale);
        let old = node(now - stale - 1);
        let ahead = node(now + 1_000);
        let kept = live(vec![old, fresh.clone(), ahead.clone()], now);
        let mut want = vec![fresh.id, ahead.id];
        want.sort_by_key(|id| id.as_uuid());
        assert_eq!(kept.iter().map(|n| n.id).collect::<Vec<_>>(), want);
    }

    #[test]
    fn node_state_reads_meta() {
        let mut n = node(0);
        n.meta =
            json!({"plugins": {"seo": {"version": "1.0.0", "status": "loaded", "error": null}}});
        let s = PluginNodeState::of(&n, "seo").unwrap();
        assert_eq!(
            (s.status.as_str(), s.version.as_deref(), s.error),
            ("loaded", Some("1.0.0"), None)
        );
        assert!(PluginNodeState::of(&n, "other").is_none());
        n.meta = json!({"plugins": {"seo": {"version": "1.0.0"}}});
        assert!(PluginNodeState::of(&n, "seo").is_none(), "no status");
    }
}
