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
//!   pre-flight F21); only it carries `nodes`, what each live node serves.
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
//!   what nodes serve, stay as they are, with no `theme.changed` (P12). An embedded
//!   theme without a row is installed from the embedded files and activated. Nodes swap
//!   on `theme.changed` through their reload task; a node that cannot load it keeps
//!   serving its current theme and reports `fallback` (pre-flight F12).
//!
//! The caller is authorized before any of an upload's body is read.

use std::{collections::BTreeMap, sync::Arc, sync::OnceLock};

use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use ferrite_core::{Actor, CoreError};
use ferrite_domain::{InstalledTheme, Node};
use ferrite_theme::{DEFAULT_THEME, MAX_BUNDLE_BYTES, Theme};
use serde::Deserialize;
use utoipa::IntoParams;

use super::{
    ApiState, Authed,
    dto::extensions::{ThemeBundle, ThemeDto, ThemeList, ThemeNodeState},
    extract::{ApiPath, ApiQuery},
    plugins::{live_nodes, upload_slot},
    problem::{Problem, ProblemDocument},
};
use crate::extensions::{ExtensionError, THEME_MIME};

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
    ApiQuery(params): ApiQuery<UploadParams>,
    req: Request,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    api.extensions.authorize_themes(actor)?;
    let _upload = upload_slot(&api)?;
    let bundle = read_bundle(req).await?;
    let (artifact, row) = api.installer.stage_theme(bundle).await?;
    let activate = params.activate.unwrap_or(false);
    record(&api, actor, &artifact, &row, activate).await?;
    let dto = entry(&api, actor, &row.name).await?;
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
    match api.extensions.get_theme(actor, &name).await? {
        Some(row) => {
            // P12: the row changes only once its files load. A concurrent upload may
            // replace the row in between; its bundle was checked the same way.
            api.installer
                .theme_from_artifact(&row)
                .await
                .map_err(does_not_load)?;
            api.extensions.activate_theme(actor, &name).await?;
        }
        None if embedded().contains_key(name.as_str()) => {
            let (artifact, row) = api
                .installer
                .stage_embedded(&name)
                .await
                .map_err(does_not_load)?;
            record(&api, actor, &artifact, &row, true).await?;
        }
        None => return Err(CoreError::NotFound.into()),
    }
    Ok(Json(entry(&api, actor, &name).await?))
}

/// Records a staged theme; when that fails, drops the staged blob unless another row
/// names the same bytes.
async fn record(
    api: &ApiState,
    actor: &Actor,
    artifact: &ferrite_domain::Artifact,
    row: &InstalledTheme,
    activate: bool,
) -> Result<(), Problem> {
    let Err(e) = api
        .extensions
        .install_theme(actor, artifact, row, activate)
        .await
    else {
        return Ok(());
    };
    if let Err(cleanup) = api.installer.discard_staged(&artifact.hash).await {
        tracing::warn!(artifact = %artifact.hash, error = %cleanup, "cannot discard a staged theme bundle");
    }
    Err(e.into())
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

/// An embedded starter theme's manifest summary.
struct Embedded {
    version: String,
    description: Option<String>,
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
                        description: m.description.clone(),
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

/// The list entry of `name` (`NotFound` if it has none).
async fn entry(api: &ApiState, actor: &Actor, name: &str) -> Result<ThemeDto, Problem> {
    entries(api, actor)
        .await?
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| CoreError::NotFound.into())
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
            let dto = ThemeDto::embedded(name, &e.version, e.description.as_deref());
            (name.clone(), dto)
        })
        .collect();
    for row in rows {
        items.insert(row.name.clone(), ThemeDto::installed(row));
    }
    if !rows.iter().any(|r| r.active)
        && let Some(default) = items.get_mut(DEFAULT_THEME)
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
    use crate::api::dto::extensions::ThemeOrigin;

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
                description: None,
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
    }

    #[test]
    fn node_state_reads_meta() {
        let n = node(json!({"theme": {"name": "dark", "hash": "h",
            "origin": {"artifact": "ab"}, "error": "e", "fallback": true}}));
        let s = ThemeNodeState::of(&n).unwrap();
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
