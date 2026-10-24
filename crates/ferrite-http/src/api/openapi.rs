//! The OpenAPI 3.1 document (P12): generated from the handlers' `#[utoipa::path]`s and
//! the DTOs' `ToSchema`, served at `/api/v1/openapi.json` and committed as
//! `crates/ferrite-http/openapi.json` (the test `openapi_snapshot_is_current` fails when
//! they differ; `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`
//! rewrites the file).

use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme},
};

use super::problem::{FieldError, ProblemDocument};

/// Name of the session cookie (P7) as the `session` security scheme describes it.
pub const SESSION_COOKIE: &str = "ferrite_session";

/// The document's fixed parts; paths come from [`super::router`].
#[derive(OpenApi)]
#[openapi(
    info(title = "FerriteCMS API", version = "1", description = "FerriteCMS REST API v1."),
    modifiers(&SecuritySchemes),
    components(schemas(ProblemDocument, FieldError)),
    tags(
        (name = "meta", description = "API metadata"),
        (name = "auth", description = "Login, session, password and TOTP"),
        (name = "content", description = "Content items, their lifecycle and revisions"),
        (name = "types", description = "Content types and their field schemas"),
        (name = "taxonomies", description = "Taxonomies and their terms"),
        (name = "menus", description = "Navigation menus"),
        (name = "redirects", description = "Redirects, with CSV import and export"),
        (name = "settings", description = "Site settings"),
        (name = "users", description = "User accounts and their API tokens"),
        (name = "plugins", description = "Plugins: inspection, approval, install, enable/disable, delete"),
        (name = "themes", description = "Themes: list, upload, activation"),
    ),
)]
pub struct ApiDoc;

/// Adds the `session` (cookie `ferrite_session`) and `bearer` (API token) schemes, and
/// drops the license the derive copies from the (empty) Cargo `license` field.
struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi.info.license = None;
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))),
        );
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
    }
}
