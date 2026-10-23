//! The per-node job runner (04 §6, rulings P5, P9, P10): claims due jobs of the
//! registered kinds, runs each on its own task, renews every in-flight lease and records
//! the outcome under the lease token, so a node that lost its lease writes nothing (P6).

use std::{collections::HashMap, sync::Arc, time::Duration};

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
    /// Renewal interval; keep it well under `lease`.
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

/// Runs the jobs of its handlers' kinds on this node.
pub struct JobRunner {
    store: Arc<dyn Store>,
    config: RunnerConfig,
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
    /// The handler kinds, sorted: only these are claimed (P5).
    kinds: Vec<String>,
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
        })
    }

    /// Runs the runner on the current runtime until `stop` is cancelled, then drains
    /// and releases (P10). The task ends at most `drain` (plus the release writes) after
    /// `stop`.
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

/// One job running on this node.
struct InFlight {
    lease: JobLease,
    abort: AbortHandle,
    span: Span,
}

/// The runner loop's state.
struct State {
    runner: JobRunner,
    workers: usize,
    /// Each task runs one handler call (under its timeout) and returns its result.
    tasks: JoinSet<Result<(), JobError>>,
    in_flight: HashMap<Id, InFlight>,
}

impl State {
    fn jobs(&self) -> &dyn JobRepo {
        self.runner.store.read().jobs()
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
            tokio::select! {
                biased;
                () = stop.cancelled() => return,
                Some(joined) = self.tasks.join_next_with_id() => {
                    self.finish(joined).await;
                    claim_now = true;
                }
                _ = extend.tick() => self.extend_all().await,
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
        let claimed = self
            .jobs()
            .claim(
                self.runner.config.node,
                &self.runner.kinds,
                limit,
                self.runner.config.lease,
            )
            .await;
        match claimed {
            Ok(jobs) => {
                let full = jobs.len() == limit as usize;
                for job in jobs {
                    self.start(job).await;
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
    async fn start(&mut self, job: Job) {
        let span =
            tracing::info_span!("job", kind = %job.kind, id = %job.id, attempt = job.attempts);
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
        self.in_flight
            .insert(abort.id(), InFlight { lease, abort, span });
    }

    /// Records the outcome of a finished task. A task cancelled by this runner (lease
    /// lost) has no entry any more and writes nothing.
    async fn finish(&mut self, joined: Result<(Id, Result<(), JobError>), JoinError>) {
        let (id, outcome) = match joined {
            Ok((id, result)) => (id, result),
            Err(e) if e.is_panic() => (e.id(), Err(JobError::Retry("handler panicked".into()))),
            Err(e) => {
                self.in_flight.remove(&e.id());
                return;
            }
        };
        let Some(job) = self.in_flight.remove(&id) else {
            return;
        };
        match outcome {
            Ok(()) => match self.jobs().complete(&job.lease).await {
                Ok(true) => tracing::debug!(parent: &job.span, "job done"),
                Ok(false) => {
                    tracing::warn!(parent: &job.span, "lease lost before completion; result dropped");
                }
                Err(e) => tracing::warn!(
                    parent: &job.span, error = %e,
                    "could not record completion; the job runs again after its lease"
                ),
            },
            Err(JobError::Retry(e)) => self.fail(&job.lease, &e, false, &job.span).await,
            Err(JobError::Permanent(e)) => self.fail(&job.lease, &e, true, &job.span).await,
        }
    }

    async fn fail(&self, lease: &JobLease, error: &str, permanent: bool, span: &Span) {
        match self.jobs().fail(lease, error, permanent).await {
            Ok(Some(status)) => {
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
        match self.jobs().release(lease, delay, false).await {
            Ok(true) => {}
            Ok(false) => tracing::debug!(parent: span, "lease lost before release"),
            Err(e) => tracing::warn!(
                parent: span, error = %e,
                "could not release the job; another node takes it after its lease"
            ),
        }
    }

    /// Renews every in-flight lease; a lease that is gone cancels its handler.
    async fn extend_all(&mut self) {
        let leases: Vec<(Id, JobLease)> = self
            .in_flight
            .iter()
            .map(|(id, job)| (*id, job.lease))
            .collect();
        for (id, lease) in leases {
            match self.jobs().extend(&lease, self.runner.config.lease).await {
                Ok(true) => {}
                Ok(false) => {
                    if let Some(job) = self.in_flight.remove(&id) {
                        job.abort.abort();
                        tracing::warn!(parent: &job.span, "lease lost; handler cancelled");
                    }
                }
                Err(e) => {
                    let span = self.in_flight.get(&id).map(|j| j.span.clone());
                    tracing::warn!(parent: span.as_ref().and_then(Span::id), error = %e,
                        "lease renewal failed; retrying at the next renewal");
                }
            }
        }
    }

    /// After `stop`: no new claims; in-flight jobs finish within `drain` (P10).
    async fn drain(&mut self, extend: &mut tokio::time::Interval) {
        let deadline = Instant::now() + self.runner.config.drain;
        while !self.in_flight.is_empty() {
            tokio::select! {
                biased;
                Some(joined) = self.tasks.join_next_with_id() => self.finish(joined).await,
                _ = extend.tick() => self.extend_all().await,
                () = tokio::time::sleep_until(deadline) => return,
            }
        }
    }

    /// Cancels what is still running and releases each lease with the attempt refunded,
    /// so another node can take the job at once (P10).
    async fn release_rest(&mut self) {
        self.tasks.abort_all();
        let rest: Vec<InFlight> = self.in_flight.drain().map(|(_, job)| job).collect();
        for job in rest {
            tracing::info!(parent: &job.span, "shutting down; job released");
            self.release(&job.lease, Duration::ZERO, &job.span).await;
        }
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
/// failure with [`RESUBSCRIBE_MIN`] doubling to [`RESUBSCRIBE_MAX`]; a backend without
/// notifications ends the listener (the runner polls).
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
                        Ok(_) => {
                            delay = RESUBSCRIBE_MIN;
                            wake.notify_one();
                        }
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
