//! [`SiteProbe`]: what a path on this site answers, for the content checks and the
//! health run (design 16 §6.2, D9). It resolves the path the way the public handler
//! does (the redirect table, the route table, the page pipeline's lookup step) but never
//! renders, never reads or fills the page cache, never counts a page view, a
//! `request.served` event or a redirect hit, and never calls a plugin.

use std::sync::Arc;

use async_trait::async_trait;
use ferrite_core::{
    DepKeys,
    quality::{PathProbe, PathStatus},
};
use ferrite_domain::ArtifactHash;

use crate::app::{
    AppState,
    pipeline::{self, Loaded, Shown},
};
use crate::route::{MAX_PATH_BYTES, Route, resolve_localized};

/// Most stored redirects followed for one path.
const MAX_HOPS: usize = 10;

/// First path segments outside the public router: plugin routes, the admin, the API,
/// MCP, OAuth and the well-known documents.
const UNCHECKED: &[&str] = &["_p", "admin", "api", "mcp", "oauth", ".well-known"];

/// The node's [`PathProbe`] over its [`AppState`] (its own route table, rebuilt on
/// `types.changed` like the public handler's).
pub struct SiteProbe {
    state: Arc<AppState>,
}

impl SiteProbe {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Where a redirect target leads: a local path (and query), or `None` for another
    /// site.
    fn local(&self, target: &str) -> Option<String> {
        if target.starts_with('/') && !target.starts_with("//") {
            return Some(target.to_owned());
        }
        let site = url::Url::parse(&self.state.config.site_url).ok()?;
        let url = url::Url::parse(target).ok()?;
        if url.origin() != site.origin() {
            return None;
        }
        let mut p = url.path().to_owned();
        if let Some(q) = url.query() {
            p.push('?');
            p.push_str(q);
        }
        Some(p)
    }

    /// What `route` (resolved for `path`) shows.
    async fn shown(&self, path: &str, query: Option<&str>) -> Result<PathStatus, ()> {
        let table = self.state.routes.load();
        let (locale, route) = resolve_localized(path, query, &table);
        let route = match route {
            Route::Canonical(to) => {
                let to_path = to.split('?').next().unwrap_or(&to).to_owned();
                let (locale, again) = resolve_localized(&to_path, query, &table);
                if matches!(again, Route::Canonical(_)) {
                    return Ok(PathStatus::NotFound);
                }
                let item = match self.found(&to_path, locale.tag.as_deref(), again).await? {
                    PathStatus::Ok { item } => item,
                    other => return Ok(other),
                };
                return Ok(PathStatus::Redirected { to: to_path, item });
            }
            r => r,
        };
        self.found(path, locale.tag.as_deref(), route).await
    }

    async fn found(&self, path: &str, tag: Option<&str>, route: Route) -> Result<PathStatus, ()> {
        let ok = Ok(PathStatus::Ok { item: None });
        match route {
            Route::Health
            | Route::Ready
            | Route::Robots
            | Route::Sitemap(_)
            | Route::Feed(_)
            | Route::Theme { .. } => ok,
            Route::Media { hash, .. } => {
                let Ok(hash) = ArtifactHash::parse(&hash) else {
                    return Ok(PathStatus::NotFound);
                };
                match self.state.reader.media_file(&hash).await {
                    Ok(Some(_)) => ok,
                    Ok(None) => Ok(PathStatus::NotFound),
                    Err(_) => Err(()),
                }
            }
            Route::Preview { .. } | Route::NotFound | Route::Canonical(_) => {
                Ok(PathStatus::NotFound)
            }
            route => {
                let l = pipeline::localized(&self.state, tag);
                let mut deps = DepKeys::default();
                match pipeline::load(&l, &route, path, &mut deps).await {
                    Ok(Some(Loaded::Shown(Shown::Item(item, _)))) => Ok(PathStatus::Ok {
                        item: Some(item.item.id),
                    }),
                    Ok(Some(Loaded::Shown(Shown::Listing(..)))) => ok,
                    Ok(Some(Loaded::FrontPermalink)) => Ok(PathStatus::Redirected {
                        to: l.root(),
                        item: None,
                    }),
                    Ok(None) => Ok(PathStatus::NotFound),
                    Err(_) => Err(()),
                }
            }
        }
    }
}

/// Whether the path's first segment is outside the public router.
fn unchecked(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    let first = percent_encoding::percent_decode_str(first).decode_utf8_lossy();
    UNCHECKED.iter().any(|w| first.eq_ignore_ascii_case(w))
}

#[async_trait]
impl PathProbe for SiteProbe {
    async fn probe(&self, path_and_query: &str) -> PathStatus {
        let (path, query) = match path_and_query.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (path_and_query, None),
        };
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.len() > MAX_PATH_BYTES
            || path_and_query.chars().any(char::is_control)
        {
            return PathStatus::NotFound;
        }
        if unchecked(path) {
            return PathStatus::Unchecked;
        }
        let mut current = path.to_owned();
        let mut current_query = query.map(str::to_owned);
        let mut redirected = false;
        for _ in 0..MAX_HOPS {
            match self.state.redirects.resolve(&current).await {
                Ok(Some(r)) if r.code == 410 => return PathStatus::Gone,
                Ok(Some(r)) => match self.local(&r.target) {
                    Some(next) => {
                        redirected = true;
                        let (p, q) = match next.split_once('?') {
                            Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
                            None => (next, None),
                        };
                        if p.len() > MAX_PATH_BYTES {
                            return PathStatus::NotFound;
                        }
                        current = p;
                        current_query = q;
                        if unchecked(&current) {
                            return PathStatus::Unchecked;
                        }
                    }
                    None => return PathStatus::External(r.target),
                },
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "link probe: redirect lookup failed");
                    return PathStatus::Unchecked;
                }
            }
        }
        let status = match self.shown(&current, current_query.as_deref()).await {
            Ok(s) => s,
            Err(()) => return PathStatus::Unchecked,
        };
        match status {
            PathStatus::Ok { item } if redirected => PathStatus::Redirected { to: current, item },
            other => other,
        }
    }
}
