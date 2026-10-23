//! Shared plugin failure counters and the cluster-wide auto-disable (plan rulings P20,
//! R12).
//!
//! [`SharedFailures`] is the node's [`FailureSink`]: the plugin host reports every
//! failed call to it synchronously, and it only bumps an in-memory counter per
//! `(plugin, hook, row version)`. The row version is the plugin row's `updated_at` that
//! the node's [`PluginReloader`](super::PluginReloader) last committed (wired with
//! [`PluginReloader::with_failures`](super::PluginReloader::with_failures)).
//!
//! A flusher task ([`SharedFailures::spawn_flusher`], every [`FLUSH_EVERY`]) does this:
//!
//! 1. In one transaction it reads each buffered plugin's row and adds the counts to
//!    1-minute buckets of `plugin_failures` (bucket time from the database clock). Counts
//!    recorded against an older row version are dropped, and so are counts for a
//!    disabled or deleted row. A plugin enabled again (which clears its counters, and
//!    bumps `updated_at`) therefore never gets its pre-enable failures back.
//! 2. For each plugin it wrote, plus any whose last check errored, it reads the row's
//!    `updated_at` as an optimistic token, sums the buckets from the start of the bucket
//!    holding `now - window`, and at or above the threshold calls
//!    [`PluginRepo::auto_disable`](ferrite_store::PluginRepo::auto_disable) with that
//!    token.
//! 3. If that call made the transition, it appends `plugin.disabled` (keys
//!    `[name, reason]`) in the same transaction. Whichever nodes cross the threshold,
//!    exactly one event is emitted, and a re-enable racing the check wins. That node
//!    also disables the plugin in its host at once; the others do on the event.
//!
//! The host's own per-node counter keeps disabling locally as before.
//!
//! Limits (05 §8):
//! * The cluster-wide disable lags the threshold by up to one flush interval.
//! * A crash loses up to one interval of buffered counts.
//! * A commit that errors after it has in fact applied is retried with the same
//!   counts, which are then counted twice.
//!
//! Step 1 locks each buffered plugin's row (`get_for_update`, in name order) for its
//! transaction, so a re-enable, which clears the counters, waits for the flush and never
//! lands between the read and the add.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ferrite_core::events::plugin_disabled;
use ferrite_domain::Timestamp;
use ferrite_plugin_host::{FailureKind, FailureSink, PluginHost};
use ferrite_store::{Store, StoreError};
use futures::future::BoxFuture;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How often buffered failures are written (P20).
pub const FLUSH_EVERY: Duration = Duration::from_secs(5);

/// Width of a `plugin_failures` bucket.
const BUCKET_MICROS: i64 = 60_000_000;

/// `(plugin, hook, row version)`; the version is `None` when no reloader told us one.
type Key = (String, String, Option<Timestamp>);

/// Test seam run between the threshold check and the conditional disable.
type BeforeDisable =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), StoreError>> + Send + Sync>;

#[derive(Default)]
struct Inner {
    /// Failures not yet written.
    buffer: HashMap<Key, u32>,
    /// Row version (`updated_at`) of each plugin the reloader last committed.
    versions: HashMap<String, Timestamp>,
    /// Plugins whose last threshold check failed; checked again on the next flush.
    retry: BTreeSet<String>,
}

/// Buffers failure counts and shares them through the database (see the module docs).
pub struct SharedFailures {
    threshold: u32,
    window: Duration,
    inner: Mutex<Inner>,
    before_disable: Option<BeforeDisable>,
}

impl SharedFailures {
    /// Disable a plugin once it has `threshold` (`plugins.disable_after_failures`, at
    /// least 1) failures summed over `window` (`plugins.failure_window`), all nodes
    /// together.
    pub fn new(threshold: u32, window: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            window,
            inner: Mutex::new(Inner::default()),
            before_disable: None,
        }
    }

    /// Test seam: run `hook(plugin)` after a check found the threshold reached and
    /// before the conditional disable; an error counts as a failed check.
    #[doc(hidden)]
    pub fn with_before_disable(
        mut self,
        hook: impl Fn(String) -> BoxFuture<'static, Result<(), StoreError>> + Send + Sync + 'static,
    ) -> Self {
        self.before_disable = Some(Arc::new(hook));
        self
    }

    /// Record the row version (`updated_at`) of every plugin the reloader just
    /// committed. Buffered counts for a plugin whose version changed or that is gone
    /// are dropped (they were recorded against an older row).
    pub(crate) fn set_versions(&self, versions: HashMap<String, Timestamp>) {
        let mut inner = self.lock();
        inner
            .buffer
            .retain(|(plugin, _, version), _| match version {
                Some(v) => versions.get(plugin) == Some(v),
                None => true,
            });
        inner.versions = versions;
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

    /// Write the buffered counts, then check the written (and retried) plugins against
    /// the threshold.
    async fn flush(&self, store: &dyn Store, host: &PluginHost) -> Result<(), StoreError> {
        let (batch, retry) = {
            let mut inner = self.lock();
            (
                std::mem::take(&mut inner.buffer),
                std::mem::take(&mut inner.retry),
            )
        };
        if batch.is_empty() && retry.is_empty() {
            return Ok(());
        }
        let (now, written) = match self.write(store, &batch).await {
            Ok(done) => done,
            Err(e) => {
                self.restore(batch, retry);
                return Err(e);
            }
        };
        let since = window_start(now, self.window)?;
        for plugin in written.iter().chain(retry.iter()).collect::<BTreeSet<_>>() {
            if let Err(e) = self.check(store, host, plugin, since).await {
                tracing::warn!(plugin, error = %e, "plugin failure check failed; retried next flush");
                self.lock().retry.insert(plugin.clone());
            }
        }
        Ok(())
    }

    /// The database clock, and the plugins whose counts were written. Counts of a
    /// disabled or missing row, or recorded against an older row version, are dropped.
    async fn write(
        &self,
        store: &dyn Store,
        batch: &HashMap<Key, u32>,
    ) -> Result<(Timestamp, BTreeSet<String>), StoreError> {
        let now = store.read().nodes().db_now().await?;
        if batch.is_empty() {
            return Ok((now, BTreeSet::new()));
        }
        let bucket = bucket_start(now)?;
        let tx = store.begin().await?;
        // Lock each row (Postgres `FOR UPDATE`; SQLite: the Tx holds the single writer)
        // in name order, so two nodes flushing the same plugins cannot deadlock, and a
        // re-enable (which clears the counters) waits for this Tx instead of landing
        // between the read and the add.
        let plugins: BTreeSet<&str> = batch.keys().map(|(p, _, _)| p.as_str()).collect();
        let mut current: BTreeMap<&str, Option<Timestamp>> = BTreeMap::new();
        for plugin in plugins {
            let row = tx.plugins().get_for_update(plugin).await?;
            current.insert(plugin, row.filter(|r| r.enabled).map(|r| r.updated_at));
        }
        let mut written = BTreeSet::new();
        let mut dropped = 0u64;
        for ((plugin, hook, version), n) in batch {
            let row = current.get(plugin.as_str()).copied().flatten();
            let live = row.is_some_and(|at| version.is_none_or(|v| v == at));
            if live {
                tx.plugin_failures().add(plugin, hook, bucket, *n).await?;
                written.insert(plugin.clone());
            } else {
                dropped += u64::from(*n);
            }
        }
        tx.commit().await?;
        if dropped > 0 {
            tracing::debug!(
                dropped,
                "dropped failure counts of changed or disabled plugins"
            );
        }
        Ok((now, written))
    }

    /// Sum `plugin`'s failures since `since` and disable it at the threshold, unless its
    /// row changed after this check read it (R12).
    async fn check(
        &self,
        store: &dyn Store,
        host: &PluginHost,
        plugin: &str,
        since: Timestamp,
    ) -> Result<(), StoreError> {
        let Some(row) = store.read().plugins().get(plugin).await? else {
            return Ok(());
        };
        if !row.enabled {
            return Ok(());
        }
        let n = store
            .read()
            .plugin_failures()
            .sum_since(plugin, since)
            .await?;
        if n < u64::from(self.threshold) {
            return Ok(());
        }
        let reason = format!("auto: {n} failures in {}", window_text(self.window));
        if let Some(hook) = &self.before_disable {
            hook(plugin.to_owned()).await?;
        }
        if disable(store, plugin, &reason, row.updated_at).await? {
            tracing::error!(plugin, failures = n, %reason, "plugin disabled on every node");
            host.disable(plugin, &reason);
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // State stays consistent even if a holder panicked: every update is one step.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put an unwritten batch back (added to whatever came in meanwhile).
    fn restore(&self, batch: HashMap<Key, u32>, retry: BTreeSet<String>) {
        let mut inner = self.lock();
        for (key, n) in batch {
            let count = inner.buffer.entry(key).or_insert(0);
            *count = count.saturating_add(n);
        }
        inner.retry.extend(retry);
    }
}

impl FailureSink for SharedFailures {
    fn record(&self, plugin: &str, hook: &str, _kind: FailureKind) {
        let mut inner = self.lock();
        let version = inner.versions.get(plugin).copied();
        let count = inner
            .buffer
            .entry((plugin.to_owned(), hook.to_owned(), version))
            .or_insert(0);
        *count = count.saturating_add(1);
    }
}

/// Disables `plugin` (if its row is still at `observed`) and appends `plugin.disabled`
/// in one transaction; `true` only when this call made the transition.
async fn disable(
    store: &dyn Store,
    plugin: &str,
    reason: &str,
    observed: Timestamp,
) -> Result<bool, StoreError> {
    let tx = store.begin().await?;
    let changed = tx.plugins().auto_disable(plugin, reason, observed).await?;
    if changed {
        tx.events().append(&plugin_disabled(plugin, reason)).await?;
    }
    tx.commit().await?;
    Ok(changed)
}

/// Where the failure window starts: the start of the bucket holding `now - window`, so
/// a bucket that overlaps the window is counted whole (a window shorter than a bucket
/// still sees the current bucket).
fn window_start(now: Timestamp, window: Duration) -> Result<Timestamp, StoreError> {
    bucket_start(minus(now, window)?)
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
    fn window_start_is_floored_to_its_bucket() {
        // 45 s into minute 3 with a 30 s window: `now - window` is 15 s into minute 3,
        // and the failures counted so far this minute sit in the bucket starting at 3:00.
        let now = Timestamp::from_micros(3 * BUCKET_MICROS + 45_000_000).unwrap();
        let start = window_start(now, Duration::from_secs(30)).unwrap();
        assert_eq!(start, Timestamp::from_micros(3 * BUCKET_MICROS).unwrap());
        // A 10 min window reaches back into the bucket starting 10 minutes earlier.
        let now = Timestamp::from_micros(12 * BUCKET_MICROS + 45_000_000).unwrap();
        let start = window_start(now, Duration::from_secs(600)).unwrap();
        assert_eq!(start, Timestamp::from_micros(2 * BUCKET_MICROS).unwrap());
    }

    #[test]
    fn record_buffers_and_restore_merges() {
        let f = SharedFailures::new(0, Duration::from_secs(1));
        assert_eq!(f.threshold, 1);
        let key = |hook: &str, v: Option<Timestamp>| ("a".to_owned(), hook.to_owned(), v);
        for _ in 0..3 {
            f.record("a", "render.head", FailureKind::Trap);
        }
        f.record("a", "content.body", FailureKind::Deadline);
        let batch = std::mem::take(&mut f.lock().buffer);
        assert_eq!(batch[&key("render.head", None)], 3);
        f.record("a", "render.head", FailureKind::Trap);
        f.restore(batch, BTreeSet::from(["b".to_owned()]));
        let inner = f.lock();
        assert_eq!(inner.buffer[&key("render.head", None)], 4);
        assert_eq!(inner.buffer[&key("content.body", None)], 1);
        assert!(inner.retry.contains("b"));
    }

    #[test]
    fn new_versions_drop_counts_of_older_rows() {
        let f = SharedFailures::new(5, Duration::from_secs(60));
        let v1 = Timestamp::from_micros(1).unwrap();
        let v2 = Timestamp::from_micros(2).unwrap();
        f.set_versions(HashMap::from([("a".to_owned(), v1), ("b".to_owned(), v1)]));
        f.record("a", "render.head", FailureKind::Trap);
        f.record("b", "render.head", FailureKind::Trap);
        f.set_versions(HashMap::from([("a".to_owned(), v2), ("b".to_owned(), v1)]));
        f.record("a", "render.head", FailureKind::Trap);
        let inner = f.lock();
        let keys: BTreeSet<_> = inner.buffer.keys().cloned().collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                ("a".to_owned(), "render.head".to_owned(), Some(v2)),
                ("b".to_owned(), "render.head".to_owned(), Some(v1)),
            ])
        );
    }
}
