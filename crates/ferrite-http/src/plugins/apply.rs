//! Running render filters for one request, with the request scope and the trace.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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
    /// `SiteReader::preview` (its `can_edit` check passed), so a public request cannot
    /// smuggle a draft into the scope.
    preview: Option<Arc<Previewed>>,
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
        }
    }

    /// The preview of `preview` at `path`, by a logged-in editor.
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
    deps: Arc<Mutex<DepKeys>>,
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

/// Record keys reported by a read into the current scope, if any.
pub(crate) fn record(deps: DepKeys) {
    // Outside a scope there is no page to invalidate, so the keys are not needed.
    let _ = SCOPE.try_with(|s| lock(&s.deps).extend(deps));
}

/// Apply filter `hook` for `req`, adding the dependency keys and volatility of every
/// plugin call to `trace`. Never fails: a failing plugin is skipped by the host.
pub async fn run_filter(
    host: &PluginHost,
    hook: &str,
    value: FilterValue,
    req: &PluginRequest,
    trace: &mut RenderTrace,
) -> FilterValue {
    let deps = Arc::new(Mutex::new(DepKeys::default()));
    let scope = Scope {
        preview: req.preview.clone(),
        deps: Arc::clone(&deps),
    };
    let outcome = SCOPE
        .scope(scope, host.apply_filter_traced(hook, value, &req.info))
        .await;
    for key in outcome.deps {
        trace.deps.insert(key);
    }
    trace.deps.extend(std::mem::take(&mut *lock(&deps)));
    trace.volatile |= outcome.volatile;
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
