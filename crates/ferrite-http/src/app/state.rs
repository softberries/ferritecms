//! What every request handler shares.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use ferrite_cache::PageCache;
use ferrite_core::{
    CoreError, DepKeys, PermalinkRules, RedirectService, SiteReader, events::option_key,
};
use ferrite_plugin_host::PluginHost;
use ferrite_store::{Store, StoreError};
use ferrite_theme::{Renderer, SiteInfo, Theme};
use tokio::sync::Semaphore;

use crate::route::{RouteTable, TypeInfo};

/// Content type that is never routed (media items are only shown inside other items).
const MEDIA_KIND: &str = "media";
/// TTL of pages marked volatile by plugins (05 §7a) and of search results (06 §6).
pub const SHORT_TTL: Duration = Duration::from_secs(60);
/// How long a render waits for a free render slot before the request gets a 503.
pub const RENDER_WAIT: Duration = Duration::from_secs(5);

/// Option holding the site title; falls back to [`HttpConfig::site_title`].
pub const TITLE_OPTION: &str = "site.title";
/// Option holding the tagline; falls back to [`HttpConfig::site_description`].
pub const DESCRIPTION_OPTION: &str = "site.description";
/// Option holding the locale; falls back to [`HttpConfig::locale`].
pub const LOCALE_OPTION: &str = "site.locale";

/// Option holding the active theme's settings: a JSON object, key = setting name.
pub fn theme_settings_option(theme: &str) -> String {
    format!("theme.{theme}.settings")
}

/// The HTTP layer's settings, mapped from `ferrite.toml` by the binary.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Absolute site URL without a trailing slash (`https://example.com`). Always from
    /// the config: it is what the operator deployed, not an editable option.
    pub site_url: String,
    /// Defaults for the `site.*` options when they are not set.
    pub site_title: String,
    pub site_description: String,
    pub locale: String,
    /// Cache TTL of pages whose plugin calls were volatile or failed, or whose theme
    /// helper reads failed.
    pub volatile_ttl: Duration,
    /// Cache TTL of search results.
    pub search_ttl: Duration,
    /// Host (and port) of `site_url`: the host of every cache key (ruling R25; the
    /// `Host` header is not used, so clients cannot multiply cache entries).
    pub default_host: String,
    /// Most template renders running at once (each holds a blocking thread); default
    /// twice the available parallelism (ruling R28).
    pub max_concurrent_renders: usize,
    /// How long a render waits for a slot before answering 503 with `Retry-After`.
    pub render_wait: Duration,
}

impl HttpConfig {
    pub fn new(site_url: &str, site_title: &str, locale: &str) -> Self {
        let site_url = site_url.trim_end_matches('/').to_owned();
        let default_host = site_url
            .split_once("://")
            .map_or(site_url.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        Self {
            site_url,
            site_title: site_title.to_owned(),
            site_description: String::new(),
            locale: locale.to_owned(),
            volatile_ttl: SHORT_TTL,
            search_ttl: SHORT_TTL,
            default_host,
            max_concurrent_renders: std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .saturating_mul(2),
            render_wait: RENDER_WAIT,
        }
    }
}

/// The parts an [`AppState`] is built from.
pub struct AppParts {
    pub store: Arc<dyn Store>,
    /// Must use the same permalink rules as `routes`.
    pub reader: Arc<SiteReader>,
    pub plugins: Arc<PluginHost>,
    pub theme: Arc<Theme>,
    pub cache: Arc<PageCache>,
    pub routes: RouteTable,
    pub config: HttpConfig,
    /// The result of the startup migration check (`/readyz` reports it).
    pub migrations_ok: bool,
}

/// Shared state of the public site. The theme and the route table can be swapped at
/// runtime (theme activation, permalink option changes; wired in later tasks).
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub reader: Arc<SiteReader>,
    pub plugins: Arc<PluginHost>,
    pub theme: ArcSwap<Theme>,
    pub cache: Arc<PageCache>,
    pub routes: ArcSwap<RouteTable>,
    pub config: HttpConfig,
    pub migrations_ok: bool,
    pub(crate) redirects: Arc<RedirectService>,
    pub(crate) renderer: Renderer,
    /// Render slots ([`HttpConfig::max_concurrent_renders`]).
    pub(crate) render_slots: Arc<Semaphore>,
    renders: AtomicU64,
}

impl AppState {
    pub fn new(parts: AppParts) -> Self {
        Self {
            redirects: Arc::new(RedirectService::new(parts.store.clone())),
            renderer: Renderer::new(),
            render_slots: Arc::new(Semaphore::new(
                parts
                    .config
                    .max_concurrent_renders
                    .min(Semaphore::MAX_PERMITS),
            )),
            renders: AtomicU64::new(0),
            store: parts.store,
            reader: parts.reader,
            plugins: parts.plugins,
            theme: ArcSwap::new(parts.theme),
            cache: parts.cache,
            routes: ArcSwap::from_pointee(parts.routes),
            config: parts.config,
            migrations_ok: parts.migrations_ok,
        }
    }

    /// Template renders started since the app was built (pages and error pages): a
    /// metric, and how tests tell a cache hit from a render.
    pub fn renders(&self) -> u64 {
        self.renders.load(Ordering::Relaxed)
    }

    pub(crate) fn count_render(&self) {
        self.renders.fetch_add(1, Ordering::Relaxed);
    }

    /// `site` for templates: the `site.*` options (config defaults when unset or not
    /// a string) and the theme's settings, read per render (cache misses only) and
    /// recorded as `option:<key>`. The renderer records the `site.*` keys itself; the
    /// settings key is recorded here.
    pub(crate) async fn site_info(
        &self,
        theme: &Theme,
        deps: &mut DepKeys,
    ) -> Result<SiteInfo, CoreError> {
        let options = self.store.read().options();
        let text = |v: Option<ferrite_domain::OptionEntry>, default: &str| {
            v.and_then(|e| e.value.as_str().map(str::to_owned))
                .unwrap_or_else(|| default.to_owned())
        };
        let title = text(options.get(TITLE_OPTION).await?, &self.config.site_title);
        let description = text(
            options.get(DESCRIPTION_OPTION).await?,
            &self.config.site_description,
        );
        let locale = text(options.get(LOCALE_OPTION).await?, &self.config.locale);
        let key = theme_settings_option(theme.name());
        deps.insert(option_key(&key));
        let stored = options
            .get(&key)
            .await?
            .and_then(|e| e.value.as_object().cloned())
            .unwrap_or_default();
        Ok(SiteInfo {
            title,
            url: self.config.site_url.clone(),
            description,
            locale,
            settings: theme.manifest().settings_values(&stored),
        })
    }

    /// `site` from the config defaults alone, for error pages when the store fails.
    pub(crate) fn fallback_site(&self, theme: &Theme) -> SiteInfo {
        SiteInfo {
            title: self.config.site_title.clone(),
            url: self.config.site_url.clone(),
            description: self.config.site_description.clone(),
            locale: self.config.locale.clone(),
            settings: theme
                .manifest()
                .settings_values(&serde_json::Map::default()),
        }
    }
}

/// The router's table from the store: every content type but media (with its
/// hierarchy and archive flags) and every taxonomy, matched with `rules`.
pub async fn load_route_table(
    store: &dyn Store,
    rules: &PermalinkRules,
) -> Result<RouteTable, StoreError> {
    let repos = store.read();
    let types: Vec<TypeInfo> = repos
        .types()
        .list()
        .await?
        .into_iter()
        .filter(|t| t.name != MEDIA_KIND)
        .map(|t| TypeInfo {
            name: t.name,
            hierarchical: t.hierarchical,
            has_archive: t.has_archive,
        })
        .collect();
    let taxonomies: Vec<String> = repos
        .taxonomies()
        .list()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    Ok(RouteTable::new(rules, &types, &taxonomies))
}
