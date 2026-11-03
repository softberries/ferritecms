//! The plugin registry and publisher keys (M6-2, design 14 §7.3).
//!
//! * `GET /plugins/registry?q=` lists the cached index (no fetch); `POST
//!   /plugins/registry/refresh` fetches and verifies it now (at most once a minute per
//!   site), then answers the same.
//! * `POST /plugins/registry/inspect` fetches a version's files, checks them against the
//!   index and answers its permission report (`trust.source.kind = "registry"`);
//!   `POST /plugins/registry/install` does the same again and installs it when
//!   `approve` is that report's fingerprint.
//! * `GET /plugins/publishers` lists the site's keys, the cached registry's keys and the
//!   revocations; `POST /plugins/publishers/{key_id}/revoke` revokes one (with
//!   `compromised`, its plugins are disabled on every node). There is deliberately no
//!   API to add a key or unbind a name (operator only, design 14 D8).
//!
//! Every route needs `manage_plugins` (reads `extensions:read`, writes
//! `extensions:write`), checked in core.

use std::sync::Arc;

use axum::{Json, extract::State, response::Response};
use ferrite_domain::Timestamp;
use ferrite_domain::plugin_trust::{KeyId, KeySource, PublisherKey, Revocation};
use serde::Deserialize;
use utoipa::IntoParams;

use super::{
    ApiState, Authed,
    dto::extensions::{
        PermissionReportDto, PublisherKeyDto, PublisherKeyList, PublisherRefDto, RegistryDto,
        RegistryInspectRequest, RegistryInstallRequest, RevokeKeyRequest,
    },
    extract::{ApiJson, ApiPath, ApiQuery},
    plugins::{install_inspected, report_dto, with_report},
    problem::{Problem, ProblemDocument},
};
use crate::extensions::TrustContext;

/// `GET /plugins/registry` query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RegistryQuery {
    /// Matches name, description and publisher name (case-insensitive).
    pub q: Option<String>,
}

#[utoipa::path(
    get,
    path = "/plugins/registry",
    tag = "plugins",
    operation_id = "get_plugin_registry",
    security(("session" = []), ("bearer" = [])),
    params(RegistryQuery),
    responses(
        (status = 200, description = "The configured registry and its cached plugins matching `q` (at most 50); `registry: null` when none is configured. Nothing is fetched", body = RegistryDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_registry(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(query): ApiQuery<RegistryQuery>,
) -> Result<Json<RegistryDto>, Problem> {
    api.extensions.authorize_plugin_reads(&who.actor)?;
    let view = api.installer.registry_view(query.q.as_deref()).await?;
    Ok(Json(view.into()))
}

#[utoipa::path(
    post,
    path = "/plugins/registry/refresh",
    tag = "plugins",
    operation_id = "refresh_plugin_registry",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Fetched, verified and stored (revocations applied); the registry as `GET` answers it", body = RegistryDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no registry is configured", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`registry-invalid`: the index's signature, schema or serial (a rollback) is refused; the cached copy stays", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: one refresh per minute per site (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 502, description = "`registry-unavailable`: the index could not be fetched; the cached copy stays", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn refresh_registry(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<RegistryDto>, Problem> {
    api.extensions
        .authorize_registry_refresh(&who.actor)
        .await?;
    let refreshed = api.installer.refresh_registry().await?;
    let mut dto: RegistryDto = api.installer.registry_view(None).await?.into();
    dto.collisions = refreshed
        .collisions
        .iter()
        .map(ToString::to_string)
        .collect();
    Ok(Json(dto))
}

#[utoipa::path(
    post,
    path = "/plugins/registry/inspect",
    tag = "plugins",
    operation_id = "inspect_registry_plugin",
    security(("session" = []), ("bearer" = [])),
    request_body = RegistryInspectRequest,
    responses(
        (status = 200, description = "The permission report of the registry version; nothing is stored", body = PermissionReportDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no registry is configured, or no such plugin or version", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`registry-invalid` (a file does not match the index, another world, not signed by its publisher) or `validation` (`signature`, `publisher`; the body's `report` is the version's report)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 502, description = "`registry-unavailable`: a file could not be fetched", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn inspect_registry_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(body): ApiJson<RegistryInspectRequest>,
) -> Result<Json<PermissionReportDto>, Problem> {
    api.extensions.authorize_plugins(&who.actor)?;
    let inspected = api
        .installer
        .inspect_registry(&body.name, body.version.as_deref())
        .await?;
    let report = report_dto(&inspected);
    if let Err(refusal) = inspected.decision() {
        return Err(with_report(
            Problem::from(ferrite_core::CoreError::from(refusal.clone())),
            &report,
        )?);
    }
    Ok(Json(report))
}

#[utoipa::path(
    post,
    path = "/plugins/registry/install",
    tag = "plugins",
    operation_id = "install_registry_plugin",
    security(("session" = []), ("bearer" = [])),
    request_body = RegistryInstallRequest,
    responses(
        (status = 201, description = "Installed (`Location`), as an upload", body = crate::api::dto::extensions::PluginDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no registry, plugin or version", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "As for the inspection; or `validation` with `errors[0].field` = `approve` when `approve` is not the fingerprint of this version's report (a file that changed in between changes it)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 502, description = "`registry-unavailable`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn install_registry_plugin(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(body): ApiJson<RegistryInstallRequest>,
) -> Result<Response, Problem> {
    api.extensions.authorize_plugins(&who.actor)?;
    let inspected = api
        .installer
        .inspect_registry(&body.name, Some(&body.version))
        .await?;
    install_inspected(&api, &who.actor, inspected, &body.approve).await
}

/// Every key this site knows: its rows (approved, operator, revocations) and the
/// cached registry's current keys.
fn publisher_keys(ctx: &TrustContext) -> Vec<PublisherKeyDto> {
    // By the whole key: a key id is 8 bytes anyone can choose (M6-2 review).
    let signed_by = |key: &PublisherKey| -> Vec<String> {
        ctx.owners
            .iter()
            .filter(|o| o.signer().as_ref() == Some(key))
            .map(|o| o.plugin.clone())
            .collect()
    };
    let listed = |key: &PublisherKey| {
        ctx.registry.as_ref().and_then(|r| {
            r.index
                .publisher_of_key(key)
                .or_else(|| r.index.revocation(key).map(|(p, _)| p))
                .map(|p| PublisherRefDto {
                    id: p.id.clone(),
                    name: p.name.clone(),
                })
        })
    };
    let mut out: Vec<PublisherKeyDto> = ctx
        .keys
        .iter()
        .map(|k| PublisherKeyDto {
            key_id: k.key.key_id.to_string(),
            public_key: k.key.base64.clone(),
            label: k.label.clone(),
            source: k.source.as_str().to_owned(),
            registry_publisher: listed(&k.key),
            trusted_at: Some(k.trusted_at.as_datetime()),
            revoked_at: k.revoked.as_ref().map(|r| r.at.as_datetime()),
            revoked_reason: k.revoked.as_ref().map(|r| r.reason.clone()),
            compromised: k.revoked.as_ref().is_some_and(|r| r.compromised),
            plugins: signed_by(&k.key),
        })
        .collect();
    if let Some(r) = &ctx.registry {
        for p in &r.index.publishers {
            for key in &p.keys {
                if out.iter().any(|k| k.public_key == key.base64) {
                    continue;
                }
                out.push(PublisherKeyDto {
                    key_id: key.key_id.to_string(),
                    public_key: key.base64.clone(),
                    label: p.name.clone(),
                    source: KeySource::Registry.as_str().to_owned(),
                    registry_publisher: Some(PublisherRefDto {
                        id: p.id.clone(),
                        name: p.name.clone(),
                    }),
                    trusted_at: None,
                    revoked_at: None,
                    revoked_reason: None,
                    compromised: false,
                    plugins: signed_by(key),
                });
            }
        }
    }
    out.sort_by(|a, b| a.key_id.cmp(&b.key_id));
    out
}

#[utoipa::path(
    get,
    path = "/plugins/publishers",
    tag = "plugins",
    operation_id = "list_plugin_publishers",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "The site's keys, the cached registry's keys and every revocation, by key id", body = PublisherKeyList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_publishers(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<PublisherKeyList>, Problem> {
    api.extensions.authorize_plugin_reads(&who.actor)?;
    let ctx = api.installer.trust_context().await?;
    Ok(Json(PublisherKeyList {
        items: publisher_keys(&ctx),
        policy: ctx.policy.as_str().to_owned(),
    }))
}

#[utoipa::path(
    post,
    path = "/plugins/publishers/{key_id}/revoke",
    tag = "plugins",
    operation_id = "revoke_plugin_publisher",
    security(("session" = []), ("bearer" = [])),
    params(("key_id" = String, Path, description = "16 hex digits")),
    request_body = RevokeKeyRequest,
    responses(
        (status = 200, description = "Revoked (audited `plugin.publisher.revoke`): installs and updates signed by it are refused; with `compromised` its plugins are disabled on every node", body = PublisherKeyDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_plugins`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such key here or in the cached registry", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `reason` empty or over 500 characters", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn revoke_publisher(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(key_id): ApiPath<String>,
    ApiJson(body): ApiJson<RevokeKeyRequest>,
) -> Result<Json<PublisherKeyDto>, Problem> {
    api.extensions.authorize_plugins(&who.actor)?;
    let id = KeyId::parse(&key_id).map_err(|_| Problem::not_found())?;
    let ctx = api.installer.trust_context().await?;
    let (key, label): (PublisherKey, String) = match ctx.key(&id) {
        Some(k) => (k.key.clone(), k.label.clone()),
        None => {
            let listed = ctx.registry.as_ref().and_then(|r| {
                r.index.publishers.iter().find_map(|p| {
                    p.keys
                        .iter()
                        .find(|k| k.key_id == id)
                        .or_else(|| {
                            p.revoked_keys
                                .iter()
                                .find(|r| r.key.key_id == id)
                                .map(|r| &r.key)
                        })
                        .map(|k| (k.clone(), p.name.clone()))
                })
            });
            listed.ok_or_else(Problem::not_found)?
        }
    };
    let revocation = Revocation {
        at: Timestamp::now(),
        reason: body.reason,
        compromised: body.compromised,
    };
    api.extensions
        .revoke_key(&who.actor, &key, &revocation, &label, KeySource::Registry)
        .await?;
    let ctx = api.installer.trust_context().await?;
    let dto = publisher_keys(&ctx)
        .into_iter()
        .find(|k| k.key_id == id.to_string())
        .ok_or_else(Problem::internal)?;
    Ok(Json(dto))
}
