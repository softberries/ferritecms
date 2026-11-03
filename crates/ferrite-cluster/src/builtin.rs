//! Built-in maintenance jobs (P11, P14, P20, P23): hourly pruning of the events outbox,
//! finished jobs, stale node rows, old plugin failure buckets and ended sessions, expired
//! API tokens, past rate-limit windows, old idempotency keys and per-node minute metrics
//! older than 7 days (M4-2 P35). Ages are measured on the database clock; rows go in
//! batches of [`MAX_PRUNE_BATCH`] (R5) so no single statement holds locks for long.

use std::{future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_domain::{IDEMPOTENCY_RETENTION, Job, NODE_METRICS_RETENTION, SESSION_IDLE, Timestamp};
use ferrite_store::{MAX_PRUNE_BATCH, Store, StoreError};
use futures::future::BoxFuture;
use serde_json::json;
use tokio::time::Instant;

use crate::jobs::{JobError, JobHandler, Periodic};

/// How often each prune job runs.
pub const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Events older than this are deleted.
pub const EVENT_RETENTION: Duration = Duration::from_secs(24 * 3600);
/// `done` jobs finished longer ago than this are deleted.
pub const DONE_JOB_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
/// `dead` jobs finished longer ago than this are deleted.
pub const DEAD_JOB_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

/// The shortest time plugin failure buckets are kept.
pub const MIN_FAILURE_RETENTION: Duration = Duration::from_secs(24 * 3600);

/// How long plugin failure buckets are kept for a failure window of `window`
/// (`plugins.failure_window`): `max(24 h, 2 × window)`, so the auto-disable sum never
/// misses a bucket and a day of history stays for operators.
pub fn failure_retention(window: Duration) -> Duration {
    MIN_FAILURE_RETENTION.max(window.saturating_mul(2))
}

/// Rate-limit rows whose window started longer ago than this are deleted. No limit has a
/// window longer than this (most are a minute or ten; the hourly password-reset and
/// client-registration limits and plugin routes' `N/h` are an hour), so a current window
/// is never cut short.
pub const RATE_LIMIT_RETENTION: Duration = Duration::from_secs(3600);

/// How long one prune run keeps deleting batches; well inside the default handler
/// timeout (300 s), so a large backlog ends the run cleanly and the next hour goes on.
pub const PRUNE_BUDGET: Duration = Duration::from_secs(240);

/// The share of [`PRUNE_BUDGET`] each of the five tables of `sessions.prune` gets, so a
/// backlog in one never starves the others.
pub const SESSIONS_PRUNE_SLICE: Duration = Duration::from_secs(48);

/// Calls `batch` until it deletes fewer than [`MAX_PRUNE_BATCH`] rows or `budget` (from
/// the first call) is used up; returns the total. A call always runs at least one batch.
async fn prune_batches<F, Fut>(
    what: &'static str,
    budget: Duration,
    mut batch: F,
) -> Result<u64, JobError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<u64, StoreError>>,
{
    let started = Instant::now();
    let mut total = 0;
    loop {
        let n = batch().await.map_err(retry)?;
        total += n;
        if n < u64::from(MAX_PRUNE_BATCH) {
            break;
        }
        if started.elapsed() >= budget {
            tracing::info!(
                what,
                deleted = total,
                "prune budget used; the rest goes next run"
            );
            break;
        }
    }
    tracing::info!(what, deleted = total, "pruned");
    Ok(total)
}

/// One table's batch for [`prune_each`].
type Batch<'a> = Box<dyn FnMut() -> BoxFuture<'a, Result<u64, StoreError>> + Send + 'a>;

/// Prunes each table in turn, each within its own `slice`, so a table whose backlog
/// uses up its slice still leaves every later table a full one.
async fn prune_each(
    slice: Duration,
    tables: Vec<(&'static str, Batch<'_>)>,
) -> Result<(), JobError> {
    for (what, mut batch) in tables {
        prune_batches(what, slice, &mut batch).await?;
    }
    Ok(())
}

/// The database clock minus `age`.
async fn db_now_minus(store: &dyn Store, age: Duration) -> Result<Timestamp, JobError> {
    let now = store.read().nodes().db_now().await.map_err(retry)?;
    minus(now, age)
}

/// `t` minus `age`.
fn minus(t: Timestamp, age: Duration) -> Result<Timestamp, JobError> {
    let age = i64::try_from(age.as_micros()).unwrap_or(i64::MAX);
    Timestamp::from_micros(t.as_micros().saturating_sub(age))
        .map_err(|e| JobError::Permanent(e.to_string()))
}

fn retry(e: StoreError) -> JobError {
    JobError::Retry(e.to_string())
}

/// `events.prune`: deletes events older than [`EVENT_RETENTION`].
pub struct EventsPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for EventsPrune {
    fn kind(&self) -> &'static str {
        "events.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.0.as_ref(), EVENT_RETENTION).await?;
        let events = self.0.read().events();
        prune_batches("events", PRUNE_BUDGET, || {
            events.prune(before, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// `jobs.prune`: deletes `done` jobs after [`DONE_JOB_RETENTION`] and `dead` ones after
/// [`DEAD_JOB_RETENTION`].
pub struct JobsPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for JobsPrune {
    fn kind(&self) -> &'static str {
        "jobs.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let done = db_now_minus(self.0.as_ref(), DONE_JOB_RETENTION).await?;
        let dead = db_now_minus(self.0.as_ref(), DEAD_JOB_RETENTION).await?;
        let jobs = self.0.read().jobs();
        prune_batches("jobs", PRUNE_BUDGET, || {
            jobs.prune(done, dead, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// `nodes.prune`: deletes node rows whose heartbeat is older than [`crate::node::NODE_EXPIRY`].
pub struct NodesPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for NodesPrune {
    fn kind(&self) -> &'static str {
        "nodes.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.0.as_ref(), crate::node::NODE_EXPIRY).await?;
        let nodes = self.0.read().nodes();
        prune_batches("nodes", PRUNE_BUDGET, || {
            nodes.prune(before, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// `plugin_failures.prune`: deletes failure buckets older than its retention (see
/// [`failure_retention`]).
pub struct PluginFailuresPrune {
    pub store: Arc<dyn Store>,
    pub retention: Duration,
}

#[async_trait]
impl JobHandler for PluginFailuresPrune {
    fn kind(&self) -> &'static str {
        "plugin_failures.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.store.as_ref(), self.retention).await?;
        let failures = self.store.read().plugin_failures();
        prune_batches("plugin_failures", PRUNE_BUDGET, || {
            failures.prune(before, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// `sessions.prune` (P23): deletes sessions past their absolute end or idle longer than
/// [`SESSION_IDLE`], API tokens past their `expires_at` (OAuth access tokens included),
/// expired OAuth requests, codes and grants (design 12 §4; a grant takes its access tokens
/// with it), rate-limit rows whose window started more than [`RATE_LIMIT_RETENTION`] ago,
/// and `Idempotency-Key` rows (finished or pending) created more than
/// [`IDEMPOTENCY_RETENTION`] ago (M2a-2b P10) — all against one reading of the database
/// clock, each table within [`SESSIONS_PRUNE_SLICE`] (five slices fit in
/// [`PRUNE_BUDGET`]).
pub struct SessionsPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for SessionsPrune {
    fn kind(&self) -> &'static str {
        "sessions.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let repos = self.0.read();
        let now = repos.nodes().db_now().await.map_err(retry)?;
        let idle_before = minus(now, SESSION_IDLE)?;
        let window_before = minus(now, RATE_LIMIT_RETENTION)?;
        let keys_before = minus(now, IDEMPOTENCY_RETENTION)?;
        let (sessions, tokens, oauth, limits, keys) = (
            repos.sessions(),
            repos.api_tokens(),
            repos.oauth(),
            repos.rate_limits(),
            repos.idempotency(),
        );
        let tables: Vec<(&'static str, Batch<'_>)> = vec![
            (
                "sessions",
                Box::new(move || sessions.prune(now, idle_before, MAX_PRUNE_BATCH)),
            ),
            (
                "api_tokens",
                Box::new(move || tokens.prune(now, MAX_PRUNE_BATCH)),
            ),
            (
                "oauth",
                // Three tables per batch: the largest count decides whether to go on.
                Box::new(move || {
                    Box::pin(async move {
                        let p = oauth.prune_expired(now, MAX_PRUNE_BATCH).await?;
                        Ok(p.requests.max(p.codes).max(p.grants))
                    })
                }),
            ),
            (
                "rate_limits",
                Box::new(move || limits.prune(window_before, MAX_PRUNE_BATCH)),
            ),
            (
                "idempotency_keys",
                Box::new(move || keys.prune(keys_before, MAX_PRUNE_BATCH)),
            ),
        ];
        prune_each(SESSIONS_PRUNE_SLICE, tables).await
    }
}

/// `node_metrics.prune` (M4-2 P35): deletes per-node minute rows older than
/// [`NODE_METRICS_RETENTION`] (7 days).
pub struct NodeMetricsPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for NodeMetricsPrune {
    fn kind(&self) -> &'static str {
        "node_metrics.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.0.as_ref(), NODE_METRICS_RETENTION).await?;
        let metrics = self.0.read().node_metrics();
        prune_batches("node_metrics", PRUNE_BUDGET, || {
            metrics.prune(before, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// `change_sets.prune` (M6-5, design 17 §12): deletes AI change sets whose last change is
/// older than the site option `mcp.change_sets_days` (1..=365; unset or unreadable: 90),
/// with their changes.
pub struct ChangeSetsPrune(pub Arc<dyn Store>);

/// The retention of change sets when the option is unset or unreadable.
pub const CHANGE_SETS_DAYS_DEFAULT: u64 = 90;

/// The days `value` names: an integer in 1..=365, else the default.
pub fn change_sets_days(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(serde_json::Value::as_u64)
        .filter(|d| (1..=365).contains(d))
        .unwrap_or(CHANGE_SETS_DAYS_DEFAULT)
}

#[async_trait]
impl JobHandler for ChangeSetsPrune {
    fn kind(&self) -> &'static str {
        "change_sets.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let stored = match self
            .0
            .read()
            .options()
            .get(ferrite_domain::CHANGE_SETS_DAYS_OPTION)
            .await
        {
            Ok(entry) => entry.map(|e| e.value),
            Err(e) => {
                tracing::warn!(error = %e, "change set retention unreadable; using the default");
                None
            }
        };
        let days = change_sets_days(stored.as_ref());
        let before = db_now_minus(self.0.as_ref(), Duration::from_secs(days * 24 * 3600)).await?;
        let sets = self.0.read().change_sets();
        prune_batches("change_sets", PRUNE_BUDGET, || {
            sets.prune(before, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// The built-in handlers, on `store`; plugin failure buckets are kept for
/// `failure_retention` (from [`failure_retention`]).
pub fn handlers(store: Arc<dyn Store>, failure_retention: Duration) -> Vec<Arc<dyn JobHandler>> {
    vec![
        Arc::new(EventsPrune(store.clone())),
        Arc::new(JobsPrune(store.clone())),
        Arc::new(NodesPrune(store.clone())),
        Arc::new(PluginFailuresPrune {
            store: store.clone(),
            retention: failure_retention,
        }),
        Arc::new(SessionsPrune(store.clone())),
        Arc::new(NodeMetricsPrune(store.clone())),
        Arc::new(ChangeSetsPrune(store)),
    ]
}

/// The built-in periodic jobs: every handler in [`handlers`], each [`PRUNE_EVERY`].
pub fn periodics() -> Vec<Periodic> {
    [
        "events.prune",
        "jobs.prune",
        "nodes.prune",
        "plugin_failures.prune",
        "sessions.prune",
        "node_metrics.prune",
        "change_sets.prune",
    ]
    .into_iter()
    .map(|kind| Periodic {
        kind,
        every: PRUNE_EVERY,
        payload: json!({"v": 1}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
        offset: Duration::ZERO,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// M4-4 P75: every built-in periodic's offset is below its period.
    #[test]
    fn builtin_offsets_fit_their_periods() {
        for p in periodics() {
            assert!(p.offset_fits(), "{p:?}");
        }
    }

    #[test]
    fn change_sets_days_default_and_range() {
        use serde_json::json;
        assert_eq!(change_sets_days(None), 90);
        assert_eq!(change_sets_days(Some(&json!(30))), 30);
        for bad in [json!(0), json!(366), json!("30"), json!(-1), json!(1.5)] {
            assert_eq!(change_sets_days(Some(&bad)), 90, "{bad}");
        }
    }

    #[test]
    fn failure_retention_is_at_least_a_day() {
        let min = Duration::from_secs(60);
        assert_eq!(failure_retention(10 * min), MIN_FAILURE_RETENTION);
        assert_eq!(failure_retention(Duration::ZERO), MIN_FAILURE_RETENTION);
        assert_eq!(
            failure_retention(Duration::from_secs(20 * 3600)),
            Duration::from_secs(40 * 3600)
        );
        assert_eq!(failure_retention(Duration::MAX), Duration::MAX);
    }

    /// A table that always returns a full batch (an endless backlog) and counts calls.
    fn table(calls: &Arc<AtomicU32>, n: u32) -> Batch<'static> {
        let calls = calls.clone();
        Box::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(u64::from(n)) })
        })
    }

    #[tokio::test]
    async fn an_exhausted_slice_leaves_later_tables_their_pass() {
        let counts: Vec<Arc<AtomicU32>> = (0..3).map(|_| Arc::default()).collect();
        // The first table never runs short: with a zero slice it gets one batch, and
        // the others still each get theirs.
        let tables = vec![
            ("endless", table(&counts[0], MAX_PRUNE_BATCH)),
            ("second", table(&counts[1], 0)),
            ("third", table(&counts[2], 0)),
        ];
        prune_each(Duration::ZERO, tables).await.unwrap();
        let calls: Vec<u32> = counts.iter().map(|c| c.load(Ordering::SeqCst)).collect();
        assert_eq!(calls, [1, 1, 1]);

        // A full slice in an earlier table does not shorten a later one: the later
        // table keeps deleting until its own slice is spent.
        let counts: Vec<Arc<AtomicU32>> = (0..2).map(|_| Arc::default()).collect();
        let slow = counts[0].clone();
        let first: Batch<'static> = Box::new(move || {
            slow.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(120)).await;
                Ok(u64::from(MAX_PRUNE_BATCH))
            })
        });
        let later = counts[1].clone();
        let second: Batch<'static> = Box::new(move || {
            let n = later.fetch_add(1, Ordering::SeqCst);
            // Three full batches, then short.
            Box::pin(async move { Ok(if n < 3 { u64::from(MAX_PRUNE_BATCH) } else { 0 }) })
        });
        prune_each(
            Duration::from_millis(200),
            vec![("slow", first), ("later", second)],
        )
        .await
        .unwrap();
        // 120 ms batches in a 200 ms slice: one or two batches, never three.
        let slow = counts[0].load(Ordering::SeqCst);
        assert!(matches!(slow, 1 | 2), "{slow} batches");
        assert_eq!(
            counts[1].load(Ordering::SeqCst),
            4,
            "the later table ran to the end"
        );
    }

    #[test]
    fn every_handler_has_a_periodic() {
        let periodic: Vec<&str> = periodics().iter().map(|p| p.kind).collect();
        let expected: Vec<&str> = [
            "events.prune",
            "jobs.prune",
            "nodes.prune",
            "plugin_failures.prune",
            "sessions.prune",
            "node_metrics.prune",
            "change_sets.prune",
        ]
        .into();
        assert_eq!(periodic, expected);
        assert!(periodics().iter().all(|p| p.every == PRUNE_EVERY));
    }
}
