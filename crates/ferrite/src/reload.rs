//! Hot swap of node-local state (rulings P18, P22; design 04 §8): the theme, the plugin
//! registry and the permalink rules with the route table.
//!
//! [`Reloader`] is the invalidation loop's [`EventHook`]. On the loop's task it only
//! sets dirty flags and wakes one background task (a burst of events costs one reload):
//!
//! | event | reloads |
//! |---|---|
//! | `theme.changed` | theme |
//! | `plugins.changed` | plugins |
//! | `plugin.disabled` | the plugin is disabled in this host at once, then plugins |
//! | `options.changed` with `option:permalink.post` or `option:permalink.page` | permalinks |
//! | history lost | all three |
//!
//! The task re-reads the state from the database (never from event payloads), swaps it
//! in and flushes the page cache after every completed swap: the loop flushed on the
//! event already, and this second flush drops pages rendered with the old state during
//! the swap (the cache refuses in-flight renders that started before a flush). A
//! reload that fails keeps the current state, logs, and reports the error in
//! `nodes.meta` (there is no runtime fallback to the `blog` theme). After each round the
//! heartbeat is asked to beat at once so `nodes.meta` shows the new state.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ferrite_cache::EventHook;
use ferrite_cluster::Registry;
use ferrite_config::ClusterMode;
use ferrite_core::{
    SwappablePermalinks,
    events::{OPTIONS_CHANGED, PLUGIN_DISABLED, PLUGINS_CHANGED, THEME_CHANGED, option_key},
};
use ferrite_domain::Event;
use ferrite_http::{AppState, PluginReloader, load_route_table};
use ferrite_media::MediaStore;
use ferrite_plugin_host::PluginHost;
use ferrite_store::Store;
use tokio::{sync::Notify, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    node::NodeStatus,
    site::{LoadedTheme, permalink_rules, reload_theme},
};

/// Option keys whose change rebuilds the permalink rules and the route table.
const PERMALINK_OPTIONS: [&str; 2] = ["permalink.post", "permalink.page"];
/// Reason given to the host when a `plugin.disabled` event carries none.
const DISABLED_ELSEWHERE: &str = "disabled on another node";

#[derive(Default)]
struct Dirty {
    theme: AtomicBool,
    plugins: AtomicBool,
    permalinks: AtomicBool,
}

/// The node's [`EventHook`]; see the module docs.
pub struct Reloader {
    dirty: Dirty,
    wake: Notify,
    host: Arc<PluginHost>,
}

/// What the reload task works on.
pub(crate) struct ReloadContext {
    pub store: Arc<dyn Store>,
    pub media: Arc<MediaStore>,
    pub data_dir: PathBuf,
    /// Resolved (`Config::effective_cluster_mode`).
    pub mode: ClusterMode,
    pub state: Arc<AppState>,
    pub permalinks: Arc<SwappablePermalinks>,
    /// The process's one reloader (it remembers what it loaded).
    pub plugins: Arc<PluginReloader>,
    /// The theme being served.
    pub theme: LoadedTheme,
    pub status: Arc<NodeStatus>,
    pub registry: Registry,
}

impl Reloader {
    /// A hook that disables plugins in `host` on `plugin.disabled`.
    pub fn new(host: Arc<PluginHost>) -> Arc<Self> {
        Arc::new(Self {
            dirty: Dirty::default(),
            wake: Notify::new(),
            host,
        })
    }

    fn mark(&self, flag: &AtomicBool) {
        flag.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    /// Runs the reload task until `stop`; a reload in progress is abandoned on `stop`.
    pub(crate) fn spawn(
        self: &Arc<Self>,
        mut ctx: ReloadContext,
        stop: CancellationToken,
    ) -> JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    () = this.wake.notified() => {}
                }
                let round = this.round(&mut ctx);
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    () = round => {}
                }
            }
        })
    }

    /// One coalesced round: every flag set so far.
    async fn round(&self, ctx: &mut ReloadContext) {
        let take = |f: &AtomicBool| f.swap(false, Ordering::AcqRel);
        let (permalinks, theme, plugins) = (
            take(&self.dirty.permalinks),
            take(&self.dirty.theme),
            take(&self.dirty.plugins),
        );
        if permalinks {
            ctx.reload_permalinks().await;
        }
        if theme {
            ctx.reload_theme().await;
        }
        if plugins {
            ctx.reload_plugins().await;
        }
        if permalinks || theme || plugins {
            ctx.registry.beat_now();
        }
    }
}

impl EventHook for Reloader {
    fn on_event(&self, event: &Event) {
        match event.kind.as_str() {
            THEME_CHANGED => self.mark(&self.dirty.theme),
            PLUGINS_CHANGED => self.mark(&self.dirty.plugins),
            PLUGIN_DISABLED => {
                if let Some(name) = event.keys.first() {
                    let reason = event.keys.get(1).map_or(DISABLED_ELSEWHERE, String::as_str);
                    self.host.disable(name, reason);
                }
                self.mark(&self.dirty.plugins);
            }
            OPTIONS_CHANGED => {
                let permalink = PERMALINK_OPTIONS
                    .iter()
                    .any(|key| event.keys.contains(&option_key(key)));
                if permalink {
                    self.mark(&self.dirty.permalinks);
                }
            }
            _ => {}
        }
    }

    fn on_history_lost(&self) {
        self.dirty.theme.store(true, Ordering::Release);
        self.dirty.plugins.store(true, Ordering::Release);
        self.dirty.permalinks.store(true, Ordering::Release);
        self.wake.notify_one();
    }
}

impl ReloadContext {
    /// New rules from the options, checked, then the route table built with them; both
    /// swapped only if both succeed.
    async fn reload_permalinks(&mut self) {
        let built = async {
            let rules = permalink_rules(self.store.as_ref()).await?;
            let table = load_route_table(self.store.as_ref(), &rules).await?;
            anyhow::Ok((rules, table))
        }
        .await;
        match built {
            Ok((rules, table)) => {
                self.permalinks.swap(rules);
                self.state.swap_routes(table);
                self.state.cache.flush();
                self.status.set_permalinks_error(None);
                tracing::info!("permalink rules and routes reloaded");
            }
            Err(e) => {
                let error = format!("{e:#}");
                tracing::error!(error = %error, "permalink reload failed; keeping the current rules");
                self.status.set_permalinks_error(Some(&error));
            }
        }
    }

    async fn reload_theme(&mut self) {
        let next = reload_theme(
            self.store.as_ref(),
            &self.media,
            &self.data_dir,
            self.mode,
            &self.theme,
        )
        .await;
        match next {
            Ok(None) => self.status.set_theme(&self.theme, None),
            Ok(Some(loaded)) => {
                self.state.swap_theme(loaded.theme.clone());
                self.state.cache.flush();
                tracing::info!(
                    theme = %loaded.theme.name(),
                    hash = %loaded.theme.hash(),
                    origin = ?loaded.origin,
                    "theme swapped"
                );
                self.status.set_theme(&loaded, loaded.error.as_deref());
                self.theme = loaded;
            }
            Err(error) => {
                tracing::error!(
                    error = %error,
                    keeping = %self.theme.theme.name(),
                    "theme reload failed; keeping the current theme"
                );
                self.status.set_theme(&self.theme, Some(&error));
            }
        }
    }

    async fn reload_plugins(&mut self) {
        match self.plugins.reload().await {
            Ok(report) => {
                // After the commit (ruling R13): drops pages that mixed versions.
                self.state.cache.flush();
                self.status.set_plugins(&report);
            }
            Err(e) => tracing::error!(
                error = %e,
                "plugin reload failed (cannot list plugins); keeping the current registry"
            ),
        }
    }
}
