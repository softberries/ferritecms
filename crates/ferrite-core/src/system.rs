//! The system API (M2a-2b P15, P18, P19; 07 §1): the node registry, the audit log
//! (with filters, M2b-3), the migration state, job counts, the job list, the dead-job
//! retry, the test mail (P49) and the metrics series (M4-2 P35). Every read needs `manage_system` (admins) and the
//! `system:read` scope ([`require_reader`]); the retry and the test mail need
//! `system:write` ([`require`]) and are audited (`job.retry`, `mail.test`).
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

use ferrite_domain::{
    AuditEntry, AuditQuery, Job, JobId, JobQuery, JobStatus, MetricCounts, NewAudit, Node, NodeId,
    Page, SeriesBucket, SeriesStep, Timestamp,
};
use ferrite_store::{MigrationStatus, Repos, Store};
use serde_json::{Value, json};

use crate::{
    Actor, Capability, CoreError,
    auth::limits,
    diagnostics,
    mail::{MailQueue, Mailer, check_address, test_mail},
    require, require_reader, settings,
    tx::finish,
};

/// Why a retry of a job that is not `dead` is refused (`422`, field `status`).
pub const RETRY_NOT_DEAD: &str = "only dead jobs can be retried";

/// Why a test mail cannot be sent without `[mail]` (`422`, field `mail`).
pub const MAIL_NOT_CONFIGURED: &str =
    "mail is not configured: set [mail] smtp_url and from in ferrite.toml";

/// A dense metrics series ([`SystemService::metrics_series`]): the range widened to
/// whole buckets and every bucket in it, ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSeries {
    pub from: Timestamp,
    pub to: Timestamp,
    pub step: SeriesStep,
    /// The node filter, if one was given.
    pub node: Option<NodeId>,
    pub buckets: Vec<SeriesBucket>,
}

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

    /// One page of the audit log, newest first, filtered by `query` (see
    /// [`AuditQuery`]). `target_id` without `target_kind` is
    /// `invalid_field("target_id")`; a bad limit or cursor is `Invalid`.
    pub async fn audit(
        &self,
        actor: &Actor,
        query: &AuditQuery,
    ) -> Result<Page<AuditEntry>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        if query.target_id.is_some() && query.target_kind.is_none() {
            return Err(CoreError::invalid_field(
                "target_id",
                "target_id needs target_kind",
            ));
        }
        Ok(self.store.read().audit().list(query).await?)
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

    /// The per-node minute metrics of `[from, to)` summed over nodes (or of `node`
    /// only) into `step` buckets, dense: every bucket of the range is there, zeros where
    /// no row is (M4-2 P35). The range is widened to whole buckets (aligned to the Unix
    /// epoch, UTC). A bucket's page latency histogram is the sum of its minutes' and
    /// nodes', so its p95 is that of all its page responses (M4-2 R7).
    /// Invalid: `from >= to` (`invalid_field("to")`); a span over
    /// [`SeriesStep::max_span`] (`invalid_field("step")`: minutes cover at most 24 h,
    /// hours and days 7 days).
    pub async fn metrics_series(
        &self,
        actor: &Actor,
        from: Timestamp,
        to: Timestamp,
        step: SeriesStep,
        node: Option<NodeId>,
    ) -> Result<MetricsSeries, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        if from >= to {
            return Err(CoreError::invalid_field("to", "to must be after from"));
        }
        let span = to.as_micros().saturating_sub(from.as_micros());
        let max = i64::try_from(step.max_span().as_micros()).unwrap_or(i64::MAX);
        if span > max {
            let limit = match step {
                SeriesStep::Minute => "24 hours",
                SeriesStep::Hour | SeriesStep::Day => "7 days",
            };
            return Err(CoreError::invalid_field(
                "step",
                format!("a {step} series spans at most {limit}"),
            ));
        }
        let width = step.micros();
        let start = step.floor(from.as_micros());
        // `to` rounded up to a bucket boundary.
        let end = step
            .floor(to.as_micros().saturating_sub(1))
            .saturating_add(width);
        let bounds = |m: i64| {
            Timestamp::from_micros(m)
                .map_err(|_| CoreError::invalid_field("to", "the range is out of bounds"))
        };
        let (start_at, end_at) = (bounds(start)?, bounds(end)?);
        let rows = self
            .store
            .read()
            .node_metrics()
            .series(start_at, end_at, step, node)
            .await?;
        let by_start: BTreeMap<i64, MetricCounts> = rows
            .into_iter()
            .map(|b| (b.start.as_micros(), b.counts))
            .collect();
        let mut buckets = Vec::new();
        let mut at = start;
        while at < end {
            buckets.push(SeriesBucket {
                start: bounds(at)?,
                counts: by_start.get(&at).copied().unwrap_or_default(),
            });
            at += width;
        }
        Ok(MetricsSeries {
            from: start_at,
            to: end_at,
            step,
            node,
            buckets,
        })
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

    /// Sends one test mail to the actor's own address through `queue` (P49) and returns
    /// that address. Needs `manage_system` with `system:write`. In order: no `[mail]`
    /// (`queue` = `None`) is `invalid_field("mail", MAIL_NOT_CONFIGURED)`; then one hit on
    /// `mail-test:user:<id>`, 3 per 10 minutes (`RateLimited`); an account without a
    /// usable address is `invalid_field("email")`; a mail failing its own checks is
    /// `Internal` (`500`); a queue that refuses (full, shutting down) is `Unavailable`
    /// (`503 busy`). Once queued it is audited `mail.test` (target the
    /// user, no address in `details`). The send itself runs in the background: an SMTP
    /// failure is logged by the queue, not returned.
    ///
    /// The mail names the site by its title at use time (the `site.title` option, else
    /// `configured_title`, P51), `site_url`, the actor's login and the UTC time.
    pub async fn send_test_mail(
        &self,
        actor: &Actor,
        queue: Option<&MailQueue>,
        site_url: &str,
        configured_title: &str,
    ) -> Result<String, CoreError> {
        require(actor, Capability::ManageSystem)?;
        let Some(queue) = queue else {
            return Err(CoreError::invalid_field("mail", MAIL_NOT_CONFIGURED));
        };
        let store = self.store.as_ref();
        limits::hit(
            store,
            &limits::mail_test_user(actor.user),
            limits::MAIL_TEST_PER_USER,
            limits::MAIL_TEST_WINDOW,
        )
        .await?;
        let user = store
            .read()
            .users()
            .get(actor.user)
            .await?
            .ok_or(CoreError::NotFound)?;
        if check_address(&user.email).is_err() {
            return Err(CoreError::invalid_field(
                "email",
                "your account has no usable e-mail address",
            ));
        }
        let title = settings::site_title_or_configured(store, configured_title).await;
        let mail = test_mail(&user.email, &title, site_url, &user.login, Timestamp::now());
        // Checked here so a refusal by the queue below can only mean it is full or
        // stopping: a mail failing its own checks is a bug (the address and title were
        // checked above), never worth a retry.
        if let Err(e) = mail.check() {
            tracing::error!(user = %user.id, error = %e, "test mail failed its checks");
            return Err(CoreError::Internal("test mail refused by the mail checks"));
        }
        if queue.send(mail).await.is_err() {
            return Err(CoreError::Unavailable(
                "the mail queue is full or shutting down",
            ));
        }
        store
            .read()
            .audit()
            .append(&NewAudit {
                actor_id: Some(actor.user),
                action: "mail.test".to_owned(),
                target_kind: "user".to_owned(),
                target_id: Some(user.id.to_string()),
                details: json!({}),
            })
            .await?;
        Ok(user.email)
    }

    /// The name of the active theme row, if one is active (none: nodes serve the
    /// embedded default).
    pub async fn active_theme(&self, actor: &Actor) -> Result<Option<String>, CoreError> {
        require_reader(actor, Capability::ManageSystem)?;
        Ok(self.store.read().themes().active().await?.map(|t| t.name))
    }

    /// Appends the audit row of a successful MCP write tool (design 12 §5.1): action
    /// `mcp.<tool>`, the actor as `actor_id`, `details` (an object; anything else is
    /// stored under `value`) with `client` set to the AI client's name. Any actor may
    /// record its own MCP calls, nothing else: the action is always under `mcp.`, and
    /// `tool` must be a tool name (`snake_case` ASCII, 1..=64 bytes), else `Invalid`.
    /// The write the tool made is already committed, so this is a separate statement.
    pub async fn audit_mcp(
        &self,
        actor: &Actor,
        tool: &str,
        target: (&str, Option<String>),
        client: &str,
        details: Value,
    ) -> Result<(), CoreError> {
        if !is_tool_name(tool) {
            return Err(CoreError::Invalid(format!("not a tool name: {tool:?}")));
        }
        let mut details = match details {
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            other => serde_json::Map::from_iter([("value".to_owned(), other)]),
        };
        details.insert("client".to_owned(), Value::String(client.to_owned()));
        let (target_kind, target_id) = target;
        self.store
            .read()
            .audit()
            .append(&NewAudit {
                actor_id: Some(actor.user),
                action: format!("mcp.{tool}"),
                target_kind: target_kind.to_owned(),
                target_id,
                details: Value::Object(details),
            })
            .await?;
        Ok(())
    }
}

/// Whether `name` is an MCP tool name: 1..=64 bytes of `a-z`, `0-9` and `_`, starting
/// with a letter.
fn is_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
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
