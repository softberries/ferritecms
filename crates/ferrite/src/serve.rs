//! `ferrite serve`: the public site.
//!
//! Startup, in order (any failure stops the process with exit code 1): load the
//! config; open the database (created if missing); the migration check (02 §8:
//! pending migrations are applied when allowed, an incompatible schema is refused);
//! open the media store and probe it; build the permalink rules from the
//! `permalink.post`/`permalink.page` options and check them against the registered
//! types and taxonomies; load the active theme (a broken one falls back to the
//! embedded `blog` theme, 06 §7); install the enabled plugins (a broken one is
//! skipped); start the cache invalidation loop (`cache.poll_interval_ms`) and the
//! scheduled-publishing ticker (every 30 s; its writes reach the cache through the
//! event outbox like any other); bind `server.listen` and print
//! `Listening on http://<addr>` on stdout.
//!
//! Permalink options, the theme and plugins are read once at startup; changing them
//! needs a restart in M1c-2 (hot swap is M1.5).
//!
//! Shutdown on SIGINT or SIGTERM: stop accepting connections, let open requests finish
//! for up to 10 s, stop the background tasks (up to 1 s), close the database pools (up
//! to 2 s); `main` then shuts the runtime down with a 1 s grace, which drops every
//! connection task still running (axum spawns them, so they cannot be aborted
//! individually) and abandons stuck blocking work. A clean shutdown exits 0, and the
//! process is gone at most about 14 s after the first signal. A second SIGINT/SIGTERM
//! during the drain exits at once with 130 (SIGINT) or 143 (SIGTERM).

use std::{future::IntoFuture, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use ferrite_cache::{InvalidationLoop, PageCache};
use ferrite_config::{Config, PluginsConfig};
use ferrite_core::{ContentService, SiteReader};
use ferrite_domain::Timestamp;
use ferrite_http::{
    AppParts, AppState, HttpConfig, StoreHostServices, build_app, load_plugins, load_route_table,
};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{HostConfig, PluginHost};
use ferrite_store::Store;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    binary_version,
    cli::DataArg,
    load_config,
    open::{OpenMode, open_store, startup_check},
    site::{load_theme, open_media, permalink_rules},
};

/// How often scheduled items are published.
const PUBLISH_DUE_EVERY: Duration = Duration::from_secs(30);
/// How long open requests may run after a shutdown signal.
const DRAIN: Duration = Duration::from_secs(10);
/// How long the background tasks may take to stop after the drain.
const BACKGROUND_STOP: Duration = Duration::from_secs(1);
/// How long closing the database pools may take at shutdown (a request dropped
/// mid-transaction can keep a connection checked out).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Grace `main` gives the runtime's remaining tasks when it shuts down.
pub const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(1);

pub async fn run(args: DataArg) -> anyhow::Result<()> {
    let config = load_config(&args.data)?;
    let store = open_store(&config, OpenMode::CreateIfMissing).await?;
    let served = serve(&config, store.clone()).await;
    if tokio::time::timeout(CLOSE_TIMEOUT, store.close())
        .await
        .is_err()
    {
        tracing::warn!("database pools did not close within {CLOSE_TIMEOUT:?}");
    }
    served
}

async fn serve(config: &Config, store: Arc<dyn Store>) -> anyhow::Result<()> {
    startup_check(store.as_ref(), &binary_version()?, &config.database).await?;
    let media = Arc::new(open_media(config)?);
    media
        .check()
        .await
        .context("the media store is not usable (write/read/delete probe failed)")?;
    let rules = Arc::new(permalink_rules(store.as_ref()).await?);
    let theme = Arc::new(load_theme(store.as_ref(), &config.data_dir).await?);
    tracing::info!(theme = %theme.name(), hash = %theme.hash(), "theme loaded");

    let mut http = HttpConfig::new(
        config.site.url.as_str(),
        &config.site.title,
        &config.site.locale,
    );
    http.trusted_proxies = config.server.trusted_proxies.clone();
    let reader = Arc::new(SiteReader::new(
        store.clone(),
        rules.clone(),
        media_url(media.clone()),
        Arc::new(Timestamp::now),
    ));
    let services = Arc::new(StoreHostServices::new(
        store.clone(),
        reader.clone(),
        &http.site_url,
    ));
    let plugins = Arc::new(
        PluginHost::new(host_config(&config.plugins), services)
            .context("cannot start the plugin host")?,
    );
    load_plugins(store.as_ref(), &media, &plugins)
        .await
        .context("cannot list the installed plugins")?;
    let routes = load_route_table(store.as_ref(), &rules)
        .await
        .context("cannot load content types and taxonomies")?;
    let cache = Arc::new(PageCache::new(config.cache.max_bytes));
    let site_url = http.site_url.clone();
    let state = Arc::new(AppState::new(AppParts {
        store: store.clone(),
        reader,
        plugins,
        media,
        theme,
        cache: cache.clone(),
        routes,
        config: http,
        migrations_ok: true,
    }));

    let background = CancellationToken::new();
    let poller = InvalidationLoop::for_store(store.clone(), cache)
        .await
        .context("cannot read the event outbox")?
        .spawn(
            Duration::from_millis(config.cache.poll_interval_ms),
            background.clone(),
        );
    let publisher = spawn_publish_due(ContentService::new(store, rules), background.clone());

    // Signal handlers before the address is announced: a signal right after it must
    // shut down cleanly, not kill the process.
    let mut signals = Signals::install().context("cannot install signal handlers")?;
    let listener = TcpListener::bind(config.server.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.server.listen))?;
    let addr = listener.local_addr()?;
    tracing::info!(listen = %format!("http://{addr}"), site = %site_url, "serving");
    println!("Listening on http://{addr}");

    let stop_accepting = CancellationToken::new();
    let mut server = tokio::spawn(
        axum::serve(
            listener,
            build_app(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(stop_accepting.clone().cancelled_owned())
        .into_future(),
    );
    let served = tokio::select! {
        r = &mut server => r,
        first = signals.recv() => {
            tracing::info!(
                signal = first.name(),
                "shutdown signal received; draining connections (up to {DRAIN:?})"
            );
            stop_accepting.cancel();
            tokio::select! {
                r = &mut server => r,
                () = tokio::time::sleep(DRAIN) => {
                    tracing::warn!(
                        "connections still open after {DRAIN:?}; they are dropped at exit"
                    );
                    server.abort();
                    Ok(Ok(()))
                }
                second = signals.recv() => {
                    tracing::error!(
                        signal = second.name(),
                        "second signal during shutdown; exiting immediately"
                    );
                    std::process::exit(second.exit_code());
                }
            }
        }
    };
    background.cancel();
    for task in [poller, publisher] {
        let abort = task.abort_handle();
        match tokio::time::timeout(BACKGROUND_STOP, task).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "background task ended abnormally"),
            Err(_) => {
                tracing::warn!("background task did not stop within {BACKGROUND_STOP:?}");
                abort.abort();
            }
        }
    }
    served
        .context("the server task failed")?
        .context("the server stopped")?;
    tracing::info!("shut down");
    Ok(())
}

/// Public media URLs as the media store builds them (`/_media/…` or the CDN).
fn media_url(media: Arc<MediaStore>) -> ferrite_core::site::MediaUrlFn {
    Arc::new(move |hash, name| media.public_url(hash, name))
}

/// The plugin host limits from `[plugins]`; the rest are the host defaults.
fn host_config(p: &PluginsConfig) -> HostConfig {
    HostConfig {
        memory_bytes: usize::try_from(p.default_memory).unwrap_or(usize::MAX),
        fuel: p.default_fuel,
        render_deadline: Duration::from_millis(p.render_deadline_ms),
        action_deadline: Duration::from_millis(p.action_deadline_ms),
        disable_after_failures: p.disable_after_failures,
        failure_window: Duration::from_secs(p.failure_window_secs),
        ..HostConfig::default()
    }
}

/// Publishes due scheduled items now and every [`PUBLISH_DUE_EVERY`] until `stop`.
/// Safe on every node of a cluster at once (`publish_due` publishes each item once).
fn spawn_publish_due(svc: ContentService, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PUBLISH_DUE_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => return,
                _ = ticker.tick() => match svc.publish_due(Timestamp::now()).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(published = n, "published scheduled items"),
                    Err(e) => tracing::warn!(error = %e, "publishing scheduled items failed"),
                },
            }
        }
    })
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

    #[test]
    fn host_config_from_plugins_section() {
        let p = PluginsConfig {
            default_memory: 16 << 20,
            default_fuel: 1000,
            render_deadline_ms: 150,
            action_deadline_ms: 900,
            disable_after_failures: 3,
            failure_window_secs: 60,
        };
        let h = host_config(&p);
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
        let total = DRAIN + BACKGROUND_STOP + CLOSE_TIMEOUT + RUNTIME_SHUTDOWN;
        assert!(total <= Duration::from_secs(14), "{total:?}");
        assert_eq!(Signal::Interrupt.exit_code(), 130);
        #[cfg(unix)]
        assert_eq!(Signal::Terminate.exit_code(), 143);
    }
}
