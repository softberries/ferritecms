//! `/settings` (07 §1, P19): the allowlisted site options
//! ([`ferrite_core::settings::SETTINGS`]).
//!
//! `GET` reads them from the store (`settings:read`, F16); `PATCH` writes the keys of
//! its body in one transaction through `OptionService::set_many` (`manage_settings`;
//! `null` resets a key to its default) and answers with the whole object as stored.
//! An unknown key is `422`, a bad value `422` with `errors[0].field` = the key. A
//! changed setting reaches the public site through `options.changed` (pages that show
//! it are flushed; `permalink.*` flushes everything and rebuilds every node's routes).
//! No `If-Match` (P11): last write wins.

use std::sync::Arc;

use axum::{Json, extract::State};
use ferrite_core::{Actor, settings};
use serde_json::{Map, Value};

use super::{
    ApiState, Authed,
    dto::site::SettingsDto,
    extract::ApiJson,
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    get,
    path = "/settings",
    tag = "settings",
    operation_id = "get_settings",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every setting (`null`: the default)", body = SettingsDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `settings:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_settings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<SettingsDto>, Problem> {
    Ok(Json(read(&api, &who.actor).await?))
}

/// Every setting as stored (`settings:read`). Shared with the MCP `settings_get` tool.
pub(crate) async fn read(api: &ApiState, actor: &Actor) -> Result<SettingsDto, Problem> {
    let all = settings::read_all(api.store.as_ref(), actor).await?;
    Ok(SettingsDto::from_map(all))
}

#[utoipa::path(
    patch,
    path = "/settings",
    tag = "settings",
    operation_id = "update_settings",
    security(("session" = []), ("bearer" = [])),
    request_body(content = SettingsDto, description = "Any of the keys; `null` resets one to its default"),
    responses(
        (status = 200, description = "Saved; every setting as stored", body = SettingsDto),
        (status = 400, description = "`malformed-json` (the body must be an object)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: an unknown key, or a bad value (`errors[0].field` = the key: type, bounds, a front page that is not a page, a permalink structure that is invalid or clashes with a type, taxonomy or route)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: a type or taxonomy change holds the lock a `permalink.*` change needs; retry", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_settings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(body): ApiJson<Map<String, Value>>,
) -> Result<Json<SettingsDto>, Problem> {
    Ok(Json(write(&api, &who.actor, body).await?))
}

/// Writes the keys of `body` in one transaction (`null` resets a key) and answers with
/// every setting as stored. Shared with the MCP `settings_update` tool.
pub(crate) async fn write(
    api: &ApiState,
    actor: &Actor,
    body: Map<String, Value>,
) -> Result<SettingsDto, Problem> {
    let changes: Vec<(String, Option<Value>)> = body
        .into_iter()
        .map(|(key, value)| {
            let value = (!value.is_null()).then_some(value);
            (key, value)
        })
        .collect();
    api.options.set_many(actor, &changes).await?;
    read(api, actor).await
}
