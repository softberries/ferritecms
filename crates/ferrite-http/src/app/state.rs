//! What every request handler shares.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use arc_swap::{ArcSwap, ArcSwapOption};
use ferrite_cache::PageCache;
use ferrite_core::{
    AuthService, CoreError, DepKeys, PermalinkRules, RedirectService, SiteReader,
    comments::CommentService, events::option_key,
};
use ferrite_media::MediaStore;
use ferrite_plugin_host::PluginHost;
use ferrite_store::{Store, StoreError};
use ferrite_theme::{Renderer, SiteInfo, Theme};
use tokio::sync::Semaphore;

use super::hits::HitCounter;
use crate::admin::{AdminAssets, AdminHeaders, EmbeddedAdmin, InvalidAdminHeader};
use crate::plugins::ServedBuffer;
use crate::route::{RouteTable, TypeInfo};

/// Content type that is never routed (media items are only shown inside other items).
const MEDIA_KIND: &str = "media";
/// Default TTL of pages marked volatile by plugins (05 §7a, `plugins.volatile_ttl`)
/// and of search results (06 §6, `cache.search_ttl`).
pub const SHORT_TTL: Duration = Duration::from_secs(60);
/// How long a render waits for a free render slot before the request gets a 503.
pub const RENDER_WAIT: Duration = Duration::from_secs(5);
/// Default [`HttpConfig::render_budget`] (`plugins.render_budget_ms`, P22).
pub const RENDER_BUDGET: Duration = Duration::from_millis(1000);

/// Option holding the site title; falls back to [`HttpConfig::site_title`].
pub const TITLE_OPTION: &str = "site.title";
/// Option holding the tagline; falls back to [`HttpConfig::site_description`].
pub const DESCRIPTION_OPTION: &str = "site.description";
/// Option holding the locale; falls back to [`HttpConfig::locale`].
pub const LOCALE_OPTION: &str = "site.locale";

/// Option holding the active theme's settings: a JSON object, key = setting name
/// ([`ferrite_core::ext_settings::theme_option`], what `PATCH /themes/{name}/settings`
/// writes).
pub fn theme_settings_option(theme: &str) -> String {
    ferrite_core::ext_settings::theme_option(theme)
}

/// How the public site treats requests of logged-in users (`cache.logged_in`, P21).
/// Either way they never read or fill the anonymous cache entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoggedIn {
    /// Rendered on every request, never cached (the default).
    #[default]
    Bypass,
    /// Cached per user ([`Variant::User`](ferrite_cache::Variant)), invalidated like
    /// anonymous pages.
    PerUser,
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
    /// helper reads failed (`plugins.volatile_ttl`, 1 s..=1 h). Default [`SHORT_TTL`].
    pub volatile_ttl: Duration,
    /// Cache TTL of search results (`cache.search_ttl`); zero: search pages are not
    /// cached. Default [`SHORT_TTL`].
    pub search_ttl: Duration,
    /// Host (and port) of `site_url`: the host of every cache key (ruling R25; the
    /// `Host` header is not used, so clients cannot multiply cache entries).
    pub default_host: String,
    /// Most pages being built at once (ruling R28): a content page holds its slot for
    /// the whole pipeline (data load, plugin filters, head, template; R32), an error
    /// page for its template render. Default twice the available parallelism.
    pub max_concurrent_renders: usize,
    /// How long a render waits for a slot before answering 503 with `Retry-After`.
    pub render_wait: Duration,
    /// Time all plugin filters of one page may take together, counted from when its
    /// render slot is taken (P22, `plugins.render_budget_ms`); past it the remaining
    /// plugins are skipped and the page is volatile. Default [`RENDER_BUDGET`].
    pub render_budget: Duration,
    /// Peers whose `X-Forwarded-For` is honoured for the logged client address
    /// (`server.trusted_proxies`; see [`client`](super::client)). Default: none.
    pub trusted_proxies: Vec<ipnet::IpNet>,
    /// Caching of logged-in requests (`cache.logged_in`). Default [`LoggedIn::Bypass`].
    pub logged_in: LoggedIn,
    /// Origin (`https://cdn.example.com`) of `[media] public_base_url` when media is
    /// served directly from S3/a CDN: the admin CSP allows images from it
    /// ([`admin_csp`](crate::admin::admin_csp)). Default `None` (media under `/_media`).
    pub media_origin: Option<String>,
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
            render_budget: RENDER_BUDGET,
            trusted_proxies: Vec::new(),
            logged_in: LoggedIn::Bypass,
            media_origin: None,
        }
    }
}

/// The editable site texts ([`AppState::site_texts`]).
pub(crate) struct SiteTexts {
    pub title: String,
    pub description: String,
    pub locale: String,
}

/// The parts an [`AppState`] is built from.
pub struct AppParts {
    pub store: Arc<dyn Store>,
    /// Must use the same permalink rules as `routes`.
    pub reader: Arc<SiteReader>,
    pub plugins: Arc<PluginHost>,
    /// Where `/_media` reads files from.
    pub media: Arc<MediaStore>,
    pub theme: Arc<Theme>,
    pub cache: Arc<PageCache>,
    pub routes: RouteTable,
    pub config: HttpConfig,
    /// The result of the startup migration check (`/readyz` reports it).
    pub migrations_ok: bool,
}

/// Shared state of the public site. The theme and the route table are swapped at
/// runtime by the node's reloader (theme activation, permalink option changes; ruling
/// P18) through [`swap_theme`](Self::swap_theme) and [`swap_routes`](Self::swap_routes).
/// A request loads each once (`load_full`) and finishes on what it loaded.
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub reader: Arc<SiteReader>,
    pub plugins: Arc<PluginHost>,
    pub media: Arc<MediaStore>,
    pub theme: ArcSwap<Theme>,
    /// The theme [`swap_theme`](Self::swap_theme) replaced last: its assets stay served
    /// until the next swap, so pages rendered just before a swap keep working.
    pub previous_theme: ArcSwapOption<Theme>,
    pub cache: Arc<PageCache>,
    pub routes: ArcSwap<RouteTable>,
    pub config: HttpConfig,
    pub migrations_ok: bool,
    pub(crate) redirects: Arc<RedirectService>,
    /// Resolves session cookies for previews and logged-in requests; `None`
    /// (no [`with_auth`](Self::with_auth)): every request is anonymous, previews 404.
    pub(crate) auth: Option<Arc<AuthService>>,
    pub(crate) renderer: Renderer,
    /// Render slots ([`HttpConfig::max_concurrent_renders`]).
    pub(crate) render_slots: Arc<Semaphore>,
    /// `scheme://host[:port]` of [`HttpConfig::site_url`], derived once (pre-flight
    /// F25): what plugin routes compare `Origin`/`Referer` with. `None` when the URL is
    /// not an absolute http(s) URL; unsafe plugin routes without `cross_origin` are
    /// then refused.
    pub(crate) origin: Option<String>,
    /// `site.url` is `https`: the session cookie is `__Host-ferrite_session` (P23, as
    /// the API's [`ApiState::cookie_secure`](crate::api::ApiState)); `false` for an
    /// `http` or unparsable URL.
    pub(crate) cookie_secure: bool,
    /// Where page responses are recorded as `request.served` events (P21); `None`
    /// (no [`with_served`](Self::with_served)): nothing is recorded.
    pub(crate) served: Option<Arc<ServedBuffer>>,
    /// Counts redirect hits (bounded, flushed by `serve`); `None` (no
    /// [`with_hits`](Self::with_hits)): hits are not counted.
    pub(crate) hits: Option<Arc<HitCounter>>,
    /// Takes theme-form comments (`POST /_comment`, P12); `None` (no
    /// [`with_comments`](Self::with_comments)): the form answers `503`.
    pub(crate) comments: Option<Arc<CommentService>>,
    /// The admin build served under `/admin/` (default [`EmbeddedAdmin`]).
    pub(crate) admin: Arc<dyn AdminAssets>,
    /// The headers of every admin response: the CSP (from
    /// [`HttpConfig::media_origin`]) and the fixed security headers.
    pub(crate) admin_headers: AdminHeaders,
    renders: AtomicU64,
}

impl AppState {
    /// The state built from `parts`; fails when the admin headers cannot be built (a
    /// [`HttpConfig::media_origin`] that is not a valid header value), so the admin is
    /// never served without its CSP.
    pub fn new(parts: AppParts) -> Result<Self, InvalidAdminHeader> {
        let admin_headers = AdminHeaders::new(parts.config.media_origin.as_deref())?;
        Ok(Self {
            redirects: Arc::new(RedirectService::new(parts.store.clone())),
            auth: None,
            served: None,
            hits: None,
            comments: None,
            admin: Arc::new(EmbeddedAdmin),
            admin_headers,
            renderer: Renderer::new(),
            render_slots: Arc::new(Semaphore::new(
                parts
                    .config
                    .max_concurrent_renders
                    .min(Semaphore::MAX_PERMITS),
            )),
            renders: AtomicU64::new(0),
            origin: crate::api::origin_of(&parts.config.site_url)
                .ok()
                .map(|(origin, _)| origin),
            cookie_secure: crate::api::origin_of(&parts.config.site_url)
                .is_ok_and(|(_, secure)| secure),
            store: parts.store,
            reader: parts.reader,
            plugins: parts.plugins,
            media: parts.media,
            theme: ArcSwap::new(parts.theme),
            previous_theme: ArcSwapOption::empty(),
            cache: parts.cache,
            routes: ArcSwap::from_pointee(parts.routes),
            config: parts.config,
            migrations_ok: parts.migrations_ok,
        })
    }

    /// Session cookies are resolved with `auth` (previews and logged-in requests, P20,
    /// P21); without it the site is anonymous-only.
    pub fn with_auth(mut self, auth: Arc<AuthService>) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Every public page response is recorded into `served` (`request.served`, P21).
    pub fn with_served(mut self, served: Arc<ServedBuffer>) -> Self {
        self.served = Some(served);
        self
    }

    /// Every redirect answered from a redirect row is counted into `hits` (the node's
    /// one counter; `serve` runs its flusher).
    pub fn with_hits(mut self, hits: Arc<HitCounter>) -> Self {
        self.hits = Some(hits);
        self
    }

    /// Theme-form comments (`POST /_comment`) go to `comments`, the node's one
    /// service (shared with the API's submit, P12).
    pub fn with_comments(mut self, comments: Arc<CommentService>) -> Self {
        self.comments = Some(comments);
        self
    }

    /// Serves `admin` under `/admin/` instead of the embedded build. Test seam
    /// (`test-seams`).
    #[cfg(feature = "test-seams")]
    pub fn with_admin_assets(mut self, admin: Arc<dyn AdminAssets>) -> Self {
        self.admin = admin;
        self
    }

    /// Makes `theme` the served theme; the replaced one becomes
    /// [`previous_theme`](Self::previous_theme) (the one before it is dropped).
    pub fn swap_theme(&self, theme: Arc<Theme>) {
        let old = self.theme.swap(theme);
        self.previous_theme.store(Some(old));
    }

    /// Replaces the route table (requests already resolved finish on the old one).
    pub fn swap_routes(&self, table: RouteTable) {
        self.routes.store(Arc::new(table));
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
        let texts = self.site_texts(deps).await?;
        let options = self.store.read().options();
        let key = theme_settings_option(theme.name());
        deps.insert(option_key(&key));
        let stored = options
            .get(&key)
            .await?
            .and_then(|e| e.value.as_object().cloned())
            .unwrap_or_default();
        Ok(SiteInfo {
            title: texts.title,
            url: self.config.site_url.clone(),
            description: texts.description,
            locale: bcp47(&texts.locale),
            settings: theme.manifest().settings_values(&stored),
        })
    }

    /// The `site.title`, `site.description` and `site.locale` options (config
    /// defaults when unset or not a string), recording `option:<key>` for each.
    pub(crate) async fn site_texts(&self, deps: &mut DepKeys) -> Result<SiteTexts, CoreError> {
        let options = self.store.read().options();
        let mut text = async |key: &str, default: &str| -> Result<String, CoreError> {
            deps.insert(option_key(key));
            Ok(options
                .get(key)
                .await?
                .and_then(|e| e.value.as_str().map(str::to_owned))
                .unwrap_or_else(|| default.to_owned()))
        };
        Ok(SiteTexts {
            title: text(TITLE_OPTION, &self.config.site_title).await?,
            description: text(DESCRIPTION_OPTION, &self.config.site_description).await?,
            locale: text(LOCALE_OPTION, &self.config.locale).await?,
        })
    }

    /// `site` from the config defaults alone, for error pages when the store fails.
    pub(crate) fn fallback_site(&self, theme: &Theme) -> SiteInfo {
        SiteInfo {
            title: self.config.site_title.clone(),
            url: self.config.site_url.clone(),
            description: self.config.site_description.clone(),
            locale: bcp47(&self.config.locale),
            settings: theme
                .manifest()
                .settings_values(&serde_json::Map::default()),
        }
    }
}

/// `locale` as a BCP 47 tag for `<html lang>`: a POSIX-style `en_GB.UTF-8@euro`
/// becomes `en-GB`; any character outside `[A-Za-z0-9-]` is dropped, and an empty
/// result is `en`.
pub(crate) fn bcp47(locale: &str) -> String {
    let base = locale.split(['.', '@']).next().unwrap_or_default();
    let tag: String = base
        .chars()
        .map(|c| if c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let tag = tag.trim_matches('-');
    if tag.is_empty() {
        "en".to_owned()
    } else {
        tag.to_owned()
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

#[cfg(test)]
mod tests {
    use super::bcp47;

    #[test]
    fn locales_become_bcp47() {
        for (raw, tag) in [
            ("en", "en"),
            ("en_GB", "en-GB"),
            ("pl_PL.UTF-8", "pl-PL"),
            ("de_DE@euro", "de-DE"),
            ("pt-BR", "pt-BR"),
            ("\"><x", "x"),
            ("", "en"),
            ("_", "en"),
        ] {
            assert_eq!(bcp47(raw), tag, "{raw:?}");
        }
    }
}
