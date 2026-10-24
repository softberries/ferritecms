//! The system API (M2a-2b P15, P18, P19; 07 §1): the node registry, the audit log, the
//! migration state, job counts, the job list and the dead-job retry. Every read needs
//! `manage_system` (admins) and the `system:read` scope ([`require_reader`]); the retry
//! needs `system:write` ([`require`]) and is audited (`job.retry`, P14).
//!
//! Job `last_error` texts are scrubbed here ([`diagnostics::scrub`], F16): the store
//! only strips NUL and cuts them to 1 KiB, so a handler's error could carry a storage
//! endpoint or a local path. Job payloads stay in core's callers' hands: the API never
//! returns them (P19).
//!
//! Node error texts in `nodes.meta` (`theme.error`, `plugins.<name>.error`,
//! `plugins_error`, `permalinks_error`) are scrubbed again here
//! ([`diagnostics::scrub`]), so a row written by an older node that did not scrub them
//! (a rolling deploy) shows no paths, endpoints or credentials either.

use std::{collections::BTreeMap, sync::Arc};

use ferrite_domain::{AuditEntry, Job, JobId, JobQuery, JobStatus, NewAudit, Node, Page};
use ferrite_store::{MigrationStatus, Repos, Store};
use serde_json::{Value, json};

use crate::{Actor, Capability, CoreError, diagnostics, require, require_reader, tx::finish};

/// Why a retry of a job that is not `dead` is refused (`422`, field `status`).
pub const RETRY_NOT_DEAD: &str = "only dead jobs can be retried";

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

    /// One page of jobs, newest first, filtered by `query.status` and `query.kind`,
    /// with `last_error` scrubbed. Invalid: `limit` outside 1..=200, a malformed kind
    /// or cursor.
    pub async fn jobs(&self, actor: &Actor, query: JobQuery) -> Result<Page<Job>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        let mut page = self.store.read().jobs().list(&query).await?;
        for job in &mut page.items {
            scrub_job(job);
        }
        Ok(page)
    }

    /// Puts dead job `id` back in the queue (due now, attempts reset, `last_error`
    /// kept; [`JobRepo::retry_dead`](ferrite_store::JobRepo::retry_dead)) and audits
    /// `job.retry` in the same transaction. Missing: `NotFound`; not `dead`:
    /// `invalid_field("status", RETRY_NOT_DEAD)`. A job whose kind no node handles is
    /// retried too: it waits in the queue for a node that does (the job list's
    /// `handled` says whether any live node does).
    pub async fn retry_job(&self, actor: &Actor, id: JobId) -> Result<Job, CoreError> {
        require(actor, Capability::ManageSystem)?;
        let tx = self.store.begin().await?;
        let done = retry_in(tx.as_ref(), actor, id).await;
        let mut job = finish(tx, done).await?;
        scrub_job(&mut job);
        Ok(job)
    }

    /// The name of the active theme row, if one is active (none: nodes serve the
    /// embedded default).
    pub async fn active_theme(&self, actor: &Actor) -> Result<Option<String>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        Ok(self.store.read().themes().active().await?.map(|t| t.name))
    }
}

async fn retry_in(repos: &dyn Repos, actor: &Actor, id: JobId) -> Result<Job, CoreError> {
    let Some(job) = repos.jobs().retry_dead(id).await? else {
        return match repos.jobs().get(id).await? {
            None => Err(CoreError::NotFound),
            Some(_) => Err(CoreError::invalid_field("status", RETRY_NOT_DEAD)),
        };
    };
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor.user),
            action: "job.retry".to_owned(),
            target_kind: "job".to_owned(),
            target_id: Some(id.to_string()),
            details: json!({ "id": id.to_string(), "kind": job.kind }),
        })
        .await?;
    Ok(job)
}

/// `job.last_error` scrubbed in place.
fn scrub_job(job: &mut Job) {
    if let Some(error) = &mut job.last_error {
        *error = diagnostics::scrub(error);
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
