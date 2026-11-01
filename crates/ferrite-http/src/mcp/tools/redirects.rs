//! Redirect tools (design 12 §5.2): `redirects_list`, `redirect_create`,
//! `redirect_update` and `redirect_delete`.
//!
//! Adapters over [`RedirectService`](ferrite_core::RedirectService) and the REST layer's
//! DTOs and merge rule (`api::redirects::merge_redirect`). Core checks every
//! permission (`manage_redirects` with `redirects:read` / `redirects:write`) and the
//! source, target, code, loop and chain rules. Redirects are last-write-wins, as in the
//! REST API (no version).

use ferrite_domain::{MAX_PAGE, RedirectId, RedirectQuery};
use ferrite_store::REDIRECT_CURSOR_TAG;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::per_page;
use crate::{
    api::{
        dto::site::RedirectDto, extract::invalid_cursor, problem::Problem,
        redirects::merge_redirect,
    },
    mcp::{Caller, FerriteMcp, results},
};

// ---- inputs ------------------------------------------------------------------------------

/// The input of `redirects_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Only redirects whose source or target holds this text, ignoring case (at most
    /// 200 characters).
    #[serde(default)]
    pub q: Option<String>,
    /// Redirects per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same query, to get the next page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// The input of `redirect_create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The old path to catch, starting with `/` (for example `/old-page/`). It matches
    /// exactly, or with the trailing slash toggled.
    pub source: String,
    /// Where to send visitors: a local path (`/new-page/`) or an absolute http(s) URL.
    pub target: String,
    /// The HTTP status: 301 (moved permanently, the default), 302, 307, 308 or 410
    /// (gone).
    #[serde(default)]
    pub code: Option<u16>,
}

/// The input of `redirect_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The redirect's id.
    pub id: Uuid,
    /// A new source path. Leave out (or `null`) to keep it.
    #[serde(default)]
    pub source: Option<String>,
    /// A new target. Leave out (or `null`) to keep it.
    #[serde(default)]
    pub target: Option<String>,
    /// A new status code (301, 302, 307, 308 or 410). Leave out (or `null`) to keep it.
    #[serde(default)]
    pub code: Option<u16>,
}

/// A redirect named by id (`redirect_delete`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    /// The redirect's id.
    pub id: Uuid,
}

// ---- outputs -----------------------------------------------------------------------------

/// One page of `redirects_list`.
#[derive(Debug, Serialize)]
pub struct ListOut {
    pub items: Vec<RedirectDto>,
    /// Pass as `cursor` for the next page; `null` on the last one.
    pub next_cursor: Option<String>,
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = redirect_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists redirects ordered by source path: id, source, target, status code, how
    /// often it was hit, and how it came to be (`manual`, `import`, or `slug-change` for
    /// the ones the site adds itself when a published address changes). Filter with `q`
    /// (text in the source or target); page with `cursor`.
    #[tool(annotations(
        title = "List redirects",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn redirects_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ListInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n.min(MAX_PAGE),
            Err(e) => return e,
        };
        if let Some(c) = &input.cursor
            && ferrite_store::text_cursor(REDIRECT_CURSOR_TAG, c).is_err()
        {
            return results::from_problem(invalid_cursor());
        }
        match self
            .api
            .redirects
            .list(
                caller.actor(),
                RedirectQuery {
                    q: input.q,
                    limit,
                    cursor: input.cursor,
                },
            )
            .await
        {
            Ok(page) => {
                let out = ListOut {
                    items: page.items.into_iter().map(RedirectDto::from).collect(),
                    next_cursor: page.next_cursor,
                };
                let n = out.items.len();
                let more = if out.next_cursor.is_some() {
                    "; more with next_cursor"
                } else {
                    ""
                };
                let summary = format!("{n} redirect{}{more}.", if n == 1 { "" } else { "s" });
                results::ok(out, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Redirects visitors from an old path to a new place (a local path or a full URL),
    /// by default permanently (301); 410 says the page is gone. If a redirect for this
    /// source already exists its target and code are replaced. Loops and chains are
    /// refused. Use it when you move or remove a page; the site already adds a 301 by
    /// itself when you change the slug of a published item. Returns the redirect.
    #[tool(annotations(
        title = "Create redirect",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn redirect_create(
        &self,
        caller: Caller,
        Parameters(input): Parameters<CreateInput>,
    ) -> CallToolResult {
        let code = input.code.unwrap_or(301);
        match self
            .api
            .redirects
            .upsert(caller.actor(), &input.source, &input.target, code)
            .await
        {
            Ok(r) => {
                self.audit(
                    &caller,
                    "redirect_create",
                    "redirect",
                    Some(r.id.to_string()),
                    json!({"source": r.source_path, "code": r.code}),
                )
                .await;
                let summary = format!("{} -> {} ({}).", r.source_path, r.target, r.code);
                results::ok(RedirectDto::from(r), summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Changes a redirect's source, target or status code; what you leave out, or send as
    /// `null`, is kept.
    /// Find the id with redirects_list. Returns the redirect.
    #[tool(annotations(
        title = "Update redirect",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn redirect_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UpdateInput>,
    ) -> CallToolResult {
        if input.source.is_none() && input.target.is_none() && input.code.is_none() {
            return results::invalid(None, "send at least one of source, target, code");
        }
        match merge_redirect(
            &self.api,
            caller.actor(),
            RedirectId::from_uuid(input.id),
            input.source.as_deref(),
            input.target.as_deref(),
            input.code,
        )
        .await
        {
            Ok(r) => {
                self.audit(
                    &caller,
                    "redirect_update",
                    "redirect",
                    Some(r.id.to_string()),
                    json!({"source": r.source_path, "code": r.code}),
                )
                .await;
                let summary = format!("{} -> {} ({}).", r.source_path, r.target, r.code);
                results::ok(RedirectDto::from(r), summary)
            }
            Err(e) => results::from_problem(Problem::from(e)),
        }
    }

    /// Deletes a redirect: visitors to its source get the site's normal answer again
    /// (usually a 404). This cannot be undone, so ask the user first. Find the id with
    /// redirects_list.
    #[tool(annotations(
        title = "Delete redirect",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn redirect_delete(
        &self,
        caller: Caller,
        Parameters(input): Parameters<IdInput>,
    ) -> CallToolResult {
        let id = RedirectId::from_uuid(input.id);
        let actor = caller.actor();
        // Named in the answer and the audit row; a failed read is the delete's own error.
        let redirect = match self.api.redirects.get(actor, id).await {
            Ok(r) => r,
            Err(e) => return results::from_core(e),
        };
        match self.api.redirects.delete(actor, id).await {
            Ok(()) => {
                self.audit(
                    &caller,
                    "redirect_delete",
                    "redirect",
                    Some(id.to_string()),
                    json!({"source": redirect.source_path, "target": redirect.target}),
                )
                .await;
                let summary = format!("Deleted the redirect from {}.", redirect.source_path);
                results::ok(
                    json!({"id": id.as_uuid(), "source": redirect.source_path, "deleted": true}),
                    summary,
                )
            }
            Err(e) => results::from_core(e),
        }
    }
}
