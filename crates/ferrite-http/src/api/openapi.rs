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

/// Name of the session cookie (P7) on an `http` site.
pub const SESSION_COOKIE: &str = "ferrite_session";
/// Name of the session cookie on an `https` site (M2a-2b P23): the `__Host-` prefix
/// makes browsers accept it only with `Secure`, `Path=/` and no `Domain`, so a sibling
/// subdomain or a plain-http response cannot plant or overwrite it.
pub const HOST_SESSION_COOKIE: &str = "__Host-ferrite_session";

/// The document's fixed parts; paths come from [`super::router`].
#[derive(OpenApi)]
#[openapi(
    info(title = "FerriteCMS API", version = "1", description = "FerriteCMS REST API v1."),
    modifiers(&SecuritySchemes),
    components(schemas(ProblemDocument, FieldError)),
    tags(
        (name = "meta", description = "API metadata"),
        (name = "auth", description = "Login, session, password and TOTP"),
        (name = "content", description = "Content items, their lifecycle, revisions and autosaves"),
        (name = "media", description = "Media uploads and items"),
        (name = "comments", description = "Comments: the public submit and moderation"),
        (name = "types", description = "Content types and their field schemas"),
        (name = "taxonomies", description = "Taxonomies and their terms"),
        (name = "menus", description = "Navigation menus"),
        (name = "redirects", description = "Redirects, with CSV import and export"),
        (name = "settings", description = "Site settings"),
        (name = "users", description = "User accounts and their API tokens"),
        (name = "plugins", description = "Plugins: inspection, approval, install, enable/disable, delete"),
        (name = "themes", description = "Themes: list, upload, activation"),
        (name = "system", description = "System status, nodes and the audit log (admins)"),
        (name = "import", description = "WordPress imports: start from an upload, poll the job, read runs and reports"),
        (name = "oauth", description = "AI assistants: the consent screen's pending authorization requests and the connected apps (OAuth grants)"),
        (name = "webhooks", description = "Outgoing webhooks: endpoints, the event catalogue, test deliveries, secret rotation, the delivery log and redelivery (admins)"),
        (name = "releases", description = "Releases: staged changes applied together, now or on a schedule; whole-site previews and signed preview links"),
        (name = "workflows", description = "Approval workflows per content type (admins configure)"),
        (name = "reviews", description = "Reviews: the queue, decisions, assignees, withdrawal"),
    ),
)]
pub struct ApiDoc;

/// Adds the `session` (cookie `__Host-ferrite_session`, or `ferrite_session` on an
/// `http` site) and `bearer` (API token) schemes, and
/// drops the license the derive copies from the (empty) Cargo `license` field.
struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi.info.license = None;
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                HOST_SESSION_COOKIE.to_owned(),
                format!(
                    "The session cookie set by `POST /auth/login`. Its name follows the \
                     scheme of `site.url`: `{HOST_SESSION_COOKIE}` on an `https` site, \
                     `{SESSION_COOKIE}` on an `http` site; only that name is read."
                ),
            ))),
        );
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
    }
}
