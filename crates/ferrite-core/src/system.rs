//! The system API's reads (M2a-2b P15, P18; 07 §1): the node registry, the audit log,
//! the migration state and job counts. Every read needs `manage_system` (admins) and
//! the `system:read` scope ([`require_reader`]).
//!
//! Node error texts in `nodes.meta` (`theme.error`, `plugins.<name>.error`,
//! `plugins_error`, `permalinks_error`) are scrubbed again here
//! ([`diagnostics::scrub`]), so a row written by an older node that did not scrub them
//! (a rolling deploy) shows no paths, endpoints or credentials either.

use std::{collections::BTreeMap, sync::Arc};

use ferrite_domain::{AuditEntry, JobStatus, Node, Page};
use ferrite_store::{MigrationStatus, Store};
use serde_json::Value;

use crate::{Actor, Capability, CoreError, diagnostics, require_reader};

/// Reads behind `GET /system/*`.
pub struct SystemService {
    store: Arc<dyn Store>,
}

impl SystemService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Every registered node (stale ones included, at most
    /// [`MAX_NODES`](ferrite_store::MAX_NODES)), with its error texts scrubbed.
    pub async fn nodes(&self, actor: &Actor) -> Result<Vec<Node>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        let mut nodes = self.store.read().nodes().list().await?;
        for node in &mut nodes {
            scrub_meta(&mut node.meta);
        }
        Ok(nodes)
    }

    /// One page of the audit log, newest first (`limit` 1..=200; a bad cursor is
    /// `Invalid`).
    pub async fn audit(
        &self,
        actor: &Actor,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<AuditEntry>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        Ok(self.store.read().audit().list(limit, cursor).await?)
    }

    /// The database's migration state relative to `binary`.
    pub async fn migration_status(
        &self,
        actor: &Actor,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        Ok(self.store.migration_status(binary).await?)
    }

    /// The number of jobs per status (every status present).
    pub async fn job_counts(&self, actor: &Actor) -> Result<BTreeMap<JobStatus, u64>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        Ok(self.store.read().jobs().counts().await?)
    }

    /// The name of the active theme row, if one is active (none: nodes serve the
    /// embedded default).
    pub async fn active_theme(&self, actor: &Actor) -> Result<Option<String>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        Ok(self.store.read().themes().active().await?.map(|t| t.name))
    }
}

/// Scrubs the error texts of one node's `meta` in place.
fn scrub_meta(meta: &mut Value) {
    if let Some(theme) = meta.get_mut("theme") {
        scrub_member(theme, "error");
    }
    if let Some(Value::Object(plugins)) = meta.get_mut("plugins") {
        for entry in plugins.values_mut() {
            scrub_member(entry, "error");
        }
    }
    scrub_member(meta, "plugins_error");
    scrub_member(meta, "permalinks_error");
}

/// `value[key]` scrubbed when it is a string; anything else but `null` becomes `null`
/// (an error text is a string; an older node cannot smuggle text in another shape).
fn scrub_member(value: &mut Value, key: &str) {
    let Some(member) = value.get_mut(key) else {
        return;
    };
    *member = match member {
        Value::String(text) => Value::String(diagnostics::scrub(text)),
        _ => Value::Null,
    };
}
