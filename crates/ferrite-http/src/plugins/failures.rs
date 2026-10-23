//! Shared plugin failure counters and the cluster-wide auto-disable (plan ruling P20).
//!
//! [`SharedFailures`] is the node's [`FailureSink`]: the plugin host reports every
//! failed call to it synchronously, and it only bumps an in-memory counter per
//! `(plugin, hook)`. A flusher task ([`SharedFailures::spawn_flusher`], every
//! [`FLUSH_EVERY`]) writes the buffered counts into 1-minute buckets of
//! `plugin_failures` (bucket time from the database clock) in one transaction, then
//! sums each flushed plugin's buckets over the failure window. At or above the
//! threshold it disables the row with one conditional statement
//! ([`PluginRepo::auto_disable`](ferrite_store::PluginRepo::auto_disable)) and, only if
//! that call made the transition, appends `plugin.disabled` (keys `[name, reason]`) in
//! the same transaction: whichever nodes cross the threshold, exactly one event is
//! emitted. That node also disables the plugin in its host at once; the others do on
//! the event. The host's own per-node counter keeps disabling locally as before.
//!
//! The cluster-wide disable lags the threshold by up to one flush interval.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ferrite_core::events::plugin_disabled;
use ferrite_domain::Timestamp;
use ferrite_plugin_host::{FailureKind, FailureSink, PluginHost};
use ferrite_store::{Store, StoreError};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How often buffered failures are written (P20).
pub const FLUSH_EVERY: Duration = Duration::from_secs(5);

/// Width of a `plugin_failures` bucket.
const BUCKET_MICROS: i64 = 60_000_000;

/// Buffers failure counts and shares them through the database (see the module docs).
pub struct SharedFailures {
    threshold: u32,
    window: Duration,
    /// `(plugin, hook)` → failures not yet written.
    buffer: Mutex<HashMap<(String, String), u32>>,
}

impl SharedFailures {
    /// Disable a plugin once it has `threshold` (`plugins.disable_after_failures`, at
    /// least 1) failures summed over `window` (`plugins.failure_window`), all nodes
    /// together.
    pub fn new(threshold: u32, window: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            window,
            buffer: Mutex::new(HashMap::new()),
        }
    }

    /// Flush every `every` until `stop`, then flush once more. A flush that fails is
    /// logged and its counts go back into the buffer for the next one.
    pub fn spawn_flusher(
        self: Arc<Self>,
        store: Arc<dyn Store>,
        host: Arc<PluginHost>,
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
                    _ = tick.tick() => self.flush_logged(store.as_ref(), &host).await,
                }
            }
            self.flush_logged(store.as_ref(), &host).await;
        })
    }

    async fn flush_logged(&self, store: &dyn Store, host: &PluginHost) {
        if let Err(e) = self.flush(store, host).await {
            tracing::warn!(error = %e, "flushing plugin failure counts failed");
        }
    }

    /// Write the buffered counts, then check each flushed plugin against the threshold.
    async fn flush(&self, store: &dyn Store, host: &PluginHost) -> Result<(), StoreError> {
        let batch = std::mem::take(&mut *self.lock());
        if batch.is_empty() {
            return Ok(());
        }
        let now = match store.read().nodes().db_now().await {
            Ok(now) => now,
            Err(e) => {
                self.restore(batch);
                return Err(e);
            }
        };
        if let Err(e) = write_counts(store, &batch, bucket_start(now)?).await {
            self.restore(batch);
            return Err(e);
        }
        let since = minus(now, self.window)?;
        let plugins: BTreeSet<&str> = batch.keys().map(|(p, _)| p.as_str()).collect();
        for plugin in plugins {
            let n = store
                .read()
                .plugin_failures()
                .sum_since(plugin, since)
                .await?;
            if n >= u64::from(self.threshold) {
                let reason = format!("auto: {n} failures in {}", window_text(self.window));
                if disable(store, plugin, &reason).await? {
                    tracing::error!(plugin, failures = n, %reason, "plugin disabled on every node");
                    host.disable(plugin, &reason);
                }
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), u32>> {
        // Counts stay consistent even if a holder panicked: every update is one add.
        self.buffer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put an unwritten batch back (added to whatever came in meanwhile).
    fn restore(&self, batch: HashMap<(String, String), u32>) {
        let mut buffer = self.lock();
        for (key, n) in batch {
            let count = buffer.entry(key).or_insert(0);
            *count = count.saturating_add(n);
        }
    }
}

impl FailureSink for SharedFailures {
    fn record(&self, plugin: &str, hook: &str, _kind: FailureKind) {
        let mut buffer = self.lock();
        let count = buffer
            .entry((plugin.to_owned(), hook.to_owned()))
            .or_insert(0);
        *count = count.saturating_add(1);
    }
}

/// Adds every buffered count to its bucket, all in one transaction.
async fn write_counts(
    store: &dyn Store,
    batch: &HashMap<(String, String), u32>,
    bucket: Timestamp,
) -> Result<(), StoreError> {
    let tx = store.begin().await?;
    for ((plugin, hook), n) in batch {
        tx.plugin_failures().add(plugin, hook, bucket, *n).await?;
    }
    tx.commit().await
}

/// Disables `plugin` and appends `plugin.disabled` in one transaction; `true` only when
/// this call made the transition.
async fn disable(store: &dyn Store, plugin: &str, reason: &str) -> Result<bool, StoreError> {
    let tx = store.begin().await?;
    let changed = tx.plugins().auto_disable(plugin, reason).await?;
    if changed {
        tx.events().append(&plugin_disabled(plugin, reason)).await?;
    }
    tx.commit().await?;
    Ok(changed)
}

/// `now` floored to the minute.
fn bucket_start(now: Timestamp) -> Result<Timestamp, StoreError> {
    let micros = now.as_micros();
    Timestamp::from_micros(micros - micros.rem_euclid(BUCKET_MICROS))
        .map_err(|e| StoreError::Database(e.to_string()))
}

fn minus(now: Timestamp, window: Duration) -> Result<Timestamp, StoreError> {
    let window = i64::try_from(window.as_micros()).unwrap_or(i64::MAX);
    Timestamp::from_micros(now.as_micros().saturating_sub(window))
        .map_err(|e| StoreError::Database(e.to_string()))
}

/// `window` in the largest whole unit: `10m`, `1h`, `90s`, `250ms`.
fn window_text(window: Duration) -> String {
    let secs = window.as_secs();
    if window.subsec_nanos() != 0 || secs == 0 {
        format!("{}ms", window.as_millis())
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_text_uses_whole_units() {
        assert_eq!(window_text(Duration::from_secs(600)), "10m");
        assert_eq!(window_text(Duration::from_secs(7200)), "2h");
        assert_eq!(window_text(Duration::from_secs(90)), "90s");
        assert_eq!(window_text(Duration::from_millis(1500)), "1500ms");
        assert_eq!(window_text(Duration::ZERO), "0ms");
    }

    #[test]
    fn buckets_are_minute_starts() {
        let t = Timestamp::from_micros(3 * BUCKET_MICROS + 59_999_999).unwrap();
        assert_eq!(
            bucket_start(t).unwrap(),
            Timestamp::from_micros(3 * BUCKET_MICROS).unwrap()
        );
        let exact = Timestamp::from_micros(BUCKET_MICROS).unwrap();
        assert_eq!(bucket_start(exact).unwrap(), exact);
    }

    #[test]
    fn record_buffers_and_restore_merges() {
        let f = SharedFailures::new(0, Duration::from_secs(1));
        assert_eq!(f.threshold, 1);
        for _ in 0..3 {
            f.record("a", "render.head", FailureKind::Trap);
        }
        f.record("a", "content.body", FailureKind::Deadline);
        let batch = std::mem::take(&mut *f.lock());
        assert_eq!(batch[&("a".to_owned(), "render.head".to_owned())], 3);
        f.record("a", "render.head", FailureKind::Trap);
        f.restore(batch);
        let buffer = f.lock();
        assert_eq!(buffer[&("a".to_owned(), "render.head".to_owned())], 4);
        assert_eq!(buffer[&("a".to_owned(), "content.body".to_owned())], 1);
    }
}
