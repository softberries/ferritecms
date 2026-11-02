//! The node's metrics registry and `/metrics` (M4-2 P34): hand-rolled, no dependency.
//!
//! **Per request** (the `track` layer of every site response): one counter of
//! `ferrite_http_requests_total{class,status}` and one bucket of
//! `ferrite_http_request_duration_seconds{class}`, both plain atomics in fixed arrays
//! indexed by [`RouteClass`] and the status class, so the counting takes no lock and
//! allocates nothing (the axum `from_fn` layer that calls it boxes the inner future, one
//! allocation per request, as the rest of the stack already does). The labels are
//! `&'static str`s. A response below 200 (none today)
//! is not counted. **Page views** ([`Metrics::count_page_view`]): the public handler
//! counts a `GET` of an HTML page answered `200`, or `304` when the browser revalidated
//! its stored copy (M4-2 R8), that is neither a preview nor a logged-in request and whose
//! `User-Agent` does not match [`is_bot`]; nothing per visitor is kept.
//!
//! **At scrape time** ([`Metrics::render`], Prometheus text format 0.0.4), read from the
//! sources the registry was given: the page cache (hits and misses since it was built,
//! entries and bytes now), the plugin host's per-plugin, per-hook call statistics
//! (labels are the plugin's name and the hook's; every route call is the hook `route`,
//! so no request path becomes a label, and the host tracks at most
//! [`MAX_STATS_HOOKS`](ferrite_plugin_host::MAX_STATS_HOOKS) hooks per plugin), and from
//! the store the job counts per status (one `GROUP BY`), the live nodes (heartbeat within
//! the stale window of the database clock) and the pool connections. A store read that
//! fails or takes over [`SCRAPE_DB_TIMEOUT`] leaves its family out of that scrape (logged).
//!
//! **[`Metrics::snapshot`]**: cumulative counters for the per-minute `node_metrics` rows
//! (Task 8 diffs two snapshots with [`MetricsSnapshot::since`]).
//!
//! The registry is served only by [`metrics_router`] on `server.metrics_listen` (no
//! auth; never on the public listener).

use std::{
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Response, StatusCode, header},
    middleware::Next,
    routing::get,
};
use ferrite_cache::PageCache;
use ferrite_domain::{MetricCounts, PAGE_LATENCY_SLOTS, Timestamp};
use ferrite_plugin_host::{DURATION_BUCKETS_MS, HookStats, PluginHost};
use ferrite_store::Store;

use crate::plugins::is_preview_path;

/// `Content-Type` of a scrape (Prometheus text format 0.0.4).
pub const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Upper bounds (seconds) of the request latency buckets; one more bucket holds the rest.
pub const LATENCY_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Buckets of a request latency histogram: [`LATENCY_BUCKETS`] plus the overflow one
/// (the same buckets as the `node_metrics` page latency histogram).
pub const LATENCY_SLOTS: usize = PAGE_LATENCY_SLOTS;

/// The longest a scrape waits for each store read.
pub const SCRAPE_DB_TIMEOUT: Duration = Duration::from_secs(2);

/// What part of the site a request is for (the `class` label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteClass {
    /// The public site: pages (content, archives, feeds, sitemaps, redirects, 404s),
    /// the theme comment form (`POST /_comment`) and other methods there (405s), so
    /// their 5xx errors show.
    Page,
    /// `/api/v1`.
    Api,
    /// The admin SPA, `/admin` and `/admin/…`.
    Admin,
    /// Asset URLs: `/_media/…` and `/_theme/…`.
    Media,
    /// Plugin routes, `/_p/…`.
    Plugin,
    /// `/_preview/…` (first segment `_preview` in any case or encoding).
    Preview,
    /// The OAuth authorization server: `/oauth/…` and the
    /// `/.well-known/oauth-protected-resource…` and `/.well-known/oauth-authorization-server…`
    /// metadata documents.
    OAuth,
    /// The MCP endpoint, `/mcp` (and anything under it).
    Mcp,
    /// Health checks (`/healthz`, `/readyz`): load-balancer noise, left out of the
    /// dashboard's requests (ruling R3).
    Other,
}

impl RouteClass {
    /// Every class, in label order.
    pub const ALL: [RouteClass; 9] = [
        RouteClass::Page,
        RouteClass::Api,
        RouteClass::Admin,
        RouteClass::Media,
        RouteClass::Plugin,
        RouteClass::Preview,
        RouteClass::OAuth,
        RouteClass::Mcp,
        RouteClass::Other,
    ];

    /// The class of a request for `path` (raw, no query); every method of a path has
    /// the same class.
    pub fn of(path: &str) -> Self {
        let under = |prefix: &str| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        if under("/api/v1") {
            RouteClass::Api
        } else if under("/admin") {
            RouteClass::Admin
        } else if path.starts_with("/_media/") || path.starts_with("/_theme/") {
            RouteClass::Media
        } else if under("/_p") {
            RouteClass::Plugin
        } else if is_preview_path(path) {
            RouteClass::Preview
        } else if under("/oauth")
            || path.starts_with("/.well-known/oauth-protected-resource")
            || path.starts_with("/.well-known/oauth-authorization-server")
        {
            RouteClass::OAuth
        } else if under("/mcp") {
            RouteClass::Mcp
        } else if path == "/healthz" || path == "/readyz" {
            RouteClass::Other
        } else {
            RouteClass::Page
        }
    }

    /// The label value.
    pub fn as_str(self) -> &'static str {
        match self {
            RouteClass::Page => "page",
            RouteClass::Api => "api",
            RouteClass::Admin => "admin",
            RouteClass::Media => "media",
            RouteClass::Plugin => "plugin",
            RouteClass::Preview => "preview",
            RouteClass::OAuth => "oauth",
            RouteClass::Mcp => "mcp",
            RouteClass::Other => "other",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The `status` label values, indexed by `status / 100 - 2`.
const STATUSES: [&str; 4] = ["2xx", "3xx", "4xx", "5xx"];

/// Whether `ua` looks like a bot for page views: it contains `bot`, `crawler`,
/// `spider`, `slurp` or `preview` in any ASCII case.
/// Allocates nothing.
pub fn is_bot(ua: &str) -> bool {
    let ua = ua.as_bytes();
    ["bot", "crawler", "spider", "slurp", "preview"]
        .iter()
        .any(|w| {
            ua.windows(w.len())
                .any(|window| window.eq_ignore_ascii_case(w.as_bytes()))
        })
}

/// A fixed-bucket latency histogram: per-bucket counts (not cumulative over `le`) and
/// the summed microseconds.
#[derive(Default)]
struct Histogram {
    buckets: [AtomicU64; LATENCY_SLOTS],
    sum_micros: AtomicU64,
}

impl Histogram {
    fn observe(&self, elapsed: Duration) {
        let secs = elapsed.as_secs_f64();
        let slot = LATENCY_BUCKETS
            .iter()
            .position(|b| secs <= *b)
            .unwrap_or(LATENCY_BUCKETS.len());
        self.buckets[slot].fetch_add(1, Ordering::Relaxed);
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);
    }

    fn counts(&self) -> [u64; LATENCY_SLOTS] {
        std::array::from_fn(|i| self.buckets[i].load(Ordering::Relaxed))
    }
}

/// Cumulative counters of a node since its registry was built (the page cache's and the
/// plugin host's since they were built: the same process). Two snapshots a minute apart
/// give that minute's figures ([`since`](Self::since)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Responses of every class but [`RouteClass::Other`] (`/healthz` and `/readyz`:
    /// load-balancer probes would swamp a quiet site's figures; M4-2 R3). `/metrics`
    /// still counts every class.
    pub requests: u64,
    /// Those of [`requests`](Self::requests) with a 5xx status.
    pub errors_5xx: u64,
    pub page_views: u64,
    /// Page cache lookups that found a valid page, and those that did not.
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// Plugin calls that ran, and those that failed (every plugin and hook).
    pub plugin_calls: u64,
    pub plugin_failures: u64,
    /// Jobs this node's runner completed, and attempts that failed (retry or permanent);
    /// 0 without a job counter source.
    pub jobs_done: u64,
    pub jobs_failed: u64,
    /// Page-class responses by latency bucket: index `i < 11` counts the responses that
    /// took at most [`LATENCY_BUCKETS`]`[i]` seconds and more than the bound before it;
    /// the last index counts those over 10 s. Not cumulative over the bounds.
    pub page_latency: [u64; LATENCY_SLOTS],
}

impl MetricsSnapshot {
    /// The counts between `earlier` and this snapshot (each field saturates at 0).
    pub fn since(&self, earlier: &MetricsSnapshot) -> MetricsSnapshot {
        MetricsSnapshot {
            requests: self.requests.saturating_sub(earlier.requests),
            errors_5xx: self.errors_5xx.saturating_sub(earlier.errors_5xx),
            page_views: self.page_views.saturating_sub(earlier.page_views),
            cache_hits: self.cache_hits.saturating_sub(earlier.cache_hits),
            cache_misses: self.cache_misses.saturating_sub(earlier.cache_misses),
            plugin_calls: self.plugin_calls.saturating_sub(earlier.plugin_calls),
            plugin_failures: self.plugin_failures.saturating_sub(earlier.plugin_failures),
            jobs_done: self.jobs_done.saturating_sub(earlier.jobs_done),
            jobs_failed: self.jobs_failed.saturating_sub(earlier.jobs_failed),
            page_latency: std::array::from_fn(|i| {
                self.page_latency[i].saturating_sub(earlier.page_latency[i])
            }),
        }
    }

    /// These counts as a `node_metrics` row (P35; meant for a delta from
    /// [`since`](Self::since)), with the page latency histogram (M4-2 R7).
    pub fn counts(&self) -> MetricCounts {
        MetricCounts {
            requests: self.requests,
            errors_5xx: self.errors_5xx,
            page_views: self.page_views,
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            plugin_calls: self.plugin_calls,
            plugin_failures: self.plugin_failures,
            jobs_done: self.jobs_done,
            jobs_failed: self.jobs_failed,
            page_latency: self.page_latency,
        }
    }
}

/// Completed jobs and failed attempts, read at snapshot time (the job runner's counters).
pub type JobTotals = Box<dyn Fn() -> (u64, u64) + Send + Sync>;

/// The node's registry; see the module docs. Built once per node, shared as an `Arc`.
pub struct Metrics {
    version: String,
    target: String,
    requests: [[AtomicU64; STATUSES.len()]; RouteClass::ALL.len()],
    durations: [Histogram; RouteClass::ALL.len()],
    page_views: AtomicU64,
    cache: Option<Arc<PageCache>>,
    plugins: Option<Arc<PluginHost>>,
    store: Option<(Arc<dyn Store>, Duration)>,
    jobs: Option<JobTotals>,
}

impl Metrics {
    /// An empty registry of a binary `version` built for `target` (`ferrite_build_info`).
    pub fn new(version: &str, target: &str) -> Self {
        Self {
            version: version.to_owned(),
            target: target.to_owned(),
            requests: Default::default(),
            durations: Default::default(),
            page_views: AtomicU64::new(0),
            cache: None,
            plugins: None,
            store: None,
            jobs: None,
        }
    }

    /// Reads the `ferrite_page_cache_*` families from `cache`.
    pub fn with_cache(mut self, cache: Arc<PageCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Reads the `ferrite_plugin_hook_*` families from `plugins`.
    pub fn with_plugins(mut self, plugins: Arc<PluginHost>) -> Self {
        self.plugins = Some(plugins);
        self
    }

    /// Reads `ferrite_jobs`, `ferrite_nodes` and `ferrite_db_pool_connections` from
    /// `store`; a node counts as live while its heartbeat is at most `stale_after` older
    /// than the database clock.
    pub fn with_store(mut self, store: Arc<dyn Store>, stale_after: Duration) -> Self {
        self.store = Some((store, stale_after));
        self
    }

    /// Reads the snapshot's `jobs_done` and `jobs_failed` from `totals`.
    pub fn with_job_totals(
        mut self,
        totals: impl Fn() -> (u64, u64) + Send + Sync + 'static,
    ) -> Self {
        self.jobs = Some(Box::new(totals));
        self
    }

    /// Counts one response of `class` with `status` that took `elapsed`. A status below
    /// 200 or above 599 is not counted.
    pub fn record(&self, class: RouteClass, status: StatusCode, elapsed: Duration) {
        let Some(slot) = (status.as_u16() / 100)
            .checked_sub(2)
            .map(usize::from)
            .filter(|s| *s < STATUSES.len())
        else {
            return;
        };
        self.requests[class.index()][slot].fetch_add(1, Ordering::Relaxed);
        self.durations[class.index()].observe(elapsed);
    }

    /// Counts one page view (the caller applies the rule; see the module docs).
    pub fn count_page_view(&self) {
        self.page_views.fetch_add(1, Ordering::Relaxed);
    }

    /// The cumulative counters now; see [`MetricsSnapshot`].
    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut requests = 0;
        let mut errors_5xx = 0;
        for class in RouteClass::ALL
            .into_iter()
            .filter(|c| *c != RouteClass::Other)
        {
            for (i, n) in self.requests[class.index()].iter().enumerate() {
                let n = n.load(Ordering::Relaxed);
                requests += n;
                if STATUSES[i] == "5xx" {
                    errors_5xx += n;
                }
            }
        }
        let (cache_hits, cache_misses) = self.cache.as_ref().map_or((0, 0), |c| c.lookups());
        let (plugin_calls, plugin_failures) = self.plugins.as_ref().map_or((0, 0), |p| p.totals());
        let (jobs_done, jobs_failed) = self.jobs.as_ref().map_or((0, 0), |f| f());
        MetricsSnapshot {
            requests,
            errors_5xx,
            page_views: self.page_views.load(Ordering::Relaxed),
            cache_hits,
            cache_misses,
            plugin_calls,
            plugin_failures,
            jobs_done,
            jobs_failed,
            page_latency: self.durations[RouteClass::Page.index()].counts(),
        }
    }

    /// The whole registry in Prometheus text format 0.0.4 (see the module docs for what
    /// is read now).
    pub async fn render(&self) -> String {
        let mut out = Text::default();
        out.family(
            "ferrite_build_info",
            "gauge",
            "The running binary: version and target triple (always 1).",
        );
        out.sample(
            "ferrite_build_info",
            &[("version", &self.version), ("target", &self.target)],
            1,
        );

        out.family(
            "ferrite_http_requests_total",
            "counter",
            "Responses by route class and status class.",
        );
        for class in RouteClass::ALL {
            for (i, status) in STATUSES.iter().enumerate() {
                let n = self.requests[class.index()][i].load(Ordering::Relaxed);
                out.sample(
                    "ferrite_http_requests_total",
                    &[("class", class.as_str()), ("status", status)],
                    n,
                );
            }
        }
        out.family(
            "ferrite_http_request_duration_seconds",
            "histogram",
            "Time to the response headers by route class.",
        );
        for class in RouteClass::ALL {
            let h = &self.durations[class.index()];
            out.histogram(
                "ferrite_http_request_duration_seconds",
                &[("class", class.as_str())],
                &LATENCY_BUCKETS,
                &h.counts(),
                h.sum_micros.load(Ordering::Relaxed),
            );
        }

        if let Some(cache) = &self.cache {
            let stats = cache.stats().await;
            for (name, kind, help, value) in [
                (
                    "ferrite_page_cache_hits_total",
                    "counter",
                    "Page cache lookups that found a valid page.",
                    stats.hits,
                ),
                (
                    "ferrite_page_cache_misses_total",
                    "counter",
                    "Page cache lookups that found none.",
                    stats.misses,
                ),
                (
                    "ferrite_page_cache_entries",
                    "gauge",
                    "Pages in the page cache.",
                    stats.entries,
                ),
                (
                    "ferrite_page_cache_bytes",
                    "gauge",
                    "Weight of the page cache's entries in bytes.",
                    stats.bytes,
                ),
            ] {
                out.family(name, kind, help);
                out.sample(name, &[], value);
            }
        }

        if let Some(plugins) = &self.plugins {
            render_plugins(&mut out, &plugins.all_call_stats());
        }

        if let Some((store, stale_after)) = &self.store {
            render_store(&mut out, store.as_ref(), *stale_after).await;
        }

        out.family(
            "ferrite_page_views_total",
            "counter",
            "Successful GETs of public HTML pages, without bots, previews and logged-in requests.",
        );
        out.sample(
            "ferrite_page_views_total",
            &[],
            self.page_views.load(Ordering::Relaxed),
        );
        out.0
    }
}

/// One plugin hook's figures, route calls merged into the hook `route`.
#[derive(Default)]
struct HookFigures {
    calls: u64,
    failures: u64,
    total_micros: u64,
    buckets: [u64; DURATION_BUCKETS_MS.len() + 1],
}

/// The hook label of `hook`: `route` for every route call (`route <METHOD> <path>`),
/// else the hook's own name.
fn hook_label(hook: &str) -> &str {
    if hook.starts_with("route ") {
        "route"
    } else {
        hook
    }
}

fn render_plugins(out: &mut Text, all: &[(String, Vec<HookStats>)]) {
    let merged: Vec<(&str, Vec<(&str, HookFigures)>)> = all
        .iter()
        .map(|(plugin, hooks)| {
            let mut figures: Vec<(&str, HookFigures)> = Vec::new();
            for h in hooks {
                let label = hook_label(&h.hook);
                let at = match figures.iter().position(|(l, _)| *l == label) {
                    Some(i) => i,
                    None => {
                        figures.push((label, HookFigures::default()));
                        figures.len() - 1
                    }
                };
                let f = &mut figures[at].1;
                f.calls += h.calls;
                f.failures += h.failures;
                f.total_micros = f.total_micros.saturating_add(h.total_micros);
                for (sum, n) in f.buckets.iter_mut().zip(h.buckets) {
                    *sum += n;
                }
            }
            figures.sort_by(|a, b| a.0.cmp(b.0));
            (plugin.as_str(), figures)
        })
        .collect();
    out.family(
        "ferrite_plugin_hook_calls_total",
        "counter",
        "Plugin calls that ran, by plugin and hook (route calls are the hook \"route\").",
    );
    for (plugin, hooks) in &merged {
        for (hook, f) in hooks {
            out.sample(
                "ferrite_plugin_hook_calls_total",
                &[("plugin", plugin), ("hook", hook)],
                f.calls,
            );
        }
    }
    out.family(
        "ferrite_plugin_hook_failures_total",
        "counter",
        "Plugin calls that failed (counted toward auto-disable), by plugin and hook.",
    );
    for (plugin, hooks) in &merged {
        for (hook, f) in hooks {
            out.sample(
                "ferrite_plugin_hook_failures_total",
                &[("plugin", plugin), ("hook", hook)],
                f.failures,
            );
        }
    }
    out.family(
        "ferrite_plugin_hook_duration_seconds",
        "histogram",
        "Duration of the plugin calls that ran, by plugin and hook.",
    );
    let bounds: Vec<f64> = DURATION_BUCKETS_MS
        .iter()
        .map(|ms| *ms as f64 / 1000.0)
        .collect();
    for (plugin, hooks) in &merged {
        for (hook, f) in hooks {
            out.histogram(
                "ferrite_plugin_hook_duration_seconds",
                &[("plugin", plugin), ("hook", hook)],
                &bounds,
                &f.buckets,
                f.total_micros,
            );
        }
    }
}

async fn render_store(out: &mut Text, store: &dyn Store, stale_after: Duration) {
    match tokio::time::timeout(SCRAPE_DB_TIMEOUT, store.read().jobs().counts()).await {
        Ok(Ok(counts)) => {
            out.family("ferrite_jobs", "gauge", "Jobs of the site by status.");
            for (status, n) in counts {
                out.sample("ferrite_jobs", &[("status", status.as_str())], n);
            }
        }
        Ok(Err(e)) => tracing::warn!(error = %e, "metrics: cannot count the jobs"),
        Err(_) => tracing::warn!("metrics: counting the jobs timed out"),
    }

    let pool = store.pool_stats();
    out.family(
        "ferrite_db_pool_connections",
        "gauge",
        "This node's pooled database connections by state.",
    );
    out.sample(
        "ferrite_db_pool_connections",
        &[("state", "idle")],
        pool.idle,
    );
    out.sample(
        "ferrite_db_pool_connections",
        &[("state", "used")],
        pool.used,
    );

    let nodes = async {
        let repo = store.read().nodes();
        let now = repo.db_now().await?;
        let nodes = repo.list().await?;
        Ok::<_, ferrite_store::StoreError>(live_nodes(&nodes, now, stale_after))
    };
    match tokio::time::timeout(SCRAPE_DB_TIMEOUT, nodes).await {
        Ok(Ok(n)) => {
            out.family(
                "ferrite_nodes",
                "gauge",
                "Live nodes of the site (heartbeat within the stale window).",
            );
            out.sample("ferrite_nodes", &[], n);
        }
        Ok(Err(e)) => tracing::warn!(error = %e, "metrics: cannot list the nodes"),
        Err(_) => tracing::warn!("metrics: listing the nodes timed out"),
    }
}

/// Nodes whose heartbeat is at most `stale_after` older than `now`.
fn live_nodes(nodes: &[ferrite_domain::Node], now: Timestamp, stale_after: Duration) -> u64 {
    let window = i64::try_from(stale_after.as_micros()).unwrap_or(i64::MAX);
    nodes
        .iter()
        .filter(|n| now.as_micros().saturating_sub(n.heartbeat_at.as_micros()) <= window)
        .count() as u64
}

/// Prometheus text being written.
#[derive(Default)]
struct Text(String);

impl Text {
    fn family(&mut self, name: &str, kind: &str, help: &str) {
        let _ = writeln!(self.0, "# HELP {name} {help}");
        let _ = writeln!(self.0, "# TYPE {name} {kind}");
    }

    fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: u64) {
        self.0.push_str(name);
        self.labels(labels, None);
        let _ = writeln!(self.0, " {value}");
    }

    fn labels(&mut self, labels: &[(&str, &str)], le: Option<&str>) {
        if labels.is_empty() && le.is_none() {
            return;
        }
        self.0.push('{');
        let mut first = true;
        for (k, v) in labels.iter().copied().chain(le.map(|le| ("le", le))) {
            if !first {
                self.0.push(',');
            }
            first = false;
            self.0.push_str(k);
            self.0.push_str("=\"");
            for c in v.chars() {
                match c {
                    '\\' => self.0.push_str("\\\\"),
                    '"' => self.0.push_str("\\\""),
                    '\n' => self.0.push_str("\\n"),
                    c => self.0.push(c),
                }
            }
            self.0.push('"');
        }
        self.0.push('}');
    }

    /// `_bucket` (cumulative, `+Inf` last), `_sum` (seconds) and `_count` of one series;
    /// `counts` has one entry per bound plus the overflow one.
    fn histogram(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        bounds: &[f64],
        counts: &[u64],
        sum_micros: u64,
    ) {
        let bucket = format!("{name}_bucket");
        let mut cumulative = 0u64;
        for (i, n) in counts.iter().enumerate() {
            cumulative += n;
            let le = bounds
                .get(i)
                .map_or_else(|| "+Inf".to_owned(), |b| b.to_string());
            self.0.push_str(&bucket);
            self.labels(labels, Some(&le));
            let _ = writeln!(self.0, " {cumulative}");
        }
        self.0.push_str(name);
        self.0.push_str("_sum");
        self.labels(labels, None);
        let _ = writeln!(self.0, " {}", sum_micros as f64 / 1_000_000.0);
        self.0.push_str(name);
        self.0.push_str("_count");
        self.labels(labels, None);
        let _ = writeln!(self.0, " {cumulative}");
    }
}

/// The site layer counting every response into `metrics` (class from the request,
/// status class from the response, the time until the response headers).
pub(crate) async fn track(
    State(metrics): State<Arc<Metrics>>,
    req: Request,
    next: Next,
) -> Response<Body> {
    let class = RouteClass::of(req.uri().path());
    let started = Instant::now();
    let res = next.run(req).await;
    metrics.record(class, res.status(), started.elapsed());
    res
}

/// The metrics listener's app: `GET /metrics` (and HEAD) only; any other path is `404`,
/// another method on `/metrics` is `405`. No auth: bind it to a private address.
pub fn metrics_router(metrics: Arc<Metrics>) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state(metrics)
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found\n") })
}

async fn scrape(State(metrics): State<Arc<Metrics>>) -> Response<Body> {
    let mut res = Response::new(Body::from(metrics.render().await));
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(METRICS_CONTENT_TYPE),
    );
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;
    use ferrite_domain::PAGE_LATENCY_BOUNDS_MS;

    #[test]
    fn bounds_in_ms_match_the_buckets() {
        for (ms, s) in PAGE_LATENCY_BOUNDS_MS.iter().zip(LATENCY_BUCKETS) {
            assert!((f64::from(*ms) / 1000.0 - s).abs() < 1e-9, "{ms} {s}");
        }
    }

    #[test]
    fn snapshot_counts_carry_every_field() {
        let mut latency = [0u64; LATENCY_SLOTS];
        latency[3] = 4;
        let s = MetricsSnapshot {
            requests: 1,
            errors_5xx: 2,
            page_views: 3,
            cache_hits: 4,
            cache_misses: 5,
            plugin_calls: 6,
            plugin_failures: 7,
            jobs_done: 8,
            jobs_failed: 9,
            page_latency: latency,
        };
        assert_eq!(
            s.counts(),
            MetricCounts::from_counters([1, 2, 3, 4, 5, 6, 7, 8, 9], latency)
        );
    }

    #[test]
    fn classes() {
        let get = Method::GET;
        for (method, path, class) in [
            (&get, "/", RouteClass::Page),
            (&get, "/hello/", RouteClass::Page),
            (&get, "/feed/", RouteClass::Page),
            (&get, "/apiary/", RouteClass::Page),
            (&get, "/api/v1", RouteClass::Api),
            (&get, "/api/v1/content", RouteClass::Api),
            (&get, "/api/v10", RouteClass::Page),
            (&get, "/admin", RouteClass::Admin),
            (&get, "/admin/content", RouteClass::Admin),
            (&get, "/administer", RouteClass::Page),
            (&get, "/_media/ab/x.png", RouteClass::Media),
            (&get, "/_theme/ab/style.css", RouteClass::Media),
            (&get, "/_p/router/hello", RouteClass::Plugin),
            (&get, "/_p", RouteClass::Plugin),
            (&Method::POST, "/_p/router/echo", RouteClass::Plugin),
            (&get, "/_preview/x", RouteClass::Preview),
            (&get, "/%5FPreview/x", RouteClass::Preview),
            (&get, "/oauth/authorize", RouteClass::OAuth),
            (&Method::POST, "/oauth/token", RouteClass::OAuth),
            (&get, "/oauth", RouteClass::OAuth),
            (&get, "/oauthx", RouteClass::Page),
            (&Method::POST, "/mcp", RouteClass::Mcp),
            (&get, "/mcp", RouteClass::Mcp),
            (&get, "/mcp/x", RouteClass::Mcp),
            (&get, "/mcpx", RouteClass::Page),
            (
                &get,
                "/.well-known/oauth-protected-resource/mcp",
                RouteClass::OAuth,
            ),
            (
                &get,
                "/.well-known/oauth-authorization-server",
                RouteClass::OAuth,
            ),
            (&get, "/.well-known/security.txt", RouteClass::Page),
            (&get, "/healthz", RouteClass::Other),
            (&get, "/readyz", RouteClass::Other),
            (&Method::POST, "/healthz", RouteClass::Other),
            (&Method::POST, "/_comment", RouteClass::Page),
            (&Method::POST, "/", RouteClass::Page),
            (&Method::HEAD, "/", RouteClass::Page),
        ] {
            assert_eq!(RouteClass::of(path), class, "{method} {path}");
        }
    }

    #[test]
    fn bots() {
        for ua in [
            "Googlebot/2.1",
            "SomeCrawler",
            "Baiduspider",
            "Yahoo! Slurp",
            "Slack link PREVIEW",
            "BOT",
            "x-bOt-y",
        ] {
            assert!(is_bot(ua), "{ua}");
        }
        for ua in [
            "Mozilla/5.0 Firefox/131.0",
            "curl/8.4.0",
            "",
            "bo",
            "Zażółć",
        ] {
            assert!(!is_bot(ua), "{ua}");
        }
    }

    #[test]
    fn statuses_and_latencies_land_in_their_slots() {
        let m = Metrics::new("1.0.0", "t");
        let ms = Duration::from_millis;
        m.record(RouteClass::Page, StatusCode::OK, ms(3));
        m.record(RouteClass::Page, StatusCode::OK, ms(5));
        m.record(RouteClass::Page, StatusCode::NOT_FOUND, ms(6));
        m.record(
            RouteClass::Page,
            StatusCode::BAD_GATEWAY,
            Duration::from_secs(11),
        );
        m.record(RouteClass::Api, StatusCode::INTERNAL_SERVER_ERROR, ms(1));
        m.record(RouteClass::Api, StatusCode::SWITCHING_PROTOCOLS, ms(1));
        // Health checks and other `other` requests stay out of the snapshot (R3).
        m.record(RouteClass::Other, StatusCode::OK, ms(1));
        m.record(RouteClass::Other, StatusCode::SERVICE_UNAVAILABLE, ms(1));
        let s = m.snapshot();
        assert_eq!((s.requests, s.errors_5xx), (5, 2));
        assert_eq!(s.page_latency, [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let later = {
            m.record(RouteClass::Page, StatusCode::OK, ms(40));
            m.snapshot()
        };
        let d = later.since(&s);
        assert_eq!(d.requests, 1);
        assert_eq!(d.page_latency[3], 1);
        assert_eq!(s.since(&later).requests, 0, "saturates");
    }

    #[test]
    fn labels_are_escaped_and_histograms_cumulative() {
        let mut t = Text::default();
        t.sample("x", &[("a", "q\"b\\c\nd")], 1);
        assert_eq!(t.0, "x{a=\"q\\\"b\\\\c\\nd\"} 1\n");
        let mut t = Text::default();
        t.histogram("h", &[("k", "v")], &[0.5, 1.0], &[1, 2, 3], 1_500_000);
        assert_eq!(
            t.0,
            "h_bucket{k=\"v\",le=\"0.5\"} 1\nh_bucket{k=\"v\",le=\"1\"} 3\n\
             h_bucket{k=\"v\",le=\"+Inf\"} 6\nh_sum{k=\"v\"} 1.5\nh_count{k=\"v\"} 6\n"
        );
    }

    #[test]
    fn route_hooks_are_one_label() {
        assert_eq!(hook_label("route GET /hello"), "route");
        assert_eq!(hook_label("render.head"), "render.head");
        let stats = |hook: &str, calls| HookStats {
            hook: hook.to_owned(),
            calls,
            buckets: [calls, 0, 0, 0, 0, 0],
            ..HookStats::default()
        };
        let mut t = Text::default();
        render_plugins(
            &mut t,
            &[(
                "p".to_owned(),
                vec![
                    stats("render.head", 1),
                    stats("route GET /a", 2),
                    stats("route POST /b", 3),
                ],
            )],
        );
        assert!(
            t.0.contains("ferrite_plugin_hook_calls_total{plugin=\"p\",hook=\"route\"} 5\n"),
            "{}",
            t.0
        );
        assert!(!t.0.contains("/a"), "{}", t.0);
    }

    #[tokio::test]
    async fn the_metrics_app_serves_get_metrics_only() {
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let metrics = Arc::new(Metrics::new("1.2.3", "t"));
        metrics.count_page_view();
        let app = metrics_router(metrics);
        let send = |method: Method, path: &str| {
            let req = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            app.clone().oneshot(req)
        };
        let res = send(Method::GET, "/metrics").await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], METRICS_CONTENT_TYPE);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("ferrite_page_views_total 1\n"), "{body}");
        assert!(body.contains("ferrite_build_info{version=\"1.2.3\",target=\"t\"} 1\n"));
        let res = send(Method::HEAD, "/metrics").await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = send(Method::POST, "/metrics").await.unwrap();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        for path in ["/", "/metrics/", "/api/v1/system/status"] {
            let res = send(Method::GET, path).await.unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[test]
    fn live_nodes_use_the_window() {
        let now = Timestamp::from_micros(100_000_000).unwrap();
        let node = |age_s: i64| ferrite_domain::Node {
            id: ferrite_domain::NodeId::new(),
            name: "n".into(),
            version: "1".into(),
            schema_version: 1,
            started_at: now,
            heartbeat_at: Timestamp::from_micros(100_000_000 - age_s * 1_000_000).unwrap(),
            meta: serde_json::json!({}),
        };
        let nodes = [node(0), node(30), node(31)];
        assert_eq!(live_nodes(&nodes, now, Duration::from_secs(30)), 2);
    }
}
