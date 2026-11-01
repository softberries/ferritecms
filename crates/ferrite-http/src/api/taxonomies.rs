//! `/taxonomies` and their terms (`/taxonomies/{taxonomy}/terms`, `/terms/{id}`; 07 §1).
//!
//! Handlers only translate: `TaxonomyService` checks `manage_settings` for taxonomy
//! writes (`content:read` for reads, F16) and `manage_terms` for term writes
//! (`terms:read` for reads), and emits the events: `types.changed` + `invalidate [all]`
//! for taxonomies, `term:<id>` + the archives of the taxonomy's types for terms. No
//! `If-Match` (P11): last write wins. `PATCH /terms/{id}` merges the body into the term
//! as read now and saves the result.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferrite_core::{Actor, CoreError};
use ferrite_domain::{Term, TermId};
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::types::{
        TaxonomyDto, TaxonomyInput, TaxonomyList, TaxonomyPatch, TermDto, TermInput, TermList,
        TermPatch,
    },
    extract::{ApiJson, ApiPath, created_at},
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    get,
    path = "/taxonomies",
    tag = "taxonomies",
    operation_id = "list_taxonomies",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every taxonomy, by name", body = TaxonomyList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_taxonomies(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<TaxonomyList>, Problem> {
    let items = api.taxonomy.list_taxonomies(&who.actor).await?;
    Ok(Json(TaxonomyList {
        items: items.into_iter().map(TaxonomyDto::from).collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    post,
    path = "/taxonomies",
    tag = "taxonomies",
    operation_id = "create_taxonomy",
    security(("session" = []), ("bearer" = [])),
    request_body = TaxonomyInput,
    responses(
        (status = 201, description = "Registered (`Location`)", body = TaxonomyDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: a taxonomy with this name exists (`errors[0].field` = `name`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `name`, `label` or `content_types`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_taxonomy(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(input): ApiJson<TaxonomyInput>,
) -> Result<Response, Problem> {
    let tax = api
        .taxonomy
        .create_taxonomy(&who.actor, input.into_taxonomy())
        .await?;
    let (status, headers) = created_at(&format!("{BASE}/taxonomies/{}", tax.name));
    Ok((status, headers, Json(TaxonomyDto::from(tax))).into_response())
}

#[utoipa::path(
    get,
    path = "/taxonomies/{name}",
    tag = "taxonomies",
    operation_id = "get_taxonomy",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Taxonomy")),
    responses(
        (status = 200, description = "The taxonomy", body = TaxonomyDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_taxonomy(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<TaxonomyDto>, Problem> {
    let tax = api.taxonomy.get_taxonomy(&who.actor, &name).await?;
    Ok(Json(tax.into()))
}

#[utoipa::path(
    patch,
    path = "/taxonomies/{name}",
    tag = "taxonomies",
    operation_id = "update_taxonomy",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Taxonomy")),
    request_body = TaxonomyPatch,
    responses(
        (status = 200, description = "Saved", body = TaxonomyDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `label` or `content_types`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_taxonomy(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
    ApiJson(patch): ApiJson<TaxonomyPatch>,
) -> Result<Json<TaxonomyDto>, Problem> {
    let tax = api
        .taxonomy
        .update_taxonomy(&who.actor, &name, patch.label, patch.content_types)
        .await?;
    Ok(Json(tax.into()))
}

#[utoipa::path(
    delete,
    path = "/taxonomies/{name}",
    tag = "taxonomies",
    operation_id = "delete_taxonomy",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Taxonomy")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: a term was created concurrently; retry", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: built in, or has terms", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_taxonomy(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, Problem> {
    api.taxonomy.delete_taxonomy(&who.actor, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/taxonomies/{taxonomy}/terms",
    tag = "taxonomies",
    operation_id = "list_terms",
    security(("session" = []), ("bearer" = [])),
    params(("taxonomy" = String, Path, description = "Taxonomy")),
    responses(
        (status = 200, description = "Every term of the taxonomy", body = TermList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `terms:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such taxonomy", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_terms(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(taxonomy): ApiPath<String>,
) -> Result<Json<TermList>, Problem> {
    let terms = api.taxonomy.list_terms_for(&who.actor, &taxonomy).await?;
    Ok(Json(TermList {
        items: terms.into_iter().map(TermDto::from).collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    post,
    path = "/taxonomies/{taxonomy}/terms",
    tag = "taxonomies",
    operation_id = "create_term",
    security(("session" = []), ("bearer" = [])),
    params(("taxonomy" = String, Path, description = "Taxonomy")),
    request_body = TermInput,
    responses(
        (status = 201, description = "Created (`Location`)", body = TermDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_terms`, `terms:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such taxonomy", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: the slug was taken by concurrent saves (`errors[0].field` = `slug`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (name, slug, parent)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_term(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(taxonomy): ApiPath<String>,
    ApiJson(input): ApiJson<TermInput>,
) -> Result<Response, Problem> {
    let term = api
        .taxonomy
        .create_term(
            &who.actor,
            &taxonomy,
            &input.name,
            input.slug,
            input.parent_id.map(TermId::from_uuid),
            &input.description,
        )
        .await?;
    let (status, headers) = created_at(&format!("{BASE}/terms/{}", term.id));
    Ok((status, headers, Json(TermDto::from(term))).into_response())
}

#[utoipa::path(
    get,
    path = "/terms/{id}",
    tag = "taxonomies",
    operation_id = "get_term",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The term", body = TermDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `terms:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_term(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<TermDto>, Problem> {
    let term = api
        .taxonomy
        .get_term(&who.actor, TermId::from_uuid(id))
        .await?;
    Ok(Json(term.into()))
}

#[utoipa::path(
    patch,
    path = "/terms/{id}",
    tag = "taxonomies",
    operation_id = "update_term",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = TermPatch,
    responses(
        (status = 200, description = "Saved", body = TermDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_terms`, `terms:write`; reading it needs `terms:read`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: the slug was taken by concurrent saves (`errors[0].field` = `slug`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (name, slug, parent)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_term(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(patch): ApiJson<TermPatch>,
) -> Result<Json<TermDto>, Problem> {
    let term = merge_term(
        &api,
        &who.actor,
        TermId::from_uuid(id),
        TermChange {
            name: patch.name,
            slug: patch.slug,
            parent_id: patch.parent_id,
            description: patch.description,
        },
    )
    .await?;
    Ok(Json(term.into()))
}

/// The members of a term change that were sent; the others keep the stored value.
pub(crate) struct TermChange {
    pub name: Option<String>,
    pub slug: Option<String>,
    /// `Some(None)` moves the term to the top level.
    pub parent_id: Option<Option<Uuid>>,
    pub description: Option<String>,
}

/// Merges `change` into term `id` as read now and saves the result (`PATCH /terms/{id}`,
/// MCP `term_update`).
pub(crate) async fn merge_term(
    api: &ApiState,
    actor: &Actor,
    id: TermId,
    change: TermChange,
) -> Result<Term, CoreError> {
    let current = api.taxonomy.get_term(actor, id).await?;
    let parent = match change.parent_id {
        None => current.parent_id,
        Some(p) => p.map(TermId::from_uuid),
    };
    api.taxonomy
        .update_term(
            actor,
            current.id,
            change.name.as_deref().unwrap_or(&current.name),
            change.slug,
            parent,
            change
                .description
                .as_deref()
                .unwrap_or(&current.description),
        )
        .await
}

#[utoipa::path(
    delete,
    path = "/terms/{id}",
    tag = "taxonomies",
    operation_id = "delete_term",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "Deleted (its links to content items go with it)"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_terms`, `terms:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: a child term was added concurrently; retry", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: has child terms", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_term(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.taxonomy
        .delete_term(&who.actor, TermId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
