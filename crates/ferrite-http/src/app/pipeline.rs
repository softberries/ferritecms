//! The render pipeline for content routes and previews (06 §5): load through
//! [`SiteReader`], plugin content filters, head, template, dependency keys.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use ferrite_cache::{CachedPage, etag_for};
use ferrite_core::{
    Actor, CoreError, DepKeys, FrontPage, Listing, PageItem, Previewed, SiteRead,
    events::{PLUGINS_KEY, THEME_KEY, archive_key},
};
use ferrite_domain::ContentId;
use ferrite_theme::resolve_template;
use ferrite_theme::{
    ArchiveInfo, Pagination, RenderContext, RenderOutput, RequestInfo, TemplateKey, ThemeError,
};
use tokio::{runtime::Handle, sync::OwnedSemaphorePermit};

use super::{AppState, bridge::BlockingBridge, error_page, feeds, response::HTML};
use crate::{
    head::{HeadPage, HeadSite, page_head},
    plugins::{PluginRequest, RenderTrace, filter_footer, filter_item},
    route::{Candidate, NormalQuery, Route, listing_page_url},
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
    Listing(Listing, TemplateKey, Box<ArchiveInfo>),
}

/// The response for a content route: the rendered page, a 503 when no render slot
/// frees up in time, the 500 page when rendering fails, or [`NotFound`]. Runs as the
/// page cache's render future under the request path (or uncached for a logged-in
/// request under `LoggedIn::Bypass`); 503 and 500 have an error status and are not
/// cached, and `NotFound` is an error result, which is never cached. `logged_in` is
/// what plugins see as `is_logged_in` (P21).
pub(crate) async fn render(
    state: Arc<AppState>,
    route: Route,
    path: String,
    query: NormalQuery,
    logged_in: bool,
) -> Result<CachedPage, NotFound> {
    let result = render_route(&state, &route, &path, query, logged_in).await;
    outcome(&state, &path, result).await
}

/// The preview of item `id` for `actor` (P20): loaded with `SiteReader::preview`
/// (R17 `can_read`; trashed, missing or not readable → [`NotFound`]) inside a render
/// slot, then rendered like a single item with the plugin preview scope and
/// `request.preview = true`. Never cached by the caller.
pub(crate) async fn preview(
    state: Arc<AppState>,
    route: Route,
    path: String,
    actor: Actor,
    id: ContentId,
) -> Result<CachedPage, NotFound> {
    let result = preview_route(&state, &route, &path, &actor, id).await;
    outcome(&state, &path, result).await
}

async fn outcome(
    state: &Arc<AppState>,
    path: &str,
    result: Result<Option<CachedPage>, RenderError>,
) -> Result<CachedPage, NotFound> {
    match result {
        Ok(Some(page)) => Ok(page),
        Ok(None) => Err(NotFound),
        Err(RenderError::Busy) => Ok(error_page::busy()),
        Err(e) => {
            tracing::error!(path = %path, error = %e, "page render failed");
            Ok(error_page::server_error(state).await)
        }
    }
}

/// The request a page answers.
struct Target<'a> {
    route: &'a Route,
    path: &'a str,
    query: NormalQuery,
    /// End of the page's plugin render budget (P22): the slot time plus
    /// `HttpConfig::render_budget`; `None` when that is past what an `Instant` holds
    /// (no budget then, each call still has its deadline).
    until: Option<Instant>,
}

/// Who a page is rendered for, as plugins and templates see it.
enum Viewer {
    Public { logged_in: bool },
    Preview(Box<Previewed>),
}

async fn render_route(
    state: &Arc<AppState>,
    route: &Route,
    path: &str,
    query: NormalQuery,
    logged_in: bool,
) -> Result<Option<CachedPage>, RenderError> {
    match route {
        Route::Feed(source) => return feeds::feed(state, source).await,
        Route::Sitemap(part) => return feeds::sitemap(state, part).await,
        Route::Robots => return feeds::robots(state).await.map(Some),
        _ => {}
    }
    // One slot for the whole pipeline (R32): data load, plugin filters, head and
    // template, so plugin calls are bounded by the same limit as renders.
    let slot = acquire_slot(state).await?;
    let until = budget_end(state.config.render_budget);
    let mut deps = DepKeys::default();
    let Some(shown) = load(state, route, path, &mut deps).await? else {
        return Ok(None);
    };
    let target = Target {
        route,
        path,
        query,
        until,
    };
    let viewer = Viewer::Public { logged_in };
    finish(state, slot, target, shown, deps, viewer)
        .await
        .map(Some)
}

async fn preview_route(
    state: &Arc<AppState>,
    route: &Route,
    path: &str,
    actor: &Actor,
    id: ContentId,
) -> Result<Option<CachedPage>, RenderError> {
    let slot = acquire_slot(state).await?;
    let until = budget_end(state.config.render_budget);
    let read = match state.reader.preview(actor, id).await {
        Ok(read) => read,
        Err(CoreError::NotFound | CoreError::Forbidden(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut deps = DepKeys::default();
    let previewed = take(read, &mut deps);
    let item = previewed.item().clone();
    let key = item_template(&item);
    let shown = Shown::Item(Box::new(item), key);
    let target = Target {
        route,
        path,
        query: NormalQuery::default(),
        until,
    };
    finish(
        state,
        slot,
        target,
        shown,
        deps,
        Viewer::Preview(Box::new(previewed)),
    )
    .await
    .map(Some)
}

/// Plugin filters, head and template for what the target's route shows, holding
/// `slot`.
async fn finish(
    state: &Arc<AppState>,
    slot: OwnedSemaphorePermit,
    target: Target<'_>,
    shown: Shown,
    mut deps: DepKeys,
    viewer: Viewer,
) -> Result<CachedPage, RenderError> {
    let Target {
        route,
        path,
        query,
        until,
    } = target;
    let theme = state.theme.load_full();
    let site = state.site_info(&theme, &mut deps).await?;
    let (mut item, mut listing, key, archive) = match shown {
        Shown::Item(item, key) => (Some(*item), None, key, None),
        Shown::Listing(listing, key, archive) => (None, Some(listing), key, Some(*archive)),
    };
    let pagination = listing.as_ref().map(|l| pagination(route, l));
    let preview = matches!(viewer, Viewer::Preview(_));
    let req = match viewer {
        Viewer::Public { logged_in } => {
            PluginRequest::public(path, &site.locale, item.as_ref().map(|i| i.item.id))
                .logged_in(logged_in)
        }
        Viewer::Preview(previewed) => PluginRequest::preview(path, &site.locale, *previewed),
    };
    let req = match until {
        Some(until) => req.until(until),
        None => req,
    };
    let mut trace = RenderTrace::default();
    if let Some(item) = item.as_mut() {
        filter_item(&state.plugins, item, &req, &mut trace).await;
    }
    if let Some(listing) = listing.as_mut() {
        for item in &mut listing.items {
            filter_item(&state.plugins, item, &req, &mut trace).await;
        }
    }
    // `render.footer` once per page, after the item filters and before the head, all
    // within the same render budget (06 §5).
    let footer_html = filter_footer(&state.plugins, &req, &mut trace).await;
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
    // The comment page (P34) is read on single items only.
    let cpage = match &item {
        Some(_) => query.cpage.unwrap_or(1),
        None => 1,
    };
    let ctx = RenderContext {
        site,
        item,
        listing,
        archive,
        pagination,
        request: RequestInfo {
            path: path.to_owned(),
            query_s: query.s,
            page,
            cpage,
        },
        head_html,
        footer_html,
        status: 200,
        preview,
    };
    let bridge = Arc::new(BlockingBridge::new(
        Handle::current(),
        state.reader.clone(),
        state.store.clone(),
    ));
    let out = render_in_slot(state, slot, theme, template, ctx, bridge.clone()).await?;
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
    Ok(html_page(200, out.html, deps.into_vec(), ttl))
}

/// When the plugin render budget of a page whose slot was just taken ends (P22);
/// `None` (no budget, never a zero one: review m2) when the end does not fit an
/// `Instant`.
fn budget_end(budget: Duration) -> Option<Instant> {
    Instant::now().checked_add(budget)
}

/// One of [`HttpConfig::max_concurrent_renders`](super::HttpConfig) render slots
/// (R28, R32); [`RenderError::Busy`] when none frees up within `render_wait`.
async fn acquire_slot(state: &AppState) -> Result<OwnedSemaphorePermit, RenderError> {
    let slot = tokio::time::timeout(
        state.config.render_wait,
        state.render_slots.clone().acquire_owned(),
    )
    .await;
    let Ok(Ok(permit)) = slot else {
        tracing::warn!("render slots saturated; answering 503");
        return Err(RenderError::Busy);
    };
    Ok(permit)
}

/// Renders on a blocking thread (MiniJinja is synchronous and the bridge blocks),
/// holding a render slot for the whole render; [`RenderError::Busy`] when none frees
/// up within `render_wait`. Used by the error pages; content routes take their slot
/// before loading data ([`render_in_slot`]).
pub(crate) async fn render_blocking(
    state: &AppState,
    theme: Arc<ferrite_theme::Theme>,
    template: String,
    ctx: RenderContext,
    bridge: Arc<dyn ferrite_theme::HelperBridge>,
) -> Result<RenderOutput, RenderError> {
    let slot = acquire_slot(state).await?;
    render_in_slot(state, slot, theme, template, ctx, bridge).await
}

/// Renders on a blocking thread holding `slot` until the render ends (even if the
/// client disconnects: the permit moves into the blocking closure).
async fn render_in_slot(
    state: &AppState,
    slot: OwnedSemaphorePermit,
    theme: Arc<ferrite_theme::Theme>,
    template: String,
    ctx: RenderContext,
    bridge: Arc<dyn ferrite_theme::HelperBridge>,
) -> Result<RenderOutput, RenderError> {
    state.count_render();
    let renderer = state.renderer.clone();
    let out = tokio::task::spawn_blocking(move || {
        let _slot = slot;
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

/// Content type of front, term, author and date listings (`SiteReader` lists posts).
const POST_KIND: &str = "post";

/// Merges a read's keys into `deps` and returns its value.
fn take<T>(read: SiteRead<T>, deps: &mut DepKeys) -> T {
    deps.extend(read.deps);
    read.value
}

/// A listing to show: `None` past the last page (an empty page 1 is still shown).
fn listing(l: Option<Listing>, key: TemplateKey, archive: ArchiveInfo) -> Option<Shown> {
    l.filter(|l| l.page <= 1 || !l.items.is_empty())
        .map(|l| Shown::Listing(l, key, Box::new(archive)))
}

/// `archive` of kind `kind` with nothing else set.
fn archive(kind: &str) -> ArchiveInfo {
    ArchiveInfo {
        kind: kind.to_owned(),
        ..ArchiveInfo::default()
    }
}

/// Position of `l` and the canonical URLs of its neighbours (ruling R30).
fn pagination(route: &Route, l: &Listing) -> Pagination {
    let per_page = u64::from(l.per_page.max(1));
    let pages = u32::try_from(l.total.div_ceil(per_page).max(1)).unwrap_or(u32::MAX);
    let page = l.page.max(1);
    Pagination {
        page,
        pages,
        prev_url: (page > 1)
            .then(|| listing_page_url(route, page - 1))
            .flatten(),
        next_url: (page < pages)
            .then(|| listing_page_url(route, page + 1))
            .flatten(),
    }
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
            Some(FrontPage::Posts(l)) => listing(
                Some(l),
                TemplateKey::FrontPosts,
                ArchiveInfo {
                    content_type: Some(POST_KIND.to_owned()),
                    ..archive("front")
                },
            ),
            None => None,
        },
        Route::Archive { kind, page } => listing(
            take(r.archive(kind, *page).await?, deps),
            TemplateKey::Archive { kind: kind.clone() },
            ArchiveInfo {
                content_type: Some(kind.clone()),
                ..archive("type")
            },
        ),
        Route::Term {
            taxonomy,
            slug,
            page,
        } => match take(r.term_archive_page(taxonomy, slug, *page).await?, deps) {
            Some((term, l)) => listing(
                Some(l),
                TemplateKey::Term {
                    tax: taxonomy.clone(),
                    slug: slug.clone(),
                },
                ArchiveInfo {
                    content_type: Some(POST_KIND.to_owned()),
                    term: Some(term),
                    ..archive("term")
                },
            ),
            None => None,
        },
        Route::Author { login, page } => {
            match take(r.author_archive_page(login, *page).await?, deps) {
                Some((author, l)) => listing(
                    Some(l),
                    TemplateKey::Author {
                        login: login.clone(),
                    },
                    ArchiveInfo {
                        content_type: Some(POST_KIND.to_owned()),
                        author: Some(author),
                        ..archive("author")
                    },
                ),
                None => None,
            }
        }
        Route::Date { year, month, page } => listing(
            take(r.date_archive(*year, *month, *page).await?, deps),
            TemplateKey::Date,
            ArchiveInfo {
                content_type: Some(POST_KIND.to_owned()),
                year: Some(*year),
                month: *month,
                ..archive("date")
            },
        ),
        Route::Search { q, page } => listing(
            take(r.search(q, *page).await?, deps),
            TemplateKey::Search,
            archive("search"),
        ),
        // Rendered by `feeds` (above), or answered before the cache by the handler.
        Route::Feed(_) | Route::Sitemap(_) | Route::Robots => None,
        Route::Media { .. } | Route::Theme { .. } => None,
        // Answered by the handler through `preview`, never through the cache.
        Route::Preview { .. } => None,
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
                let key = item_template(&item);
                return Ok(Some(Shown::Item(Box::new(item), key)));
            }
            None => deps.insert(archive_key(kind)),
        }
    }
    Ok(None)
}

/// The template key of a single item: `page` items use the page hierarchy.
fn item_template(item: &PageItem) -> TemplateKey {
    if item.item.kind == "page" {
        TemplateKey::Page {
            slug: item.item.slug.clone(),
            id: item.item.id.to_string(),
        }
    } else {
        TemplateKey::Single {
            kind: item.item.kind.clone(),
            slug: item.item.slug.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review m2: an end past what an `Instant` holds means no budget, not a zero one.
    #[test]
    fn budget_end_never_collapses_to_now() {
        let before = Instant::now();
        let end = budget_end(Duration::from_secs(1)).expect("fits");
        assert!(end >= before + Duration::from_secs(1));
        assert_eq!(budget_end(Duration::MAX), None);
    }
}
