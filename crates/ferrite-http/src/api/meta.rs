//! `GET /meta` and `GET /openapi.json`: unauthenticated; the admin SPA's first calls.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use super::{ApiState, problem::Problem};

/// The API version and the site it serves.
#[derive(Debug, Serialize, ToSchema)]
pub struct MetaDto {
    /// Always `v1` under `/api/v1`.
    pub api: String,
    /// The FerriteCMS version serving the request.
    pub version: String,
    /// `site.url`.
    pub site_url: String,
    /// The default locale's tag (M6-1).
    pub default_locale: String,
    /// Every locale of the site, the default first (public: the `hreflang` links show
    /// them anyway, design 13 D19).
    pub locales: Vec<MetaLocaleDto>,
}

/// One of the site's locales.
#[derive(Debug, Serialize, ToSchema)]
pub struct MetaLocaleDto {
    pub tag: String,
    pub label: String,
    /// The URL prefix (`de` for `/de/`); `null` for the default locale.
    pub prefix: Option<String>,
}

#[utoipa::path(
    get,
    path = "/meta",
    tag = "meta",
    operation_id = "get_meta",
    responses((status = 200, description = "API and site", body = MetaDto)),
)]
pub async fn meta(State(api): State<Arc<ApiState>>) -> Result<Json<MetaDto>, Problem> {
    let locales = ferrite_core::locales::read(api.store.read()).await?;
    Ok(Json(MetaDto {
        api: "v1".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        site_url: api.site_url.clone(),
        default_locale: locales.default.tag.clone(),
        locales: locales
            .all()
            .map(|d| MetaLocaleDto {
                tag: d.tag.clone(),
                label: d.label.clone(),
                prefix: locales.prefix_of(&d.tag),
            })
            .collect(),
    }))
}

/// The OpenAPI document as served (request extension set by [`router`](super::router)).
#[derive(Debug, Clone)]
pub(crate) struct SpecJson(pub Arc<str>);

#[utoipa::path(
    get,
    path = "/openapi.json",
    tag = "meta",
    operation_id = "get_openapi",
    responses((
        status = 200,
        description = "This OpenAPI 3.1 document",
        content_type = "application/json",
    )),
)]
pub(crate) async fn openapi_json(Extension(spec): Extension<SpecJson>) -> Response {
    if spec.0.is_empty() {
        return Problem::internal().into_response();
    }
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        spec.0.to_string(),
    )
        .into_response()
}
