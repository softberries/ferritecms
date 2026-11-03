//! Running render filters for one request, with the request scope and the trace.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use ferrite_core::{DepKeys, PageItem, Previewed};
use ferrite_domain::ContentId;
use ferrite_plugin_host::{FilterValue, PluginHost, RequestInfo};

/// What a page's plugin calls touched: merged into the page's dependency keys, and a
/// volatile page gets the short cache TTL (05 §7a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderTrace {
    pub deps: DepKeys,
    pub volatile: bool,
}

impl RenderTrace {
    pub fn merge(&mut self, other: RenderTrace) {
        self.deps.extend(other.deps);
        self.volatile |= other.volatile;
    }
}

/// One request as plugins see it.
#[derive(Debug, Clone)]
pub struct PluginRequest {
    pub info: RequestInfo,
    /// The item being previewed by an editor, during an authenticated preview (whose
    /// response is never cached). Plugins see it through `content.get-post` on its id
    /// and `meta.get` on it; nothing else changes. A [`Previewed`] can only come from
    /// `SiteReader::preview` (its `can_read` check passed, R17), so a public request
    /// cannot smuggle a draft into the scope.
    preview: Option<Arc<Previewed>>,
    /// End of the page's render budget (P22), set when the render slot is taken: the
    /// filters of the whole page share it. `None`: no budget (each call still has its
    /// deadline).
    until: Option<Instant>,
    /// A link or whole-site preview (M6-3, D12): plugin writes are refused.
    read_only: bool,
}

impl PluginRequest {
    /// A public request for `path`, rendering `item` if it is a single-item page.
    pub fn public(path: &str, locale: &str, item: Option<ContentId>) -> Self {
        Self {
            info: RequestInfo {
                path: path.to_owned(),
                locale: locale.to_owned(),
                is_logged_in: false,
                post: item.map(|id| id.to_string()),
            },
            preview: None,
            until: None,
            read_only: false,
        }
    }

    /// Refuses plugin writes (`meta.set`, `kv.set`, `kv.delete`, `notify-admins`) for
    /// this request: a link or whole-site preview (D12). On SQLite a write would also
    /// wait for the writer the preview's sandbox holds.
    pub fn read_only(mut self, on: bool) -> Self {
        self.read_only = on;
        self
    }

    /// The page's render budget ends at `until` (`HttpConfig::render_budget` after the
    /// render slot was taken).
    pub fn until(mut self, until: Instant) -> Self {
        self.until = Some(until);
        self
    }

    /// Marks the request as made by a logged-in user (`RequestInfo.is_logged_in`, P21).
    pub fn logged_in(mut self, logged_in: bool) -> Self {
        self.info.is_logged_in = logged_in;
        self
    }

    /// The preview of `preview` at `path`, by a logged-in user who may read it.
    pub fn preview(path: &str, locale: &str, preview: Previewed) -> Self {
        let mut req = Self::public(path, locale, Some(preview.item().item.id));
        req.info.is_logged_in = true;
        req.preview = Some(Arc::new(preview));
        req
    }

    pub fn previewed(&self) -> Option<&Previewed> {
        self.preview.as_deref()
    }
}

/// Per-request state visible to [`StoreHostServices`](super::StoreHostServices) while
/// plugins run (see the module docs of `plugins`).
pub(crate) struct Scope {
    pub(crate) preview: Option<Arc<Previewed>>,
    read_only: bool,
    deps: Arc<Mutex<DepKeys>>,
    /// Set when a service swallowed a store error during the call.
    degraded: Arc<AtomicBool>,
}

tokio::task_local! {
    static SCOPE: Scope;
}

fn lock(deps: &Mutex<DepKeys>) -> MutexGuard<'_, DepKeys> {
    // Each update is one `extend`; a poisoned lock still holds a usable set.
    deps.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The preview item if a scope is set and it previews `id`.
pub(crate) fn scoped_preview(id: ContentId) -> Option<Arc<Previewed>> {
    SCOPE
        .try_with(|s| s.preview.clone())
        .ok()
        .flatten()
        .filter(|p| p.item().item.id == id)
}

/// Whether the current scope refuses plugin writes (a preview, D12).
pub(crate) fn read_only_scope() -> bool {
    SCOPE.try_with(|s| s.read_only).unwrap_or(false)
}

/// Record keys reported by a read into the current scope, if any.
pub(crate) fn record(deps: DepKeys) {
    // Outside a scope there is no page to invalidate, so the keys are not needed.
    let _ = SCOPE.try_with(|s| lock(&s.deps).extend(deps));
}

/// Mark the current scope degraded: a store error was swallowed (the plugin got "not
/// found" or nothing), so the page must get the short TTL like a failed call (R19).
pub(crate) fn mark_degraded() {
    let _ = SCOPE.try_with(|s| s.degraded.store(true, Ordering::Relaxed));
}

/// Apply filter `hook` for `req`, adding the dependency keys and volatility of every
/// plugin call to `trace`; a store error swallowed by a service also makes it
/// volatile. Never fails: a failing plugin is skipped by the host. Within the request's
/// render budget ([`PluginRequest::until`]) when it has one: past it the remaining
/// plugins are skipped and the page is volatile.
pub async fn run_filter(
    host: &PluginHost,
    hook: &str,
    value: FilterValue,
    req: &PluginRequest,
    trace: &mut RenderTrace,
) -> FilterValue {
    let deps = Arc::new(Mutex::new(DepKeys::default()));
    let degraded = Arc::new(AtomicBool::new(false));
    let scope = Scope {
        preview: req.preview.clone(),
        read_only: req.read_only,
        deps: Arc::clone(&deps),
        degraded: Arc::clone(&degraded),
    };
    let outcome = match req.until {
        Some(until) => {
            SCOPE
                .scope(
                    scope,
                    host.apply_filter_until(hook, value, &req.info, until),
                )
                .await
        }
        None => {
            SCOPE
                .scope(scope, host.apply_filter_traced(hook, value, &req.info))
                .await
        }
    };
    for key in outcome.deps {
        trace.deps.insert(key);
    }
    trace.deps.extend(std::mem::take(&mut *lock(&deps)));
    trace.volatile |= outcome.volatile || degraded.load(Ordering::Relaxed);
    outcome.value
}

/// Run `content.body` over `item.html` and `content.excerpt` over its excerpt. The
/// host sanitises `html` output unless a plugin is `render.html = "raw"`, which keeps
/// the [`PageItem::html`] invariant (sanitised HTML) for every non-raw plugin. Build the
/// head afterwards so a description derived from the excerpt sees the filtered text.
pub async fn filter_item(
    host: &PluginHost,
    item: &mut PageItem,
    req: &PluginRequest,
    trace: &mut RenderTrace,
) {
    let body = FilterValue::Html(item.html.clone());
    // The host always returns the variant it was given; the fallbacks keep the input.
    if let FilterValue::Html(h) = run_filter(host, "content.body", body, req, trace).await {
        item.html = h;
    }
    let excerpt = FilterValue::Text(item.item.excerpt.clone());
    if let FilterValue::Text(t) = run_filter(host, "content.excerpt", excerpt, req, trace).await {
        item.item.excerpt = t;
    }
}

/// The page footer: `render.footer` run once over `""` (06 §5), within the page's
/// render budget like every other filter. The host sanitises `html` output like
/// `content.body` (unless a plugin is `render.html = "raw"`); a failing plugin is
/// skipped, so the footer is what the others returned (or empty).
pub async fn filter_footer(
    host: &PluginHost,
    req: &PluginRequest,
    trace: &mut RenderTrace,
) -> String {
    let value = FilterValue::Html(String::new());
    match run_filter(host, "render.footer", value, req, trace).await {
        FilterValue::Html(h) => h,
        // The host always returns the variant it was given.
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_in_sets_request_info() {
        let req = PluginRequest::public("/a/", "en", None);
        assert!(!req.info.is_logged_in);
        assert!(req.clone().logged_in(true).info.is_logged_in);
        assert!(!req.logged_in(true).logged_in(false).info.is_logged_in);
    }
}
