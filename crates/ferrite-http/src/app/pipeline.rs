//! The render pipeline for content routes (06 §5): load through [`SiteReader`], plugin
//! content filters, head, template, dependency keys.

use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use ferrite_cache::{CachedPage, etag_for};
use ferrite_core::{
    CoreError, DepKeys, FrontPage, Listing, PageItem, SiteRead,
    events::{PLUGINS_KEY, THEME_KEY, archive_key},
};
use ferrite_theme::resolve_template;
use ferrite_theme::{RenderContext, RenderOutput, RequestInfo, TemplateKey, ThemeError};
use tokio::runtime::Handle;

use super::{AppState, bridge::BlockingBridge, error_page, feeds, response::HTML};
use crate::{
    head::{HeadPage, HeadSite, page_head},
    plugins::{PluginRequest, RenderTrace, filter_item},
    route::{Candidate, NormalQuery, Route},
};

/// Why a page could not be rendered: [`Busy`](Self::Busy) is answered with 503, every
/// other error with the 500 page.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RenderError {
    #[error("read failed: {0}")]
    Core(#[from] CoreError),
    #[error("{0}")]
    Theme(#[from] ThemeError),
    #[error("render task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("no render slot free")]
    Busy,
}

/// The route shows nothing: the handler answers with the shared 404 page (never
/// cached under the request path).
#[derive(Debug, Clone, Copy)]
pub(crate) struct NotFound;

/// What a route shows.
enum Shown {
    Item(Box<PageItem>, TemplateKey),
    Listing(Listing, TemplateKey),
}

/// The response for a content route: the rendered page, a 503 when no render slot
/// frees up in time, the 500 page when rendering fails, or [`NotFound`]. Runs as the
/// page cache's render future under the request path; 503 and 500 have an error status
/// and are not cached, and `NotFound` is an error result, which is never cached.
pub(crate) async fn render(
    state: Arc<AppState>,
    route: Route,
    path: String,
    query: NormalQuery,
) -> Result<CachedPage, NotFound> {
    match render_route(&state, &route, &path, query).await {
        Ok(Some(page)) => Ok(page),
        Ok(None) => Err(NotFound),
        Err(RenderError::Busy) => Ok(error_page::busy()),
        Err(e) => {
            tracing::error!(path = %path, error = %e, "page render failed");
            Ok(error_page::server_error(&state).await)
        }
    }
}

async fn render_route(
    state: &Arc<AppState>,
    route: &Route,
    path: &str,
    query: NormalQuery,
) -> Result<Option<CachedPage>, RenderError> {
    match route {
        Route::Feed(source) => return feeds::feed(state, source).await,
        Route::Sitemap(part) => return feeds::sitemap(state, part).await,
        Route::Robots => return feeds::robots(state).await.map(Some),
        _ => {}
    }
    let mut deps = DepKeys::default();
    let Some(shown) = load(state, route, path, &mut deps).await? else {
        return Ok(None);
    };
    let theme = state.theme.load_full();
    let site = state.site_info(&theme, &mut deps).await?;
    let (mut item, mut listing, key) = match shown {
        Shown::Item(item, key) => (Some(*item), None, key),
        Shown::Listing(listing, key) => (None, Some(listing), key),
    };
    let req = PluginRequest::public(path, &site.locale, item.as_ref().map(|i| i.item.id));
    let mut trace = RenderTrace::default();
    if let Some(item) = item.as_mut() {
        filter_item(&state.plugins, item, &req, &mut trace).await;
    }
    if let Some(listing) = listing.as_mut() {
        for item in &mut listing.items {
            filter_item(&state.plugins, item, &req, &mut trace).await;
        }
    }
    let head_html = page_head(
        &state.plugins,
        state.store.as_ref(),
        HeadSite {
            title: &site.title,
            url: &site.url,
            description: &site.description,
        },
        HeadPage {
            item: item.as_ref(),
            path,
        },
        &req,
        &mut trace,
    )
    .await;
    let template = resolve_template(&key, |n| theme.has_template(n));
    let page = page_number(route);
    let search = matches!(route, Route::Search { .. });
    let ctx = RenderContext {
        site,
        item,
        listing,
        request: RequestInfo {
            path: path.to_owned(),
            query_s: query.s,
            page,
        },
        head_html,
        status: 200,
        preview: false,
    };
    let bridge = Arc::new(BlockingBridge::new(
        Handle::current(),
        state.reader.clone(),
        state.store.clone(),
    ));
    let out = render_blocking(state, theme, template, ctx, bridge.clone()).await?;
    deps.extend(trace.deps);
    deps.extend(out.deps);
    deps.insert(THEME_KEY.to_owned());
    deps.insert(PLUGINS_KEY.to_owned());
    let mut ttl: Option<Duration> = None;
    if trace.volatile || bridge.degraded() {
        ttl = Some(state.config.volatile_ttl);
    }
    if search {
        ttl = Some(ttl.map_or(state.config.search_ttl, |t| t.min(state.config.search_ttl)));
    }
    Ok(Some(html_page(200, out.html, deps.into_vec(), ttl)))
}

/// Renders on a blocking thread (MiniJinja is synchronous and the bridge blocks),
/// holding one of [`HttpConfig::max_concurrent_renders`](super::HttpConfig) slots for
/// the whole render; [`RenderError::Busy`] when none frees up within `render_wait`.
pub(crate) async fn render_blocking(
    state: &AppState,
    theme: Arc<ferrite_theme::Theme>,
    template: String,
    ctx: RenderContext,
    bridge: Arc<dyn ferrite_theme::HelperBridge>,
) -> Result<RenderOutput, RenderError> {
    // Bounded wait for a slot (R28): renders hold blocking threads.
    let slot = tokio::time::timeout(
        state.config.render_wait,
        state.render_slots.clone().acquire_owned(),
    )
    .await;
    let Ok(Ok(permit)) = slot else {
        tracing::warn!("render slots saturated; answering 503");
        return Err(RenderError::Busy);
    };
    state.count_render();
    let renderer = state.renderer.clone();
    let out = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        renderer.render(&theme, &template, ctx, bridge)
    })
    .await??;
    Ok(out)
}

/// A non-HTML response (feeds, sitemaps, robots.txt) as the cache stores it.
pub(crate) fn cached_page(content_type: &str, body: String, deps: DepKeys) -> CachedPage {
    let mut page = html_page(200, body, deps.into_vec(), None);
    page.headers = vec![("content-type".to_owned(), content_type.to_owned())];
    page
}

/// An HTML response as the cache stores it.
pub(crate) fn html_page(
    status: u16,
    html: String,
    deps: Vec<String>,
    ttl: Option<Duration>,
) -> CachedPage {
    let body = Bytes::from(html);
    CachedPage {
        status,
        headers: vec![("content-type".to_owned(), HTML.to_owned())],
        etag: etag_for(&body),
        body,
        deps,
        ttl,
    }
}

fn page_number(route: &Route) -> u32 {
    match route {
        Route::Front { page }
        | Route::Search { page, .. }
        | Route::Archive { page, .. }
        | Route::Term { page, .. }
        | Route::Author { page, .. }
        | Route::Date { page, .. } => *page,
        _ => 1,
    }
}

/// Merges a read's keys into `deps` and returns its value.
fn take<T>(read: SiteRead<T>, deps: &mut DepKeys) -> T {
    deps.extend(read.deps);
    read.value
}

/// A listing to show: `None` past the last page (an empty page 1 is still shown).
fn listing(l: Option<Listing>, key: TemplateKey) -> Option<Shown> {
    l.filter(|l| l.page <= 1 || !l.items.is_empty())
        .map(|l| Shown::Listing(l, key))
}

/// Loads what `route` shows through [`SiteReader`], merging every read's keys into
/// `deps`. `None`: nothing to show (404).
async fn load(
    state: &AppState,
    route: &Route,
    path: &str,
    deps: &mut DepKeys,
) -> Result<Option<Shown>, CoreError> {
    let r = &state.reader;
    Ok(match route {
        Route::Content(candidates) => content(state, candidates, path, deps).await?,
        Route::Front { page } => match take(r.front_page(*page).await?, deps) {
            // A static front page has no `/page/N/`.
            Some(FrontPage::Page(_)) if *page > 1 => None,
            Some(FrontPage::Page(item)) => {
                let key = TemplateKey::FrontPage {
                    slug: item.item.slug.clone(),
                    id: item.item.id.to_string(),
                };
                Some(Shown::Item(item, key))
            }
            Some(FrontPage::Posts(l)) => listing(Some(l), TemplateKey::FrontPosts),
            None => None,
        },
        Route::Archive { kind, page } => listing(
            take(r.archive(kind, *page).await?, deps),
            TemplateKey::Archive { kind: kind.clone() },
        ),
        Route::Term {
            taxonomy,
            slug,
            page,
        } => listing(
            take(r.term_archive(taxonomy, slug, *page).await?, deps),
            TemplateKey::Term {
                tax: taxonomy.clone(),
                slug: slug.clone(),
            },
        ),
        Route::Author { login, page } => listing(
            take(r.author_archive(login, *page).await?, deps),
            TemplateKey::Author {
                login: login.clone(),
            },
        ),
        Route::Date { year, month, page } => listing(
            take(r.date_archive(*year, *month, *page).await?, deps),
            TemplateKey::Date,
        ),
        Route::Search { q, page } => {
            listing(take(r.search(q, *page).await?, deps), TemplateKey::Search)
        }
        // Rendered by `feeds` (above), or answered before the cache by the handler.
        Route::Feed(_) | Route::Sitemap(_) | Route::Robots => None,
        Route::Media { .. } | Route::Theme { .. } => None,
        Route::Canonical(_) | Route::Health | Route::Ready | Route::NotFound => None,
    })
}

/// The first candidate whose item exists, is public and has exactly `path` as its URL
/// (the router's handler obligation). A missed candidate records `archive:<kind>`, so a
/// page that a later-created item of an earlier candidate would shadow is refreshed.
async fn content(
    state: &AppState,
    candidates: &[Candidate],
    path: &str,
    deps: &mut DepKeys,
) -> Result<Option<Shown>, CoreError> {
    for candidate in candidates {
        let (kind, parts) = match candidate {
            Candidate::Single { kind, parts } => (kind.as_str(), parts.clone()),
            Candidate::PostByStructure { slug, .. } => ("post", vec![slug.clone()]),
        };
        let found = take(state.reader.single(kind, &parts).await?, deps);
        match found.filter(|item| item.url == path) {
            Some(item) => {
                let key = if item.item.kind == "page" {
                    TemplateKey::Page {
                        slug: item.item.slug.clone(),
                        id: item.item.id.to_string(),
                    }
                } else {
                    TemplateKey::Single {
                        kind: item.item.kind.clone(),
                        slug: item.item.slug.clone(),
                    }
                };
                return Ok(Some(Shown::Item(Box::new(item), key)));
            }
            None => deps.insert(archive_key(kind)),
        }
    }
    Ok(None)
}
