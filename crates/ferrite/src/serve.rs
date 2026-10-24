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
//! 8. bind `server.listen` (before any background task starts, so a busy port fails
//!    cleanly);
//! 9. the background tasks: the invalidation loop (`cache.poll_interval_ms`; with
//!    `LISTEN` wake-ups in cluster mode) with the [`Reloader`] hook (hot swap of theme,
//!    plugins and permalinks, P18), the shared plugin-failure flusher (P20), the
//!    `request.served` flusher (every 10 s or 1 000 events, P21), the job
//!    runner (`cluster.workers`; built-in prune jobs, scheduled publishing and the
//!    caller's handlers), the periodic scheduler (prune jobs hourly,
//!    `content.publish_due` every 30 s) and the heartbeat (`nodes.meta` carries the
//!    theme, plugin and permalink state; the reloader beats after every reload);
//! 10. serve the public site and the API under `/api/v1` (`build_site`; the API's
//!     services and the site key are set up in step 7, so a missing or bad key stops
//!     startup); [`run`] prints `Listening on http://<addr>` on stdout.
//!
//! Shutdown ([`RunningNode::shutdown`]; SIGINT or SIGTERM in [`run`]): stop accepting
//! connections **and** stop the job runner at once; open requests get up to 10 s while
//! running jobs get 9 s (P10) to finish, after which the runner releases the rest
//! without using an attempt (the runner is abandoned after 12 s, R6). Then the
//! background tasks stop (the failure flusher writes once more, the `request.served`
//! buffer is flushed into jobs once more, for the next runner to claim), mail sends still
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
use ferrite_cache::{InvalidationLoop, PageCache};
use ferrite_cluster::{
    ClusterMode as NodeMode, JobHandler, JobRunner, NodeIdentity, Periodic, Registry, RunnerConfig,
    SCHEDULER_TICK, Scheduler, builtin, node_name,
};
use ferrite_config::{Config, LoggedInCache, PluginsConfig};
use ferrite_core::mail::MailQueue;
use ferrite_core::{
    AuthService, ContentService, ContentTypeService, ExtensionAdmin, ExtensionService,
    MediaService, MenuService, OptionService, Permalinks, RedirectService, SiteReader,
    SwappablePermalinks, TaxonomyService, UserService,
};
use ferrite_domain::{NodeId, Timestamp};
use ferrite_http::{
    AppParts, AppState, FLUSH_EVERY, HttpConfig, LoadSummary, LoggedIn, PluginReloader,
    SERVED_FLUSH_EVERY, ServedBuffer, SharedFailures, StoreHostServices,
    api::{ApiParts, ApiState},
    build_site,
    extensions::Installer,
    load_route_table,
    mail::SmtpMailer,
    media::Uploader,
};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{HostConfig, PluginHost};
use ferrite_store::{Backend, Store};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    task::{JoinError, JoinHandle},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    binary_version,
    cli::DataArg,
    jobs, keys, load_config,
    node::{NodeStatus, node_mode},
    open::{OpenMode, open_store, startup_check},
    reload::{ReloadContext, Reloader},
    site::{load_theme, open_media, permalink_rules},
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

/// Job runner timings that tests shorten; `None` keeps the production value.
#[derive(Debug, Clone, Default)]
pub struct RunnerOverrides {
    pub lease: Option<Duration>,
    pub extend_every: Option<Duration>,
    pub idle_poll: Option<Duration>,
}

/// How [`start`] deviates from a production node; `Default` is production.
#[derive(Default)]
pub struct NodeOptions {
    /// The version this node registers and checks the schema with (rolling-upgrade
    /// tests); `None` is this binary's.
    pub version: Option<semver::Version>,
    /// Handlers run besides the built-in ones.
    pub extra_handlers: Vec<Arc<dyn JobHandler>>,
    pub runner: RunnerOverrides,
    /// Replaces the periodic jobs (default: the built-in prune jobs and
    /// `content.publish_due`).
    pub periodics: Option<Vec<Periodic>>,
    /// Scheduler tick (default `SCHEDULER_TICK`, 10 s).
    pub scheduler_tick: Option<Duration>,
    /// Test seam: the `request.served` flush interval (default `SERVED_FLUSH_EVERY`,
    /// 10 s).
    #[doc(hidden)]
    pub served_flush: Option<Duration>,
    /// Test seam: whether the invalidation loop and the job runner use `LISTEN`
    /// wake-ups (default: in cluster mode). `Some(false)` leaves the node on polling only.
    #[doc(hidden)]
    pub listen: Option<bool>,
    /// Test seam: [`start`] pauses right after the invalidation cursor is set.
    #[doc(hidden)]
    pub boot_barrier: Option<BootBarrier>,
}

/// The node side of a boot pause (test seam; see [`NodeOptions::boot_barrier`]).
#[doc(hidden)]
pub struct BootBarrier {
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

/// The test side of a boot pause.
#[doc(hidden)]
pub struct BootGate {
    reached: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
}

impl BootBarrier {
    /// A barrier for [`NodeOptions::boot_barrier`] and the gate that controls it.
    pub fn new() -> (BootBarrier, BootGate) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
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
    pub node_id: NodeId,
    pub state: Arc<AppState>,
    /// The API's state (its services).
    pub api: Arc<ApiState>,
    server: Option<JoinHandle<std::io::Result<()>>>,
    /// The server's result when it ended before shutdown.
    server_result: Option<Result<std::io::Result<()>, JoinError>>,
    stop_accepting: CancellationToken,
    runner_stop: CancellationToken,
    runner: JoinHandle<()>,
    background: CancellationToken,
    /// Reloader, invalidation loop, failure flusher, `request.served` flusher, scheduler,
    /// heartbeat.
    tasks: Vec<(&'static str, JoinHandle<()>)>,
    /// The outgoing-mail queue (`[mail]` set); its sends are cancelled at shutdown.
    mail: Option<Arc<MailQueue>>,
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
    let version = match &opts.version {
        Some(v) => v.clone(),
        None => binary_version()?,
    };
    startup_check(store.as_ref(), &version, &config.database).await?;
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
    let mode = config.effective_cluster_mode();
    let listen = opts.listen.unwrap_or(node_mode(mode) == NodeMode::Cluster);
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
        .with_site_key(site_key.clone());
    if let Some(queue) = &mail {
        services = services.with_mailer(queue.clone(), config.site.title.clone());
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
            .with_issuer_name(config.site.title.clone()),
    );
    // `request.served` events of this node's page responses (P21).
    let served = Arc::new(ServedBuffer::new(store.clone(), host.clone()));
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
        .with_auth(auth.clone())
        .with_served(served.clone()),
    );

    let status = Arc::new(NodeStatus::default());
    status.set_theme(&loaded, loaded.error.as_deref(), loaded.fallback);
    status.set_plugins(&report);

    // The API's services; `content` also runs the scheduled-publishing jobs.
    let content = Arc::new(ContentService::new(store.clone(), dyn_permalinks.clone()));
    let media_service = Arc::new(MediaService::new(
        store.clone(),
        content.clone(),
        dyn_permalinks,
    ));
    // One upload pipeline per node: its slots bound the node (M2a-2b P9).
    let max_upload = config.uploads.max_upload;
    let uploader = Arc::new(
        Uploader::new(media.clone(), max_upload)
            .with_idle_timeout(Duration::from_secs(config.uploads.idle_timeout_secs)),
    );
    // One install pipeline per node: its compile slots bound the node (P8).
    let installer = Arc::new(
        Installer::new(store.clone(), media.clone(), host_config(&config.plugins))
            .context("cannot start the plugin installer")?,
    );
    let api = Arc::new(
        ApiState::new(ApiParts {
            store: store.clone(),
            auth,
            users: Arc::new(UserService::new(store.clone())),
            content: content.clone(),
            types: Arc::new(ContentTypeService::new(store.clone())),
            taxonomy: Arc::new(TaxonomyService::new(store.clone())),
            menus: Arc::new(MenuService::new(store.clone())),
            options: Arc::new(OptionService::new(store.clone())),
            redirects: Arc::new(RedirectService::new(store.clone())),
            extensions: Arc::new(ExtensionAdmin::new(store.clone(), site_key)),
            installer,
            media: media.clone(),
            media_service,
            uploader,
            max_upload,
            mailer: mail.clone(),
            site_url: state.config.site_url.clone(),
            site_title: config.site.title.clone(),
        })
        .context("site.url")?,
    );
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
    handlers.extend(opts.extra_handlers);
    let mut runner_config = RunnerConfig::new(registry.id(), config.cluster.workers);
    let o = &opts.runner;
    runner_config.lease = o.lease.unwrap_or(runner_config.lease);
    runner_config.extend_every = o.extend_every.unwrap_or(runner_config.extend_every);
    runner_config.idle_poll = o.idle_poll.unwrap_or(runner_config.idle_poll);
    runner_config.listen = listen;
    let runner = JobRunner::new(store.clone(), runner_config, handlers)
        .context("cannot set up the job runner")?;
    let periodics = opts.periodics.unwrap_or_else(|| {
        let mut p = builtin::periodics();
        p.push(jobs::publish_due_periodic());
        p.push(jobs::plugin_purge_sweep_periodic());
        p
    });
    let scheduler = Scheduler::new(
        store.clone(),
        periodics,
        opts.scheduler_tick.unwrap_or(SCHEDULER_TICK),
    );

    let listener = TcpListener::bind(config.server.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.server.listen))?;
    let addr = listener.local_addr()?;

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
    let served_flusher = served.spawn_flusher(
        opts.served_flush.unwrap_or(SERVED_FLUSH_EVERY),
        background.clone(),
    );
    let runner_stop = CancellationToken::new();
    let runner = runner.spawn(runner_stop.clone());
    let scheduler = scheduler.spawn(background.clone());
    let heartbeat = registry.spawn_heartbeat(status.meta_fn(), background.clone());
    // The row was registered without the node's state; publish it now.
    registry.beat_now();

    let stop_accepting = CancellationToken::new();
    let server = tokio::spawn(
        axum::serve(
            listener,
            build_site(state.clone(), api.clone())
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(stop_accepting.clone().cancelled_owned())
        .into_future(),
    );
    tracing::info!(listen = %format!("http://{addr}"), site = %state.config.site_url, node = %registry.id(), "serving");
    Ok(RunningNode {
        addr,
        node_id: registry.id(),
        state,
        api,
        server: Some(server),
        server_result: None,
        stop_accepting,
        runner_stop,
        runner,
        background,
        tasks: vec![
            ("reloader", reload_task),
            ("invalidation loop", poller),
            ("plugin failure flusher", flusher),
            ("request.served flusher", served_flusher),
            ("scheduler", scheduler),
            ("heartbeat", heartbeat),
        ],
        mail,
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
    #[doc(hidden)]
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
    http.trusted_proxies = config.server.trusted_proxies.clone();
    http.render_budget = Duration::from_millis(config.plugins.render_budget_ms);
    http.logged_in = match config.cache.logged_in {
        LoggedInCache::Bypass => LoggedIn::Bypass,
        LoggedInCache::PerUser => LoggedIn::PerUser,
    };
    http
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

#[cfg(test)]
mod tests {
    use super::*;

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
