//! The MCP tools, one file per area (design 12 §5.2, §6). Each file is a
//! `#[tool_router(router = <area>_tools, vis = "pub(crate)")] impl FerriteMcp { … }`
//! block whose router [`FerriteMcp::routers`](super::FerriteMcp::routers) adds to the
//! others.
//!
//! A tool takes `caller: Caller` (and `Parameters<Input>` for its input), returns a
//! `CallToolResult` built with [`results`](super::results), and calls core with
//! `caller.actor()`: no tool checks a permission itself. The per-user call limit is
//! counted before the tool runs (`call_tool`). A successful write ends with
//! [`FerriteMcp::audit`]; reads are not audited.

pub mod site;

use ferrite_core::CoreError;
use serde_json::Value;

use super::{Caller, FerriteMcp};

/// The default and the largest `per_page` of a listing tool (design 12 §5.1).
pub const PER_PAGE: u32 = 20;
/// See [`PER_PAGE`].
pub const MAX_PER_PAGE: u32 = 100;

impl FerriteMcp {
    /// The audit row of a successful write by tool `tool` (design 12 §5.1): action
    /// `mcp.<tool>`, the target, `details` plus `client`. A failure to write it is
    /// returned: the change is made, but say so rather than claim an audited write.
    pub async fn audit(
        &self,
        caller: &Caller,
        tool: &str,
        target_kind: &str,
        target_id: Option<String>,
        details: Value,
    ) -> Result<(), CoreError> {
        self.api
            .system
            .audit_mcp(
                caller.actor(),
                tool,
                (target_kind, target_id),
                &caller.client,
                details,
            )
            .await
    }
}

/// `per_page` checked: absent is [`PER_PAGE`], else 1..=[`MAX_PER_PAGE`].
pub fn per_page(per_page: Option<u32>) -> Result<u32, rmcp::model::CallToolResult> {
    match per_page {
        None => Ok(PER_PAGE),
        Some(n) if (1..=MAX_PER_PAGE).contains(&n) => Ok(n),
        Some(_) => Err(super::results::invalid(
            Some("per_page"),
            format!("per_page must be 1..={MAX_PER_PAGE}"),
        )),
    }
}
