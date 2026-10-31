//! The per-node job runner (04 §6, rulings P5, P9, P10): claims due jobs of the
//! registered kinds, runs each on its own task, renews every in-flight lease and records
//! the outcome under the lease token, so a node that lost its lease writes nothing (P6).

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ferrite_domain::{Job, JobLease, NodeId};
use ferrite_store::{
    Channel, JobRepo, MAX_CLAIM, MAX_CLAIM_KINDS, Store, StoreError, check_job_kind, job_backoff,
};
use serde_json::Value;
use tokio::{
    sync::Notify,
    task::{AbortHandle, Id, JoinError, JoinHandle, JoinSet},
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span};

use super::{JobError, JobHandler, RESUBSCRIBE_MAX, RESUBSCRIBE_MIN};
use crate::ClusterError;

/// How long a claim holds a job before another node may take it over.
pub const LEASE: Duration = Duration::from_secs(60);
/// How often the runner renews the leases of its in-flight jobs.
pub const EXTEND_EVERY: Duration = Duration::from_secs(20);
/// How often an idle runner polls for due jobs (notices wake it sooner).
pub const IDLE_POLL: Duration = Duration::from_secs(1);
/// How long in-flight jobs may run on after shutdown starts (P10).
pub const JOB_DRAIN: Duration = Duration::from_secs(9);

/// Settings of one [`JobRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerConfig {
    /// The node that holds the leases.
    pub node: NodeId,
    /// Jobs run at once (`cluster.workers`); 0 counts as 1.
    pub workers: usize,
    /// Lease length per claim and per renewal.
    pub lease: Duration,
    /// Renewal interval. A job whose lease was not renewed for
    /// `lease - extend_every / 2` is cancelled locally (R4), so keep it under
    /// `2 / 3 × lease` (default 20 s of 60 s) or jobs are cancelled before any renewal.
    /// Half of it is also the client-side timeout of every store call the runner makes
    /// (the store-call timeout, 10 s by default).
    pub extend_every: Duration,
    /// Poll interval while idle.
    pub idle_poll: Duration,
    /// Time in-flight jobs get after the stop signal before they are released.
    pub drain: Duration,
    /// Subscribe to job notices (cluster mode, P1). Backends without notifications
    /// (SQLite) fall back to polling.
    pub listen: bool,
}

impl RunnerConfig {
    /// [`LEASE`], [`EXTEND_EVERY`], [`IDLE_POLL`], [`JOB_DRAIN`], no listening.
    pub fn new(node: NodeId, workers: usize) -> Self {
        Self {
            node,
            workers,
            lease: LEASE,
            extend_every: EXTEND_EVERY,
            idle_poll: IDLE_POLL,
            drain: JOB_DRAIN,
            listen: false,
        }
    }
}

/// What a runner finished since it started (M4-2 P35: the per-minute `node_metrics`
/// rows diff these): jobs it completed, and attempts that failed (a retry or a
/// permanent failure, recorded under the lease). A `Busy` release, a lost lease and an
/// outcome that could not be written are not counted.
#[derive(Debug, Default)]
pub struct JobCounters {
    done: AtomicU64,
    failed: AtomicU64,
}

impl JobCounters {
    /// Jobs completed.
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Attempts that failed.
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

/// Runs the jobs of its handlers' kinds on this node.
pub struct JobRunner {
    store: Arc<dyn Store>,
    config: RunnerConfig,
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
    /// The handler kinds, sorted: only these are claimed (P5).
    kinds: Vec<String>,
    counters: Arc<JobCounters>,
}

impl JobRunner {
    /// A runner for `handlers`. Two handlers of one kind are
    /// [`ClusterError::DuplicateHandler`]; an invalid kind, or more than
    /// [`MAX_CLAIM_KINDS`] kinds, is a [`StoreError::Invalid`].
    pub fn new(
        store: Arc<dyn Store>,
        config: RunnerConfig,
        handlers: Vec<Arc<dyn JobHandler>>,
    ) -> Result<Self, ClusterError> {
        let mut map: HashMap<&'static str, Arc<dyn JobHandler>> = HashMap::new();
        for handler in handlers {
            let kind = handler.kind();
            check_job_kind(kind)?;
            if map.insert(kind, handler).is_some() {
                return Err(ClusterError::DuplicateHandler(kind.to_owned()));
            }
        }
        if map.len() > MAX_CLAIM_KINDS {
            return Err(StoreError::Invalid(format!(
                "{} job kinds; a runner handles at most {MAX_CLAIM_KINDS}",
                map.len()
            ))
            .into());
        }
        let mut kinds: Vec<String> = map.keys().map(|k| (*k).to_owned()).collect();
        kinds.sort();
        Ok(Self {
            store,
            config,
            handlers: map,
            kinds,
            counters: Arc::default(),
        })
    }

    /// Counts finished jobs into `counters` (default: counters of its own, unread).
    pub fn with_counters(mut self, counters: Arc<JobCounters>) -> Self {
        self.counters = counters;
        self
    }

    /// Runs the runner on the current runtime until `stop` is cancelled, then drains
    /// and releases (P10). Once the loop sees `stop` (after its current claim or outcome
    /// write) the task ends within `drain` + [`ABORT_WAIT`] + [`RELEASE_BUDGET`].
    pub fn spawn(self, stop: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(self.run(stop))
    }

    async fn run(self, stop: CancellationToken) {
        let wake = Arc::new(Notify::new());
        let listener = (self.config.listen && !self.kinds.is_empty())
            .then(|| tokio::spawn(listen(self.store.clone(), wake.clone(), stop.clone())));
        let mut state = State {
            workers: self.config.workers.max(1),
            runner: self,
            tasks: JoinSet::new(),
            in_flight: HashMap::new(),
        };
        let mut extend = tokio::time::interval_at(
            Instant::now() + state.runner.config.extend_every,
            state.runner.config.extend_every,
        );
        extend.set_missed_tick_behavior(MissedTickBehavior::Delay);
        state.work(&stop, &wake, &mut extend).await;
        state.drain(&mut extend).await;
        state.release_rest().await;
        if let Some(listener) = listener
            && let Err(e) = listener.await
        {
            tracing::warn!(error = %e, "job notice listener ended abnormally");
        }
    }
}

/// How long shutdown waits for aborted handlers to stop before releasing their jobs.
pub const ABORT_WAIT: Duration = Duration::from_millis(500);
/// How long shutdown spends on the final release and outcome writes, all together.
pub const RELEASE_BUDGET: Duration = Duration::from_secs(2);

/// One job running on this node.
struct InFlight {
    lease: JobLease,
    abort: AbortHandle,
    span: Span,
    /// When the lease was last known to run until `now + lease`: taken just before the
    /// claim or the successful renewal was sent (R4).
    renewed_at: Instant,
    /// Aborted because the lease is lost or may have lapsed. The entry (and its worker
    /// slot) stays until the task is joined; nothing is written for it.
    cancelled: bool,
}

impl InFlight {
    fn cancel(&mut self, why: &'static str) {
        self.cancelled = true;
        self.abort.abort();
        tracing::warn!(parent: &self.span, "{why}; handler cancelled");
    }
}

/// The runner loop's state.
struct State {
    runner: JobRunner,
    workers: usize,
    /// Each task runs one handler call (under its timeout) and returns its result.
    tasks: JoinSet<Result<(), JobError>>,
    in_flight: HashMap<Id, InFlight>,
}

/// A task's result as `join_next_with_id` reports it.
type Joined = Result<(Id, Result<(), JobError>), JoinError>;

impl State {
    fn jobs(&self) -> &dyn JobRepo {
        self.runner.store.read().jobs()
    }

    /// The client-side bound on every store call the runner awaits (the store-call
    /// timeout, `extend_every / 2`: 10 s with the defaults). Server-side limits such as
    /// Postgres `statement_timeout` do not help against a black-holed connection; this
    /// does, and because it equals the fence's safety margin a stuck call can delay the
    /// fence ([`Self::enforce_fence`], checked after every call) by at most that margin,
    /// so a job is still dropped before its lease ends.
    fn call_timeout(&self) -> Duration {
        self.runner.config.extend_every / 2
    }

    /// Runs one store call under [`Self::call_timeout`]; running out is an error
    /// (`Database`), handled like any failed call.
    async fn bounded<T>(
        &self,
        call: impl Future<Output = Result<T, StoreError>>,
    ) -> Result<T, StoreError> {
        let limit = self.call_timeout();
        tokio::time::timeout(limit, call).await.unwrap_or_else(|_| {
            Err(StoreError::Database(format!(
                "store call timed out after {limit:?} (client side)"
            )))
        })
    }

    /// Local lease fencing (R4): how long after `renewed_at` a job is cancelled when no
    /// renewal succeeded since; half a renewal interval before the lease itself ends.
    fn fence(&self) -> Duration {
        let config = &self.runner.config;
        config.lease.saturating_sub(config.extend_every / 2)
    }

    /// The earliest local lease deadline among the running (not cancelled) jobs.
    fn next_fence(&self) -> Option<Instant> {
        let fence = self.fence();
        self.in_flight
            .values()
            .filter(|job| !job.cancelled)
            .map(|job| job.renewed_at + fence)
            .min()
    }

    /// Cancels every job whose lease may lapse before a renewal could save it (R4).
    fn enforce_fence(&mut self) {
        let fence = self.fence();
        let now = Instant::now();
        for job in self.in_flight.values_mut() {
            if !job.cancelled && now.duration_since(job.renewed_at) >= fence {
                job.cancel("lease not renewed in time and may lapse");
            }
        }
    }

    /// Claims and runs jobs until `stop`.
    async fn work(
        &mut self,
        stop: &CancellationToken,
        wake: &Notify,
        extend: &mut tokio::time::Interval,
    ) {
        let idle_poll = self.runner.config.idle_poll;
        let poll = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(poll);
        let mut claim_now = true;
        loop {
            // Every inline store call is bounded; check the fence after each one.
            self.enforce_fence();
            if claim_now && !self.runner.kinds.is_empty() && self.in_flight.len() < self.workers {
                claim_now = self.claim().await;
                poll.as_mut().reset(Instant::now() + idle_poll);
            } else {
                claim_now = false;
            }
            if claim_now {
                // The claim filled its whole limit: more may be due.
                continue;
            }
            let fence = self.next_fence();
            tokio::select! {
                biased;
                () = stop.cancelled() => return,
                _ = extend.tick() => {
                    // Renewals are idempotent, so dropping them on `stop` is harmless.
                    tokio::select! {
                        biased;
                        () = stop.cancelled() => return,
                        () = self.extend_all() => {}
                    }
                }
                () = sleep_until_opt(fence) => self.enforce_fence(),
                Some(joined) = self.tasks.join_next_with_id() => {
                    self.finish(joined).await;
                    claim_now = true;
                }
                () = wake.notified() => claim_now = true,
                () = &mut poll => {
                    poll.as_mut().reset(Instant::now() + idle_poll);
                    claim_now = true;
                }
            }
        }
    }

    /// Claims up to the free slots and starts the jobs. `true` when the claim returned
    /// its full limit and slots are still free (claim again at once).
    async fn claim(&mut self) -> bool {
        let free = self.workers - self.in_flight.len();
        let limit = u32::try_from(free).unwrap_or(MAX_CLAIM).min(MAX_CLAIM);
        let sent = Instant::now();
        // A timed-out claim returns nothing; jobs it may have claimed in the database
        // run again once their lease expires.
        let claimed = self
            .bounded(self.jobs().claim(
                self.runner.config.node,
                &self.runner.kinds,
                limit,
                self.runner.config.lease,
            ))
            .await;
        match claimed {
            Ok(jobs) => {
                let full = jobs.len() == limit as usize;
                for job in jobs {
                    self.start(job, sent).await;
                    self.enforce_fence();
                }
                full && self.in_flight.len() < self.workers
            }
            Err(e) => {
                tracing::warn!(error = %e, "job claim failed; retrying at the next poll");
                false
            }
        }
    }

    /// Checks the payload version and spawns the handler, or settles the job at once.
    async fn start(&mut self, job: Job, claimed_at: Instant) {
        // The node too: the job row clears `locked_by` once the job is over.
        let span = tracing::info_span!(
            "job",
            kind = %job.kind,
            id = %job.id,
            attempt = job.attempts,
            node = %self.runner.config.node,
        );
        let Some(lease) = job.lease() else {
            tracing::warn!(parent: &span, "claimed job has no lease; skipped");
            return;
        };
        // The refunded attempt count, for the backoff of a job that goes back unrun.
        let backoff = job_backoff(job.attempts.saturating_sub(1));
        let Some(handler) = self.runner.handlers.get(job.kind.as_str()).cloned() else {
            // Not reachable: the claim filters by the handler kinds.
            tracing::warn!(parent: &span, "no handler for claimed job; released");
            self.release(&lease, backoff, &span).await;
            return;
        };
        let Some(version) = payload_version(&job.payload) else {
            self.fail(&lease, "payload has no valid v", true, &span)
                .await;
            return;
        };
        if !handler.versions().contains(&version) {
            tracing::info!(parent: &span, version, "unsupported payload version; released for a newer node");
            self.release(&lease, backoff, &span).await;
            return;
        }
        let limit = handler.timeout();
        let task = async move {
            match tokio::time::timeout(limit, handler.run(&job)).await {
                Ok(result) => result,
                Err(_) => Err(JobError::Retry(format!(
                    "handler timed out after {limit:?}"
                ))),
            }
        };
        let abort = self.tasks.spawn(task.instrument(span.clone()));
        self.in_flight.insert(
            abort.id(),
            InFlight {
                lease,
                abort,
                span,
                renewed_at: claimed_at,
                cancelled: false,
            },
        );
    }

    /// Takes a joined task's entry off the books. `None` when there is nothing to
    /// write: unknown, cancelled by this runner, or aborted.
    fn settle(&mut self, joined: Joined) -> Option<(InFlight, Result<(), JobError>)> {
        let (id, outcome) = match joined {
            Ok((id, result)) => (id, result),
            Err(e) if e.is_panic() => (e.id(), Err(JobError::Retry("handler panicked".into()))),
            Err(e) => {
                self.in_flight.remove(&e.id());
                return None;
            }
        };
        let job = self.in_flight.remove(&id)?;
        (!job.cancelled).then_some((job, outcome))
    }

    /// Records the outcome of a finished task.
    async fn finish(&mut self, joined: Joined) {
        if let Some((job, outcome)) = self.settle(joined) {
            self.record(&job, outcome).await;
        }
    }

    async fn record(&self, job: &InFlight, outcome: Result<(), JobError>) {
        match outcome {
            Ok(()) => match self.bounded(self.jobs().complete(&job.lease)).await {
                Ok(true) => {
                    self.runner.counters.done.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(parent: &job.span, "job done");
                }
                Ok(false) => {
                    tracing::warn!(parent: &job.span, "lease lost before completion; result dropped");
                }
                Err(e) => tracing::warn!(
                    parent: &job.span, error = %e,
                    "could not record completion; the job runs again after its lease"
                ),
            },
            Err(JobError::Retry(e)) => self.fail(&job.lease, &e, false, &job.span).await,
            Err(JobError::Busy(e)) => {
                tracing::debug!(parent: &job.span, error = %e, "job busy; released, attempt refunded");
                self.release(&job.lease, super::busy_delay(), &job.span)
                    .await;
            }
            Err(JobError::Permanent(e)) => self.fail(&job.lease, &e, true, &job.span).await,
        }
    }

    async fn fail(&self, lease: &JobLease, error: &str, permanent: bool, span: &Span) {
        match self
            .bounded(self.jobs().fail(lease, error, permanent))
            .await
        {
            Ok(Some(status)) => {
                self.runner.counters.failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(parent: span, error, %status, "job attempt failed");
            }
            Ok(None) => {
                tracing::warn!(parent: span, error, "lease lost before the failure was recorded");
            }
            Err(e) => tracing::warn!(
                parent: span, error = %e, job_error = error,
                "could not record the failure; the job runs again after its lease"
            ),
        }
    }

    /// Puts the job back with the attempt refunded.
    async fn release(&self, lease: &JobLease, delay: Duration, span: &Span) {
        match self.bounded(self.jobs().release(lease, delay, false)).await {
            Ok(true) => {}
            Ok(false) => tracing::debug!(parent: span, "lease lost before release"),
            Err(e) => tracing::warn!(
                parent: span, error = %e,
                "could not release the job; another node takes it after its lease"
            ),
        }
    }

    /// Renews every running lease at once, each bounded by [`Self::call_timeout`]. A lease
    /// that is gone cancels its handler; a failed renewal leaves the job to the local
    /// fence ([`Self::enforce_fence`]).
    async fn extend_all(&mut self) {
        let lease_len = self.runner.config.lease;
        let targets: Vec<(Id, JobLease)> = self
            .in_flight
            .iter()
            .filter(|(_, job)| !job.cancelled)
            .map(|(id, job)| (*id, job.lease))
            .collect();
        let jobs = self.jobs();
        let limit = self.call_timeout();
        let results = futures::future::join_all(targets.iter().map(|(id, lease)| async move {
            let sent = Instant::now();
            let renewed = tokio::time::timeout(limit, jobs.extend(lease, lease_len)).await;
            (*id, sent, renewed)
        }))
        .await;
        for (id, sent, renewed) in results {
            let Some(job) = self.in_flight.get_mut(&id) else {
                continue;
            };
            match renewed {
                Ok(Ok(true)) => job.renewed_at = sent,
                Ok(Ok(false)) => job.cancel("lease lost"),
                Ok(Err(e)) => tracing::warn!(
                    parent: &job.span, error = %e,
                    "lease renewal failed; retrying at the next renewal"
                ),
                Err(_) => tracing::warn!(
                    parent: &job.span, timeout = ?limit,
                    "lease renewal timed out; retrying at the next renewal"
                ),
            }
        }
        self.enforce_fence();
    }

    /// After `stop`: no new claims; in-flight jobs finish within `drain` (P10). Every
    /// step is cut off at the deadline, so this returns by then.
    async fn drain(&mut self, extend: &mut tokio::time::Interval) {
        let deadline = Instant::now() + self.runner.config.drain;
        while !self.in_flight.is_empty() {
            let fence = self.next_fence();
            let in_time = tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => return,
                _ = extend.tick() => {
                    tokio::time::timeout_at(deadline, self.extend_all()).await.is_ok()
                }
                () = sleep_until_opt(fence) => {
                    self.enforce_fence();
                    true
                }
                Some(joined) = self.tasks.join_next_with_id() => {
                    if let Some((job, outcome)) = self.settle(joined) {
                        let written = tokio::time::timeout_at(deadline, self.record(&job, outcome)).await;
                        if written.is_err() {
                            tracing::warn!(parent: &job.span, "outcome not written before the drain deadline; the job runs again after its lease");
                        }
                    }
                    true
                }
            };
            if !in_time {
                return;
            }
        }
    }

    /// Aborts what is still running, waits up to [`ABORT_WAIT`] for the tasks to end,
    /// then releases the jobs whose tasks ended (attempt refunded, P10) and records the
    /// outcome of any that finished meanwhile, within [`RELEASE_BUDGET`] overall. A task
    /// that does not stop in time (a handler blocking its thread) keeps its job, which
    /// another node takes once the lease expires.
    async fn release_rest(&mut self) {
        if self.in_flight.is_empty() {
            return;
        }
        self.tasks.abort_all();
        let deadline = Instant::now() + ABORT_WAIT;
        let mut settled: Vec<(InFlight, Option<Result<(), JobError>>)> = Vec::new();
        while let Ok(Some(joined)) =
            tokio::time::timeout_at(deadline, self.tasks.join_next_with_id()).await
        {
            match joined {
                Err(e) if e.is_cancelled() => {
                    if let Some(job) = self.in_flight.remove(&e.id())
                        && !job.cancelled
                    {
                        settled.push((job, None));
                    }
                }
                other => {
                    if let Some((job, outcome)) = self.settle(other) {
                        settled.push((job, Some(outcome)));
                    }
                }
            }
        }
        for (_, job) in self.in_flight.drain() {
            if !job.cancelled {
                tracing::warn!(parent: &job.span, "handler did not stop within {ABORT_WAIT:?}; job left to expire on its lease");
            }
        }
        let this = &*self;
        let writes = settled.iter().map(|(job, outcome)| async move {
            match outcome {
                None => {
                    tracing::info!(parent: &job.span, "shutting down; job released");
                    this.release(&job.lease, Duration::ZERO, &job.span).await;
                }
                Some(outcome) => this.record(job, outcome.clone()).await,
            }
        });
        if tokio::time::timeout(RELEASE_BUDGET, futures::future::join_all(writes))
            .await
            .is_err()
        {
            tracing::warn!(
                "shutdown releases not finished within {RELEASE_BUDGET:?}; the rest expire on their leases"
            );
        }
    }
}

/// Sleeps until `at`, or forever without one.
async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// `payload.v`: absent is 1; anything but a `u32` is `None`.
fn payload_version(payload: &Value) -> Option<u32> {
    match payload.get("v") {
        None => Some(1),
        Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()),
    }
}

/// Turns job notices into wake-ups until `stop`, draining the subscription continuously
/// (a full Postgres notify queue fails every notifying commit). Re-subscribes after a
/// failure with [`RESUBSCRIBE_MIN`] doubling to [`RESUBSCRIBE_MAX`] (reset by every
/// successful subscribe); a backend without notifications ends the listener (the runner
/// polls).
async fn listen(store: Arc<dyn Store>, wake: Arc<Notify>, stop: CancellationToken) {
    let mut delay = RESUBSCRIBE_MIN;
    loop {
        let subscribed = tokio::select! {
            biased;
            () = stop.cancelled() => return,
            sub = store.subscribe(Channel::Jobs) => sub,
        };
        match subscribed {
            Ok(None) => return,
            Ok(Some(mut sub)) => {
                delay = RESUBSCRIBE_MIN;
                // Jobs enqueued while not subscribed sent their notice to nobody.
                wake.notify_one();
                loop {
                    let notice = tokio::select! {
                        biased;
                        () = stop.cancelled() => return,
                        notice = sub.recv() => notice,
                    };
                    match notice {
                        // `Reconnected` means notices may be lost: poll now, like a notice.
                        Ok(_) => wake.notify_one(),
                        Err(e) => {
                            tracing::warn!(error = %e, retry_in = ?delay, "job notice subscription failed");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, retry_in = ?delay, "could not subscribe to job notices");
            }
        }
        tokio::select! {
            biased;
            () = stop.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(RESUBSCRIBE_MAX);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn payload_versions() {
        assert_eq!(payload_version(&json!({})), Some(1));
        assert_eq!(payload_version(&json!({"v": 1})), Some(1));
        assert_eq!(payload_version(&json!({"v": 7})), Some(7));
        for bad in [
            json!({"v": "1"}),
            json!({"v": 1.5}),
            json!({"v": -1}),
            json!({"v": null}),
            json!({"v": u64::from(u32::MAX) + 1}),
        ] {
            assert_eq!(payload_version(&bad), None, "{bad}");
        }
    }

    #[test]
    fn config_defaults() {
        let node = NodeId::new();
        let c = RunnerConfig::new(node, 4);
        assert_eq!((c.node, c.workers, c.listen), (node, 4, false));
        assert_eq!(c.lease, Duration::from_secs(60));
        assert_eq!(c.extend_every, Duration::from_secs(20));
        assert_eq!(c.idle_poll, Duration::from_secs(1));
        assert_eq!(c.drain, Duration::from_secs(9));
    }
}
