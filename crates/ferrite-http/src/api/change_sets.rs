//! `/change-sets` (M6-5, design 17 §9): the AI change sets of the caller (or anyone's,
//! for administrators), their changes with before/after images, and revert.
//!
//! Handlers only translate: `ChangeSetService` checks `read_admin` with `users:read`,
//! the set's owner or `manage_users`, the viewer's read right for images, and every
//! reverse write as the reverting person. Revert takes a signed-in session only (an API
//! token is `403`, as the consent API: undoing is a person's decision), CSRF as every
//! write, and an optional `Idempotency-Key`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use ferrite_core::{
    Actor, Via,
    changes::{ChangeSetFilter, Images, RevertError, RevertRequest, is_open},
};
use ferrite_domain::{AgentKind, ChangeSet, ChangeSetId, ChangeSummary, MAX_PAGE, Timestamp};
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed,
    dto::change_sets::{
        ChangeDetailDto, ChangeSetAgentDto, ChangeSetDetailDto, ChangeSetDto, ChangeSetList,
        ChangeSetUserDto, RevertConflictDto, RevertInput, RevertReportDto, change_dto,
    },
    extract::{ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit},
    idempotency,
    problem::{Problem, ProblemDocument},
};

/// The query of `GET /change-sets`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChangeSetListQuery {
    /// Another user's sets (`manage_users`), or `all` for every user's.
    pub user: Option<String>,
    /// Only one connection's (a grant or token id).
    pub agent: Option<Uuid>,
    /// Only open (`true`) or closed (`false`) sets.
    pub open: Option<bool>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

fn status_of(set: &ChangeSet, now: Timestamp) -> &'static str {
    if set.reverted_changes > 0 && set.reverted_changes >= set.changes {
        "reverted"
    } else if set.reverted_changes > 0 {
        "partly_reverted"
    } else if is_open(set, now) {
        "open"
    } else {
        "closed"
    }
}

/// The DTO of `set`, with its user's names and whether the connection still exists.
async fn set_dto(api: &ApiState, set: ChangeSet) -> Result<ChangeSetDto, Problem> {
    let repos = api.store.read();
    let user = match set.user_id {
        Some(id) => repos.users().get(id).await?.map(|u| ChangeSetUserDto {
            id: u.id.as_uuid(),
            login: u.login,
            display_name: u.display_name,
        }),
        None => None,
    };
    let connected = match (set.agent_kind, set.user_id) {
        (AgentKind::OAuth, _) => repos
            .oauth()
            .get_grant(ferrite_domain::OAuthGrantId::from_uuid(set.agent_id))
            .await?
            .is_some(),
        (AgentKind::Token, Some(u)) => repos
            .api_tokens()
            .list_for_user(u)
            .await?
            .iter()
            .any(|t| t.id.as_uuid() == set.agent_id),
        (AgentKind::Token, None) => false,
    };
    let now = Timestamp::now();
    Ok(ChangeSetDto {
        id: set.id.as_uuid(),
        user,
        status: status_of(&set, now).to_owned(),
        open: is_open(&set, now),
        agent: ChangeSetAgentDto {
            kind: set.agent_kind.as_str().to_owned(),
            id: set.agent_id,
            client: set.client,
            connected,
        },
        started_at: set.started_at.as_datetime(),
        last_at: set.last_at.as_datetime(),
        changes: set.changes,
        reverted: set.reverted_changes,
    })
}

/// The admin screen of a change's object while it exists (relative to `/admin`).
async fn admin_path(api: &ApiState, c: &ChangeSummary) -> Result<Option<String>, Problem> {
    let repos = api.store.read();
    let id = c.object_id.as_str();
    Ok(match c.object_kind.as_str() {
        "content" => match id.parse() {
            Ok(cid) => repos
                .content()
                .get(cid)
                .await?
                .map(|i| format!("/content/{}/{}", i.kind, i.id)),
            Err(_) => None,
        },
        "media" => match id.parse() {
            Ok(cid) => repos.content().get(cid).await?.map(|_| "/media".to_owned()),
            Err(_) => None,
        },
        "term" => match id.parse() {
            Ok(tid) => repos
                .terms()
                .get(tid)
                .await?
                .map(|t| format!("/structure/taxonomies/{}", t.taxonomy)),
            Err(_) => None,
        },
        "menu" => repos
            .menus()
            .get(id)
            .await?
            .map(|m| format!("/structure/menus/{}", m.name)),
        "type" => repos
            .types()
            .get(id)
            .await?
            .map(|t| format!("/structure/types/{}", t.name)),
        "redirect" => Some("/structure/redirects".to_owned()),
        "comment" => Some("/comments".to_owned()),
        "option" if id.starts_with("theme.") => Some("/appearance/themes".to_owned()),
        "option" if id.starts_with("plugin.") => id
            .strip_prefix("plugin.")
            .and_then(|r| r.split('.').next())
            .map(|name| format!("/plugins/{name}")),
        "option" => Some("/settings".to_owned()),
        "theme" => Some("/appearance/themes".to_owned()),
        "plugin" => repos
            .plugins()
            .get(id)
            .await?
            .map(|p| format!("/plugins/{}", p.name)),
        _ => None,
    })
}

#[utoipa::path(
    get,
    path = "/change-sets",
    tag = "change-sets",
    operation_id = "list_change_sets",
    security(("session" = []), ("bearer" = [])),
    params(ChangeSetListQuery),
    responses(
        (status = 200, description = "Change sets, newest last change first", body = ChangeSetList),
        (status = 400, description = "`malformed-query` or `invalid-cursor`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`read_admin`, `users:read`; another user's or `all`: `manage_users`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`limit`, `user`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_change_sets(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(q): ApiQuery<ChangeSetListQuery>,
) -> Result<Json<ChangeSetList>, Problem> {
    let limit = check_limit(q.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    let (user, all_users) = match q.user.as_deref() {
        None => (None, false),
        Some("all") => (None, true),
        Some(raw) => match raw.parse::<ferrite_domain::UserId>() {
            Ok(id) => (Some(id), false),
            Err(_) => {
                return Err(Problem::validation("user must be a user id or `all`")
                    .with_error(Some("user"), "user must be a user id or `all`"));
            }
        },
    };
    let page = api
        .change_sets
        .list(
            &who.actor,
            &ChangeSetFilter {
                user,
                all_users,
                agent: q.agent,
                open: q.open,
                limit,
                cursor: q.cursor,
            },
        )
        .await
        .map_err(|e| match e {
            ferrite_core::CoreError::Store(ferrite_store::StoreError::Invalid(_)) => {
                super::extract::invalid_cursor()
            }
            e => e.into(),
        })?;
    let mut items = Vec::with_capacity(page.items.len());
    for s in page.items {
        items.push(set_dto(&api, s).await?);
    }
    Ok(Json(ChangeSetList {
        items,
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    get,
    path = "/change-sets/{id}",
    tag = "change-sets",
    operation_id = "get_change_set",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The set and its changes, newest first", body = ChangeSetDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`read_admin`, `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found` (also another user's set without `manage_users`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_change_set(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ChangeSetDetailDto>, Problem> {
    let detail = api
        .change_sets
        .get(&who.actor, ChangeSetId::from_uuid(id))
        .await?;
    let mut changes = Vec::with_capacity(detail.changes.len());
    for c in &detail.changes {
        changes.push(change_dto(c, admin_path(&api, c).await?));
    }
    Ok(Json(ChangeSetDetailDto {
        set: set_dto(&api, detail.set).await?,
        changes,
    }))
}

#[utoipa::path(
    get,
    path = "/change-sets/{id}/changes/{seq}",
    tag = "change-sets",
    operation_id = "get_change_set_change",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("seq" = u32, Path)),
    responses(
        (status = 200, description = "The change with its before and after images (`images: hidden` without your read right on the object's area; plugin secrets as `{\"secret\": \"set\"}`)", body = ChangeDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`read_admin`, `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_change_set_change(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, seq)): ApiPath<(Uuid, u32)>,
) -> Result<Json<ChangeDetailDto>, Problem> {
    let d = api
        .change_sets
        .change(&who.actor, ChangeSetId::from_uuid(id), seq)
        .await?;
    let path = admin_path(&api, &d.change).await?;
    let change = change_dto(&d.change, path);
    Ok(Json(match d.images {
        Images::Hidden => ChangeDetailDto {
            change,
            images: "hidden".into(),
            before: None,
            after: None,
        },
        Images::Shown { before, after } => ChangeDetailDto {
            change,
            images: "shown".into(),
            before,
            after,
        },
    }))
}

/// `403` unless `actor` signed in with a session.
fn session_only(via: &Via) -> Result<(), Problem> {
    match via {
        Via::Session(_) => Ok(()),
        Via::Token(_) => Err(Problem::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Forbidden",
            "reverting changes needs a signed-in session, not an API token",
        )),
    }
}

fn revert_problem(e: RevertError) -> Problem {
    match e {
        RevertError::Conflicts(list) => {
            let conflicts: Vec<RevertConflictDto> = list.into_iter().map(Into::into).collect();
            Problem::new(
                StatusCode::CONFLICT,
                "change-conflict",
                "Changes conflict",
                "some changes cannot be reverted as asked; see report.conflicts",
            )
            .with_report(json!({ "conflicts": conflicts }))
        }
        RevertError::Failed { seq, error } => {
            Problem::from(error).with_report(json!({ "change": seq }))
        }
        RevertError::Core(e) => e.into(),
    }
}

async fn run_revert(
    api: &ApiState,
    actor: &Actor,
    id: Uuid,
    input: RevertInput,
) -> Result<RevertReportDto, Problem> {
    let report = api
        .change_sets
        .revert(
            actor,
            ChangeSetId::from_uuid(id),
            RevertRequest {
                changes: input.changes,
                force: input.force,
                dry_run: input.dry_run,
            },
        )
        .await
        .map_err(revert_problem)?;
    Ok(report.into())
}

#[utoipa::path(
    post,
    path = "/change-sets/{id}/revert",
    tag = "change-sets",
    operation_id = "revert_change_set",
    security(("session" = [])),
    params(("id" = Uuid, Path)),
    request_body = RevertInput,
    responses(
        (status = 200, description = "Reverted (or, with `dry_run`, what would be)", body = RevertReportDto),
        (status = 400, description = "`malformed-json` or `malformed-header` (`Idempotency-Key`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (a token, not a session; or a reverse write's missing capability, `report.change` naming the change), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`change-conflict` (`report.conflicts`), or an `Idempotency-Key` still in use", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`changes`: unknown seq or more than 500; or a reverse write's own error, `report.change` naming the change)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn revert_change_set(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ApiJson(input): ApiJson<RevertInput>,
) -> Response {
    if let Err(p) = session_only(&who.via) {
        return p.into_response();
    }
    let key = match idempotency::parse_key(&headers) {
        Ok(key) => key,
        Err(p) => return p.into_response(),
    };
    let guard = match key {
        Some(key) => {
            let path = uri
                .path_and_query()
                .map_or_else(|| uri.path().to_owned(), |p| p.as_str().to_owned());
            match idempotency::begin(&api, who.actor.user, &key, &method, &path).await {
                Ok(g) => Some(g),
                Err(res) => return *res,
            }
        }
        None => None,
    };
    let result = run_revert(&api, &who.actor, id, input).await;
    match (guard, result) {
        (Some(guard), Ok(report)) => {
            let body = serde_json::to_value(&report).unwrap_or_default();
            guard.finish(StatusCode::OK, &HeaderMap::new(), &body).await;
            Json(report).into_response()
        }
        (Some(guard), Err(p)) => {
            guard.release().await;
            p.into_response()
        }
        (None, Ok(report)) => Json(report).into_response(),
        (None, Err(p)) => p.into_response(),
    }
}
