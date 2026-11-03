//! Link and whole-site previews (M6-3, design 15 §5.3–§5.5, §7).
//!
//! * `/_preview/link/<token>`: an item link renders the item's stored state as its
//!   creator (read-only plugins, the core preview bar); a release link sets the
//!   preview cookie (`__Host-ferrite_preview` on https, `ferrite_preview` on http) and
//!   answers `303` to `?to=<local path>` or `/`.
//! * With the cookie, every public page request is answered through a sandbox in which
//!   the release is applied ([`ReleaseService::sandbox`]): redirects and the route are
//!   resolved and rendered by the normal pipeline over a [`preview_view`] of the state,
//!   **never** through the page cache, `private, no-store`, `noindex`,
//!   `Referrer-Policy: no-referrer`, not counted. An invalid or expired cookie is
//!   cleared and the request served normally.
//! * `/_preview/exit` clears the cookie and answers `303` to `/`.
//!
//! At most [`preview_slots`] release previews render at once per node (SQLite holds
//! its one writer for the request, D10 as clarified by P7); a request waits
//! `render_wait` for one (then the 503 page), a sandbox lives at most
//! [`PREVIEW_DEADLINE`], each link renders 60 pages a minute.
//!
//! [`preview_view`]: super::AppState::preview_view

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Response, StatusCode, header},
};
use bytes::Bytes;
use ferrite_cache::CachedPage;
use ferrite_core::{
    CoreError,
    preview_link::{PreviewLinkService, PreviewPrincipal},
    release::ReleaseService,
};
use ferrite_domain::{PreviewTarget, ReleaseId, Timestamp};
use ferrite_store::{Backend, Store};
use tokio::sync::Semaphore;

use super::{
    AppState,
    error_page::{self, Missing},
    handler::{follow, missing},
    pipeline::{self, PreviewKind},
    response::{noindex, page_response, private_no_store, redirect},
};
use crate::api::cookie::{preview_cookie, read_preview_cookie};
use crate::route::{MAX_PATH_BYTES, NormalQuery, Route, resolve_localized};

/// Longest a release preview's sandbox lives (D34).
pub(crate) const PREVIEW_DEADLINE: Duration = Duration::from_secs(15);

/// Release previews rendering at once on a node: 1 on SQLite (one writer), 2 on
/// Postgres (D34).
pub(crate) fn preview_slots(backend: Backend) -> usize {
    match backend {
        Backend::Sqlite => 1,
        _ => 2,
    }
}

/// The services previews use.
pub(crate) struct Editorial {
    releases: Arc<ReleaseService>,
    links: Arc<PreviewLinkService>,
    slots: Arc<Semaphore>,
}

impl Editorial {
    pub(crate) fn new(
        releases: Arc<ReleaseService>,
        links: Arc<PreviewLinkService>,
        backend: Backend,
    ) -> Self {
        Self {
            releases,
            links,
            slots: Arc::new(Semaphore::new(preview_slots(backend))),
        }
    }
}

/// `?working=1` (the editor's live preview) or `?release=<id>` (change mode) on
/// `/_preview/<id>`; anything else is the stored item.
pub(crate) fn kind_of(raw_query: Option<&str>) -> PreviewKind {
    let mut kind = PreviewKind::Stored;
    for pair in raw_query.unwrap_or_default().split('&') {
        match pair.split_once('=') {
            Some(("working", "1")) => kind = PreviewKind::Working,
            Some(("release", id)) => {
                if let Ok(id) = id.parse::<ReleaseId>() {
                    return PreviewKind::Staged(id);
                }
            }
            _ => {}
        }
    }
    kind
}

/// The core preview bar (D28): what a shared preview always shows, whatever the theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewBar {
    pub(crate) title: String,
    /// Entries applied (release previews), `None` for an item.
    pub(crate) changes: Option<u32>,
    pub(crate) skipped: u32,
}

fn escape(s: &str) -> String {
    html_escape::encode_text(s).replace('"', "&quot;")
}

/// `html` with `bar` after the first `<body…>` tag (or at the start without one).
/// Every text is escaped.
pub(crate) fn inject_preview_bar(html: &str, bar: &PreviewBar) -> String {
    let mut text = format!("Preview of \u{201c}{}\u{201d}", escape(&bar.title));
    if let Some(n) = bar.changes {
        text.push_str(&format!(
            " \u{b7} {n} change{}",
            if n == 1 { "" } else { "s" }
        ));
    }
    if bar.skipped > 0 {
        text.push_str(&format!(" \u{b7} {} not shown", bar.skipped));
    }
    let markup = format!(
        "<div role=\"region\" aria-label=\"Preview\" style=\"position:sticky;top:0;\
         z-index:2147483647;display:flex;gap:1rem;align-items:center;\
         justify-content:space-between;padding:.5rem 1rem;background:#1f2937;\
         color:#fff;font:14px/1.4 system-ui,sans-serif\"><span>{text}</span>\
         <a href=\"/_preview/exit\" style=\"color:#fff;text-decoration:underline\">\
         Exit preview</a></div>"
    );
    let lower = html.to_ascii_lowercase();
    if let Some(start) = lower.find("<body")
        && let Some(end) = lower[start..].find('>')
    {
        let at = start + end + 1;
        return format!("{}{markup}{}", &html[..at], &html[at..]);
    }
    format!("{markup}{html}")
}

/// The headers of every link or cookie preview answer.
fn preview_headers(res: &mut Response<Body>) {
    res.headers_mut().remove(header::ETAG);
    private_no_store(res);
    noindex(res);
    res.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

/// `page` as a response with `bar` injected into its HTML.
fn with_bar(page: &CachedPage, bar: &PreviewBar) -> Response<Body> {
    let html = page.headers.iter().any(|(n, v)| {
        n.eq_ignore_ascii_case(header::CONTENT_TYPE.as_str()) && v.starts_with("text/html")
    });
    let mut res = if html {
        let mut page = page.clone();
        let body = String::from_utf8_lossy(&page.body).into_owned();
        page.body = Bytes::from(inject_preview_bar(&body, bar));
        page_response(&page, None)
    } else {
        page_response(page, None)
    };
    preview_headers(&mut res);
    res
}

/// `?to=<path>`: a local path (starts with `/`, not `//`, no backslash), else `/`.
fn local_target(raw_query: Option<&str>) -> String {
    raw_query
        .unwrap_or_default()
        .split('&')
        .find_map(|p| p.strip_prefix("to="))
        .map(|t| {
            percent_encoding::percent_decode_str(t)
                .decode_utf8_lossy()
                .into_owned()
        })
        .filter(|t| {
            t.starts_with('/')
                && !t.starts_with("//")
                && !t.contains('\\')
                && t.len() <= MAX_PATH_BYTES
                && t.chars().all(|c| !c.is_control())
        })
        .unwrap_or_else(|| "/".to_owned())
}

/// What resolving a preview token gave.
enum Resolved {
    Valid(Arc<Editorial>, PreviewPrincipal),
    /// Unknown, expired, revoked, or its creator may no longer see it.
    Invalid,
    /// The lookup failed (a store error): neither valid nor known invalid.
    Failed,
}

async fn resolve(state: &AppState, token: &str) -> Resolved {
    let Some(ed) = state.editorial.clone() else {
        return Resolved::Invalid;
    };
    match ed.links.resolve(token).await {
        Ok(Some(p)) => Resolved::Valid(ed, p),
        Ok(None) => Resolved::Invalid,
        Err(e) => {
            tracing::warn!(error = %e, "preview link lookup failed");
            Resolved::Failed
        }
    }
}

fn busy_page() -> Response<Body> {
    let mut res = page_response(&error_page::busy(), None);
    preview_headers(&mut res);
    res
}

/// `/_preview/link/<token>`.
pub(crate) async fn link(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    token: &str,
    raw_query: Option<&str>,
) -> Response<Body> {
    let (ed, p) = match resolve(state, token).await {
        Resolved::Valid(ed, p) => (ed, p),
        Resolved::Failed => return busy_page(),
        Resolved::Invalid => {
            let mut res = missing(state, Missing::NotFound).await;
            preview_headers(&mut res);
            return res;
        }
    };
    match p.link.target {
        PreviewTarget::Release(_) => {
            let left =
                (p.link.expires_at.as_micros() - Timestamp::now().as_micros()).max(0) / 1_000_000;
            let to = local_target(raw_query);
            let mut res = match HeaderValue::from_str(&to) {
                Ok(location) => redirect(StatusCode::SEE_OTHER, location),
                Err(_) => redirect(StatusCode::SEE_OTHER, HeaderValue::from_static("/")),
            };
            res.headers_mut().append(
                header::SET_COOKIE,
                preview_cookie(token, left as u64, state.cookie_secure),
            );
            preview_headers(&mut res);
            res
        }
        PreviewTarget::Item(id) => {
            if let Err(e) = ed.links.hit_render(p.link.id).await {
                return refused(state, e).await;
            }
            let title = state
                .store
                .read()
                .content()
                .get(id)
                .await
                .ok()
                .flatten()
                .map(|i| i.title)
                .unwrap_or_default();
            let view = Arc::new(state.preview_view(state.store.clone(), None));
            // The token is a credential: it never reaches plugins, themes or logs.
            let path = "/_preview/link".to_owned();
            let route = Route::Preview { id };
            // `?working=1`: the creator's autosave slot (the editor's live preview loads
            // through a link, P7 D16); `?release=<id>`: an entry's staged change. Only for
            // the creator's own signed-in browser (the frame's navigation carries the
            // session cookie); anyone else holding the link sees the stored item.
            let mut kind = kind_of(raw_query);
            if kind != PreviewKind::Stored {
                let own = super::handler::session(state, headers)
                    .await
                    .is_some_and(|who| who.user.id == p.link.created_by);
                if !own {
                    kind = PreviewKind::Stored;
                }
            }
            match pipeline::preview(view, route, path, p.actor, id, kind).await {
                Ok(page) => with_bar(
                    &page,
                    &PreviewBar {
                        title,
                        changes: None,
                        skipped: 0,
                    },
                ),
                Err(pipeline::NotFound) => {
                    let mut res = missing(state, Missing::NotFound).await;
                    preview_headers(&mut res);
                    res
                }
            }
        }
    }
}

/// `/_preview/exit`.
pub(crate) fn exit(state: &AppState) -> Response<Body> {
    let mut res = redirect(StatusCode::SEE_OTHER, HeaderValue::from_static("/"));
    res.headers_mut().append(
        header::SET_COOKIE,
        preview_cookie("", 0, state.cookie_secure),
    );
    preview_headers(&mut res);
    res
}

/// What the cookie check decided.
pub(crate) enum CookiePage {
    /// No preview cookie: serve normally.
    None,
    /// An invalid or expired cookie: serve normally and clear it.
    Clear,
    /// The page under the preview.
    Served(Response<Body>),
}

async fn refused(state: &Arc<AppState>, e: CoreError) -> Response<Body> {
    let mut res = match e {
        CoreError::RateLimited { .. } | CoreError::Store(_) => {
            page_response(&error_page::busy(), None)
        }
        _ => missing(state, Missing::NotFound).await,
    };
    preview_headers(&mut res);
    res
}

/// A public page request carrying the preview cookie (design 15 §5.3).
pub(crate) async fn cookie_page(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    path: &str,
    raw_query: Option<&str>,
) -> CookiePage {
    let Some(token) = read_preview_cookie(headers, state.cookie_secure) else {
        return CookiePage::None;
    };
    let (ed, p) = match resolve(state, token).await {
        Resolved::Valid(ed, p) => (ed, p),
        // A failed lookup keeps the cookie (review finding 5).
        Resolved::Failed => return CookiePage::Served(busy_page()),
        Resolved::Invalid => return CookiePage::Clear,
    };
    let PreviewTarget::Release(release) = p.link.target else {
        return CookiePage::Clear;
    };
    if let Err(e) = ed.links.hit_render(p.link.id).await {
        return CookiePage::Served(refused(state, e).await);
    }
    let Ok(Ok(_slot)) =
        tokio::time::timeout(state.config.render_wait, ed.slots.clone().acquire_owned()).await
    else {
        let mut res = page_response(&error_page::busy(), None);
        preview_headers(&mut res);
        return CookiePage::Served(res);
    };
    let work = render_release(state, &ed, &p, release, path, raw_query);
    let res = match tokio::time::timeout(PREVIEW_DEADLINE, work).await {
        Ok(res) => res,
        Err(_) => {
            tracing::warn!(release = %release, path, "release preview hit its deadline");
            let mut res = page_response(&error_page::busy(), None);
            preview_headers(&mut res);
            res
        }
    };
    CookiePage::Served(res)
}

async fn render_release(
    state: &Arc<AppState>,
    ed: &Editorial,
    p: &PreviewPrincipal,
    release: ReleaseId,
    path: &str,
    raw_query: Option<&str>,
) -> Response<Body> {
    // Entries are applied with the creator's own rights (the sandbox always rolls
    // back); the page renders as the link's `content:read` principal (D27).
    let applier = ferrite_core::Actor::new(p.actor.user, p.actor.role);
    let (sandbox, report, r) = match ed.releases.sandbox(&applier, release).await {
        Ok(v) => v,
        Err(e) => return refused(state, e).await,
    };
    let sandbox: Arc<dyn Store> = Arc::new(sandbox);
    let view = Arc::new(state.preview_view(sandbox, Some(r.title.clone())));
    let bar = PreviewBar {
        title: r.title,
        changes: Some(report.applied),
        skipped: u32::try_from(report.skipped.len()).unwrap_or(u32::MAX),
    };
    // Redirect rows as the release leaves them; no hit is counted (no counter).
    if path.len() <= MAX_PATH_BYTES + 1 {
        match view.redirects.resolve(path).await {
            Ok(Some(row)) => {
                let mut res = follow(&view, row).await;
                preview_headers(&mut res);
                return res;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, "preview redirect lookup failed");
                let mut res = page_response(&error_page::server_error(&view).await, None);
                preview_headers(&mut res);
                return res;
            }
        }
    }
    let (locale, route) = resolve_localized(path, raw_query, &view.routes.load());
    if let Route::Canonical(to) = &route {
        let mut res = match HeaderValue::from_str(to) {
            Ok(location) => redirect(StatusCode::MOVED_PERMANENTLY, location),
            Err(_) => missing(&view, Missing::NotFound).await,
        };
        preview_headers(&mut res);
        return res;
    }
    let mut query = NormalQuery::parse(raw_query);
    if !route.may_show_item() {
        query.cpage = None;
    }
    match pipeline::render(view.clone(), route, locale, path.to_owned(), query, false).await {
        Ok(page) => with_bar(&page, &bar),
        Err(pipeline::NotFound) => {
            let page = error_page::missing(&view, Missing::NotFound).await;
            with_bar(&page, &bar)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_goes_after_body_and_is_escaped() {
        let bar = PreviewBar {
            title: "<b>Spring</b> & \"co\"".into(),
            changes: Some(3),
            skipped: 1,
        };
        let out = inject_preview_bar("<html><BODY class=\"x\"><p>hi</p></body>", &bar);
        assert!(
            out.starts_with("<html><BODY class=\"x\"><div role=\"region\""),
            "{out}"
        );
        assert!(out.contains("&lt;b&gt;Spring&lt;/b&gt; &amp; &quot;co&quot;"));
        assert!(out.contains("3 changes"));
        assert!(out.contains("1 not shown"));
        assert!(out.contains("href=\"/_preview/exit\""));
        let bare = inject_preview_bar("<p>no body</p>", &bar);
        assert!(bare.starts_with("<div role=\"region\""));
    }

    #[test]
    fn to_is_local_only() {
        assert_eq!(local_target(Some("to=%2Fabout%2F")), "/about/");
        assert_eq!(local_target(Some("to=//evil.example")), "/");
        assert_eq!(local_target(Some("to=https://evil.example")), "/");
        assert_eq!(local_target(Some("to=/%5Cevil")), "/");
        assert_eq!(local_target(None), "/");
    }

    #[test]
    fn query_kinds() {
        let id = ReleaseId::new();
        assert_eq!(kind_of(Some("working=1")), PreviewKind::Working);
        assert_eq!(
            kind_of(Some(&format!("release={id}"))),
            PreviewKind::Staged(id)
        );
        assert_eq!(kind_of(Some("release=nope")), PreviewKind::Stored);
        assert_eq!(kind_of(None), PreviewKind::Stored);
    }
}
