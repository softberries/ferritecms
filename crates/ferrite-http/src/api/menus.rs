//! `/menus` (07 §1).
//!
//! Handlers only translate: `MenuService` checks `manage_menus` for writes and
//! `menus:read` for reads (F16), validates the tree and emits `menu:<name>`, which
//! flushes the public pages that show the menu. No `If-Match` (P11): `PUT` replaces the
//! whole menu, last write wins. `GET /menus` also lists the menu locations of the theme
//! this node serves now (its `[menus]` table, after any hot swap), each with whether a
//! menu of that name exists.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use ferrite_core::{Actor, CoreError, menu_locations};
use ferrite_domain::{Menu, MenuItem};

use super::{
    ApiState, Authed,
    dto::site::{MenuDto, MenuInput, MenuList, MenuSummary},
    extract::{ApiJson, ApiPath},
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    get,
    path = "/menus",
    tag = "menus",
    operation_id = "list_menus",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every menu (name and label), by name, and the menu locations of the theme this node serves", body = MenuList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `menus:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_menus(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<MenuList>, Problem> {
    Ok(Json(menu_list(&api, &who.actor).await?))
}

/// Every menu and the locations of the theme this node serves (`GET /menus`, MCP
/// `menus_list`).
pub(crate) async fn menu_list(api: &ApiState, actor: &Actor) -> Result<MenuList, CoreError> {
    let menus = api.menus.list_for(actor).await?;
    let theme = api.site.theme.load();
    let locations = menu_locations(
        theme
            .manifest()
            .menus
            .iter()
            .map(|(name, label)| (name.as_str(), label.as_str())),
        &menus,
    );
    Ok(MenuList {
        items: menus
            .into_iter()
            .map(|m| MenuSummary {
                name: m.name,
                label: m.label,
                locale: m.locale,
                translation_of: m.translation_of,
            })
            .collect(),
        next_cursor: None,
        locations: locations.into_iter().map(Into::into).collect(),
    })
}

#[utoipa::path(
    get,
    path = "/menus/{name}",
    tag = "menus",
    operation_id = "get_menu",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Menu")),
    responses(
        (status = 200, description = "The menu with its entries", body = MenuDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `menus:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_menu(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<MenuDto>, Problem> {
    Ok(Json(api.menus.get_for(&who.actor, &name).await?.into()))
}

#[utoipa::path(
    put,
    path = "/menus/{name}",
    tag = "menus",
    operation_id = "put_menu",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Menu")),
    request_body = MenuInput,
    responses(
        (status = 200, description = "Created or replaced; the menu as stored", body = MenuDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_menus`, `menus:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: an entry id is used by another menu (`errors[0].field` = `items`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: name, label, entry kind/target/url, parent outside the menu or in a cycle, more than 500 entries", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn put_menu(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
    ApiJson(input): ApiJson<MenuInput>,
) -> Result<Json<MenuDto>, Problem> {
    if input.name.as_ref().is_some_and(|n| *n != name) {
        return Err(CoreError::invalid_field("name", "name must match the path").into());
    }
    let menu = Menu {
        name: name.clone(),
        label: input.label,
        items: input.items.into_iter().map(MenuItem::from).collect(),
        // Empty: the stored menu's locale, or the default for a new menu.
        locale: input.locale.unwrap_or_default(),
        translation_of: input.translation_of,
    };
    let actor = &who.actor;
    api.menus.put(actor, &menu).await?;
    Ok(Json(api.menus.get_for(actor, &name).await?.into()))
}

#[utoipa::path(
    delete,
    path = "/menus/{name}",
    tag = "menus",
    operation_id = "delete_menu",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Menu")),
    responses(
        (status = 204, description = "Deleted with its entries"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_menus`, `menus:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_menu(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, Problem> {
    api.menus.delete(&who.actor, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}
