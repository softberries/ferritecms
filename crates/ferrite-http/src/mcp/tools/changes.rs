//! `changes_list` (M6-5, design 17 §10): the calling assistant's own change sets, so it
//! can tell its user what it changed and where to review or undo it. Read-only: there
//! is no revert tool (D14); undoing is a person's decision in the admin.
//!
//! It lists only this connection's sets (the grant or token the request came with),
//! never images, and needs no scope beyond the token itself.

use chrono::{DateTime, Utc};
use ferrite_core::changes::is_open;
use ferrite_domain::{ChangeSetId, Timestamp};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mcp::{Caller, FerriteMcp, results};

/// The input of `changes_list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesInput {
    /// How many of your most recent change sets, 1 to 20 (default 5).
    #[serde(default)]
    #[schemars(range(min = 1, max = 20))]
    pub limit: Option<u32>,
    /// One change set by id instead.
    #[serde(default)]
    pub set: Option<Uuid>,
}

/// One change of a set.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ChangeOut {
    pub seq: u32,
    pub at: DateTime<Utc>,
    /// What was done, e.g. `content.update`, `term.delete`.
    pub op: String,
    /// The object's kind: `content`, `media`, `term`, `menu`, `type`, `redirect`,
    /// `comment`, `option`, `theme` or `plugin`.
    pub kind: String,
    pub id: String,
    /// Its title or name when it was written.
    pub label: String,
    /// A person reverted this change.
    pub reverted: bool,
}

/// One change set.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ChangeSetOut {
    pub id: Uuid,
    /// Your next write still lands in this set.
    pub open: bool,
    pub started_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    pub changes: u32,
    pub reverted: u32,
    /// Where the user reviews and undoes this set.
    pub review_url: String,
    /// Newest first.
    pub items: Vec<ChangeOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChangesOut {
    /// Your change sets, the open one first, then newest first.
    pub sets: Vec<ChangeSetOut>,
}

#[tool_router(router = change_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists what you changed on this site through this connection: your recent change
    /// sets (writes close together in time, a new set after 30 minutes without one)
    /// with each change's operation, object and label, and a review_url where the user
    /// can review the diffs and undo a whole set or one change. Use it to tell the user
    /// what you did. You cannot undo through this connection: point the user to the
    /// review_url, or make a new edit (which is recorded too).
    #[tool(annotations(
        title = "List my changes",
        read_only_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn changes_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ChangesInput>,
    ) -> CallToolResult {
        let limit = input.limit.unwrap_or(5);
        if !(1..=20).contains(&limit) {
            return results::invalid(Some("limit"), "limit must be between 1 and 20");
        }
        let sets = match self
            .api
            .change_sets
            .agent_sets(caller.actor(), input.set.map(ChangeSetId::from_uuid), limit)
            .await
        {
            Ok(s) => s,
            Err(e) => return results::from_core(e),
        };
        let now = Timestamp::now();
        let mut out: Vec<ChangeSetOut> = sets
            .into_iter()
            .map(|d| ChangeSetOut {
                id: d.set.id.as_uuid(),
                open: is_open(&d.set, now),
                started_at: d.set.started_at.as_datetime(),
                last_at: d.set.last_at.as_datetime(),
                changes: d.set.changes,
                reverted: d.set.reverted_changes,
                review_url: format!("{}/admin/ai-activity/{}", self.api.site_url, d.set.id),
                items: d
                    .changes
                    .into_iter()
                    .map(|c| ChangeOut {
                        seq: c.seq,
                        at: c.at.as_datetime(),
                        op: c.op,
                        kind: c.object_kind,
                        id: c.object_id,
                        label: c.label,
                        reverted: c.reverted_at.is_some(),
                    })
                    .collect(),
            })
            .collect();
        // The open set first; the rest stay newest first.
        out.sort_by_key(|s| !s.open);
        let n: u32 = out.iter().map(|s| s.changes).sum();
        let summary = match out.len() {
            0 => "You have no recorded changes through this connection.".to_owned(),
            sets => format!(
                "{sets} change set(s) with {n} change(s); the user reviews and undoes them at the review_url."
            ),
        };
        results::ok(ChangesOut { sets: out }, summary)
    }
}
