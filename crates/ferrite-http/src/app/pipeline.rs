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

use super::{
    AppState,
    bridge::BlockingBridge,
    error_page::{self, ErrorKind},
    response::HTML,
};
use crate::{
    head::{HeadPage, HeadSite, page_head},
    plugins::{PluginRequest, RenderTrace, filter_item},
    route::{Candidate, NormalQuery, Route},
};

/// Why a page could not be rendered (answered with the 500 page).
#[derive(Debug, thiserror::Error)]
pub(crate) enum RenderError {
    #[error("read failed: {0}")]
    Core(#[from] CoreError),
    #[error("{0}")]
    Theme(#[from] ThemeError),
    #[error("render task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// What a route shows.
enum Shown {
    Item(Box<PageItem>, TemplateKey),
    Listing(Listing, TemplateKey),
}

/// The response for a content route: the rendered page, the 404 page when the route
/// shows nothing, the 500 page when rendering fails. Never fails, so it can run as the
/// page cache's render future; error pages have a status >= 400 and are not cached.
pub(crate) async fn render(
    state: Arc<AppState>,
    route: Route,
    path: String,
    query: NormalQuery,
) -> CachedPage {
    match render_route(&state, &route, &path, query).await {
        Ok(Some(page)) => page,
        Ok(None) => error_page::render(&state, &path, ErrorKind::NotFound).await,
        Err(e) => {
            tracing::error!(path = %path, error = %e, "page render failed");
            error_page::render(&state, &path, ErrorKind::Server).await
        }
    }
}

async fn render_route(
    state: &Arc<AppState>,
    route: &Route,
    path: &str,
    query: NormalQuery,
) -> Result<Option<CachedPage>, RenderError> {
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

/// Renders on a blocking thread (MiniJinja is synchronous and the bridge blocks).
pub(crate) async fn render_blocking(
    state: &AppState,
    theme: Arc<ferrite_theme::Theme>,
    template: String,
    ctx: RenderContext,
    bridge: Arc<dyn ferrite_theme::HelperBridge>,
) -> Result<RenderOutput, RenderError> {
    let renderer = state.renderer.clone();
    let out = tokio::task::spawn_blocking(move || renderer.render(&theme, &template, ctx, bridge))
        .await??;
    Ok(out)
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
        // todo(Task 8b): feeds, sitemaps, robots.txt, /_media and /_theme; 404 until then.
        Route::Feed(_) | Route::Sitemap(_) | Route::Robots => None,
        Route::Media { .. } | Route::Theme { .. } => None,
        // Answered before the cache by the handler.
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
