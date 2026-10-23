//! The outbox-polling invalidation loop (04 §4 read path, solo and cluster).
//!
//! Every tick the loop reads `events.since(last_seq, POLL_BATCH)` (continuing with
//! further batches while they come back full) and applies each event to the cache:
//!
//! | kind | effect |
//! |------|--------|
//! | `invalidate`, `options.changed` | [`PageCache::invalidate`] with the event's keys (`all` flushes) |
//! | `theme.changed`, `plugins.changed`, `plugin.disabled` | flush |
//! | `node.drain` | none |
//! | anything else | logged, and its keys invalidated (unknown means "may have changed") |
//!
//! **Sequence gaps** (04 §4): sequence numbers are allocated at insert and committed
//! out of order, so seq 11 can be visible before 10. Every event read is applied at
//! once (invalidation is idempotent), but `last_seq` advances only up to the first gap,
//! so the next poll re-reads from there and sees the late commit. A gap that stays
//! open for [`GAP_TIMEOUT`] is skipped (a rolled-back transaction leaves a permanent
//! hole). Events already applied past a gap are remembered and not re-applied.
//!
//! **Skipping a gap** (ruling R24) flushes the whole cache, and every skipped seq goes
//! into a *late set* watched for [`LATE_HORIZON`] (10 minutes): a writer that held its
//! seq uncommitted for longer than the gap timeout must still invalidate when it
//! commits. Each tick probes once with `since(min_late - 1, POLL_BATCH)`, paging
//! forward (resuming where the last tick stopped) within the per-tick batch budget,
//! applies any late seq that has appeared and drops it from the set. A seq not seen
//! within the horizon is dropped with a warning. At most [`MAX_LATE`] seqs are watched;
//! beyond that a skip only flushes (warned).
//!
//! **Late-probe bound**: the probe pages through committed rows from the smallest late
//! seq, [`POLL_BATCH`] rows per batch, resuming where the previous tick stopped, so a
//! late arrival ahead of the probe is found within ⌈span / (POLL_BATCH × probe budget)⌉
//! ticks, where span is the distance between the smallest and largest late seq and the
//! probe budget is the batches the tick has left after the main read (at least 1, at
//! most 10). An arrival the probe had already passed is found after it wraps to the
//! bottom of the span. Worst case, at the minimum budget of one batch per tick:
//! ⌈span / POLL_BATCH⌉ + 1 ticks.
//!
//! **Wake-ups and lost history (P3)**: with `listen` (cluster mode), a subtask owns the
//! events [`Subscription`] and drains it continuously: `Notified` wakes the loop for an
//! immediate poll (wake-ups coalesce, so a burst of notices costs one extra poll), and
//! `Reconnected` (notices may have been lost) also requests a *history check*. A
//! subscription error drops the subscription and resubscribes after 1 s, doubling up to
//! 30 s (the delay resets only once a subscription has delivered a notice or lived 30 s,
//! so one that fails right away keeps backing off); once that succeeds the history
//! check is requested too. Polling on the ticker
//! continues regardless: notices are hints, the table is the truth. The history check
//! also runs on the first poll after one or more failed polls, and when a poll first
//! sees a gap right at the cursor (`last_seq + 1`; once per gap), which catches pruned
//! history even when no `Reconnected` arrives. It reads
//! `events.min_seq()`: if it is above `last_seq + 1`, events were pruned past the cursor,
//! so the loop warns, flushes the cache, calls [`EventHook::on_history_lost`] (the node
//! reloads its state), moves the cursor to `min_seq - 1` and forgets gaps and late seqs
//! below it. `events.prune` never deletes the newest row (R9), so `min_seq` is 0 only
//! for an outbox that never had an event.
//!
//! **Hook**: [`InvalidationLoop::with_hook`] passes every event applied after
//! [`InvalidationLoop::new`] returned (late arrivals included) to an [`EventHook`],
//! after its cache effect. Events applied inside `new` (the boot window) are not passed
//! on: the caller reads node state after creating the loop (P22).
//!
//! **Boot**: `last_seq` starts at `max_seq()`, except that the last [`POLL_BATCH`]
//! events are inspected first: a hole below `max_seq` may be a transaction that
//! allocated its seq before boot and commits after it, so the cursor starts at the
//! first such hole instead (the events read are applied; on a fresh cache that is a
//! no-op). The hole before the oldest event read is history (pruned or long gone).
//! Create the loop **before** serving traffic, so no page is cached before the cursor
//! is set.
//!
//! Store errors are logged and retried on the next tick; the loop never panics. The
//! subscription is dropped before the loop's task ends.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use ferrite_domain::Event;
use ferrite_store::{Channel, EventRepo, Notice, Store, StoreError, Subscription};
use tokio::{
    sync::Notify,
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::cache::PageCache;

/// Events read per store call.
pub const POLL_BATCH: u32 = 1000;
/// How long a sequence gap is re-read before it is skipped.
pub const GAP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a skipped seq is watched for a late commit (ruling R24).
pub const LATE_HORIZON: Duration = Duration::from_secs(10 * 60);
/// Most skipped seqs watched at once.
pub const MAX_LATE: usize = 10_000;
/// Most batches read in one poll, shared by the main read and the late probe (the
/// probe always gets at least one); bounds one tick's work while catching up.
const MAX_BATCHES: usize = 10;
/// First delay before resubscribing after a subscription error; doubles per failure.
const RESUBSCRIBE_MIN: Duration = Duration::from_secs(1);
/// Longest delay between resubscribe attempts.
const RESUBSCRIBE_MAX: Duration = Duration::from_secs(30);

/// Where the loop reads events from ([`InvalidationLoop::for_store`] wraps a [`Store`]).
#[async_trait]
pub trait EventSource: Send + Sync {
    fn events(&self) -> &dyn EventRepo;

    /// Wake-up notices for new events; `Ok(None)` when there are none (the default, and
    /// SQLite), in which case the loop only polls.
    async fn subscribe(&self) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        Ok(None)
    }
}

struct StoreEvents(Arc<dyn Store>);

#[async_trait]
impl EventSource for StoreEvents {
    fn events(&self) -> &dyn EventRepo {
        self.0.read().events()
    }

    async fn subscribe(&self) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        self.0.subscribe(Channel::Events).await
    }
}

/// Told about what the loop applies, so the node can reload state beyond the page cache
/// (theme, plugins, permalinks). Called on the loop's task: must not block or await
/// (flip a flag and notify a task instead).
pub trait EventHook: Send + Sync {
    /// An event was applied (after its cache effect), at most once per seq.
    fn on_event(&self, event: &Event);
    /// Events were pruned past the cursor; anything may have changed (the cache was
    /// flushed already).
    fn on_history_lost(&self);
}

/// Polls the outbox and applies events to a [`PageCache`]; see the module docs.
pub struct InvalidationLoop {
    source: Arc<dyn EventSource>,
    cache: Arc<PageCache>,
    last_seq: i64,
    /// Open gaps, keyed by the seq of the event right after the gap, with the time
    /// the gap was first seen.
    gaps: BTreeMap<i64, Instant>,
    /// Seqs above `last_seq` already applied (read past a gap).
    applied: BTreeSet<i64>,
    /// Skipped seqs still watched, with the time they were skipped.
    late: BTreeMap<i64, Instant>,
    /// Where the late probe resumes (`None` = from the smallest late seq).
    probe_from: Option<i64>,
    /// The next poll starts with a history check (P3).
    check_pending: bool,
    hook: Option<Arc<dyn EventHook>>,
}

impl InvalidationLoop {
    /// Sets the cursor (see "Boot" in the module docs). Call before serving traffic.
    pub async fn new(
        source: Arc<dyn EventSource>,
        cache: Arc<PageCache>,
    ) -> Result<Self, StoreError> {
        let max = source.events().max_seq().await?;
        let recent = source
            .events()
            .since(max.saturating_sub(i64::from(POLL_BATCH)).max(0), POLL_BATCH)
            .await?;
        let mut this = Self {
            source,
            cache,
            last_seq: max,
            gaps: BTreeMap::new(),
            applied: BTreeSet::new(),
            late: BTreeMap::new(),
            probe_from: None,
            check_pending: false,
            hook: None,
        };
        if let Some(first) = recent.first() {
            this.last_seq = first.seq - 1;
            this.process(recent).await;
        }
        Ok(this)
    }

    /// Passes every event applied from now on to `hook` (see "Hook" in the module docs).
    #[must_use]
    pub fn with_hook(mut self, hook: Arc<dyn EventHook>) -> Self {
        self.hook = Some(hook);
        self
    }

    /// The sequence number up to which every event has been applied (or skipped as a
    /// timed-out gap).
    pub fn last_seq(&self) -> i64 {
        self.last_seq
    }

    /// One poll: runs a pending history check, reads the events after `last_seq` and
    /// applies the new ones, then probes the late set; returns how many events were
    /// applied. On a store error in the main read nothing is applied and the cursor does
    /// not move; the next poll starts with a history check.
    pub async fn poll_once(&mut self) -> Result<usize, StoreError> {
        let result = self.poll_inner().await;
        if result.is_err() {
            self.check_pending = true;
        }
        result
    }

    async fn poll_inner(&mut self) -> Result<usize, StoreError> {
        if self.check_pending {
            self.check_history().await?;
            self.check_pending = false;
        }
        let mut events: Vec<Event> = Vec::new();
        let mut from = self.last_seq;
        let mut batches = 0;
        for _ in 0..MAX_BATCHES {
            batches += 1;
            let batch = self.source.events().since(from, POLL_BATCH).await?;
            let full = batch.len() >= POLL_BATCH as usize;
            match batch.last() {
                Some(last) => from = last.seq,
                None => break,
            }
            events.extend(batch);
            if !full {
                break;
            }
        }
        // A new gap at the cursor may be pruned history rather than a late commit.
        if let Some(first) = events.first()
            && first.seq > self.last_seq + 1
            && !self.gaps.contains_key(&first.seq)
        {
            self.check_history().await?;
        }
        let applied = self.process(events).await;
        let late = self
            .probe_late(MAX_BATCHES.saturating_sub(batches).max(1))
            .await?;
        Ok(applied + late)
    }

    /// The P3 history check (see the module docs).
    async fn check_history(&mut self) -> Result<(), StoreError> {
        let min = self.source.events().min_seq().await?;
        if min > self.last_seq + 1 {
            tracing::warn!(
                last_seq = self.last_seq,
                min_seq = min,
                "events were pruned past the cursor; flushing the cache and reloading"
            );
            self.cache.flush();
            if let Some(hook) = &self.hook {
                hook.on_history_lost();
            }
            self.last_seq = min - 1;
            self.gaps.clear();
            self.applied = self.applied.split_off(&min);
            self.late = self.late.split_off(&min);
        } else {
            tracing::debug!(
                last_seq = self.last_seq,
                min_seq = min,
                "event history intact"
            );
        }
        Ok(())
    }

    /// Applies late arrivals of skipped seqs (see the module docs).
    async fn probe_late(&mut self, budget: usize) -> Result<usize, StoreError> {
        let now = Instant::now();
        let before = self.late.len();
        self.late
            .retain(|_, skipped_at| now.duration_since(*skipped_at) < LATE_HORIZON);
        if self.late.len() < before {
            tracing::warn!(
                dropped = before - self.late.len(),
                "skipped event seqs never committed within 10 min; no longer watched"
            );
        }
        let (Some(&min), Some(&max)) = (self.late.keys().next(), self.late.keys().next_back())
        else {
            self.probe_from = None;
            return Ok(0);
        };
        let mut from = self
            .probe_from
            .filter(|f| *f < max)
            .map_or(min - 1, |f| f.max(min - 1));
        let mut applied = 0;
        let mut covered = false;
        for _ in 0..budget {
            let batch = self.source.events().since(from, POLL_BATCH).await?;
            let full = batch.len() >= POLL_BATCH as usize;
            for event in &batch {
                if self.late.remove(&event.seq).is_some() {
                    tracing::info!(
                        seq = event.seq,
                        "late event committed after its gap was skipped"
                    );
                    self.apply(event).await;
                    applied += 1;
                }
            }
            if let Some(last) = batch.last() {
                from = last.seq;
            }
            if !full || from >= max {
                covered = true;
                break;
            }
        }
        self.probe_from = (!covered).then_some(from);
        Ok(applied)
    }

    /// Applies `events` (ascending, all above `last_seq`) that were not applied yet,
    /// then advances the cursor to the first open gap.
    async fn process(&mut self, events: Vec<Event>) -> usize {
        let mut applied = 0;
        for event in &events {
            if event.seq > self.last_seq && self.applied.insert(event.seq) {
                self.apply(event).await;
                applied += 1;
            }
        }

        let now = Instant::now();
        let mut prev = self.last_seq;
        let mut cursor = self.last_seq;
        let mut blocked = false;
        let mut skipped = false;
        let mut untracked = 0usize;
        for event in events.iter().filter(|e| e.seq > self.last_seq) {
            if event.seq > prev + 1 {
                let first_seen = *self.gaps.entry(event.seq).or_insert(now);
                if !blocked && now.duration_since(first_seen) >= GAP_TIMEOUT {
                    tracing::warn!(
                        from = prev + 1,
                        to = event.seq - 1,
                        "event sequence gap open for 5 s; skipping it, flushing the cache"
                    );
                    skipped = true;
                    let room = MAX_LATE.saturating_sub(self.late.len());
                    let hole = usize::try_from(event.seq - prev - 1).unwrap_or(usize::MAX);
                    for seq in (prev + 1..event.seq).take(room) {
                        self.late.insert(seq, now);
                    }
                    untracked += hole.saturating_sub(room);
                } else {
                    blocked = true;
                }
            }
            if !blocked {
                cursor = event.seq;
            }
            prev = event.seq;
        }
        self.last_seq = cursor;
        self.gaps = self.gaps.split_off(&(cursor + 1));
        self.applied = self.applied.split_off(&(cursor + 1));
        if skipped {
            self.cache.flush();
        }
        if untracked > 0 {
            tracing::warn!(
                untracked,
                "late set full; skipped seqs not watched for late commits"
            );
        }
        applied
    }

    async fn apply(&self, event: &Event) {
        match event.kind.as_str() {
            "invalidate" | "options.changed" => self.cache.invalidate(&event.keys).await,
            "theme.changed" | "plugins.changed" | "plugin.disabled" => self.cache.flush(),
            "node.drain" => {}
            other => {
                tracing::warn!(
                    kind = other,
                    seq = event.seq,
                    "unknown event kind; invalidating its keys"
                );
                self.cache.invalidate(&event.keys).await;
            }
        }
        if let Some(hook) = &self.hook {
            hook.on_event(event);
        }
    }

    /// A loop over `store`'s outbox; see [`InvalidationLoop::new`].
    pub async fn for_store(
        store: Arc<dyn Store>,
        cache: Arc<PageCache>,
    ) -> Result<Self, StoreError> {
        Self::new(Arc::new(StoreEvents(store)), cache).await
    }

    /// Runs the loop on the current runtime, polling every `poll_interval` (config
    /// `cache.poll_interval_ms`, default 250 ms) and, with `listen`, also on every
    /// notice (see "Wake-ups and lost history" in the module docs), until `shutdown` is
    /// cancelled.
    pub fn spawn(
        mut self,
        poll_interval: Duration,
        listen: bool,
        shutdown: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let wake = Arc::new(Wake::default());
            let listener = listen.then(|| {
                tokio::spawn(run_listener(
                    self.source.clone(),
                    wake.clone(),
                    shutdown.clone(),
                ))
            });
            let mut ticker = tokio::time::interval(poll_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => break,
                    _ = ticker.tick() => {}
                    () = wake.notify.notified() => {}
                }
                if wake.check_history.swap(false, Ordering::AcqRel) {
                    self.check_pending = true;
                }
                if let Err(e) = self.poll_once().await {
                    tracing::warn!(error = %e, last_seq = self.last_seq, "event poll failed; retrying next tick");
                }
            }
            // The listener ends on `shutdown` and drops its subscription on this runtime.
            if let Some(handle) = listener
                && let Err(e) = handle.await
            {
                tracing::error!(error = %e, "event listener task failed");
            }
        })
    }
}

/// From the listener subtask to the loop.
#[derive(Default)]
struct Wake {
    /// Poll now (a permit coalesces any number of notices).
    notify: Notify,
    /// Notices may have been lost: run the history check.
    check_history: AtomicBool,
}

impl Wake {
    fn history_unknown(&self) {
        self.check_history.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

/// Owns the events subscription until `shutdown`: forwards notices to `wake` and
/// resubscribes with backoff after errors (see the module docs).
async fn run_listener(source: Arc<dyn EventSource>, wake: Arc<Wake>, shutdown: CancellationToken) {
    let mut backoff = RESUBSCRIBE_MIN;
    let mut failed = false;
    loop {
        let subscribed = tokio::select! {
            () = shutdown.cancelled() => return,
            r = source.subscribe() => r,
        };
        match subscribed {
            Ok(None) => {
                tracing::debug!("no event notifications on this backend; polling only");
                return;
            }
            Ok(Some(mut sub)) => {
                let subscribed_at = Instant::now();
                let mut delivered = false;
                if failed {
                    tracing::info!("event listener resubscribed");
                    wake.history_unknown();
                }
                loop {
                    let notice = tokio::select! {
                        () = shutdown.cancelled() => return,
                        n = sub.recv() => n,
                    };
                    if notice.is_ok() && !delivered {
                        delivered = true;
                        backoff = RESUBSCRIBE_MIN;
                    }
                    match notice {
                        Ok(Notice::Notified) => wake.notify.notify_one(),
                        Ok(Notice::Reconnected) => wake.history_unknown(),
                        Err(e) => {
                            if subscribed_at.elapsed() >= RESUBSCRIBE_MAX {
                                backoff = RESUBSCRIBE_MIN;
                            }
                            tracing::warn!(error = %e, retry_in = ?backoff, "event listener failed; resubscribing");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, retry_in = ?backoff, "cannot subscribe to event notifications; retrying");
            }
        }
        failed = true;
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(RESUBSCRIBE_MAX);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use ferrite_domain::{NewEvent, Timestamp};
    use ferrite_store::{Notice, Subscription};
    use tokio::sync::mpsc;

    use super::*;
    use crate::page::{CacheKey, CachedPage};

    /// An outbox whose visible events the test controls (out-of-order commits), with a
    /// scripted subscription.
    #[derive(Default)]
    struct Fake {
        events: Mutex<Vec<Event>>,
        fail: Mutex<bool>,
        /// `after_seq` of every `since` call.
        since_calls: Mutex<Vec<i64>>,
        min_seq_calls: AtomicUsize,
        /// The subscription `subscribe` hands out; `None` = the backend has none.
        script: Option<Arc<Script>>,
        /// How many `subscribe` calls fail before one succeeds.
        subscribe_failures: AtomicUsize,
        /// When each `subscribe` call was made.
        subscribe_at: Mutex<Vec<Instant>>,
    }

    impl Fake {
        fn listening() -> Self {
            Self {
                script: Some(Arc::default()),
                ..Self::default()
            }
        }

        fn commit(&self, seq: i64, kind: &str, keys: &[&str]) {
            let mut v = self.events.lock().unwrap();
            v.push(Event {
                seq,
                kind: kind.into(),
                keys: keys.iter().map(|k| (*k).to_owned()).collect(),
                origin_node: None,
                created_at: Timestamp::now(),
            });
            v.sort_by_key(|e| e.seq);
        }

        /// Deletes every event below `seq` except the newest, as `events.prune` would (R9).
        fn prune_below(&self, seq: i64) {
            let mut v = self.events.lock().unwrap();
            let newest = v.last().map(|e| e.seq);
            v.retain(|e| e.seq >= seq || Some(e.seq) == newest);
        }

        fn notice(&self, notice: Notice) {
            self.script.as_ref().unwrap().push(Ok(notice));
        }

        /// The next `recv` fails (the connection could not be re-established).
        fn recv_error(&self) {
            self.script.as_ref().unwrap().push(Err(()));
        }

        fn since_calls(&self) -> Vec<i64> {
            self.since_calls.lock().unwrap().clone()
        }
    }

    /// Notices (or `recv` errors) queued by the test, over a tokio channel, so `recv`
    /// yields under tokio's coop budget like a real socket read. The live subscription
    /// owns the receiver and hands it back when dropped.
    struct Script {
        tx: mpsc::UnboundedSender<Result<Notice, ()>>,
        rx: Mutex<Option<mpsc::UnboundedReceiver<Result<Notice, ()>>>>,
        dropped: AtomicBool,
    }

    impl Default for Script {
        fn default() -> Self {
            let (tx, rx) = mpsc::unbounded_channel();
            Self {
                tx,
                rx: Mutex::new(Some(rx)),
                dropped: AtomicBool::new(false),
            }
        }
    }

    impl Script {
        fn push(&self, item: Result<Notice, ()>) {
            self.tx.send(item).unwrap();
        }
    }

    struct ScriptedSub {
        script: Arc<Script>,
        rx: Option<mpsc::UnboundedReceiver<Result<Notice, ()>>>,
    }

    impl ScriptedSub {
        fn new(script: Arc<Script>) -> Self {
            let rx = script.rx.lock().unwrap().take();
            assert!(rx.is_some(), "one live subscription at a time");
            Self { script, rx }
        }
    }

    #[async_trait]
    impl Subscription for ScriptedSub {
        async fn recv(&mut self) -> Result<Notice, StoreError> {
            match self.rx.as_mut().unwrap().recv().await {
                Some(Ok(notice)) => Ok(notice),
                Some(Err(())) => Err(StoreError::Busy),
                None => std::future::pending().await,
            }
        }
    }

    impl Drop for ScriptedSub {
        fn drop(&mut self) {
            *self.script.rx.lock().unwrap() = self.rx.take();
            self.script.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl EventSource for Fake {
        fn events(&self) -> &dyn EventRepo {
            self
        }

        async fn subscribe(&self) -> Result<Option<Box<dyn Subscription>>, StoreError> {
            self.subscribe_at.lock().unwrap().push(Instant::now());
            let Some(script) = &self.script else {
                return Ok(None);
            };
            if self
                .subscribe_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(StoreError::Busy);
            }
            Ok(Some(Box::new(ScriptedSub::new(script.clone()))))
        }
    }

    #[async_trait]
    impl EventRepo for Fake {
        async fn append(&self, _event: &NewEvent) -> Result<i64, StoreError> {
            unreachable!()
        }
        async fn since(&self, after_seq: i64, limit: u32) -> Result<Vec<Event>, StoreError> {
            self.since_calls.lock().unwrap().push(after_seq);
            if *self.fail.lock().unwrap() {
                return Err(StoreError::Busy);
            }
            Ok(self
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.seq > after_seq)
                .take(limit as usize)
                .cloned()
                .collect())
        }
        async fn max_seq(&self) -> Result<i64, StoreError> {
            if *self.fail.lock().unwrap() {
                return Err(StoreError::Busy);
            }
            Ok(self.events.lock().unwrap().last().map_or(0, |e| e.seq))
        }
        async fn min_seq(&self) -> Result<i64, StoreError> {
            self.min_seq_calls.fetch_add(1, Ordering::SeqCst);
            if *self.fail.lock().unwrap() {
                return Err(StoreError::Busy);
            }
            Ok(self.events.lock().unwrap().first().map_or(0, |e| e.seq))
        }
        async fn prune(&self, _older_than: Timestamp, _limit: u32) -> Result<u64, StoreError> {
            unreachable!()
        }
    }

    /// Records what the loop passes to its [`EventHook`].
    #[derive(Default)]
    struct Recorder {
        seqs: Mutex<Vec<i64>>,
        lost: AtomicUsize,
    }

    impl Recorder {
        fn seqs(&self) -> Vec<i64> {
            self.seqs.lock().unwrap().clone()
        }
        fn lost(&self) -> usize {
            self.lost.load(Ordering::SeqCst)
        }
    }

    impl EventHook for Recorder {
        fn on_event(&self, event: &Event) {
            self.seqs.lock().unwrap().push(event.seq);
        }
        fn on_history_lost(&self) {
            self.lost.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn key(path: &str) -> CacheKey {
        CacheKey::anon("h", path, "")
    }

    fn page(deps: &[&str]) -> CachedPage {
        CachedPage {
            status: 200,
            headers: vec![],
            body: Bytes::from_static(b"p"),
            etag: "\"p\"".into(),
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
            ttl: None,
        }
    }

    async fn setup() -> (Arc<Fake>, Arc<PageCache>, InvalidationLoop) {
        let fake = Arc::new(Fake::default());
        let cache = Arc::new(PageCache::new(1 << 20));
        let lp = InvalidationLoop::new(fake.clone(), cache.clone())
            .await
            .unwrap();
        (fake, cache, lp)
    }

    #[tokio::test(start_paused = true)]
    async fn gap_holds_cursor_until_filled() {
        let (fake, cache, mut lp) = setup().await;
        assert_eq!(lp.last_seq(), 0);
        cache.insert(key("/four/"), page(&["post:4"])).await;
        fake.commit(1, "invalidate", &["post:1"]);
        fake.commit(2, "invalidate", &["post:2"]);
        fake.commit(4, "invalidate", &["post:4"]);
        assert_eq!(lp.poll_once().await.unwrap(), 3);
        assert_eq!(lp.last_seq(), 2, "stops before the gap at 3");
        assert!(
            cache.get(&key("/four/")).await.is_none(),
            "events past the gap apply now"
        );

        // Event 4 is not applied twice: a page cached after it survives re-reads.
        cache.insert(key("/four/"), page(&["post:4"])).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(lp.poll_once().await.unwrap(), 0);
        assert_eq!(lp.last_seq(), 2);
        assert!(cache.get(&key("/four/")).await.is_some());

        // 3 commits late: it is applied and the cursor moves past 4.
        cache.insert(key("/three/"), page(&["post:3"])).await;
        fake.commit(3, "invalidate", &["post:3"]);
        assert_eq!(lp.poll_once().await.unwrap(), 1);
        assert_eq!(lp.last_seq(), 4);
        assert!(cache.get(&key("/three/")).await.is_none());
        assert!(lp.gaps.is_empty() && lp.applied.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn gap_is_skipped_after_timeout() {
        let (fake, _cache, mut lp) = setup().await;
        fake.commit(1, "invalidate", &["a"]);
        fake.commit(2, "invalidate", &["b"]);
        fake.commit(4, "invalidate", &["c"]);
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 2);
        tokio::time::advance(Duration::from_millis(4900)).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 2, "still within 5 s");
        tokio::time::advance(Duration::from_millis(100)).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 4, "skipped after 5 s");
        assert!(lp.gaps.is_empty() && lp.applied.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn skip_flushes_and_late_arrival_is_applied() {
        let (fake, cache, mut lp) = setup().await;
        fake.commit(1, "invalidate", &["a"]);
        fake.commit(2, "invalidate", &["b"]);
        fake.commit(4, "invalidate", &["c"]);
        lp.poll_once().await.unwrap();
        cache.insert(key("/any/"), page(&["post:1"])).await;
        tokio::time::advance(GAP_TIMEOUT).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 4);
        assert!(cache.is_empty().await, "the skip flushed the cache");
        assert_eq!(lp.late.keys().copied().collect::<Vec<_>>(), [3]);

        // Cached between the skip and the late commit, under seq 3's keys.
        tokio::time::advance(Duration::from_secs(10)).await;
        lp.poll_once().await.unwrap();
        cache.insert(key("/three/"), page(&["post:3"])).await;
        cache.insert(key("/other/"), page(&["post:9"])).await;
        tokio::time::advance(Duration::from_secs(15)).await;
        fake.commit(3, "invalidate", &["post:3"]);
        assert_eq!(
            lp.poll_once().await.unwrap(),
            1,
            "the late event is applied"
        );
        assert!(cache.get(&key("/three/")).await.is_none());
        assert!(
            cache.get(&key("/other/")).await.is_some(),
            "no second flush"
        );
        assert!(lp.late.is_empty());
        assert_eq!(lp.last_seq(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn late_seq_is_dropped_after_the_horizon() {
        let (fake, _cache, mut lp) = setup().await;
        fake.commit(1, "invalidate", &["a"]);
        fake.commit(3, "invalidate", &["b"]);
        lp.poll_once().await.unwrap();
        tokio::time::advance(GAP_TIMEOUT).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.late.len(), 1);
        tokio::time::advance(LATE_HORIZON - Duration::from_secs(1)).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.late.len(), 1, "still watched just inside the horizon");
        tokio::time::advance(Duration::from_secs(1)).await;
        lp.poll_once().await.unwrap();
        assert!(lp.late.is_empty(), "dropped at the horizon");
        assert_eq!(lp.probe_from, None);
    }

    #[tokio::test(start_paused = true)]
    async fn late_probe_pages_across_batches() {
        let (fake, cache, mut lp) = setup().await;
        // Holes at 3 and 1503 with 1499 events between them.
        let n = 1600;
        for seq in (1..=n).filter(|s| *s != 3 && *s != 1503) {
            fake.commit(seq, "invalidate", &["k"]);
        }
        lp.poll_once().await.unwrap();
        tokio::time::advance(GAP_TIMEOUT).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), n);
        assert_eq!(lp.late.keys().copied().collect::<Vec<_>>(), [3, 1503]);

        cache.insert(key("/a/"), page(&["post:3"])).await;
        cache.insert(key("/b/"), page(&["post:1503"])).await;
        fake.commit(3, "invalidate", &["post:3"]);
        fake.commit(1503, "invalidate", &["post:1503"]);
        // One tick has budget for both probe batches (1000 rows each).
        assert_eq!(lp.poll_once().await.unwrap(), 2);
        assert!(cache.is_empty().await);
        assert!(lp.late.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn every_gap_in_a_batch_gets_its_own_timer() {
        let (fake, _cache, mut lp) = setup().await;
        for seq in [1, 3, 5, 6] {
            fake.commit(seq, "invalidate", &["k"]);
        }
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 1);
        tokio::time::advance(GAP_TIMEOUT).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 6, "both gaps time out together");

        // A gap seen later starts its own timer.
        fake.commit(8, "invalidate", &["k"]);
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 6);
        tokio::time::advance(Duration::from_secs(2)).await;
        fake.commit(10, "invalidate", &["k"]);
        lp.poll_once().await.unwrap();
        tokio::time::advance(Duration::from_secs(3)).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 8, "gap at 7 timed out, gap at 9 not yet");
        tokio::time::advance(Duration::from_secs(2)).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 10);
    }

    #[tokio::test(start_paused = true)]
    async fn boot_starts_at_the_first_recent_gap() {
        let fake = Arc::new(Fake::default());
        for seq in [5, 6, 8] {
            fake.commit(seq, "invalidate", &["k"]);
        }
        let cache = Arc::new(PageCache::new(1 << 20));
        let mut lp = InvalidationLoop::new(fake.clone(), cache.clone())
            .await
            .unwrap();
        // 1..4 is history; 7 may be a transaction still in flight at boot.
        assert_eq!(lp.last_seq(), 6);
        cache.insert(key("/seven/"), page(&["post:7"])).await;
        fake.commit(7, "invalidate", &["post:7"]);
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 8);
        assert!(cache.get(&key("/seven/")).await.is_none());

        // Without holes the cursor is max_seq.
        let fake = Arc::new(Fake::default());
        for seq in 1..=3 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        let lp = InvalidationLoop::new(fake, cache).await.unwrap();
        assert_eq!(lp.last_seq(), 3);
    }

    #[tokio::test]
    async fn kinds_map_to_cache_effects() {
        let (fake, cache, mut lp) = setup().await;
        let fill = || async {
            cache
                .insert(key("/a/"), page(&["option:site.title", "theme"]))
                .await;
            cache.insert(key("/b/"), page(&["post:1", "theme"])).await;
        };
        fill().await;
        fake.commit(1, "options.changed", &["option:site.title"]);
        lp.poll_once().await.unwrap();
        assert!(cache.get(&key("/a/")).await.is_none());
        assert!(cache.get(&key("/b/")).await.is_some());

        for (seq, kind) in [
            (2, "theme.changed"),
            (3, "plugins.changed"),
            (4, "plugin.disabled"),
        ] {
            fill().await;
            fake.commit(seq, kind, &["whatever"]);
            lp.poll_once().await.unwrap();
            assert!(cache.is_empty().await, "{kind} flushes");
        }

        fill().await;
        fake.commit(5, "invalidate", &["all"]);
        lp.poll_once().await.unwrap();
        assert!(cache.is_empty().await);

        fill().await;
        fake.commit(6, "node.drain", &["post:1"]);
        lp.poll_once().await.unwrap();
        assert_eq!(cache.len().await, 2);

        fake.commit(7, "something.new", &["post:1"]);
        lp.poll_once().await.unwrap();
        assert!(
            cache.get(&key("/b/")).await.is_none(),
            "unknown kind invalidates its keys"
        );
        assert_eq!(lp.last_seq(), 7);
    }

    #[tokio::test]
    async fn store_errors_leave_the_cursor_and_retry() {
        let (fake, cache, mut lp) = setup().await;
        cache.insert(key("/a/"), page(&["post:1"])).await;
        fake.commit(1, "invalidate", &["post:1"]);
        *fake.fail.lock().unwrap() = true;
        assert!(lp.poll_once().await.is_err());
        assert_eq!(lp.last_seq(), 0);
        *fake.fail.lock().unwrap() = false;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 1);
        assert!(cache.get(&key("/a/")).await.is_none());
    }

    #[tokio::test]
    async fn catches_up_past_one_batch() {
        let (fake, _cache, mut lp) = setup().await;
        let n = i64::from(POLL_BATCH) * 2 + 5;
        for seq in 1..=n {
            fake.commit(seq, "invalidate", &["k"]);
        }
        assert_eq!(lp.poll_once().await.unwrap(), n as usize);
        assert_eq!(lp.last_seq(), n);
    }

    #[tokio::test(start_paused = true)]
    async fn spawned_loop_applies_events_and_stops_on_shutdown() {
        let (fake, cache, lp) = setup().await;
        let shutdown = CancellationToken::new();
        // `listen` on a backend without notifications (SQLite) just polls.
        let handle = lp.spawn(Duration::from_millis(250), true, shutdown.clone());
        cache.insert(key("/a/"), page(&["post:1"])).await;
        fake.commit(1, "invalidate", &["post:1"]);
        // While the store is down the loop keeps running.
        *fake.fail.lock().unwrap() = true;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(cache.get(&key("/a/")).await.is_some());
        *fake.fail.lock().unwrap() = false;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(cache.get(&key("/a/")).await.is_none());
        shutdown.cancel();
        handle.await.unwrap();
    }

    /// A listening loop over `fake` with a [`Recorder`] hook, spawned with a 60 s poll
    /// interval (so only wake-ups poll), after its first tick has run.
    async fn spawn_listening(
        fake: &Arc<Fake>,
        cache: &Arc<PageCache>,
    ) -> (Arc<Recorder>, CancellationToken, JoinHandle<()>) {
        let hook = Arc::new(Recorder::default());
        let lp = InvalidationLoop::new(fake.clone(), cache.clone())
            .await
            .unwrap()
            .with_hook(hook.clone());
        let shutdown = CancellationToken::new();
        let handle = lp.spawn(Duration::from_secs(60), true, shutdown.clone());
        tokio::time::sleep(Duration::from_millis(10)).await;
        (hook, shutdown, handle)
    }

    #[tokio::test(start_paused = true)]
    async fn notify_triggers_immediate_poll() {
        let fake = Arc::new(Fake::listening());
        let cache = Arc::new(PageCache::new(1 << 20));
        let (_hook, shutdown, handle) = spawn_listening(&fake, &cache).await;
        cache.insert(key("/a/"), page(&["post:1"])).await;
        fake.commit(1, "invalidate", &["post:1"]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            cache.get(&key("/a/")).await.is_some(),
            "nothing polls without a notice"
        );
        fake.notice(Notice::Notified);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            cache.get(&key("/a/")).await.is_none(),
            "applied within 100 ms"
        );
        shutdown.cancel();
        handle.await.unwrap();
        assert!(
            fake.script.as_ref().unwrap().dropped.load(Ordering::SeqCst),
            "the subscription is dropped before the loop's task ends"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn notifications_coalesce() {
        let fake = Arc::new(Fake::listening());
        let cache = Arc::new(PageCache::new(1 << 20));
        let (_hook, shutdown, handle) = spawn_listening(&fake, &cache).await;
        let before = fake.since_calls().len();
        fake.commit(1, "invalidate", &["post:1"]);
        for _ in 0..1000 {
            fake.notice(Notice::Notified);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let polls = fake.since_calls().len() - before;
        // The listener yields to the loop at most every 64 notices (tokio's coop budget of
        // 128, at most two units per notice: `recv` and the shutdown branch).
        assert!(
            (1..=1000_usize.div_ceil(64)).contains(&polls),
            "{polls} since() calls for 1000 notices"
        );
        shutdown.cancel();
        handle.await.unwrap();
    }

    /// Cursor 10, then events 11..=60 while the listener was down, of which 11..=49 were
    /// pruned before the node caught up.
    async fn reconnect_after(prune_below: i64) -> (Arc<Fake>, Arc<PageCache>, Arc<Recorder>) {
        let fake = Arc::new(Fake::listening());
        for seq in 1..=10 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        let cache = Arc::new(PageCache::new(1 << 20));
        let (hook, shutdown, handle) = spawn_listening(&fake, &cache).await;
        assert_eq!(fake.since_calls().last(), Some(&10));
        cache.insert(key("/other/"), page(&["post:other"])).await;
        cache.insert(key("/twenty/"), page(&["post:20"])).await;
        for seq in 11..=60 {
            fake.commit(seq, "invalidate", &[&format!("post:{seq}")]);
        }
        fake.prune_below(prune_below);
        fake.notice(Notice::Reconnected);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            fake.min_seq_calls.load(Ordering::SeqCst),
            1,
            "history checked"
        );
        shutdown.cancel();
        handle.await.unwrap();
        (fake, cache, hook)
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_with_pruned_history_flushes() {
        let (fake, cache, hook) = reconnect_after(50).await;
        assert!(cache.is_empty().await, "flushed");
        assert_eq!(hook.lost(), 1);
        assert_eq!(
            fake.since_calls().last(),
            Some(&49),
            "the cursor moved to min_seq - 1 = 49 before reading"
        );
        assert_eq!(hook.seqs(), (50..=60).collect::<Vec<_>>());
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_without_loss_only_catches_up() {
        let (fake, cache, hook) = reconnect_after(5).await;
        assert_eq!(hook.lost(), 0);
        assert!(cache.get(&key("/other/")).await.is_some(), "no flush");
        assert!(
            cache.get(&key("/twenty/")).await.is_none(),
            "missed event applied"
        );
        assert_eq!(hook.seqs(), (11..=60).collect::<Vec<_>>());
        assert_eq!(
            fake.since_calls().last(),
            Some(&10),
            "read on from the cursor"
        );
    }

    #[tokio::test]
    async fn poll_error_recovery_checks_history() {
        let (fake, cache, lp) = setup().await;
        let hook = Arc::new(Recorder::default());
        let mut lp = lp.with_hook(hook.clone());
        for seq in 1..=10 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 10);
        assert_eq!(
            fake.min_seq_calls.load(Ordering::SeqCst),
            0,
            "no check while healthy"
        );

        *fake.fail.lock().unwrap() = true;
        for _ in 0..3 {
            assert!(lp.poll_once().await.is_err());
        }
        cache.insert(key("/a/"), page(&["post:other"])).await;
        for seq in 11..=60 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        fake.prune_below(50);
        *fake.fail.lock().unwrap() = false;
        lp.poll_once().await.unwrap();
        assert!(cache.is_empty().await, "flushed");
        assert_eq!(hook.lost(), 1);
        assert_eq!(lp.last_seq(), 60);
        assert_eq!(hook.seqs(), (1..=10).chain(50..=60).collect::<Vec<_>>());

        // Retried on each failing poll (polls 2 and 3), done once it succeeded; healthy
        // polls do not repeat it.
        assert_eq!(fake.min_seq_calls.load(Ordering::SeqCst), 3);
        lp.poll_once().await.unwrap();
        assert_eq!(fake.min_seq_calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn hook_sees_applied_events_not_boot_window() {
        let fake = Arc::new(Fake::default());
        for seq in 1..=2 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        let cache = Arc::new(PageCache::new(1 << 20));
        let hook = Arc::new(Recorder::default());
        let mut lp = InvalidationLoop::new(fake.clone(), cache.clone())
            .await
            .unwrap()
            .with_hook(hook.clone());
        assert_eq!(lp.last_seq(), 2);
        assert!(
            hook.seqs().is_empty(),
            "boot-window events are not passed on"
        );

        for (seq, kind) in [(3, "invalidate"), (4, "theme.changed"), (6, "node.drain")] {
            fake.commit(seq, kind, &["k"]);
        }
        lp.poll_once().await.unwrap();
        assert_eq!(hook.seqs(), [3, 4, 6], "every kind, in seq order");
        tokio::time::advance(GAP_TIMEOUT).await;
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 6, "gap at 5 skipped");
        fake.commit(5, "plugins.changed", &["p"]);
        lp.poll_once().await.unwrap();
        assert_eq!(hook.seqs(), [3, 4, 6, 5], "the late arrival is passed on");
        lp.poll_once().await.unwrap();
        assert_eq!(hook.seqs(), [3, 4, 6, 5], "once");
        assert_eq!(hook.lost(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn subscribe_error_retries_with_backoff() {
        let fake = Arc::new(Fake::listening());
        fake.subscribe_failures.store(2, Ordering::SeqCst);
        let cache = Arc::new(PageCache::new(1 << 20));
        let (_hook, shutdown, handle) = spawn_listening(&fake, &cache).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        let at = fake.subscribe_at.lock().unwrap().clone();
        assert_eq!(at.len(), 3, "two failures, then a subscription");
        assert_eq!(at[1] - at[0], Duration::from_secs(1));
        assert_eq!(at[2] - at[1], Duration::from_secs(2));
        assert_eq!(
            fake.min_seq_calls.load(Ordering::SeqCst),
            1,
            "history checked once subscribed after an error"
        );

        // The new subscription is live.
        cache.insert(key("/a/"), page(&["post:1"])).await;
        fake.commit(1, "invalidate", &["post:1"]);
        fake.notice(Notice::Notified);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cache.get(&key("/a/")).await.is_none());
        shutdown.cancel();
        handle.await.unwrap();
    }

    /// The deferred M1c-2 item: a probe that cannot cover the late span in one tick
    /// resumes where it stopped (`probe_from`) instead of restarting at the bottom.
    #[tokio::test(start_paused = true)]
    async fn late_probe_resumes_where_it_stopped() {
        let (fake, cache, mut lp) = setup().await;
        // Holes at 3 and 25 003: a late span of 25 000 seqs.
        let n = 25_010;
        for seq in (1..=n).filter(|s| *s != 3 && *s != 25_003) {
            fake.commit(seq, "invalidate", &["k"]);
        }
        for _ in 0..10 {
            lp.poll_once().await.unwrap();
            tokio::time::advance(GAP_TIMEOUT).await;
        }
        assert_eq!(lp.last_seq(), n);
        assert_eq!(lp.late.keys().copied().collect::<Vec<_>>(), [3, 25_003]);

        cache.insert(key("/top/"), page(&["post:25003"])).await;
        fake.commit(25_003, "invalidate", &["post:25003"]);
        // One batch per tick (a tick whose main read used the rest of the budget).
        let mut prev = lp.probe_from;
        let mut ticks = 0;
        while lp.late.contains_key(&25_003) {
            ticks += 1;
            assert!(ticks <= 25, "not found within ⌈25 000 / 1000⌉ ticks");
            lp.probe_late(1).await.unwrap();
            if lp.late.contains_key(&25_003) {
                let from = lp.probe_from.expect("the probe stopped mid-span");
                if let Some(p) = prev {
                    assert_eq!(from - p, i64::from(POLL_BATCH), "resumes, one batch on");
                }
                prev = Some(from);
            }
        }
        assert!(cache.get(&key("/top/")).await.is_none(), "applied");
        assert_eq!(lp.late.keys().copied().collect::<Vec<_>>(), [3]);
    }

    #[tokio::test(start_paused = true)]
    async fn recv_error_backoff_resets_only_after_a_notice_or_30s() {
        let fake = Arc::new(Fake::listening());
        for _ in 0..3 {
            fake.recv_error();
        }
        let cache = Arc::new(PageCache::new(1 << 20));
        let (_hook, shutdown, handle) = spawn_listening(&fake, &cache).await;
        let at = || fake.subscribe_at.lock().unwrap().clone();
        tokio::time::sleep(Duration::from_secs(10)).await;
        let a = at();
        assert_eq!(
            a.len(),
            4,
            "three subscriptions failed at once, the fourth waits"
        );
        assert_eq!(
            [a[1] - a[0], a[2] - a[1], a[3] - a[2]],
            [1, 2, 4].map(Duration::from_secs),
            "a subscription that fails right away does not reset the backoff"
        );

        // A delivered notice resets it.
        fake.notice(Notice::Notified);
        fake.recv_error();
        let pushed = Instant::now();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(at().len(), 5);
        assert_eq!(at()[4] - pushed, Duration::from_secs(1));

        // So does a subscription that lived 30 s (the delay would be 2 s otherwise).
        tokio::time::sleep(Duration::from_secs(40)).await;
        fake.recv_error();
        let pushed = Instant::now();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(at().len(), 6);
        assert_eq!(at()[5] - pushed, Duration::from_secs(1));
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn new_gap_at_cursor_checks_history() {
        let (fake, cache, lp) = setup().await;
        let hook = Arc::new(Recorder::default());
        let mut lp = lp.with_hook(hook.clone());
        for seq in 1..=10 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        lp.poll_once().await.unwrap();
        assert_eq!(fake.min_seq_calls.load(Ordering::SeqCst), 0);

        // Pruned history with no reconnect and no failed poll: the gap at the cursor.
        cache.insert(key("/a/"), page(&["post:other"])).await;
        for seq in 11..=60 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        fake.prune_below(50);
        lp.poll_once().await.unwrap();
        assert_eq!(fake.min_seq_calls.load(Ordering::SeqCst), 1);
        assert!(cache.is_empty().await, "flushed");
        assert_eq!(hook.lost(), 1);
        assert_eq!(lp.last_seq(), 60);

        // A late commit at the cursor costs one check per gap, not one per poll.
        fake.commit(62, "invalidate", &["k"]);
        for _ in 0..3 {
            lp.poll_once().await.unwrap();
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        assert_eq!(lp.last_seq(), 60, "held at the gap");
        assert_eq!(fake.min_seq_calls.load(Ordering::SeqCst), 2);
        assert_eq!(hook.lost(), 1, "intact history is not flushed");
        fake.commit(61, "invalidate", &["k"]);
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 62);
    }

    #[tokio::test]
    async fn history_loss_does_not_reapply_events_past_a_gap() {
        let (fake, _cache, lp) = setup().await;
        let hook = Arc::new(Recorder::default());
        let mut lp = lp.with_hook(hook.clone());
        for seq in 1..=10 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        lp.poll_once().await.unwrap();
        // 11 still in flight; 12..=60 applied past the gap.
        for seq in 12..=60 {
            fake.commit(seq, "invalidate", &["k"]);
        }
        lp.poll_once().await.unwrap();
        assert_eq!(lp.last_seq(), 10);
        fake.prune_below(50);
        *fake.fail.lock().unwrap() = true;
        assert!(lp.poll_once().await.is_err());
        *fake.fail.lock().unwrap() = false;
        lp.poll_once().await.unwrap();
        assert_eq!(hook.lost(), 1);
        assert_eq!(lp.last_seq(), 60);
        assert_eq!(
            hook.seqs(),
            (1..=10).chain(12..=60).collect::<Vec<_>>(),
            "each seq passed to the hook once"
        );
    }

    /// Checks, from inside `on_event`, that the event's cache effect already happened.
    struct OrderCheck {
        cache: Arc<PageCache>,
        page: CacheKey,
        gone: Mutex<Vec<(String, bool)>>,
    }

    impl EventHook for OrderCheck {
        fn on_event(&self, event: &Event) {
            let cached = self.cache.state().snapshot().0.contains(&self.page);
            self.gone
                .lock()
                .unwrap()
                .push((event.kind.clone(), !cached));
        }
        fn on_history_lost(&self) {}
    }

    #[tokio::test]
    async fn cache_effect_happens_before_on_event() {
        let (fake, cache, lp) = setup().await;
        let hook = Arc::new(OrderCheck {
            cache: cache.clone(),
            page: key("/a/"),
            gone: Mutex::default(),
        });
        let mut lp = lp.with_hook(hook.clone());
        cache.insert(key("/a/"), page(&["post:1"])).await;
        fake.commit(1, "theme.changed", &["theme"]);
        lp.poll_once().await.unwrap();
        cache.insert(key("/a/"), page(&["post:1"])).await;
        fake.commit(2, "invalidate", &["post:1"]);
        lp.poll_once().await.unwrap();
        assert_eq!(
            *hook.gone.lock().unwrap(),
            [
                ("theme.changed".to_owned(), true),
                ("invalidate".to_owned(), true)
            ]
        );
    }
}
