//! Bounded, coalesced redirect hit counting (06 §6).
//!
//! The public handler [`record`](HitCounter::record)s a hit for every redirect row it
//! answers with: an increment in this node's in-memory map under a lock held only for
//! that update, never any I/O, so a redirect never waits for the store. At most
//! [`MAX_BUFFERED_REDIRECTS`] distinct redirects are buffered between flushes; hits on
//! further new ones are dropped and counted ([`dropped`](HitCounter::dropped)), hits on
//! buffered ones always count.
//!
//! A flusher task ([`spawn_flusher`](HitCounter::spawn_flusher)) empties the map every
//! [`HIT_FLUSH_EVERY`] and once more on stop: one
//! [`RedirectService::add_hits`] (one `UPDATE`) per redirect, with the number of hits
//! and the time of the last one, so 1 000 hits on one redirect are one write.
//!
//! **Loss.** Counting is best effort: buffered hits are lost when the node crashes. A
//! failed write puts its batch back into the map (within the cap; what does not fit is
//! counted as dropped), so a transient database error only delays it to the next flush.
//! `serve` stops the flusher after the HTTP drain and before the database pools close; a
//! final flush cut short by the shutdown bound leaves the unwritten redirects in the map,
//! and `serve` logs how many ids and hits were not written
//! ([`pending`](HitCounter::pending)). Each cluster node keeps its own map; the
//! `UPDATE`s add up.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ferrite_core::RedirectService;
use ferrite_domain::{RedirectId, Timestamp};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How often buffered hits are written.
pub const HIT_FLUSH_EVERY: Duration = Duration::from_secs(10);
/// Most distinct redirects buffered between flushes.
pub const MAX_BUFFERED_REDIRECTS: usize = 10_000;

/// Hits on one redirect since the last flush.
#[derive(Debug, Clone, Copy)]
struct Pending {
    n: u32,
    last: Timestamp,
}

/// The node's redirect hit buffer (see the module docs).
pub struct HitCounter {
    redirects: Arc<RedirectService>,
    pending: Mutex<HashMap<RedirectId, Pending>>,
    dropped: AtomicU64,
    /// [`dropped`](Self::dropped) as last logged by the flusher.
    reported: AtomicU64,
}

impl HitCounter {
    /// A buffer whose flushes write through `redirects`.
    pub fn new(redirects: Arc<RedirectService>) -> Self {
        Self {
            redirects,
            pending: Mutex::new(HashMap::new()),
            dropped: AtomicU64::new(0),
            reported: AtomicU64::new(0),
        }
    }

    /// Counts one hit on `id` now; never waits for I/O. A new redirect beyond
    /// [`MAX_BUFFERED_REDIRECTS`] buffered ones is dropped and counted instead.
    pub fn record(&self, id: RedirectId) {
        let now = Timestamp::now();
        let mut pending = self.lock();
        let len = pending.len();
        match pending.get_mut(&id) {
            Some(p) => {
                // 4 billion hits on one redirect within one flush interval: saturate.
                p.n = p.n.saturating_add(1);
                p.last = p.last.max(now);
            }
            None if len >= MAX_BUFFERED_REDIRECTS => {
                drop(pending);
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            None => {
                pending.insert(id, Pending { n: 1, last: now });
            }
        }
    }

    /// Puts `p` (hits not written) back: added to a buffered entry, else inserted while
    /// fewer than [`MAX_BUFFERED_REDIRECTS`] are buffered, else counted as dropped.
    fn merge(&self, id: RedirectId, p: Pending) {
        let mut pending = self.lock();
        let len = pending.len();
        match pending.get_mut(&id) {
            Some(q) => {
                q.n = q.n.saturating_add(p.n);
                q.last = q.last.max(p.last);
            }
            None if len >= MAX_BUFFERED_REDIRECTS => {
                drop(pending);
                self.dropped.fetch_add(u64::from(p.n), Ordering::Relaxed);
            }
            None => {
                pending.insert(id, p);
            }
        }
    }

    /// Hits dropped so far because the buffer was full (new redirects beyond the cap,
    /// or a failed write's batch that no longer fit).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Distinct redirects buffered now.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no hit is buffered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Buffered redirects and the hits on them (not written yet).
    pub fn pending(&self) -> (usize, u64) {
        let pending = self.lock();
        let hits = pending.values().map(|p| u64::from(p.n)).sum();
        (pending.len(), hits)
    }

    /// Writes every buffered redirect's hits (one `add_hits` each) and empties the
    /// buffer; returns how many writes succeeded. A failed write is logged and its hits
    /// go back into the buffer for the next flush. If the flush is cancelled (the task
    /// aborted at shutdown), the redirects not written yet go back too; a write that
    /// committed just before the cancel goes back as well, so only cancel a flush when
    /// no flush follows (at most one row is then over-reported as unwritten).
    pub async fn flush(&self) -> usize {
        let mut batch = Unwritten {
            counter: self,
            left: std::mem::take(&mut *self.lock()),
        };
        let mut written = 0;
        while let Some((&id, &p)) = batch.left.iter().next() {
            let result = self.redirects.add_hits(id, p.n, p.last).await;
            batch.left.remove(&id);
            match result {
                Ok(()) => written += 1,
                Err(e) => {
                    tracing::warn!(redirect = %id, hits = p.n, error = %e, "redirect hits not recorded; kept for the next flush");
                    self.merge(id, p);
                }
            }
        }
        written
    }

    async fn flush_logged(&self) {
        let dropped = self.dropped();
        let before = self.reported.swap(dropped, Ordering::Relaxed);
        if dropped > before {
            tracing::warn!(
                dropped = dropped - before,
                "redirect hits dropped since the last flush (buffer full)"
            );
        }
        let written = self.flush().await;
        if written > 0 {
            tracing::debug!(redirects = written, "flushed redirect hits");
        }
    }

    /// Flushes every `every` ([`HIT_FLUSH_EVERY`] in `serve`) until `stop`, then once
    /// more.
    pub fn spawn_flusher(
        self: Arc<Self>,
        every: Duration,
        stop: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break,
                    _ = tick.tick() => self.flush_logged().await,
                }
            }
            self.flush_logged().await;
        })
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<RedirectId, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The rest of a flush's batch: merged back into the buffer when dropped, so a flush cut
/// short (its task aborted) loses nothing that a later flush or `pending` cannot see.
struct Unwritten<'a> {
    counter: &'a HitCounter,
    left: HashMap<RedirectId, Pending>,
}

impl Drop for Unwritten<'_> {
    fn drop(&mut self) {
        for (id, p) in self.left.drain() {
            self.counter.merge(id, p);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ferrite_store::Store;
    use ferrite_store_tests::sqlite_store;

    use super::*;

    async fn counter() -> (HitCounter, Arc<dyn Store>, tempfile::TempDir) {
        let (store, dir) = sqlite_store().await;
        let store: Arc<dyn Store> = Arc::new(store);
        let counter = HitCounter::new(Arc::new(RedirectService::new(store.clone())));
        (counter, store, dir)
    }

    /// Review round 1, minor 1: a failed write keeps its batch for the next flush.
    #[tokio::test]
    async fn failed_write_keeps_the_batch() {
        let (hits, store, _dir) = counter().await;
        let id = RedirectId::new();
        for _ in 0..3 {
            hits.record(id);
        }
        hits.record(RedirectId::new());
        // A closed pool fails every statement.
        store.close().await;
        assert_eq!(hits.flush().await, 0);
        assert_eq!(hits.pending(), (2, 4));
        assert_eq!(hits.dropped(), 0);
        // Hits recorded since add to the kept batch.
        hits.record(id);
        assert_eq!(hits.pending(), (2, 5));
    }

    /// A merged-back batch respects the cap: it joins a buffered entry, else needs room,
    /// else its hits count as dropped.
    #[tokio::test]
    async fn merge_respects_the_cap() {
        let (hits, _store, _dir) = counter().await;
        let first = RedirectId::new();
        hits.record(first);
        for _ in 1..MAX_BUFFERED_REDIRECTS {
            hits.record(RedirectId::new());
        }
        let p = |n| Pending {
            n,
            last: Timestamp::now(),
        };
        hits.merge(RedirectId::new(), p(5));
        assert_eq!(hits.dropped(), 5);
        hits.merge(first, p(2));
        assert_eq!(hits.pending(), (MAX_BUFFERED_REDIRECTS, 10_002));
        assert_eq!(hits.dropped(), 5);
    }

    /// Review round 1, minor 2: a flush aborted mid-way (the shutdown bound) leaves the
    /// unwritten redirects visible through `pending`.
    #[tokio::test]
    async fn aborted_flush_keeps_the_rest() {
        let (hits, store, _dir) = counter().await;
        let hits = Arc::new(hits);
        for _ in 0..3 {
            hits.record(RedirectId::new());
        }
        // An open transaction holds SQLite's one writer: the first write waits.
        let tx = store.begin().await.unwrap();
        let flusher = hits.clone();
        let task = tokio::spawn(async move { flusher.flush().await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!task.is_finished());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(hits.pending(), (3, 3));
        drop(tx);
    }
}
