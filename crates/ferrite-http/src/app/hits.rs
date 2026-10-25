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
//! **Loss.** Counting is best effort: buffered hits are lost when the node crashes, and
//! a failed write loses that redirect's batch (logged). `serve` stops the flusher after
//! the HTTP drain and before the database pools close, so a clean shutdown writes
//! everything counted. Each cluster node keeps its own map; the `UPDATE`s add up.

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

    /// Hits on new redirects dropped so far because the buffer was full.
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

    /// Writes every buffered redirect's hits (one `add_hits` each) and empties the
    /// buffer; returns how many writes succeeded. A failed write is logged and its
    /// hits are lost.
    pub async fn flush(&self) -> usize {
        let pending = std::mem::take(&mut *self.lock());
        let mut written = 0;
        for (id, p) in pending {
            match self.redirects.add_hits(id, p.n, p.last).await {
                Ok(()) => written += 1,
                Err(e) => {
                    tracing::warn!(redirect = %id, hits = p.n, error = %e, "redirect hits not recorded")
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
