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
//! **Boot**: `last_seq` starts at `max_seq()`, except that the last [`POLL_BATCH`]
//! events are inspected first: a hole below `max_seq` may be a transaction that
//! allocated its seq before boot and commits after it, so the cursor starts at the
//! first such hole instead (the events read are applied; on a fresh cache that is a
//! no-op). The hole before the oldest event read is history (pruned or long gone).
//! Create the loop **before** serving traffic, so no page is cached before the cursor
//! is set.
//!
//! Store errors are logged and retried on the next tick; the loop never panics.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use ferrite_domain::Event;
use ferrite_store::{EventRepo, Store, StoreError};
use tokio::{
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

/// Where the loop reads events from ([`InvalidationLoop::for_store`] wraps a [`Store`]).
pub trait EventSource: Send + Sync {
    fn events(&self) -> &dyn EventRepo;
}

struct StoreEvents(Arc<dyn Store>);

impl EventSource for StoreEvents {
    fn events(&self) -> &dyn EventRepo {
        self.0.read().events()
    }
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
        };
        if let Some(first) = recent.first() {
            this.last_seq = first.seq - 1;
            this.process(recent).await;
        }
        Ok(this)
    }

    /// The sequence number up to which every event has been applied (or skipped as a
    /// timed-out gap).
    pub fn last_seq(&self) -> i64 {
        self.last_seq
    }

    /// One poll: reads the events after `last_seq` and applies the new ones, then
    /// probes the late set; returns how many events were applied. On a store error in
    /// the main read nothing is applied and the cursor does not move.
    pub async fn poll_once(&mut self) -> Result<usize, StoreError> {
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
        let applied = self.process(events).await;
        let late = self
            .probe_late(MAX_BATCHES.saturating_sub(batches).max(1))
            .await?;
        Ok(applied + late)
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
    }

    /// A loop over `store`'s outbox; see [`InvalidationLoop::new`].
    pub async fn for_store(
        store: Arc<dyn Store>,
        cache: Arc<PageCache>,
    ) -> Result<Self, StoreError> {
        Self::new(Arc::new(StoreEvents(store)), cache).await
    }

    /// Runs the loop on the current runtime, polling every `poll_interval` (config
    /// `cache.poll_interval_ms`, default 250 ms), until `shutdown` is cancelled.
    pub fn spawn(mut self, poll_interval: Duration, shutdown: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(poll_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => return,
                    _ = ticker.tick() => {
                        if let Err(e) = self.poll_once().await {
                            tracing::warn!(error = %e, last_seq = self.last_seq, "event poll failed; retrying next tick");
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use bytes::Bytes;
    use ferrite_domain::{NewEvent, Timestamp};

    use super::*;
    use crate::page::{CacheKey, CachedPage};

    /// An outbox whose visible events the test controls (out-of-order commits).
    #[derive(Default)]
    struct Fake {
        events: Mutex<Vec<Event>>,
        fail: Mutex<bool>,
    }

    impl Fake {
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
    }

    impl EventSource for Fake {
        fn events(&self) -> &dyn EventRepo {
            self
        }
    }

    #[async_trait]
    impl EventRepo for Fake {
        async fn append(&self, _event: &NewEvent) -> Result<i64, StoreError> {
            unreachable!()
        }
        async fn since(&self, after_seq: i64, limit: u32) -> Result<Vec<Event>, StoreError> {
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
            unreachable!()
        }
        async fn prune(&self, _older_than: Timestamp) -> Result<u64, StoreError> {
            unreachable!()
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
        let handle = lp.spawn(Duration::from_millis(250), shutdown.clone());
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
}
