//! Error pages (06 §7): the theme's template when it has one, else a built-in page.
//!
//! **404 and 410 are shared bodies (ruling R26).** Their template gets no request path
//! (`request.path` is empty) and the head has no canonical URL, so one body serves every
//! missing URL. Each is rendered through
//! [`PageCache::get_or_render_error`](ferrite_cache::PageCache::get_or_render_error)
//! under its own [internal key](ferrite_cache::CacheKey::internal) with the keys the
//! render recorded (menus, site options, `theme`, `plugins`), so random 404s render
//! once and a menu edit still refreshes the page. Nothing is ever cached under the
//! request path of a missing URL, so content published there shows at once.
//!
//! 500 pages and 503 (render slots saturated) are never cached.

use std::{convert::Infallible, sync::Arc};

use ferrite_cache::{CacheKey, CachedPage};
use ferrite_core::{
    DepKeys,
    events::{PLUGINS_KEY, THEME_KEY},
};
use ferrite_plugin_host::{Head, MetaTag};
use ferrite_theme::{
    HelperBridge, NoBridge, RenderContext, RequestInfo, TemplateKey, error_template,
    resolve_template,
};
use tokio::runtime::Handle;

use super::{
    AppState,
    bridge::BlockingBridge,
    pipeline::{RenderError, html_page, render_blocking},
};
use crate::head::{HeadSite, render_head};

/// `Retry-After` of the 503 page, in seconds.
const RETRY_AFTER_SECS: &str = "5";

/// Which shared error page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missing {
    /// 404: the theme's hierarchy for not-found (`404.html`, else `index.html`).
    NotFound,
    /// 410 (a `410` redirect row): the same template, `request.status` 410.
    Gone,
}

impl Missing {
    fn status(self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::Gone => 410,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::NotFound => "Page not found",
            Self::Gone => "Page gone",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::NotFound => "The page you asked for does not exist.",
            Self::Gone => "The page you asked for has been removed.",
        }
    }

    fn key(self) -> CacheKey {
        CacheKey::internal(&self.status().to_string())
    }
}

const SERVER_TITLE: &str = "Something went wrong";
const SERVER_MESSAGE: &str = "The page could not be shown. Please try again later.";
const BUSY_TITLE: &str = "Busy";
const BUSY_MESSAGE: &str = "The site is busy. Please try again in a few seconds.";

/// The shared 404/410 page, from the cache or rendered once (see the module docs).
pub(crate) async fn missing(state: &Arc<AppState>, kind: Missing) -> Arc<CachedPage> {
    let render = async { Ok::<_, Infallible>(render_missing(state, kind).await) };
    match state.cache.get_or_render_error(kind.key(), render).await {
        Ok(page) => page,
        Err(never) => match *never {},
    }
}

async fn render_missing(state: &Arc<AppState>, kind: Missing) -> CachedPage {
    let status = kind.status();
    let theme = state.theme.load_full();
    let mut deps = DepKeys::default();
    deps.insert(THEME_KEY.to_owned());
    deps.insert(PLUGINS_KEY.to_owned());
    let (site, mut degraded) = match state.site_info(&theme, None, &mut deps).await {
        Ok(site) => (site, false),
        Err(e) => {
            tracing::warn!(error = %e, "site options unreadable for the error page");
            (state.fallback_site(&theme), true)
        }
    };
    let head_html = noindex_head(kind.title(), &site.title, &site.url, &site.description);
    let template = resolve_template(&TemplateKey::NotFound, |n| theme.has_template(n));
    let bridge = Arc::new(BlockingBridge::new(
        Handle::current(),
        state.reader.clone(),
        state.store.clone(),
    ));
    let site_title = site.title.clone();
    let ctx = RenderContext {
        site,
        request: RequestInfo {
            path: String::new(),
            query_s: None,
            page: 1,
            cpage: 1,
        },
        head_html,
        status,
        ..RenderContext::default()
    };
    let rendered = render_blocking(state, theme, template.clone(), ctx, bridge.clone()).await;
    degraded |= bridge.degraded();
    let ttl = degraded.then_some(state.config.volatile_ttl);
    match rendered {
        Ok(out) => {
            deps.extend(out.deps);
            html_page(status, out.html, deps.into_vec(), ttl)
        }
        Err(RenderError::Busy) => busy(),
        Err(e) => {
            tracing::error!(%template, status, error = %e, "error page render failed; built-in page used");
            // Recorded `theme`/`plugins`: a fixed theme replaces the built-in page.
            let html = builtin(kind.title(), kind.message(), &site_title);
            html_page(
                status,
                html,
                deps.into_vec(),
                Some(state.config.volatile_ttl),
            )
        }
    }
}

/// The 500 page: the theme's `500.html` with minimal context (config defaults, no data
/// helpers), else the built-in page. Never cached.
pub(crate) async fn server_error(state: &Arc<AppState>) -> CachedPage {
    let theme = state.theme.load_full();
    let site = state.fallback_site(&theme);
    let builtin_page = || {
        html_page(
            500,
            builtin(SERVER_TITLE, SERVER_MESSAGE, &state.config.site_title),
            Vec::new(),
            None,
        )
    };
    let Some(template) = error_template(|n| theme.has_template(n)) else {
        return builtin_page();
    };
    let head_html = noindex_head(SERVER_TITLE, &site.title, &site.url, &site.description);
    let ctx = RenderContext {
        site,
        head_html,
        status: 500,
        request: RequestInfo {
            path: String::new(),
            query_s: None,
            page: 1,
            cpage: 1,
        },
        ..RenderContext::default()
    };
    let bridge: Arc<dyn HelperBridge> = Arc::new(NoBridge);
    match render_blocking(state, theme, template.to_owned(), ctx, bridge).await {
        Ok(out) => html_page(500, out.html, Vec::new(), None),
        Err(RenderError::Busy) => busy(),
        Err(e) => {
            tracing::error!(template, error = %e, "500 page render failed; built-in page used");
            builtin_page()
        }
    }
}

/// 503 with `Retry-After`, built in (no render slot is free for a theme page).
pub(crate) fn busy() -> CachedPage {
    let mut page = html_page(503, builtin(BUSY_TITLE, BUSY_MESSAGE, ""), Vec::new(), None);
    page.headers
        .push(("retry-after".to_owned(), RETRY_AFTER_SECS.to_owned()));
    page
}

/// The built-in 500 page as a plain response body, for the panic handler.
pub(crate) fn builtin_server_error() -> String {
    builtin(SERVER_TITLE, SERVER_MESSAGE, "")
}

/// `<title>` (`<page> – <site>`) and `robots: noindex`; no canonical or Open Graph
/// URL, so the head is the same for every path.
fn noindex_head(page: &str, site_title: &str, site_url: &str, description: &str) -> String {
    let head = Head {
        title: Some(if site_title.is_empty() {
            page.to_owned()
        } else {
            format!("{page} – {site_title}")
        }),
        metas: vec![MetaTag {
            name: Some("robots".to_owned()),
            property: None,
            content: "noindex".to_owned(),
        }],
        links: Vec::new(),
        scripts: Vec::new(),
    };
    render_head(
        &head,
        HeadSite {
            title: site_title,
            url: site_url,
            description,
        },
    )
}

/// The built-in error page: plain HTML, no theme involved.
fn builtin(title: &str, message: &str, site_title: &str) -> String {
    let title = html_escape::encode_text(title);
    let full = if site_title.is_empty() {
        title.to_string()
    } else {
        format!("{title} – {}", html_escape::encode_text(site_title))
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\">\
         <title>{full}</title></head>\n<body><main><h1>{title}</h1>\
         <p>{message}</p><p><a href=\"/\">Home</a></p></main></body>\n</html>\n"
    )
}
