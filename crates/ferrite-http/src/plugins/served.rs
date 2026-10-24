//! Batched `request.served` (M2a-2 P21, design 05 §4a).
//!
//! The public handler [`record`](ServedBuffer::record)s one [`ServedEvent`] per page
//! response (cache hits included) into this node's bounded in-memory buffer: a push
//! under a lock held only for that push, never any I/O, so a request never waits for
//! the store or a plugin. Beyond [`SERVED_BUFFER_CAP`] buffered events new ones are
//! dropped and counted ([`dropped`](ServedBuffer::dropped)).
//!
//! A flusher task ([`spawn_flusher`](ServedBuffer::spawn_flusher)) empties the buffer
//! every [`SERVED_FLUSH_EVERY`], as soon as [`SERVED_FLUSH_AT`] events are buffered,
//! and once more on stop: in one transaction it enqueues one
//! [`plugin.action`](ferrite_core::jobs::PLUGIN_ACTION_KIND) job per enabled loaded
//! plugin of this node that subscribes to `request.served`
//! ([`PluginHost::subscribers`]) and per [`SERVED_FLUSH_AT`] events, with payload
//! `{"events":[…]}` and no subject. Without subscribers or events nothing is enqueued.
//! The plugin calls run later in the job runner (bounded, failures counted towards
//! auto-disable, row re-checked first: 04 §6), never inside a request.
//!
//! **Privacy.** An event holds the raw (percent-encoded) path without the query, cut to
//! [`MAX_SERVED_PATH`] bytes (F36), the status, the time, the lowercased host of a
//! single `Referer` header, a coarse [`UaClass`] and the [`CacheOutcome`]: never the
//! query string, an IP or its hash, a cookie, a header value, the full user agent or a
//! user id.
//!
//! **Loss.** Buffered events are lost when the node crashes, and so is a batch whose
//! enqueue fails (logged, counted as dropped). A clean shutdown flushes once after the
//! HTTP drain (`serve`'s background stop, inside the shutdown bound); the jobs run on
//! whichever node claims them next. Delivery after the enqueue is at least once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::http::{HeaderMap, Uri, header};
use ferrite_core::jobs::{PLUGIN_ACTION_VERSION, PluginAction, plugin_action_job};
use ferrite_domain::Timestamp;
use ferrite_plugin_host::PluginHost;
use ferrite_store::{Store, StoreError};
use serde::Serialize;
use serde_json::json;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The batched action hook.
pub const SERVED_HOOK: &str = "request.served";
/// How often the buffer is flushed (P21).
pub const SERVED_FLUSH_EVERY: Duration = Duration::from_secs(10);
/// Buffered events that trigger a flush before the interval; also the most events in
/// one job.
pub const SERVED_FLUSH_AT: usize = 1_000;
/// Most events buffered per node; beyond it new events are dropped.
pub const SERVED_BUFFER_CAP: usize = 10_000;
/// Longest recorded path in bytes (F36).
pub const MAX_SERVED_PATH: usize = 512;
/// Longest recorded referrer host (a DNS name's limit).
const MAX_REFERRER_HOST: usize = 253;

/// Coarse user-agent class: the full user agent is never recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UaClass {
    Browser,
    Bot,
    Other,
}

/// How the page cache answered the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheOutcome {
    /// Served from the cache: this request did not render the page.
    Hit,
    /// Rendered by this request (and stored, if cacheable).
    Miss,
    /// The cache was not consulted: a logged-in request under `cache.logged_in =
    /// "bypass"`, or a redirect answered before the cache.
    Bypass,
}

/// One public page response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServedEvent {
    /// Raw percent-encoded path, no query, at most [`MAX_SERVED_PATH`] bytes.
    pub path: String,
    pub status: u16,
    pub ts: Timestamp,
    /// Lowercased host of the `Referer` header (one header, at most 253 bytes).
    pub referrer_host: Option<String>,
    pub ua_class: UaClass,
    pub cache: CacheOutcome,
}

impl ServedEvent {
    /// The event for a response with `status` to a request for the raw path `path`
    /// (without its query) with `headers`, at the current time.
    pub fn new(path: &str, headers: &HeaderMap, status: u16, cache: CacheOutcome) -> Self {
        let ua = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map_or(UaClass::Other, ua_class);
        Self {
            path: served_path(path),
            status,
            ts: Timestamp::now(),
            referrer_host: referrer_host(headers),
            ua_class: ua,
            cache,
        }
    }
}

/// `bot` when the user agent contains `bot`, `crawl`, `spider` or `slurp` (ASCII case
/// insensitive), else `browser` when it starts with `Mozilla/`, else `other`.
pub fn ua_class(ua: &str) -> UaClass {
    let lower = ua.to_ascii_lowercase();
    if ["bot", "crawl", "spider", "slurp"]
        .iter()
        .any(|w| lower.contains(w))
    {
        UaClass::Bot
    } else if ua.starts_with("Mozilla/") {
        UaClass::Browser
    } else {
        UaClass::Other
    }
}

/// `path` cut to [`MAX_SERVED_PATH`] bytes (on a character boundary).
fn served_path(path: &str) -> String {
    let mut end = path.len().min(MAX_SERVED_PATH);
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    path[..end].to_owned()
}

/// The lowercased ASCII host of the request's only `Referer` header (an absolute URL);
/// `None` without one, with several, or when it has no usable host.
fn referrer_host(headers: &HeaderMap) -> Option<String> {
    let mut all = headers.get_all(header::REFERER).iter();
    let (Some(value), None) = (all.next(), all.next()) else {
        return None;
    };
    let uri: Uri = value.to_str().ok()?.parse().ok()?;
    let host = uri.host()?;
    (!host.is_empty() && host.len() <= MAX_REFERRER_HOST && host.is_ascii())
        .then(|| host.to_ascii_lowercase())
}

/// The node's `request.served` buffer (see the module docs).
pub struct ServedBuffer {
    store: Arc<dyn Store>,
    host: Arc<PluginHost>,
    events: Mutex<Vec<ServedEvent>>,
    dropped: AtomicU64,
    /// [`dropped`](Self::dropped) as last logged by the flusher.
    reported: AtomicU64,
    /// Woken when the buffer reaches [`SERVED_FLUSH_AT`].
    full: Notify,
}

impl ServedBuffer {
    /// A buffer whose flushes enqueue into `store` for `host`'s subscribers.
    pub fn new(store: Arc<dyn Store>, host: Arc<PluginHost>) -> Self {
        Self {
            store,
            host,
            events: Mutex::new(Vec::new()),
            dropped: AtomicU64::new(0),
            reported: AtomicU64::new(0),
            full: Notify::new(),
        }
    }

    /// Buffers `ev`; never waits for I/O. With [`SERVED_BUFFER_CAP`] events buffered
    /// it is dropped and counted instead.
    pub fn record(&self, ev: ServedEvent) {
        let len = {
            let mut events = self.lock();
            if events.len() >= SERVED_BUFFER_CAP {
                drop(events);
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            events.push(ev);
            events.len()
        };
        if len == SERVED_FLUSH_AT {
            self.full.notify_one();
        }
    }

    /// Events dropped so far (buffer full, or a failed enqueue).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Events buffered now.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no event is buffered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Empties the buffer into `plugin.action` jobs (one per subscriber and per
    /// [`SERVED_FLUSH_AT`] events, one transaction); returns how many were enqueued.
    /// Without subscribers the events are discarded. On error the batch is lost and
    /// counted as dropped.
    pub async fn flush(&self) -> Result<usize, StoreError> {
        let events = std::mem::take(&mut *self.lock());
        if events.is_empty() {
            return Ok(0);
        }
        let subscribers = self.host.subscribers(SERVED_HOOK);
        if subscribers.is_empty() {
            return Ok(0);
        }
        let n = events.len();
        match self.enqueue(&subscribers, events).await {
            Ok(jobs) => Ok(jobs),
            Err(e) => {
                self.dropped
                    .fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
                Err(e)
            }
        }
    }

    async fn enqueue(
        &self,
        subscribers: &[String],
        events: Vec<ServedEvent>,
    ) -> Result<usize, StoreError> {
        let tx = self.store.begin().await?;
        let mut jobs = 0;
        for chunk in events.chunks(SERVED_FLUSH_AT) {
            let payload = json!({ "events": chunk });
            for plugin in subscribers {
                let job = plugin_action_job(&PluginAction {
                    v: PLUGIN_ACTION_VERSION,
                    plugin: plugin.clone(),
                    hook: SERVED_HOOK.to_owned(),
                    subject: None,
                    payload: payload.clone(),
                });
                tx.jobs().enqueue(&job).await?;
                jobs += 1;
            }
        }
        tx.commit().await?;
        Ok(jobs)
    }

    async fn flush_logged(&self) {
        let dropped = self.dropped();
        let before = self.reported.swap(dropped, Ordering::Relaxed);
        if dropped > before {
            tracing::warn!(
                dropped = dropped - before,
                "request.served events dropped since the last flush"
            );
        }
        match self.flush().await {
            Ok(0) => {}
            Ok(jobs) => tracing::debug!(jobs, "flushed request.served events"),
            Err(e) => {
                tracing::warn!(error = %e, "flushing request.served events failed; batch dropped")
            }
        }
    }

    /// Flushes every `every` ([`SERVED_FLUSH_EVERY`] in `serve`) and whenever
    /// [`SERVED_FLUSH_AT`] events are buffered, until `stop`; then flushes once more.
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
                    () = self.full.notified() => self.flush_logged().await,
                    _ = tick.tick() => self.flush_logged().await,
                }
            }
            self.flush_logged().await;
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ServedEvent>> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn ua_classes() {
        for (ua, class) in [
            (
                "Mozilla/5.0 (X11; Linux x86_64) Firefox/131.0",
                UaClass::Browser,
            ),
            ("Mozilla/5.0 (compatible; Googlebot/2.1)", UaClass::Bot),
            ("Mozilla/5.0 (compatible; bingBOT/2.0)", UaClass::Bot),
            ("Baiduspider/2.0", UaClass::Bot),
            ("Mozilla/5.0 (compatible; Yahoo! Slurp)", UaClass::Bot),
            ("ia_archiver CRAWLER", UaClass::Bot),
            ("curl/8.4.0", UaClass::Other),
            ("mozilla/5.0", UaClass::Other),
            ("", UaClass::Other),
        ] {
            assert_eq!(ua_class(ua), class, "{ua:?}");
        }
    }

    #[test]
    fn event_fields() {
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, "Mozilla/5.0 Safari".parse().unwrap());
        headers.insert(
            header::REFERER,
            "https://News.Example.COM/a?b=c".parse().unwrap(),
        );
        let ev = ServedEvent::new("/caf%C3%A9/%00", &headers, 200, CacheOutcome::Miss);
        assert_eq!(ev.path, "/caf%C3%A9/%00");
        assert_eq!(ev.referrer_host.as_deref(), Some("news.example.com"));
        assert_eq!(ev.ua_class, UaClass::Browser);
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["cache"], "miss");
        assert_eq!(json["ua_class"], "browser");

        // Several Referer headers, or a relative one: no host. No user agent: other.
        let mut two = HeaderMap::new();
        for r in ["https://a.example/", "https://b.example/"] {
            two.append(header::REFERER, r.parse().unwrap());
        }
        let ev = ServedEvent::new("/", &two, 404, CacheOutcome::Hit);
        assert_eq!(ev.referrer_host, None);
        assert_eq!(ev.ua_class, UaClass::Other);
        let mut rel = HeaderMap::new();
        rel.insert(header::REFERER, "/local".parse().unwrap());
        assert_eq!(referrer_host(&rel), None);
    }

    #[test]
    fn long_paths_are_cut() {
        let long = format!("/{}", "a".repeat(600));
        assert_eq!(served_path(&long).len(), MAX_SERVED_PATH);
        let multi = format!("/{}é", "a".repeat(510));
        assert_eq!(served_path(&multi), format!("/{}", "a".repeat(510)));
    }
}
