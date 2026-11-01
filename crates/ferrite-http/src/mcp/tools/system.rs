//! System tools (design 12 §5.2): `system_status`, `jobs_list` and `audit_list`.
//! Read-only: retrying jobs, imports, backups, restores and updates stay with a person
//! (design 12 §5.4).
//!
//! Adapters over the REST layer's system functions (`api::system::{status, job_page,
//! audit_page}`); core checks `manage_system` (administrators) with `system:read` before
//! anything is read, so other roles and narrower tokens learn nothing, not even the
//! version.

use ferrite_domain::MAX_PAGE;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::per_page;
use crate::{
    api::system::{AuditListParams, JobListParams, audit_page, job_page, status},
    mcp::{Caller, FerriteMcp, results},
};

/// The state of a job.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Waiting for a node to run it (or for its retry time).
    Queued,
    /// A node is running it now.
    Running,
    /// Finished.
    Done,
    /// Failed too many times; a person can retry it in the admin.
    Dead,
}

impl JobStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Dead => "dead",
        }
    }
}

/// The input of `jobs_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobsInput {
    /// Only jobs in this state. Leave out for all.
    #[serde(default)]
    pub status: Option<JobStatus>,
    /// Only jobs of this kind, for example `media.variants` (letters, digits, `.`, `_`
    /// and `-`, at most 64 characters).
    #[serde(default)]
    pub kind: Option<String>,
    /// Jobs per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same query, to get the next page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// The input of `audit_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditInput {
    /// Only rows written by this user (their id).
    #[serde(default)]
    pub actor: Option<String>,
    /// An exact action such as `settings.update`, or a prefix ending in `.` such as
    /// `mcp.` (everything assistants did) or `user.`. Case-sensitive, at most 100
    /// characters.
    #[serde(default)]
    pub action: Option<String>,
    /// Only rows about this kind of target (`content`, `settings`, `plugin`, `theme`,
    /// ...), at most 100 characters.
    #[serde(default)]
    pub target_kind: Option<String>,
    /// Only rows about this target (needs `target_kind`), at most 255 characters.
    #[serde(default)]
    pub target_id: Option<String>,
    /// Rows per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same filters, to get the next page.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[tool_router(router = system_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Reports the health of the site: the FerriteCMS version, solo or cluster mode,
    /// database and media backends, whether the database schema matches the program,
    /// the active theme (and nodes that fell back to another), job counts by state
    /// (`dead` ones need attention), live and stale server nodes, and whether mail is
    /// configured. Needs an administrator.
    #[tool(annotations(
        title = "System status",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn system_status(&self, caller: Caller) -> CallToolResult {
        match status(&self.api, caller.actor()).await {
            Ok(s) => {
                let summary = format!(
                    "FerriteCMS {} ({}, {}): {} queued, {} dead jobs; {} live nodes.",
                    s.version, s.mode, s.database, s.jobs.queued, s.jobs.dead, s.nodes.live
                );
                results::ok(s, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Lists background jobs (media variants, mail, imports, backups...) newest first:
    /// kind, state, attempts, when due, the last error (scrubbed of addresses and
    /// paths) and whether a live node handles that kind. Filter by `status` (`dead` for
    /// failures) or `kind`. Jobs cannot be retried through this connection; point the user
    /// to System, Jobs in the admin. Needs an administrator.
    #[tool(annotations(title = "List jobs", read_only_hint = true, open_world_hint = false))]
    pub async fn jobs_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<JobsInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n.min(MAX_PAGE),
            Err(e) => return e,
        };
        let params = JobListParams {
            status: input.status.map(|s| s.as_str().to_owned()),
            kind: input.kind,
            limit: None,
            cursor: input.cursor,
        };
        match job_page(&self.api, caller.actor(), params, limit).await {
            Ok(page) => {
                let n = page.items.len();
                let more = if page.next_cursor.is_some() {
                    "; more with next_cursor"
                } else {
                    ""
                };
                let summary = format!("{n} job{}{more}.", if n == 1 { "" } else { "s" });
                results::ok(page, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Reads the audit log newest first: when, who (login), the action (`<area>.<verb>`,
    /// and `mcp.<tool>` for what AI assistants did, with the client's name in `details`),
    /// the target and what changed (never secrets or setting values). Filter by `actor`,
    /// `action` (a prefix ending in `.` works), `target_kind` and `target_id`. Needs an
    /// administrator.
    #[tool(annotations(title = "Audit log", read_only_hint = true, open_world_hint = false))]
    pub async fn audit_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<AuditInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n.min(MAX_PAGE),
            Err(e) => return e,
        };
        let params = AuditListParams {
            actor: input.actor,
            action: input.action,
            target_kind: input.target_kind,
            target_id: input.target_id,
            limit: Some(limit),
            cursor: input.cursor,
        };
        match audit_page(&self.api, caller.actor(), params).await {
            Ok(page) => {
                let n = page.items.len();
                let more = if page.next_cursor.is_some() {
                    "; more with next_cursor"
                } else {
                    ""
                };
                let summary = format!("{n} audit row{}{more}.", if n == 1 { "" } else { "s" });
                results::ok(page, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }
}
