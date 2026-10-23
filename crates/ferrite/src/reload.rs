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
//!
//! **Retry (ruling R14).** A failed reload (theme or permalinks that do not load, a
//! plugin row that does not load, or the rows that cannot be read) is retried per
//! component after [`RETRY_MIN`], doubling up to [`RETRY_MAX`], until it succeeds; a
//! newer event for that component reloads at once and restarts the backoff. So a
//! transient failure (media store briefly unreachable) heals without another change.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ferrite_cache::EventHook;
use ferrite_cluster::Registry;
use ferrite_config::ClusterMode;
use ferrite_core::{
    SwappablePermalinks,
    events::{OPTIONS_CHANGED, PLUGIN_DISABLED, PLUGINS_CHANGED, THEME_CHANGED, option_key},
};
use ferrite_domain::Event;
use ferrite_http::{AppState, PluginReloader, PluginStatus, ReloadReport, load_route_table};
use ferrite_media::MediaStore;
use ferrite_plugin_host::PluginHost;
use ferrite_store::Store;
use tokio::{sync::Notify, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    node::NodeStatus,
    site::{LoadedTheme, permalink_rules, reload_theme},
};

/// Option keys whose change rebuilds the permalink rules and the route table.
const PERMALINK_OPTIONS: [&str; 2] = ["permalink.post", "permalink.page"];
/// First delay before a failed reload is retried.
pub const RETRY_MIN: Duration = Duration::from_secs(5);
/// The retry delay doubles up to this.
pub const RETRY_MAX: Duration = Duration::from_secs(5 * 60);

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
    /// What the last plugin reload reported (startup's first), and its list error.
    pub plugins_report: ReloadReport,
    pub plugins_error: Option<String>,
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
            let mut retries = [Retry::default(); 3];
            loop {
                let next = retries.iter().filter_map(|r| r.due).min();
                let retry_due = async {
                    match next {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    () = this.wake.notified() => {}
                    () = retry_due => {}
                }
                let round = this.round(&mut ctx, &mut retries);
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    () = round => {}
                }
            }
        })
    }

    /// One coalesced round: every component whose flag is set or whose retry is due.
    async fn round(&self, ctx: &mut ReloadContext, retries: &mut [Retry; 3]) {
        let take = |f: &AtomicBool| f.swap(false, Ordering::AcqRel);
        let events = [
            take(&self.dirty.permalinks),
            take(&self.dirty.theme),
            take(&self.dirty.plugins),
        ];
        let now = Instant::now();
        let mut changed = false;
        for (i, (event, retry)) in events.into_iter().zip(retries.iter_mut()).enumerate() {
            if !event && !retry.due.is_some_and(|at| at <= now) {
                continue;
            }
            let ok = match i {
                0 => {
                    changed = true;
                    ctx.reload_permalinks().await
                }
                1 => {
                    changed = true;
                    ctx.reload_theme().await
                }
                _ => {
                    let (ok, plugins_changed) = ctx.reload_plugins(event).await;
                    changed |= plugins_changed;
                    ok
                }
            };
            retry.after(ok, event, Instant::now());
        }
        if changed {
            ctx.registry.beat_now();
        }
    }
}

/// The retry state of one component.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Retry {
    /// When the failed reload runs again (`None`: nothing failed).
    due: Option<Instant>,
    /// The delay used for the pending retry.
    delay: Duration,
}

impl Retry {
    /// Updates after a reload: success clears; a failure after an event retries after
    /// [`RETRY_MIN`], a failed retry after twice the last delay (at most [`RETRY_MAX`]).
    fn after(&mut self, ok: bool, event: bool, now: Instant) {
        if ok {
            *self = Retry::default();
            return;
        }
        self.delay = if event || self.due.is_none() {
            RETRY_MIN
        } else {
            self.delay.saturating_mul(2).min(RETRY_MAX)
        };
        self.due = Some(now + self.delay);
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
    async fn reload_permalinks(&mut self) -> bool {
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
                true
            }
            Err(e) => {
                let error = format!("{e:#}");
                tracing::error!(error = %error, "permalink reload failed; keeping the current rules");
                self.status.set_permalinks_error(Some(&error));
                false
            }
        }
    }

    async fn reload_theme(&mut self) -> bool {
        let next = reload_theme(
            self.store.as_ref(),
            &self.media,
            &self.data_dir,
            self.mode,
            &self.theme,
        )
        .await;
        match next {
            Ok(None) => {
                self.status.set_theme(&self.theme, None);
                true
            }
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
                true
            }
            Err(error) => {
                tracing::error!(
                    error = %error,
                    keeping = %self.theme.theme.name(),
                    "theme reload failed; keeping the current theme"
                );
                self.status.set_theme(&self.theme, Some(&error));
                false
            }
        }
    }

    /// `(ok, changed)`: `ok` is `false` when the rows cannot be read or any row failed
    /// to load; `changed` when the reload was asked for by an event, or its report or
    /// list error differs from the last reload's. Only a changed reload flushes the
    /// page cache, so a retry that finds the same state (a row still broken) costs no
    /// cache. An event-driven reload always flushes: a row change the report does not
    /// show (a new manifest or grant with the same version and hash) may still have
    /// swapped an instance (R13).
    async fn reload_plugins(&mut self, event: bool) -> (bool, bool) {
        match self.plugins.reload().await {
            Ok(report) => {
                let changed =
                    event || report != self.plugins_report || self.plugins_error.is_some();
                if changed {
                    // After the commit (ruling R13): drops pages that mixed versions.
                    self.state.cache.flush();
                    self.status.set_plugins(&report);
                    self.status.set_plugins_error(None);
                }
                let ok = !report
                    .states
                    .values()
                    .any(|s| matches!(s.status, PluginStatus::Failed(_)));
                self.plugins_report = report;
                self.plugins_error = None;
                (ok, changed)
            }
            Err(e) => {
                let error = format!("cannot list the plugins: {e}");
                tracing::error!(error = %error, "plugin reload failed; keeping the current registry");
                let changed = self.plugins_error.as_deref() != Some(error.as_str());
                if changed {
                    self.status.set_plugins_error(Some(&error));
                }
                self.plugins_error = Some(error);
                (false, changed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ferrite_plugin_host::{HostConfig, InMemoryServices, Manifest};

    use super::*;
    use crate::test_support;

    fn event(kind: &str, keys: &[&str]) -> Event {
        Event {
            seq: 1,
            kind: kind.to_owned(),
            keys: keys.iter().map(|k| (*k).to_owned()).collect(),
            origin_node: None,
            created_at: ferrite_domain::Timestamp::now(),
        }
    }

    fn flags(r: &Reloader) -> [bool; 3] {
        [
            r.dirty.theme.load(Ordering::Acquire),
            r.dirty.plugins.load(Ordering::Acquire),
            r.dirty.permalinks.load(Ordering::Acquire),
        ]
    }

    fn host() -> Arc<PluginHost> {
        Arc::new(PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new())).unwrap())
    }

    #[test]
    fn plugin_disabled_disables_at_once_and_marks_plugins() {
        let host = host();
        let dir = test_support::repo_root().join("plugins/examples/hello");
        let manifest =
            Manifest::parse(&std::fs::read_to_string(dir.join("ferrite-plugin.toml")).unwrap())
                .unwrap();
        host.install(manifest, &test_support::plugin_wasm("hello"))
            .unwrap();
        let r = Reloader::new(host.clone());
        r.on_event(&event(
            PLUGIN_DISABLED,
            &["hello", "auto: 20 failures in window"],
        ));
        assert_eq!(
            host.disabled_reason("hello").as_deref(),
            Some("auto: 20 failures in window")
        );
        assert_eq!(flags(&r), [false, true, false]);
        // Unknown plugin, no reason: only the flag.
        r.on_event(&event(PLUGIN_DISABLED, &["other"]));
        assert_eq!(flags(&r), [false, true, false]);
    }

    #[test]
    fn events_mark_their_component() {
        let r = Reloader::new(host());
        r.on_event(&event(OPTIONS_CHANGED, &["option:site.title"]));
        r.on_event(&event("invalidate", &["all"]));
        assert_eq!(flags(&r), [false, false, false]);
        r.on_event(&event(OPTIONS_CHANGED, &["option:permalink.page"]));
        assert_eq!(flags(&r), [false, false, true]);
        r.on_event(&event(THEME_CHANGED, &["b"]));
        assert_eq!(flags(&r), [true, false, true]);
    }

    #[test]
    fn history_lost_marks_all_three() {
        let r = Reloader::new(host());
        r.on_history_lost();
        assert_eq!(flags(&r), [true, true, true]);
    }

    #[test]
    fn retry_backoff_doubles_and_resets() {
        let now = Instant::now();
        let mut r = Retry::default();
        r.after(false, true, now);
        assert_eq!(r.delay, RETRY_MIN);
        assert_eq!(r.due, Some(now + RETRY_MIN));
        for expect in [10, 20, 40, 80, 160, 300, 300] {
            r.after(false, false, now);
            assert_eq!(r.delay, Duration::from_secs(expect));
        }
        // A newer event restarts the backoff; success clears it.
        r.after(false, true, now);
        assert_eq!(r.delay, RETRY_MIN);
        r.after(true, false, now);
        assert_eq!(r, Retry::default());
    }
}
