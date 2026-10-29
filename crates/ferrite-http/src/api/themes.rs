//! `/themes` (07 §1; M2a-2 P8, P10, P11, P12, P30; Review Focus 5).
//!
//! Handlers translate; `ExtensionAdmin` checks `manage_themes` with `extensions:write`
//! for writes and `manage_themes` with `extensions:read` for the list, records the rows
//! under the `extensions` lock and audits every write. The node's one
//! [`Installer`](crate::extensions::Installer) unpacks and compiles uploads. No
//! `If-Match` (P11), and no delete (P30: unused theme artifacts stay until M4's GC).
//!
//! * `GET /themes` lists every installed theme and every embedded theme without a row.
//!   The active one is the active row or, with none, the embedded default (`blog`,
//!   pre-flight F21) while it has no row; only it carries `nodes`, what each live node
//!   serves. (With no row active and an inactive `blog` row, no entry is active and no
//!   node states are shown: nodes serve the embedded files, which no entry describes.)
//! * `POST /themes` takes a raw ustar bundle (`application/x-tar`, at most
//!   [`MAX_BUNDLE_BYTES`]; P11). It is unpacked (regular files only; no links, absolute
//!   paths or `..`) and every template compiled before anything is stored; a theme
//!   that does not load is `422` with one `errors[]` entry per problem
//!   (`templates/<file>:<line>: …`). An upload named like an embedded theme becomes
//!   that theme's row (the list then shows the row, and activation uses it); one named
//!   like an installed theme replaces it (an active one stays active). `?activate=true`
//!   activates it in the same write.
//! * `POST /themes/{name}/activate` loads the row's artifact first (fetched, hash-checked,
//!   unpacked, compiled); one that does not load is `422` and the active theme, and
//!   what nodes serve, stay as they are, with no `theme.changed` (P12). The activation
//!   then records only if the row still names the artifact just loaded (checked under
//!   the `extensions` lock): a row replaced in between is `409 conflict` (R10). An embedded
//!   theme without a row is installed from the embedded files and activated. Nodes swap
//!   on `theme.changed` through their reload task; a node that cannot load it keeps
//!   serving its current theme and reports `fallback` (pre-flight F12).
//!
//! * `GET /themes/{name}/settings` answers the theme's settings schema and every
//!   setting's value as its templates see it; `PATCH` writes the keys of its body
//!   (`null` resets one) and answers the same. The schema is the one the row stores,
//!   else the row's artifact's (a row stored before rows carried it), else the
//!   embedded theme's when there is no row (pre-flight F22). A changed value reaches
//!   the public site through `options.changed` with `option:theme.<name>.settings`,
//!   which every page of the theme records (P14: read per cache miss).
//!
//! The caller is authorized before any of an upload's body is read, and before a
//! theme's files are loaded for its schema.

use std::{collections::BTreeMap, sync::Arc, sync::OnceLock};

use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use ferrite_core::{Actor, CoreError, ext_settings::SettingsSchema};
use ferrite_domain::{InstalledTheme, Node};
use ferrite_theme::{DEFAULT_THEME, MAX_BUNDLE_BYTES, Theme};
use serde::Deserialize;
use serde_json::{Map, Value};
use utoipa::IntoParams;

use super::{
    ApiState, Authed,
    dto::extensions::{
        SettingsPatch, ThemeBundle, ThemeDto, ThemeList, ThemeNodeState, ThemeOrigin,
        ThemeSettingsDto,
    },
    extract::{ApiJson, ApiPath, parse_query},
    plugins::{live_nodes, upload_slot},
    problem::{Problem, ProblemDocument},
};
use crate::extensions::{ExtensionError, THEME_MIME, theme_row_manifest, theme_schema};

/// The body limit of `POST /themes` (P11): a theme bundle's limit.
pub const MAX_THEME_UPLOAD_BODY: usize = MAX_BUNDLE_BYTES as usize;

/// Longest theme name (the `theme.toml` rule).
const MAX_THEME_NAME: usize = 64;

#[utoipa::path(
    get,
    path = "/themes",
    tag = "themes",
    operation_id = "list_themes",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every installed theme and every embedded theme without a row, by name", body = ThemeList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_themes`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_themes(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<ThemeList>, Problem> {
    Ok(Json(ThemeList {
        items: entries(&api, &who.actor).await?,
        next_cursor: None,
    }))
}

/// `POST /themes` query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadParams {
    /// `true`: make the uploaded theme the active one in the same write (default
    /// `false`).
    pub activate: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/themes",
    tag = "themes",
    operation_id = "install_theme",
    security(("session" = []), ("bearer" = [])),
    params(UploadParams),
    request_body(content = ThemeBundle, content_type = "application/x-tar", description = "A theme bundle: a ustar archive of `theme.toml`, `templates/` and `assets/` (regular files only), at most 23 020 544 bytes"),
    responses(
        (status = 201, description = "Installed (replacing a theme of that name; an active one stays active), active with `activate=true`", body = ThemeDto),
        (status = 400, description = "`malformed-query`: `activate` is not `true` or `false`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_themes`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: the body is over the bundle limit", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `application/x-tar`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the theme does not load; one `errors[]` entry per problem (`templates/<file>:<line>: …` for a template); nothing is stored", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: every upload slot of this node is taken, or another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn install_theme(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    req: Request,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    api.extensions.authorize_themes(actor)?;
    let params: UploadParams = parse_query(req.uri())?;
    let _upload = upload_slot(&api)?;
    let bundle = read_bundle(req).await?;
    let (artifact, row) = api.installer.stage_theme(bundle).await?;
    let activate = params.activate.unwrap_or(false);
    record(&api, actor, &artifact, &row, activate).await?;
    let dto = written(&api, actor, &row, activate).await;
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

#[utoipa::path(
    post,
    path = "/themes/{name}/activate",
    tag = "themes",
    operation_id = "activate_theme",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Theme")),
    responses(
        (status = 200, description = "Active (an embedded theme without a row is installed first); nodes swap on `theme.changed`", body = ThemeDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_themes`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no installed or embedded theme of that name", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: the theme was replaced while it was being checked; the active theme is unchanged (retry)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the theme does not load (its artifact is missing, tampered with or does not compile); the active theme is unchanged", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn activate_theme(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<ThemeDto>, Problem> {
    let actor = &who.actor;
    api.extensions.authorize_themes(actor)?;
    if !valid_name(&name) {
        return Err(CoreError::NotFound.into());
    }
    let row = match api.extensions.get_theme(actor, &name).await? {
        Some(row) => {
            // P12: the row changes only once its files load, and only if it still names
            // the bundle just loaded (R10: a row replaced in between is `409 conflict`).
            api.installer
                .theme_from_artifact(&row)
                .await
                .map_err(does_not_load)?;
            api.extensions
                .activate_theme(actor, &name, &row.artifact_hash)
                .await?;
            row
        }
        None if embedded().contains_key(name.as_str()) => {
            let (artifact, row) = api
                .installer
                .stage_embedded(&name)
                .await
                .map_err(does_not_load)?;
            record(&api, actor, &artifact, &row, true).await?;
            row
        }
        None => return Err(CoreError::NotFound.into()),
    };
    Ok(Json(written(&api, actor, &row, true).await))
}

#[utoipa::path(
    get,
    path = "/themes/{name}/settings",
    tag = "themes",
    operation_id = "get_theme_settings",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Theme (installed or embedded)")),
    responses(
        (status = 200, description = "The settings schema and every setting's value as templates see it", body = ThemeSettingsDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_themes`, `extensions:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no installed or embedded theme of that name", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the row stores no schema and its artifact does not load", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_theme_settings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<ThemeSettingsDto>, Problem> {
    let actor = &who.actor;
    let schema = theme_settings_schema(&api, actor, &name).await?;
    let values = api.extensions.theme_settings(actor, &name, &schema).await?;
    Ok(Json(ThemeSettingsDto::new(&schema, values)))
}

#[utoipa::path(
    patch,
    path = "/themes/{name}/settings",
    tag = "themes",
    operation_id = "update_theme_settings",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Theme (installed or embedded)")),
    request_body(content = SettingsPatch, description = "Declared keys and their new values; `null` resets a key to its default"),
    responses(
        (status = 200, description = "Saved; the schema and every value as templates see it. Pages showing the theme are flushed on every node (`options.changed`)", body = ThemeSettingsDto),
        (status = 400, description = "`malformed-json` (the body must be an object)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_themes`, `extensions:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no installed or embedded theme of that name", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: an undeclared key, or a bad value (`errors[0].field` = the key: type, length, option, colour or media id); nothing is written", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: another extension change holds the lock (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_theme_settings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
    ApiJson(patch): ApiJson<Map<String, Value>>,
) -> Result<Json<ThemeSettingsDto>, Problem> {
    let actor = &who.actor;
    api.extensions.authorize_themes(actor)?;
    let schema = theme_settings_schema(&api, actor, &name).await?;
    let values = api
        .extensions
        .set_theme_settings(actor, &name, &schema, patch)
        .await?;
    Ok(Json(ThemeSettingsDto::new(&schema, values)))
}

/// The settings schema of theme `name` (pre-flight F22): the row's (stored, else from
/// its artifact), else the embedded theme's when there is no row; no theme of that name
/// is `404`. `actor` must read themes (checked by `get_theme` before anything loads).
async fn theme_settings_schema(
    api: &ApiState,
    actor: &Actor,
    name: &str,
) -> Result<SettingsSchema, Problem> {
    match api.extensions.get_theme(actor, name).await? {
        Some(row) => api
            .installer
            .theme_row_schema(&row)
            .await
            .map_err(does_not_load),
        None => embedded()
            .get(name)
            .map(|e| e.schema.clone())
            .ok_or_else(|| CoreError::NotFound.into()),
    }
}

/// Records a staged theme; when that fails, the staged blob has no row and is left to
/// the artifacts GC (M4-1 P12).
async fn record(
    api: &ApiState,
    actor: &Actor,
    artifact: &ferrite_domain::Artifact,
    row: &InstalledTheme,
    activate: bool,
) -> Result<(), Problem> {
    api.extensions
        .install_theme(actor, artifact, row, activate)
        .await?;
    Ok(())
}

/// The body of `req` if it is `application/x-tar` of at most [`MAX_THEME_UPLOAD_BODY`]
/// bytes; a declared `Content-Length` over the limit is refused before anything is read,
/// and the body is read no further than the limit.
async fn read_bundle(req: Request) -> Result<Bytes, Problem> {
    let essence = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap_or_default()
        .trim();
    if !essence.eq_ignore_ascii_case(THEME_MIME) {
        return Err(Problem::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-media-type",
            "Unsupported media type",
            "the body must be a theme bundle (application/x-tar)",
        ));
    }
    let too_large = || {
        Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too-large",
            "Request too large",
            format!("the body is larger than {MAX_THEME_UPLOAD_BODY} bytes"),
        )
    };
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BUNDLE_BYTES) {
        return Err(too_large());
    }
    axum::body::to_bytes(req.into_body(), MAX_THEME_UPLOAD_BODY)
        .await
        .map_err(|_| too_large())
}

/// A `422` about files that do not load, said as such (pre-flight F11).
fn does_not_load(e: ExtensionError) -> Problem {
    const PREFIX: &str = "the theme does not load";
    let mut p = Problem::from(e);
    if p.status == StatusCode::UNPROCESSABLE_ENTITY && !p.detail.starts_with(PREFIX) {
        p.detail = format!("{PREFIX}: {}", p.detail);
    }
    p
}

/// The `theme.toml` name rule: 1–64 of `[a-z0-9-]`. Anything else names no theme.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_THEME_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An embedded starter theme's manifest: the object a row would store, and its
/// settings schema.
struct Embedded {
    version: String,
    manifest: Value,
    schema: SettingsSchema,
}

/// The embedded starter themes, by name (loaded once; one that does not load is logged
/// and left out).
fn embedded() -> &'static BTreeMap<String, Embedded> {
    static EMBEDDED: OnceLock<BTreeMap<String, Embedded>> = OnceLock::new();
    EMBEDDED.get_or_init(|| {
        Theme::embedded_names()
            .into_iter()
            .filter_map(|name| match Theme::load_embedded(&name) {
                Ok(theme) => {
                    let m = theme.manifest();
                    let summary = Embedded {
                        version: m.version.to_string(),
                        manifest: theme_row_manifest(m),
                        schema: theme_schema(m),
                    };
                    Some((name, summary))
                }
                Err(e) => {
                    tracing::error!(theme = %name, error = %e, "embedded theme does not load");
                    None
                }
            })
            .collect()
    })
}

/// The response after a committed write of `row`: its list entry (with what nodes
/// serve and the state another write may have given it since). The write succeeded, so
/// a failing re-read does not fail the request: the entry is then built from `row`
/// (`active` as written, no node states) and the failure logged.
async fn written(api: &ApiState, actor: &Actor, row: &InstalledTheme, active: bool) -> ThemeDto {
    match entries(api, actor).await {
        Ok(items) => {
            if let Some(dto) = items.into_iter().find(|t| t.name == row.name) {
                return dto;
            }
            tracing::warn!(theme = %row.name, "a written theme is not listed");
        }
        Err(p) => {
            tracing::warn!(theme = %row.name, detail = %p.detail, "cannot list the themes after a write");
        }
    }
    let mut dto = ThemeDto::installed(row);
    dto.active = dto.active || active;
    dto
}

async fn entries(api: &ApiState, actor: &Actor) -> Result<Vec<ThemeDto>, Problem> {
    let rows = api.extensions.list_themes(actor).await?;
    let nodes = live_nodes(api).await?;
    Ok(list(&rows, embedded(), &nodes))
}

/// Every row plus every embedded theme without a row, by name. The active entry is the
/// active row or, with none, [`DEFAULT_THEME`] (what nodes serve then); it alone gets
/// the node states.
fn list(
    rows: &[InstalledTheme],
    embedded: &BTreeMap<String, Embedded>,
    nodes: &[Node],
) -> Vec<ThemeDto> {
    let mut items: BTreeMap<String, ThemeDto> = embedded
        .iter()
        .map(|(name, e)| {
            let dto = ThemeDto::embedded(name, &e.version, &e.manifest);
            (name.clone(), dto)
        })
        .collect();
    for row in rows {
        items.insert(row.name.clone(), ThemeDto::installed(row));
    }
    // No row active: nodes serve the embedded default, which is that entry while it has
    // no row (an inactive `blog` row is not what they serve, so it stays inactive).
    if !rows.iter().any(|r| r.active)
        && let Some(default) = items.get_mut(DEFAULT_THEME)
        && default.origin == ThemeOrigin::Embedded
    {
        default.active = true;
    }
    for dto in items.values_mut().filter(|t| t.active) {
        dto.nodes = nodes.iter().filter_map(ThemeNodeState::of).collect();
    }
    items.into_values().collect()
}

#[cfg(test)]
mod tests {
    use ferrite_domain::{ArtifactHash, NodeId, Timestamp};
    use serde_json::json;

    use super::*;

    fn row(name: &str, active: bool) -> InstalledTheme {
        InstalledTheme {
            name: name.into(),
            version: "1.0.0".into(),
            artifact_hash: ArtifactHash::parse(&"a".repeat(64)).unwrap(),
            manifest: json!({"name": name, "description": "d"}),
            active,
            installed_at: Timestamp::from_micros(0).unwrap(),
        }
    }

    fn node(meta: serde_json::Value) -> Node {
        let at = Timestamp::from_micros(0).unwrap();
        Node {
            id: NodeId::new(),
            name: "n".into(),
            version: "0".into(),
            schema_version: 1,
            started_at: at,
            heartbeat_at: at,
            meta,
        }
    }

    fn embedded_blog() -> BTreeMap<String, Embedded> {
        BTreeMap::from([(
            "blog".to_owned(),
            Embedded {
                version: "0.1.0".into(),
                manifest: json!({}),
                schema: SettingsSchema::default(),
            },
        )])
    }

    #[test]
    fn rows_shadow_embedded_and_default_is_active_without_a_row() {
        let n = node(
            json!({"theme": {"name": "blog", "hash": "h", "origin": "embedded",
            "error": null, "fallback": false}}),
        );
        let items = list(
            &[row("dark", false)],
            &embedded_blog(),
            std::slice::from_ref(&n),
        );
        let names: Vec<_> = items.iter().map(|t| (t.name.as_str(), t.active)).collect();
        assert_eq!(names, [("blog", true), ("dark", false)]);
        assert_eq!(items[0].origin, ThemeOrigin::Embedded);
        assert_eq!(items[0].nodes.len(), 1);
        assert!(items[1].nodes.is_empty());

        let items = list(
            &[row("blog", false), row("dark", true)],
            &embedded_blog(),
            &[n],
        );
        assert_eq!(items.len(), 2, "the row shadows the embedded theme");
        assert_eq!(items[0].origin, ThemeOrigin::Installed);
        assert!(!items[0].active && items[0].nodes.is_empty());
        assert!(items[1].active && items[1].nodes.len() == 1);

        // No row active and an inactive `blog` row: that row is not what nodes serve.
        let n = node(json!({"theme": {"name": "blog", "hash": "h", "origin": "embedded"}}));
        let items = list(
            &[row("blog", false), row("dark", false)],
            &embedded_blog(),
            &[n],
        );
        assert!(items.iter().all(|t| !t.active && t.nodes.is_empty()));
    }

    #[test]
    fn node_state_reads_meta() {
        let n = node(json!({"theme": {"name": "dark", "hash": "h",
            "origin": {"artifact": "ab"}, "error": "e", "fallback": true}}));
        let s = ThemeNodeState::of(&n).unwrap();
        assert_eq!((s.node_name.as_str(), s.node_version.as_str()), ("n", "0"));
        assert_eq!(
            (
                s.name.as_str(),
                s.origin.as_str(),
                s.error.as_deref(),
                s.fallback
            ),
            ("dark", "artifact", Some("e"), true)
        );
        assert!(ThemeNodeState::of(&node(json!({}))).is_none());
        assert!(ThemeNodeState::of(&node(json!({"theme": {"name": "x"}}))).is_none());
    }

    #[test]
    fn names_follow_the_manifest_rule() {
        for ok in ["blog", "a", "dark-2", &"x".repeat(64)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "Blog", "a_b", "a.b", "a b", "../x", &"x".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
