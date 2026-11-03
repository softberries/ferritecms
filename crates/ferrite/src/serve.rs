//! `ferrite serve`: the public site as one node of a site (solo or cluster, 04).
//!
//! [`start`] boots a node; [`run`] (the CLI) is `start` + signal handling +
//! [`RunningNode::shutdown`]. Startup, in order (any failure stops the process with
//! exit code 1; rulings P15, P22):
//!
//! 1. load the config, open the database (created if missing);
//! 2. the migration check under the migrate lock (02 §8: pending migrations are applied
//!    when allowed, an incompatible schema is refused);
//! 3. join the node registry (`Registry::join`: refuses another site or `site.url`; in
//!    solo mode waits for another live node to go stale). On SQLite the data-folder lock
//!    already keeps other processes out, so rows left by a crashed process are deleted
//!    instead: no wait and no `site.url` check (ruling R17);
//! 4. open the media store and probe it;
//! 5. the page cache and the invalidation loop's cursor; **everything below is read
//!    after the cursor is set**, so a theme, plugin or permalink change committed while
//!    the node boots is applied by the first poll instead of being missed (P22);
//! 6. the permalink rules from the `permalink.post`/`permalink.page` options, checked
//!    against the registered types and taxonomies (invalid: exit 1);
//! 7. the active theme (a broken one falls back to the embedded `blog` theme at startup
//!    only, 06 §7); the enabled plugins (a broken one is skipped); the route table. A
//!    fallback or a skipped plugin is retried by the reload task with the R14 backoff
//!    (R17);
//! 8. bind `server.listen`, `server.metrics_listen` when set, and with
//!    `[server.tls] mode = "acme"` `server.tls.listen` and `redirect_http` (before any
//!    background task starts, so a busy port fails cleanly; `<data>/tls/` is created
//!    0700 here);
//! 9. the background tasks: the invalidation loop (`cache.poll_interval_ms`; with
//!    `LISTEN` wake-ups in cluster mode) with the [`Reloader`] hook (hot swap of theme,
//!    plugins and permalinks, P18), the shared plugin-failure flusher (P20), the
//!    `request.served` flusher (every 10 s or 1 000 events, P21), the redirect hits
//!    flusher (every 10 s, at most 10 000 redirects buffered), the job
//!    runner (`cluster.workers`; built-in prune jobs, scheduled publishing, plugin
//!    actions, `media.variants` bounded by `media.image_workers`, and the caller's
//!    handlers), the periodic scheduler (prune jobs hourly,
//!    `content.publish_due` every 30 s, `artifacts.gc` daily, `site.backup` daily or weekly
//!    when `[backup]` is on), the heartbeat (`nodes.meta` carries the
//!    theme, plugin and permalink state; the first beat is awaited before serving, and
//!    the reloader beats after every reload) and the
//!    `node_metrics` writer (one row per whole minute from the registry's deltas, P35);
//! 10. serve the public site and the API under `/api/v1` (`build_site`; the API's
//!     services and the site key are set up in step 7, so a missing or bad key stops
//!     startup); [`run`] prints `Listening on http://<addr>` on stdout. With
//!     `server.metrics_listen`, `GET /metrics` (Prometheus text, P34) is served there and
//!     only there, until the background tasks stop. The node's metrics registry counts
//!     every response of the site and reads the page cache, the plugin host, the job
//!     runner's counters and the store at scrape time. With TLS on (P33, [`crate::tls`]),
//!     the same site is also served over HTTPS on `server.tls.listen`, every response
//!     there with `Strict-Transport-Security`, while `server.listen` keeps serving plain
//!     HTTP (bind it to loopback); the ACME client runs as a background task;
//!     `redirect_http` answers only `301` to the https site. [`run`] then also prints
//!     `Listening on https://<addr>`. The listeners drain together at shutdown.
//!
//! Shutdown ([`RunningNode::shutdown`]; SIGINT or SIGTERM in [`run`]): stop accepting
//! connections **and** stop the job runner at once; open requests get up to 10 s while
//! running jobs get 9 s (P10) to finish, after which the runner releases the rest
//! without using an attempt (the runner is abandoned after 12 s, R6). Then the
//! background tasks stop (the failure flusher writes once more, the `request.served`
//! buffer is flushed into jobs once more, for the next runner to claim, the buffered
//! redirect hits are written once more, and the partial minute's `node_metrics` row,
//! before the pools close), mail sends still
//! running (`[mail]`) are cancelled, and the node leaves the registry, together within
//! 1 s; [`run`] closes the database pools (up to 2 s, cut so that the steps so far end
//! within 13 s of the signal), and `main` shuts the runtime
//! down with a 1 s grace, which drops every connection task still running and abandons
//! stuck blocking work. A clean shutdown exits 0 and the process is gone at most 14 s
//! after the first signal. A signal during startup (after joining the registry the node
//! first leaves it, within 1 s), or a second signal during shutdown, exits at once with
//! 130 (SIGINT) or 143 (SIGTERM).

use std::{future::IntoFuture, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use axum::serve::ListenerExt;
use ferrite_cache::{InvalidationLoop, PageCache};
use ferrite_cluster::{
    ClusterMode as NodeMode, JobCounters, JobRunner, NodeIdentity, Registry, RunnerConfig,
    SCHEDULER_TICK, STALE_AFTER, Scheduler, builtin, node_name,
};
use ferrite_config::{Config, LoggedInCache, MediaSection, PluginsConfig, TlsConfig};
use ferrite_core::mail::MailQueue;
use ferrite_core::webhooks::{NoTransport, UrlPolicy, WebhookService, WebhookTransport};
use ferrite_core::{
    AuthService, ContentService, ContentTypeService, ExtensionAdmin, ExtensionService,
    MediaService, MenuService, OptionService, Permalinks, RedirectService, SiteReader,
    SwappablePermalinks, SystemService, TaxonomyService, UserService,
    comments::CommentService,
    import::ImportService,
    oauth::{ClientMetadataFetcher, OAuthService},
};
use ferrite_domain::{NodeId, Timestamp};
use ferrite_http::{
    AppParts, AppState, FLUSH_EVERY, HttpConfig, LoadSummary, LoggedIn, PluginReloader,
    SERVED_FLUSH_EVERY, ServedBuffer, SharedFailures, StoreHostServices,
    api::{ApiMail, ApiParts, ApiState, NodeInfo},
    app::hits::{HIT_FLUSH_EVERY, HitCounter},
    build_site,
    extensions::Installer,
    import_blobs::MediaArtifacts,
    load_route_table,
    mail::SmtpMailer,
    media::{Uploader, variant_specs},
    metrics::{Metrics, metrics_router},
    oauth_fetch::{NoFetcher, OAuthFetcher},
    plugins::egress::Egress,
    webhook_send::{WebhookClient, webhook_ip_rule},
};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{HostConfig, PluginHost};
use ferrite_store::{Backend, Store};
use tokio::{
    net::TcpListener,
    sync::Semaphore,
    task::{JoinError, JoinHandle},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    binary_version,
    cli::DataArg,
    jobs::{self, MediaVariantsHandler},
    keys, load_config,
    node::{NodeStatus, node_mode},
    node_metrics::MinuteWriter,
    open::{OpenMode, open_store, startup_check},
    reload::{ReloadContext, Reloader},
    site::{load_theme, media_backend_name, open_media, permalink_rules},
};

/// How long open requests may run after a shutdown signal.
const DRAIN: Duration = Duration::from_secs(10);
/// How long the job runner may take to stop after the signal (ruling R6; its own bound
/// is `JOB_DRAIN` + `ABORT_WAIT` + `RELEASE_BUDGET` = 11.5 s). Runs alongside [`DRAIN`].
const RUNNER_STOP: Duration = Duration::from_secs(12);
/// How long the background tasks and leaving the registry may take after the drain.
const BACKGROUND_STOP: Duration = Duration::from_secs(1);
/// Part of [`BACKGROUND_STOP`] kept for leaving the registry.
const LEAVE_BUDGET: Duration = Duration::from_millis(300);
/// How long a task that missed its stop is given to unwind after `abort`.
const ABORT_GRACE: Duration = Duration::from_millis(50);
/// How long closing the database pools may take at shutdown (a request dropped
/// mid-transaction can keep a connection checked out).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Shutdown, up to and including closing the pools, ends this long after the signal.
const SHUTDOWN_BOUND: Duration = Duration::from_secs(13);
/// Grace `main` gives the runtime's remaining tasks when it shuts down.
pub const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(1);

/// Test seam: job runner timings that tests shorten; `None` keeps the production value.
#[cfg(feature = "test-seams")]
#[derive(Debug, Clone, Default)]
pub struct RunnerOverrides {
    pub lease: Option<Duration>,
    pub extend_every: Option<Duration>,
    pub idle_poll: Option<Duration>,
}

/// How [`start`] deviates from a production node; `Default` is production. Every field
/// is a test seam, so without the dev-only `test-seams` feature it has none.
#[derive(Default)]
pub struct NodeOptions {
    /// Test seam: the version this node registers and checks the schema with
    /// (rolling-upgrade tests); `None` is this binary's.
    #[cfg(feature = "test-seams")]
    pub version: Option<semver::Version>,
    /// Test seam: handlers run besides the built-in ones.
    #[cfg(feature = "test-seams")]
    pub extra_handlers: Vec<Arc<dyn ferrite_cluster::JobHandler>>,
    /// Test seam: shorter job runner timings.
    #[cfg(feature = "test-seams")]
    pub runner: RunnerOverrides,
    /// Test seam: replaces the periodic jobs (default: the built-in prune jobs,
    /// `content.publish_due` and the plugin purge sweep).
    #[cfg(feature = "test-seams")]
    pub periodics: Option<Vec<ferrite_cluster::Periodic>>,
    /// Test seam: the scheduler tick (default `SCHEDULER_TICK`, 10 s).
    #[cfg(feature = "test-seams")]
    pub scheduler_tick: Option<Duration>,
    /// Test seam: the `request.served` flush interval (default `SERVED_FLUSH_EVERY`,
    /// 10 s).
    #[cfg(feature = "test-seams")]
    pub served_flush: Option<Duration>,
    /// Test seam: the `node_metrics` writer writes every this long from the start
    /// instead of at every whole minute (P35).
    #[cfg(feature = "test-seams")]
    pub metrics_tick: Option<Duration>,
    /// Test seam: whether the invalidation loop and the job runner use `LISTEN`
    /// wake-ups (default: in cluster mode). `Some(false)` leaves the node on polling only.
    #[cfg(feature = "test-seams")]
    pub listen: Option<bool>,
    /// Test seam: [`start`] pauses right after the invalidation cursor is set.
    #[cfg(feature = "test-seams")]
    pub boot_barrier: Option<BootBarrier>,
    /// Test seam: what the `media.variants` handler decodes with (default
    /// `ferrite_media::process_image`).
    #[cfg(feature = "test-seams")]
    pub image_processor: Option<jobs::ImageProcessor>,
    /// Test seam: every `import.wordpress` attempt on this node fails after this many
    /// batches.
    #[cfg(feature = "test-seams")]
    pub import_fail_after_batches: Option<u32>,
    /// Test seam: the TLS listener's (extra ACME roots, or a fixed certificate).
    #[cfg(feature = "test-seams")]
    pub tls: crate::tls::TlsSeams,
    /// Test seam: the webhook retry schedule (default design 18 §4.3's).
    #[cfg(feature = "test-seams")]
    pub webhook_schedule: Option<Vec<Duration>>,
    /// Test seam: what webhooks are sent through (default the SSRF-checked client).
    #[cfg(feature = "test-seams")]
    pub webhook_transport: Option<Arc<dyn ferrite_core::webhooks::WebhookTransport>>,
    /// Test seam: where the webhook auto-disable notice goes (default the `[mail]`
    /// queue, when configured).
    #[cfg(feature = "test-seams")]
    pub webhook_mailer: Option<Arc<dyn ferrite_core::mail::Mailer>>,
}

/// The node side of a boot pause (test seam; see [`NodeOptions::boot_barrier`]).
#[cfg(feature = "test-seams")]
pub struct BootBarrier {
    reached: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

/// The test side of a boot pause.
#[cfg(feature = "test-seams")]
pub struct BootGate {
    reached: tokio::sync::oneshot::Receiver<()>,
    resume: tokio::sync::oneshot::Sender<()>,
}

#[cfg(feature = "test-seams")]
impl BootBarrier {
    /// A barrier for [`NodeOptions::boot_barrier`] and the gate that controls it.
    pub fn new() -> (BootBarrier, BootGate) {
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        (
            BootBarrier {
                reached: reached_tx,
                resume: resume_rx,
            },
            BootGate {
                reached: reached_rx,
                resume: resume_tx,
            },
        )
    }

    /// Tells the gate the barrier is reached, then waits until it resumes (or is
    /// dropped).
    async fn wait(self) {
        let _ = self.reached.send(());
        let _ = self.resume.await;
    }
}

#[cfg(feature = "test-seams")]
impl BootGate {
    /// Resolves once `start` is paused at the barrier (or has failed before it).
    pub async fn reached(&mut self) {
        let _ = (&mut self.reached).await;
    }

    /// Lets `start` continue.
    pub fn resume(self) {
        let _ = self.resume.send(());
    }
}

/// A started node. [`shutdown`](Self::shutdown) stops it cleanly; dropping it leaves
/// its tasks running.
pub struct RunningNode {
    pub addr: SocketAddr,
    /// Where `/metrics` is served (`server.metrics_listen`); `None` when it is off.
    pub metrics_addr: Option<SocketAddr>,
    /// The HTTPS listener (`[server.tls]` on); `None` when TLS is off.
    pub tls_addr: Option<SocketAddr>,
    /// The redirect listener (`server.tls.redirect_http`); `None` when it is off.
    pub redirect_addr: Option<SocketAddr>,
    pub node_id: NodeId,
    pub state: Arc<AppState>,
    /// The API's state (its services).
    pub api: Arc<ApiState>,
    /// The node's metrics registry (P34); the per-minute writer reads its snapshots.
    pub metrics: Arc<Metrics>,
    server: Option<JoinHandle<std::io::Result<()>>>,
    /// The server's result when it ended before shutdown.
    server_result: Option<Result<std::io::Result<()>, JoinError>>,
    stop_accepting: CancellationToken,
    runner_stop: CancellationToken,
    runner: JoinHandle<()>,
    background: CancellationToken,
    /// Reloader, invalidation loop, failure flusher, `request.served` flusher, redirect
    /// hits flusher, scheduler, heartbeat, `node_metrics` writer, and the metrics
    /// listener when configured.
    tasks: Vec<(&'static str, JoinHandle<()>)>,
    /// The outgoing-mail queue (`[mail]` set); its sends are cancelled at shutdown.
    mail: Option<Arc<MailQueue>>,
    /// Redirect hits; what its final flush could not write is logged at shutdown.
    hits: Arc<HitCounter>,
    registry: Registry,
}

/// Boots a node on `store` (see the module docs for the order). `config` must be the
/// site's; `store` is not closed by the node. A SQLite `store` must be this process's
/// alone (as `open_store` makes it, with the data-folder lock): other node rows are
/// taken to be from dead processes.
pub async fn start(
    config: &Config,
    store: Arc<dyn Store>,
    opts: NodeOptions,
) -> anyhow::Result<RunningNode> {
    let (registry, http) = join(config, &store, &opts).await?;
    boot_or_leave(config, store, opts, http, registry).await
}

/// Startup up to and including joining the node registry.
async fn join(
    config: &Config,
    store: &Arc<dyn Store>,
    opts: &NodeOptions,
) -> anyhow::Result<(Registry, HttpConfig)> {
    #[cfg(feature = "test-seams")]
    let version = match &opts.version {
        Some(v) => v.clone(),
        None => binary_version()?,
    };
    // A production node has no options to read.
    #[cfg(not(feature = "test-seams"))]
    let version = {
        let NodeOptions {} = opts;
        binary_version()?
    };
    startup_check(
        store.as_ref(),
        &version,
        &config.database,
        &config.data_dir,
        &config.site.locale,
    )
    .await?;
    let schema_version = store
        .migration_status(&version)
        .await
        .context("cannot read the migration status")?
        .binary_version;
    let http = http_config(config);
    let mode = config.effective_cluster_mode();
    let registry = Registry::join(
        store.clone(),
        NodeIdentity {
            id: NodeId::new(),
            name: node_name(config.cluster.node_name.as_deref()),
            version,
            schema_version,
            mode: node_mode(mode),
            // Normalised (no trailing slash), so every node compares the same text.
            site_url: http.site_url.clone(),
            // SQLite: the data-folder lock keeps every other process out (R17).
            owns_database: store.backend() == Backend::Sqlite,
        },
    )
    .await
    .context("cannot join the node registry")?;
    Ok((registry, http))
}

/// [`boot`]; on error leaves the registry at once, so a restart (solo mode) need not
/// wait for the row to go stale.
async fn boot_or_leave(
    config: &Config,
    store: Arc<dyn Store>,
    opts: NodeOptions,
    http: HttpConfig,
    registry: Registry,
) -> anyhow::Result<RunningNode> {
    match boot(config, store, opts, http, registry.clone()).await {
        Ok(node) => Ok(node),
        Err(e) => {
            leave_bounded(&registry).await;
            Err(e)
        }
    }
}

/// `Registry::leave`, bounded by [`BACKGROUND_STOP`]; failures are logged.
async fn leave_bounded(registry: &Registry) {
    match tokio::time::timeout(BACKGROUND_STOP, registry.leave()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "could not leave the node registry"),
        Err(_) => tracing::warn!("leaving the node registry timed out"),
    }
}

/// Everything after joining the registry; on error the caller leaves it.
async fn boot(
    config: &Config,
    store: Arc<dyn Store>,
    opts: NodeOptions,
    http: HttpConfig,
    registry: Registry,
) -> anyhow::Result<RunningNode> {
    // Close to the registry's `started_at` (database clock), which `boot` cannot see.
    let started_at = Timestamp::now();
    // A production node has no options to read.
    #[cfg(not(feature = "test-seams"))]
    let NodeOptions {} = opts;
    let mode = config.effective_cluster_mode();
    let listen = node_mode(mode) == NodeMode::Cluster;
    #[cfg(feature = "test-seams")]
    let listen = opts.listen.unwrap_or(listen);
    let media = Arc::new(open_media(config)?);
    media
        .check()
        .await
        .context("the media store is not usable (write/read/delete probe failed)")?;
    let cache = Arc::new(PageCache::new(config.cache.max_bytes));
    // P22: the cursor first; every piece of node state below is read after it.
    let events = InvalidationLoop::for_store(store.clone(), cache.clone())
        .await
        .context("cannot read the event outbox")?;
    #[cfg(feature = "test-seams")]
    if let Some(barrier) = opts.boot_barrier {
        barrier.wait().await;
    }
    let permalinks = Arc::new(SwappablePermalinks::new(
        permalink_rules(store.as_ref()).await?,
    ));
    let loaded = load_theme(store.as_ref(), &media, &config.data_dir, mode).await?;
    tracing::info!(
        theme = %loaded.theme.name(),
        hash = %loaded.theme.hash(),
        origin = ?loaded.origin,
        "theme loaded"
    );

    let dyn_permalinks: Arc<dyn Permalinks> = permalinks.clone();
    let reader = Arc::new(SiteReader::new(
        store.clone(),
        dyn_permalinks.clone(),
        media_url(media.clone()),
        Arc::new(Timestamp::now),
    ));
    // One site key: sessions and tokens, sealed plugin secrets (sealed by the API's
    // `ExtensionAdmin`, opened by the owning plugin's `settings.get`).
    let site_key = keys::site_key(config)?;
    // `[mail]`: one queue per node (bounded background sends, F28); lettre parses the
    // URL and `from` here, so a value it refuses stops startup (F30).
    let mail = match &config.mail {
        Some(mail) => {
            let smtp = SmtpMailer::new(mail).context("invalid [mail] section")?;
            Some(Arc::new(MailQueue::new(Arc::new(smtp))))
        }
        None => None,
    };
    let mut services = StoreHostServices::new(store.clone(), reader.clone(), &http.site_url)
        .with_site_key(site_key.clone())
        .with_site_title(config.site.title.clone());
    if let Some(queue) = &mail {
        services = services.with_mailer(queue.clone());
    }
    // The plugin `http` import (P16): one outbound client per node; never a reason not
    // to boot (ruling R19).
    if let Some(egress) = plugin_egress(Egress::new(env!("CARGO_PKG_VERSION"))) {
        services = services.with_egress(egress);
    }
    let services = Arc::new(services);
    let host = Arc::new(
        PluginHost::new(host_config(&config.plugins), services)
            .context("cannot start the plugin host")?,
    );
    let failure_window = Duration::from_secs(config.plugins.failure_window_secs);
    let failures = Arc::new(SharedFailures::new(
        config.plugins.disable_after_failures,
        failure_window,
    ));
    host.set_failure_sink(failures.clone());
    let plugins = Arc::new(
        PluginReloader::new(store.clone(), media.clone(), host.clone())
            .with_failures(failures.clone()),
    );
    let report = plugins
        .reload()
        .await
        .context("cannot list the installed plugins")?;
    let summary = LoadSummary::from(&report);
    tracing::info!(
        loaded = summary.loaded.len(),
        skipped = summary.skipped.len(),
        "plugins loaded"
    );
    let routes = load_route_table(store.as_ref(), &permalinks.current())
        .await
        .context("cannot load content types and taxonomies")?;
    // One auth service for the API and the public site (previews, logged-in requests).
    let auth = Arc::new(
        AuthService::new(store.clone(), site_key.clone())
            .with_site_title(config.site.title.clone()),
    );
    // `request.served` events of this node's page responses (P21).
    let served = Arc::new(ServedBuffer::new(store.clone(), host.clone()));
    // Redirect hits of this node, one write per redirect per flush (06 §6).
    let hits = Arc::new(HitCounter::new(Arc::new(RedirectService::new(
        store.clone(),
    ))));
    // One comment service per node: the API submit and moderation, and the theme form
    // (`POST /_comment`, P12).
    let comments = Arc::new(CommentService::new(
        store.clone(),
        auth.clone(),
        dyn_permalinks.clone(),
    ));
    // The node's metrics registry (P34): every response, page views, and at scrape time
    // the page cache, the plugin host, the jobs, the nodes and the pools.
    let job_counters = Arc::new(JobCounters::default());
    let metrics = {
        let jobs = job_counters.clone();
        Arc::new(
            Metrics::new(&binary_version()?.to_string(), crate::binary_target())
                .with_cache(cache.clone())
                .with_plugins(host.clone())
                .with_store(store.clone(), STALE_AFTER)
                .with_job_totals(move || (jobs.done(), jobs.failed())),
        )
    };
    // The API's services; `content` also runs the scheduled-publishing jobs.
    let content = Arc::new(ContentService::new(store.clone(), dyn_permalinks.clone()));
    let releases = Arc::new(ferrite_core::release::ReleaseService::new(
        store.clone(),
        dyn_permalinks.clone(),
        content.clone(),
    ));
    let preview_links = Arc::new(ferrite_core::preview_link::PreviewLinkService::new(
        store.clone(),
    ));
    let state = Arc::new(
        AppState::new(AppParts {
            store: store.clone(),
            reader,
            plugins: host.clone(),
            media: media.clone(),
            theme: loaded.theme.clone(),
            cache: cache.clone(),
            routes,
            config: http,
            migrations_ok: true,
        })
        .context("admin headers")?
        .with_auth(auth.clone())
        .with_served(served.clone())
        .with_hits(hits.clone())
        .with_comments(comments.clone())
        .with_metrics(metrics.clone())
        .with_editorial(releases.clone(), preview_links.clone()),
    );

    let status = Arc::new(NodeStatus::default());
    status.set_theme(&loaded, loaded.error.as_deref(), loaded.fallback);
    status.set_plugins(&report);

    // The node's import service (M3-2): enqueues and reads imports, checks the job's
    // importer and stores run reports in the media store. The `import.wordpress` job
    // and the API share it.
    let import = Arc::new(
        ImportService::new(store.clone(), dyn_permalinks.clone())
            .with_artifacts(Arc::new(MediaArtifacts::new(media.clone()))),
    );
    let media_service = Arc::new(MediaService::new(
        store.clone(),
        content.clone(),
        dyn_permalinks,
    ));
    // One upload pipeline per node: its slots bound the node (M2a-2b P9).
    let max_upload = config.uploads.max_upload;
    let uploader = Arc::new(
        Uploader::new(media.clone(), max_upload)
            .with_strip_metadata(config.uploads.strip_metadata)
            .with_idle_timeout(Duration::from_secs(config.uploads.idle_timeout_secs)),
    );
    // One install pipeline per node: its compile slots bound the node (P8).
    let installer = Arc::new(
        Installer::new(store.clone(), media.clone(), host_config(&config.plugins))
            .context("cannot start the plugin installer")?
            .with_trust(crate::registry::trust_settings(&config.plugins.trust)?),
    );
    // The OAuth authorization server (design 12 §3.2): its issuer is `site.url`; client
    // metadata documents come through the SSRF-checked egress rules.
    let issuer = url::Url::parse(&state.config.site_url).context("site.url")?;
    let oauth = Arc::new(OAuthService::new(
        store.clone(),
        auth.clone(),
        metadata_fetcher(OAuthFetcher::new(env!("CARGO_PKG_VERSION"))),
        issuer,
    ));
    // Outgoing webhooks (design 18): one service per node, the SSRF-checked client (or
    // one that sends nothing when no TLS client can be built; boot goes on).
    let local_targets = config.webhooks.local_targets;
    let webhooks = WebhookService::new(
        store.clone(),
        keys::site_key(config)?,
        UrlPolicy {
            local_targets,
            allow_ip: webhook_ip_rule(local_targets),
        },
    );
    #[cfg(feature = "test-seams")]
    let webhooks = match opts.webhook_schedule.clone() {
        Some(schedule) => webhooks.with_schedule(schedule),
        None => webhooks,
    };
    let webhooks = Arc::new(webhooks);
    if local_targets {
        tracing::warn!(
            "[webhooks] local_targets is on: webhooks may call http and private addresses"
        );
    }
    let api = Arc::new(
        ApiState::new(ApiParts {
            store: store.clone(),
            oauth,
            auth,
            comments,
            system: Arc::new(SystemService::new(store.clone())),
            import: import.clone(),
            site: state.clone(),
            node: NodeInfo {
                id: registry.id(),
                started_at,
                mode: node_mode(mode).as_str(),
                media_backend: media_backend_name(config),
            },
            users: Arc::new(UserService::new(store.clone())),
            content: content.clone(),
            types: Arc::new(ContentTypeService::new(store.clone())),
            taxonomy: Arc::new(TaxonomyService::new(store.clone())),
            menus: Arc::new(MenuService::new(store.clone())),
            options: Arc::new(OptionService::new(store.clone())),
            redirects: Arc::new(RedirectService::new(store.clone())),
            extensions: Arc::new(ExtensionAdmin::new(store.clone(), site_key)),
            installer: installer.clone(),
            media: media.clone(),
            media_service: media_service.clone(),
            uploader,
            max_upload,
            mail: mail
                .as_ref()
                .zip(config.mail.as_ref())
                .map(|(queue, m)| ApiMail {
                    queue: queue.clone(),
                    from: m.from.clone(),
                }),
            site_url: state.config.site_url.clone(),
            site_title: config.site.title.clone(),
            webhooks: webhooks.clone(),
            editorial: ferrite_http::api::releases::EditorialApi::new(
                store.clone(),
                releases.clone(),
                preview_links.clone(),
            ),
        })
        .context("site.url")?,
    );
    let webhook_transport: Arc<dyn WebhookTransport> = match WebhookClient::new(
        env!("CARGO_PKG_VERSION"),
        local_targets,
    ) {
        Ok(client) => Arc::new(client),
        Err(e) => {
            tracing::error!(error = %e, "webhooks cannot be sent from this node: the outbound client could not be built");
            Arc::new(NoTransport)
        }
    };
    #[cfg(feature = "test-seams")]
    let webhook_transport = opts.webhook_transport.clone().unwrap_or(webhook_transport);
    let webhook_mailer: Option<Arc<dyn ferrite_core::mail::Mailer>> = mail
        .clone()
        .map(|q| q as Arc<dyn ferrite_core::mail::Mailer>);
    #[cfg(feature = "test-seams")]
    let webhook_mailer = opts.webhook_mailer.clone().or(webhook_mailer);
    let mut handlers = builtin::handlers(store.clone(), builtin::failure_retention(failure_window));
    let extensions = Arc::new(ExtensionService::new(store.clone()));
    let action_slots =
        jobs::action_slots(config.cluster.workers, config.plugins.max_concurrent_calls);
    if config.cluster.workers < 2 {
        tracing::warn!(
            workers = config.cluster.workers,
            "cluster.workers = 1: plugin actions can hold the only job worker"
        );
    }
    handlers.extend(jobs::handlers(
        store.clone(),
        content,
        extensions,
        host.clone(),
        action_slots,
    ));
    // At most `media.image_workers` images decode at once on this node (M1b R4, P8),
    // with the sizes of the theme active when the job runs.
    let theme_state = state.clone();
    let variants = MediaVariantsHandler::new(
        store.clone(),
        media.clone(),
        media_service.clone(),
        Arc::new(Semaphore::new(config.uploads.image_workers)),
        Arc::new(move || variant_specs(&theme_state.theme.load())),
    );
    #[cfg(feature = "test-seams")]
    let variants = match opts.image_processor {
        Some(processor) => variants.with_processor(processor),
        None => variants,
    };
    handlers.push(Arc::new(variants));
    handlers.push(Arc::new(jobs::ImportWordpressHandler {
        store: store.clone(),
        import,
        media: media.clone(),
        data_dir: config.data_dir.clone(),
        site_url: config.site.url.as_str().trim_end_matches('/').to_owned(),
        max_upload,
        strip_metadata: config.uploads.strip_metadata,
        version: binary_version()?.to_string(),
        #[cfg(feature = "test-seams")]
        fail_after_batches: opts.import_fail_after_batches,
    }));
    handlers.push(Arc::new(jobs::WebhookDeliverHandler::new(
        webhooks.clone(),
        webhook_transport,
        url::Url::parse(config.site.url.as_str()).context("site.url")?,
        jobs::webhook_slots(config.cluster.workers),
        webhook_mailer.map(|mailer| jobs::WebhookNotice {
            mailer,
            store: store.clone(),
            site_title: config.site.title.clone(),
        }),
    )));
    handlers.push(Arc::new(jobs::WebhooksPruneHandler {
        service: webhooks.clone(),
    }));
    // M6-3: releases, editorial mail, preview-link pruning.
    handlers.push(Arc::new(jobs::ReleaseApplyHandler {
        releases: releases.clone(),
    }));
    handlers.push(Arc::new(jobs::ReleaseApplyDueHandler {
        store: store.clone(),
        releases: releases.clone(),
    }));
    handlers.push(Arc::new(jobs::PreviewLinksPruneHandler {
        store: store.clone(),
    }));
    handlers.push(Arc::new(jobs::EditorialNotifyHandler {
        store: store.clone(),
        mailer: mail
            .clone()
            .map(|q| q as Arc<dyn ferrite_core::mail::Mailer>),
        site_url: url::Url::parse(config.site.url.as_str()).context("site.url")?,
        site_title: config.site.title.clone(),
    }));
    handlers.push(Arc::new(jobs::ArtifactsGcHandler::new(
        store.clone(),
        media.clone(),
        crate::gc::grace_days(config.uploads.gc_grace_days),
    )));
    // `plugins.registry_check` (M6-2, ruling P5): the handler always (a check is a
    // job like any other), the daily periodic only with `registry_check = "daily"` and
    // a registry.
    handlers.push(Arc::new(jobs::RegistryCheckHandler {
        installer: installer.clone(),
    }));
    let registry_periodic = (config.plugins.trust.registry_check
        == ferrite_config::RegistryCheck::Daily
        && installer.trust_settings().registry.is_some())
    .then(jobs::registry_check_periodic);
    // `[backup]` (P15): nothing is registered while the schedule is off.
    let backup_periodic = match jobs::site_backup(config, store.clone(), media.clone())
        .context("cannot set up the scheduled backup ([backup])")?
    {
        Some((handler, periodic)) => {
            handlers.push(Arc::new(handler));
            Some(periodic)
        }
        None => None,
    };
    #[cfg(feature = "test-seams")]
    handlers.extend(opts.extra_handlers);
    // What the job list's `handled` reads (P19).
    status.set_job_kinds(handlers.iter().map(|h| h.kind().to_owned()).collect());
    let mut runner_config = RunnerConfig::new(registry.id(), config.cluster.workers);
    #[cfg(feature = "test-seams")]
    {
        let o = &opts.runner;
        runner_config.lease = o.lease.unwrap_or(runner_config.lease);
        runner_config.extend_every = o.extend_every.unwrap_or(runner_config.extend_every);
        runner_config.idle_poll = o.idle_poll.unwrap_or(runner_config.idle_poll);
    }
    runner_config.listen = listen;
    let runner = JobRunner::new(store.clone(), runner_config, handlers)
        .context("cannot set up the job runner")?
        .with_counters(job_counters);
    let mut periodics = builtin::periodics();
    periodics.extend(jobs::core_periodics());
    periodics.extend(backup_periodic);
    periodics.extend(registry_periodic);
    let tick = SCHEDULER_TICK;
    #[cfg(feature = "test-seams")]
    let (periodics, tick) = (
        opts.periodics.unwrap_or(periodics),
        opts.scheduler_tick.unwrap_or(tick),
    );
    let scheduler =
        Scheduler::new(store.clone(), periodics, tick).context("cannot set up the scheduler")?;

    let listener = TcpListener::bind(config.server.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.server.listen))?;
    let addr = listener.local_addr()?;
    // `/metrics` only on its own listener, when configured (P34).
    let metrics_listener = match config.server.metrics_listen {
        Some(at) => Some(
            TcpListener::bind(at)
                .await
                .with_context(|| format!("cannot listen on {at} (server.metrics_listen)"))?,
        ),
        None => None,
    };
    let metrics_addr = metrics_listener
        .as_ref()
        .map(TcpListener::local_addr)
        .transpose()?;
    // Built-in TLS (P33): the config refuses it in cluster mode.
    #[cfg(feature = "test-seams")]
    let tls_seams = opts.tls;
    #[cfg(not(feature = "test-seams"))]
    let tls_seams = crate::tls::TlsSeams::default();
    let tls = match &config.server.tls {
        TlsConfig::Acme(acme) => Some(crate::tls::bind(acme, &config.data_dir, tls_seams).await?),
        TlsConfig::Off => None,
    };
    let tls_addr = tls.as_ref().map(|t| t.addr);
    let redirect_addr = tls
        .as_ref()
        .and_then(|t| t.redirect.as_ref().map(|(_, a)| *a));

    // Nothing below fails: start the background tasks.
    let background = CancellationToken::new();
    let reloader = Reloader::new(host.clone());
    let reload_task = reloader.spawn(
        ReloadContext {
            store: store.clone(),
            media,
            data_dir: config.data_dir.clone(),
            mode,
            state: state.clone(),
            permalinks,
            plugins,
            theme: loaded,
            plugins_report: report,
            plugins_error: None,
            status: status.clone(),
            registry: registry.clone(),
        },
        background.clone(),
    );
    let poller = events.with_hook(reloader).spawn(
        Duration::from_millis(config.cache.poll_interval_ms),
        listen,
        background.clone(),
    );
    let flusher = failures.spawn_flusher(store.clone(), host, FLUSH_EVERY, background.clone());
    let served_flush = SERVED_FLUSH_EVERY;
    #[cfg(feature = "test-seams")]
    let served_flush = opts.served_flush.unwrap_or(served_flush);
    let served_flusher = served.spawn_flusher(served_flush, background.clone());
    let hits_flusher = hits
        .clone()
        .spawn_flusher(HIT_FLUSH_EVERY, background.clone());
    let runner_stop = CancellationToken::new();
    let runner = runner.spawn(runner_stop.clone());
    let scheduler = scheduler.spawn(background.clone());
    let heartbeat = registry.spawn_heartbeat(status.meta_fn(), background.clone());
    // One `node_metrics` row per minute (P35); flushed once more at shutdown.
    let metrics_every: Option<Duration> = None;
    #[cfg(feature = "test-seams")]
    let metrics_every = opts.metrics_tick.or(metrics_every);
    let minute_writer = MinuteWriter::new(
        Arc::new(store.clone()),
        registry.id(),
        metrics.clone(),
        metrics_every,
    )
    .spawn(background.clone());
    // Served until the background tasks stop, so the drain can still be scraped.
    let metrics_server = metrics_listener.map(|listener| {
        let app = metrics_router(metrics.clone());
        let stop = background.clone();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
            {
                tracing::warn!(error = %e, "the metrics listener failed");
            }
        })
    });
    // The row was registered without the node's state; publish it before serving, so
    // the row is complete (job kinds, theme, plugins) when `start` returns.
    registry.beat_once(status.meta_fn().as_ref()).await;

    let stop_accepting = CancellationToken::new();
    let plain = axum::serve(
        listener,
        build_site(state.clone(), api.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stop_accepting.clone().cancelled_owned())
    .into_future();
    // With TLS: the HTTPS and redirect listeners, served and drained with the plain one;
    // the ACME client runs until the background tasks stop.
    let mut acme_task = None;
    let secure = match tls {
        Some(crate::tls::Bound {
            listener: tls_listener,
            redirect,
            acme,
            ..
        }) => {
            acme_task = acme.map(|state| crate::tls::spawn_acme(state, background.clone()));
            // `tap_io` gives the listener axum's `ConnectInfo<SocketAddr>`.
            let https = axum::serve(
                tls_listener.tap_io(|_| {}),
                ferrite_http::tls::with_hsts(build_site(state.clone(), api.clone()))
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(stop_accepting.clone().cancelled_owned())
            .into_future();
            let redirect = redirect.map(|(l, _)| {
                let origin = config.site.url.origin().ascii_serialization();
                axum::serve(l, ferrite_http::tls::redirect_app(&origin))
                    .with_graceful_shutdown(stop_accepting.clone().cancelled_owned())
                    .into_future()
            });
            Some(async move {
                match redirect {
                    Some(redirect) => tokio::try_join!(https, redirect).map(|_| ()),
                    None => https.await,
                }
            })
        }
        None => None,
    };
    let server = tokio::spawn(async move {
        match secure {
            Some(secure) => tokio::try_join!(plain, secure).map(|_| ()),
            None => plain.await,
        }
    });
    tracing::info!(listen = %format!("http://{addr}"), site = %state.config.site_url, node = %registry.id(), "serving");
    if let Some(at) = tls_addr {
        tracing::info!(listen = %format!("https://{at}"), "serving over TLS");
    }
    if let Some(at) = redirect_addr {
        tracing::info!(listen = %format!("http://{at}"), "redirecting to https");
    }
    if let Some(at) = metrics_addr {
        tracing::info!(listen = %format!("http://{at}/metrics"), "serving metrics");
    }
    let mut tasks = vec![
        ("reloader", reload_task),
        ("invalidation loop", poller),
        ("plugin failure flusher", flusher),
        ("request.served flusher", served_flusher),
        ("redirect hits flusher", hits_flusher),
        ("scheduler", scheduler),
        ("heartbeat", heartbeat),
        ("node metrics writer", minute_writer),
    ];
    tasks.extend(metrics_server.map(|task| ("metrics listener", task)));
    tasks.extend(acme_task.map(|task| ("acme client", task)));
    Ok(RunningNode {
        addr,
        metrics_addr,
        tls_addr,
        redirect_addr,
        node_id: registry.id(),
        state,
        api,
        metrics,
        server: Some(server),
        server_result: None,
        stop_accepting,
        runner_stop,
        runner,
        background,
        tasks,
        mail,
        hits,
        registry,
    })
}

impl RunningNode {
    /// Stops the node (see the module docs): stop accepting and stop the runner at once,
    /// drain requests (10 s) and jobs (runner abandoned after 12 s), stop the background
    /// tasks with a final failure flush, leave the registry (1 s for both). An error
    /// is the server's (it failed while serving).
    pub async fn shutdown(self) -> anyhow::Result<()> {
        let RunningNode {
            server,
            server_result,
            stop_accepting,
            runner_stop,
            runner,
            background,
            tasks,
            mail,
            hits,
            registry,
            ..
        } = self;
        let started = Instant::now();
        stop_accepting.cancel();
        runner_stop.cancel();
        let http = async move {
            let Some(mut server) = server else {
                return server_result;
            };
            match tokio::time::timeout(DRAIN, &mut server).await {
                Ok(r) => Some(r),
                Err(_) => {
                    tracing::warn!(
                        "connections still open after {DRAIN:?}; they are dropped at exit"
                    );
                    server.abort();
                    None
                }
            }
        };
        let jobs = join_until("job runner", runner, started + RUNNER_STOP);
        let (served, ()) = tokio::join!(http, jobs);

        background.cancel();
        // Normally 1 s from now; anchored so that an abandoned runner (12 s plus its
        // abort grace) still ends shutdown within RUNNER_STOP + BACKGROUND_STOP.
        let deadline =
            (Instant::now() + BACKGROUND_STOP).min(started + RUNNER_STOP + BACKGROUND_STOP);
        let stop_tasks = futures::future::join_all(
            tasks
                .into_iter()
                .map(|(name, task)| join_until(name, task, deadline - LEAVE_BUDGET)),
        );
        // Mail sends had the drain to finish; any still running are cancelled now.
        let stop_mail = async {
            if let Some(mail) = &mail {
                let grace = (deadline - LEAVE_BUDGET).saturating_duration_since(Instant::now());
                mail.shutdown(grace).await;
            }
        };
        tokio::join!(stop_tasks, stop_mail);
        // A final flush cut short by the bound leaves the rest in the buffer.
        let (ids, lost) = hits.pending();
        if ids > 0 {
            tracing::warn!(
                redirects = ids,
                hits = lost,
                "redirect hits not written before shutdown"
            );
        }
        match tokio::time::timeout_at(deadline, registry.leave()).await {
            Ok(Ok(())) => tracing::info!(node = %registry.id(), "left the node registry"),
            Ok(Err(e)) => tracing::warn!(error = %e, "could not leave the node registry"),
            Err(_) => tracing::warn!("leaving the node registry timed out"),
        }
        if let Some(served) = served {
            served
                .context("the server task failed")?
                .context("the server stopped")?;
        }
        tracing::info!("node stopped");
        Ok(())
    }

    /// Crash simulation (test seam, ruling P25): drops every task at once; running
    /// jobs are not released and the node does not leave the registry.
    #[cfg(feature = "test-seams")]
    pub fn abort(self) {
        if let Some(server) = &self.server {
            server.abort();
        }
        self.runner.abort();
        for (_, task) in &self.tasks {
            task.abort();
        }
        // Detached subtasks (notification listeners) end on their tokens.
        self.stop_accepting.cancel();
        self.runner_stop.cancel();
        self.background.cancel();
    }

    /// Waits until the server ends on its own (`None`) or `signals` delivers one.
    async fn wait(&mut self, signals: &mut Signals) -> Option<Signal> {
        let Some(server) = self.server.as_mut() else {
            return Some(signals.recv().await);
        };
        let ended = tokio::select! {
            r = server => Ok(r),
            s = signals.recv() => Err(s),
        };
        match ended {
            Ok(r) => {
                self.server = None;
                self.server_result = Some(r);
                None
            }
            Err(s) => Some(s),
        }
    }
}

/// Awaits `task` until `deadline`, then aborts it.
async fn join_until(name: &'static str, mut task: JoinHandle<()>, deadline: Instant) {
    match tokio::time::timeout_at(deadline, &mut task).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(task = name, error = %e, "task ended abnormally"),
        Err(_) => {
            tracing::warn!(task = name, "task did not stop in time; abandoning it");
            task.abort();
            let _ = tokio::time::timeout(ABORT_GRACE, task).await;
        }
    }
}

pub async fn run(args: DataArg) -> anyhow::Result<()> {
    let config = load_config(&args.data)?;
    let mut signals = Signals::install().context("cannot install signal handlers")?;
    let store = open_store(&config, OpenMode::CreateIfMissing).await?;
    let (served, close_by) = serve(&config, store.clone(), &mut signals).await;
    let close = close_by
        .saturating_duration_since(Instant::now())
        .min(CLOSE_TIMEOUT);
    if tokio::time::timeout(close, store.close()).await.is_err() {
        tracing::warn!("database pools did not close within {close:?}");
    }
    served
}

/// Runs a node until a signal; returns the result and when closing the pools must end.
async fn serve(
    config: &Config,
    store: Arc<dyn Store>,
    signals: &mut Signals,
) -> (anyhow::Result<()>, Instant) {
    let opts = NodeOptions::default();
    let joined = tokio::select! {
        joined = join(config, &store, &opts) => joined,
        signal = signals.recv() => {
            tracing::error!(signal = signal.name(), "signal during startup; exiting immediately");
            std::process::exit(signal.exit_code());
        }
    };
    let (registry, http) = match joined {
        Ok(joined) => joined,
        Err(e) => return (Err(e), Instant::now() + CLOSE_TIMEOUT),
    };
    let started = tokio::select! {
        node = boot_or_leave(config, store, opts, http, registry.clone()) => node,
        signal = signals.recv() => {
            // Nothing is spawned before boot returns; only the row needs removing.
            tracing::error!(signal = signal.name(), "signal during startup; leaving and exiting");
            leave_bounded(&registry).await;
            std::process::exit(signal.exit_code());
        }
    };
    let mut node = match started {
        Ok(node) => node,
        Err(e) => return (Err(e), Instant::now() + CLOSE_TIMEOUT),
    };
    println!("Listening on http://{}", node.addr);
    if let Some(at) = node.tls_addr {
        println!("Listening on https://{at}");
    }
    match node.wait(signals).await {
        Some(signal) => tracing::info!(
            signal = signal.name(),
            "shutdown signal received; draining connections (up to {DRAIN:?})"
        ),
        None => tracing::error!("the server stopped; shutting down"),
    }
    let close_by = Instant::now() + SHUTDOWN_BOUND;
    let result = tokio::select! {
        r = node.shutdown() => r,
        second = signals.recv() => {
            tracing::error!(
                signal = second.name(),
                "second signal during shutdown; exiting immediately"
            );
            std::process::exit(second.exit_code());
        }
    };
    if result.is_ok() {
        tracing::info!("shut down");
    }
    (result, close_by)
}

/// The HTTP settings from the config (`site.url` normalised without a trailing slash).
fn http_config(config: &Config) -> HttpConfig {
    let mut http = HttpConfig::new(
        config.site.url.as_str(),
        &config.site.title,
        &config.site.locale,
    );
    http.timezone = config.site.timezone.clone();
    http.trusted_proxies = config.server.trusted_proxies.clone();
    http.render_budget = Duration::from_millis(config.plugins.render_budget_ms);
    http.search_ttl = Duration::from_secs(config.cache.search_ttl_secs);
    http.volatile_ttl = Duration::from_secs(config.plugins.volatile_ttl_secs);
    http.logged_in = match config.cache.logged_in {
        LoggedInCache::Bypass => LoggedIn::Bypass,
        LoggedInCache::PerUser => LoggedIn::PerUser,
    };
    http.media_origin = media_origin(config);
    http
}

/// The origin of `[media] public_base_url` (Direct S3 mode; the admin CSP's `img-src`).
fn media_origin(config: &Config) -> Option<String> {
    match &config.media {
        MediaSection::S3 {
            public_base_url: Some(url),
            ..
        } => Some(url.origin().ascii_serialization()),
        _ => None,
    }
}

/// Public media URLs as the media store builds them (`/_media/…` or the CDN).
fn media_url(media: Arc<MediaStore>) -> ferrite_core::site::MediaUrlFn {
    Arc::new(move |hash, name| media.public_url(hash, name))
}

/// The plugin host limits from `[plugins]`; the rest are the host defaults.
pub(crate) fn host_config(p: &PluginsConfig) -> HostConfig {
    HostConfig {
        memory_bytes: usize::try_from(p.default_memory).unwrap_or(usize::MAX),
        fuel: p.default_fuel,
        render_deadline: Duration::from_millis(p.render_deadline_ms),
        action_deadline: Duration::from_millis(p.action_deadline_ms),
        disable_after_failures: p.disable_after_failures,
        failure_window: Duration::from_secs(p.failure_window_secs),
        max_concurrent_calls: p.max_concurrent_calls,
        ..HostConfig::default()
    }
}

/// Which shutdown signal arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Interrupt,
    #[cfg(unix)]
    Terminate,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Signal::Interrupt => "SIGINT",
            #[cfg(unix)]
            Signal::Terminate => "SIGTERM",
        }
    }

    /// 128 + the signal number, as shells report a process killed by it.
    fn exit_code(self) -> i32 {
        match self {
            Signal::Interrupt => 130,
            #[cfg(unix)]
            Signal::Terminate => 143,
        }
    }
}

/// SIGINT and SIGTERM (Ctrl-C only on other platforms), installed up front.
struct Signals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl Signals {
    fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// Resolves on the next signal; may be awaited again for the one after.
    #[cfg(unix)]
    async fn recv(&mut self) -> Signal {
        tokio::select! {
            _ = self.interrupt.recv() => Signal::Interrupt,
            _ = self.terminate.recv() => Signal::Terminate,
        }
    }

    /// Resolves on the next Ctrl-C (never, if it cannot be watched).
    #[cfg(not(unix))]
    async fn recv(&mut self) -> Signal {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
        Signal::Interrupt
    }
}

/// The node's plugin egress, or `None` (plugins get "http is not configured") when it
/// could not be built, with or without the `webpki-roots` fallback: logged as an error,
/// never fatal (ruling R19).
fn plugin_egress(built: Result<Egress, String>) -> Option<Arc<Egress>> {
    match built {
        Ok(egress) => Some(Arc::new(egress)),
        Err(e) => {
            tracing::error!(error = %e, "plugin http is unavailable: the outbound client could not be built");
            None
        }
    }
}

/// The node's metadata-document fetcher, or one that fetches nothing when no outbound
/// client could be built (logged; boot goes on, as for plugin `http`, ruling R19).
fn metadata_fetcher(built: Result<OAuthFetcher, String>) -> Arc<dyn ClientMetadataFetcher> {
    match built {
        Ok(fetcher) => Arc::new(fetcher),
        Err(e) => {
            tracing::error!(error = %e, "OAuth client metadata documents cannot be fetched: the outbound client could not be built");
            Arc::new(NoFetcher)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ruling R19: an egress that cannot be built leaves `http` unconfigured; boot goes
    /// on.
    #[test]
    fn egress_failure_does_not_stop_boot() {
        assert!(plugin_egress(Err("no roots at all".to_owned())).is_none());
        assert!(plugin_egress(Egress::new("0.0.0")).is_some());
    }

    /// `cache.logged_in` reaches the public site's settings (P21).
    #[test]
    fn http_config_maps_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let env = |v: Option<&str>| {
            let mut e = vec![(
                "FERRITE_SITE__URL".to_owned(),
                "http://localhost:8080".to_owned(),
            )];
            if let Some(v) = v {
                e.push(("FERRITE_CACHE__LOGGED_IN".to_owned(), v.to_owned()));
            }
            e
        };
        for (value, want) in [
            (None, LoggedIn::Bypass),
            (Some("bypass"), LoggedIn::Bypass),
            (Some("per-user"), LoggedIn::PerUser),
        ] {
            let config = ferrite_config::load(dir.path(), env(value)).unwrap();
            assert_eq!(http_config(&config).logged_in, want, "{value:?}");
        }
        // P22: the render budget reaches the pipeline.
        let mut e = env(None);
        e.push((
            "FERRITE_PLUGINS__RENDER_BUDGET_MS".to_owned(),
            "250".to_owned(),
        ));
        let config = ferrite_config::load(dir.path(), e).unwrap();
        assert_eq!(
            http_config(&config).render_budget,
            Duration::from_millis(250)
        );
        // Task 14: the short TTLs reach the pipeline.
        const SHORT: Duration = ferrite_http::app::SHORT_TTL;
        let http = http_config(&config);
        assert_eq!((http.search_ttl, http.volatile_ttl), (SHORT, SHORT));
        let mut e = env(None);
        e.push(("FERRITE_CACHE__SEARCH_TTL".to_owned(), "0".to_owned()));
        e.push(("FERRITE_PLUGINS__VOLATILE_TTL".to_owned(), "5".to_owned()));
        let http = http_config(&ferrite_config::load(dir.path(), e).unwrap());
        assert_eq!(
            (http.search_ttl, http.volatile_ttl),
            (Duration::ZERO, Duration::from_secs(5))
        );
    }

    /// M2b-1 Task 1: only Direct S3 media (`public_base_url`) adds an origin to the
    /// admin CSP.
    #[test]
    fn http_config_maps_the_media_origin() {
        let dir = tempfile::tempdir().unwrap();
        let env = |extra: &[(&str, &str)]| {
            let mut e = vec![(
                "FERRITE_SITE__URL".to_owned(),
                "http://localhost:8080".to_owned(),
            )];
            e.extend(
                extra
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
            );
            e
        };
        let local = ferrite_config::load(dir.path(), env(&[])).unwrap();
        assert_eq!(http_config(&local).media_origin, None);
        let s3 = [
            ("FERRITE_MEDIA__BACKEND", "s3"),
            ("FERRITE_MEDIA__S3__BUCKET", "media"),
            ("FERRITE_MEDIA__S3__REGION", "eu-central-1"),
        ];
        let via_app = ferrite_config::load(dir.path(), env(&s3)).unwrap();
        assert_eq!(http_config(&via_app).media_origin, None);
        let mut direct = s3.to_vec();
        direct.push((
            "FERRITE_MEDIA__S3__PUBLIC_BASE_URL",
            "https://cdn.example.com:8443/site/media/",
        ));
        let direct = ferrite_config::load(dir.path(), env(&direct)).unwrap();
        assert_eq!(
            http_config(&direct).media_origin.as_deref(),
            Some("https://cdn.example.com:8443")
        );
    }

    #[test]
    fn host_config_from_plugins_section() {
        let p = PluginsConfig {
            default_memory: 16 << 20,
            default_fuel: 1000,
            render_deadline_ms: 150,
            action_deadline_ms: 900,
            disable_after_failures: 3,
            failure_window_secs: 60,
            max_concurrent_calls: 5,
            render_budget_ms: 300,
            volatile_ttl_secs: 60,
            trust: Default::default(),
        };
        let h = host_config(&p);
        assert_eq!(h.max_concurrent_calls, 5);
        assert_eq!(h.memory_bytes, 16 << 20);
        assert_eq!(h.fuel, 1000);
        assert_eq!(h.render_deadline, Duration::from_millis(150));
        assert_eq!(h.action_deadline, Duration::from_millis(900));
        assert_eq!(h.disable_after_failures, 3);
        assert_eq!(h.failure_window, Duration::from_secs(60));
        assert_eq!(h.epoch_tick, HostConfig::default().epoch_tick);
    }

    #[test]
    fn shutdown_fits_the_documented_bound() {
        use ferrite_cluster::{ABORT_WAIT, JOB_DRAIN, RELEASE_BUDGET};
        // The runner drains alongside the HTTP drain and ends within its bound.
        assert!(DRAIN <= RUNNER_STOP);
        assert!(JOB_DRAIN + ABORT_WAIT + RELEASE_BUDGET <= RUNNER_STOP);
        assert!(LEAVE_BUDGET < BACKGROUND_STOP);
        // Normal case: HTTP drain, background stop and the full pool close fit.
        assert!(DRAIN + BACKGROUND_STOP + CLOSE_TIMEOUT <= SHUTDOWN_BOUND);
        // Worst case: the runner is abandoned at RUNNER_STOP and gets ABORT_GRACE; the
        // background deadline is anchored at RUNNER_STOP + BACKGROUND_STOP, so that grace
        // and the tasks' own abort grace (inside LEAVE_BUDGET) are absorbed.
        assert!(ABORT_GRACE < BACKGROUND_STOP);
        assert!(ABORT_GRACE < LEAVE_BUDGET);
        let background_end = RUNNER_STOP + BACKGROUND_STOP;
        assert!(background_end <= SHUTDOWN_BOUND);
        // Then the pool close gets what is left of SHUTDOWN_BOUND, then the runtime.
        let close = SHUTDOWN_BOUND
            .saturating_sub(background_end)
            .min(CLOSE_TIMEOUT);
        let total = background_end + close + RUNTIME_SHUTDOWN;
        assert!(total <= Duration::from_secs(14), "{total:?}");
        assert!(SHUTDOWN_BOUND + RUNTIME_SHUTDOWN <= Duration::from_secs(14));
        assert_eq!(Signal::Interrupt.exit_code(), 130);
        #[cfg(unix)]
        assert_eq!(Signal::Terminate.exit_code(), 143);
    }
}
