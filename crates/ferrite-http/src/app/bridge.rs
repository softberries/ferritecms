//! The store-backed [`HelperBridge`] (plan ruling P2): theme helpers that need data
//! block on the async [`SiteReader`] from the rendering thread.

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ferrite_core::{
    CoreError, DepKeys, MenuService, PageItem, SiteReader,
    events::{archive_key, menu_key},
};
use ferrite_domain::{ContentId, MenuItem, MenuItemId, MenuItemKind, TermId};
use ferrite_store::Store;
use ferrite_theme::{Bridged, HelperBridge, ImageRef, MenuNode, MenuTree};
use tokio::runtime::Handle;

/// Content type of term archives (`SiteReader::term_archive` lists posts).
const POST_KIND: &str = "post";
/// Deepest menu nesting rendered; deeper items are dropped.
const MAX_MENU_DEPTH: usize = 16;

/// Helpers over [`SiteReader`] (published-only by construction) and the menu and term
/// repositories. Owns everything it uses (`'static`), so it can move to the blocking
/// render thread; each call runs the async read with [`Handle::block_on`], which is
/// only allowed outside the runtime's worker threads (the renderer runs inside
/// `spawn_blocking`).
///
/// A failed read is logged, the helper returns nothing, and [`degraded`](Self::degraded)
/// turns true: the caller must not cache the page for long, since the failure may be
/// transient.
pub(crate) struct BlockingBridge {
    handle: Handle,
    reader: Arc<SiteReader>,
    store: Arc<dyn Store>,
    degraded: AtomicBool,
}

impl BlockingBridge {
    pub(crate) fn new(handle: Handle, reader: Arc<SiteReader>, store: Arc<dyn Store>) -> Self {
        Self {
            handle,
            reader,
            store,
            degraded: AtomicBool::new(false),
        }
    }

    /// Whether any helper read failed during the render.
    pub(crate) fn degraded(&self) -> bool {
        self.degraded.load(Ordering::Relaxed)
    }

    fn run<T>(&self, helper: &str, fut: impl Future<Output = Result<T, CoreError>>) -> Option<T> {
        match self.handle.block_on(fut) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(helper, error = %e, "theme helper read failed");
                self.degraded.store(true, Ordering::Relaxed);
                None
            }
        }
    }

    /// Label and URL of each shown entry of `menu`. Item targets are loaded with one
    /// batched [`SiteReader::links`] call (published only; `post:<id>` recorded for
    /// every target, hidden ones too); each term target is one cheap
    /// [`SiteReader::term_link`] probe (`term:<id>` and `archive:post`).
    async fn entries(
        &self,
        items: &[MenuItem],
        deps: &mut DepKeys,
    ) -> Result<HashMap<MenuItemId, (String, String)>, CoreError> {
        let label = |item: &MenuItem, fallback: &str| {
            if item.label.trim().is_empty() {
                fallback.to_owned()
            } else {
                item.label.clone()
            }
        };
        let targets: Vec<ContentId> = items
            .iter()
            .filter(|i| i.kind == MenuItemKind::Item)
            .filter_map(|i| i.target_id.map(ContentId::from_uuid))
            .collect();
        let links = if targets.is_empty() {
            HashMap::new()
        } else {
            let read = self.reader.links(&targets).await?;
            deps.extend(read.deps);
            read.value
        };
        let mut shown = HashMap::new();
        for item in items {
            let entry = match item.kind {
                MenuItemKind::Url => match item.url.as_deref() {
                    Some(u) if safe_menu_url(u) => Some((label(item, u), u.to_owned())),
                    Some(u) => {
                        tracing::warn!(url = u, "menu entry with a disallowed URL scheme dropped");
                        None
                    }
                    None => None,
                },
                MenuItemKind::Item => item
                    .target_id
                    .and_then(|id| links.get(&ContentId::from_uuid(id)))
                    .map(|l| (label(item, &l.title), l.url.clone())),
                MenuItemKind::Term => match item.target_id {
                    Some(id) => {
                        let read = self.reader.term_link(TermId::from_uuid(id)).await?;
                        deps.extend(read.deps);
                        read.value.map(|t| (label(item, &t.name), t.url))
                    }
                    None => None,
                },
            };
            if let Some(entry) = entry {
                shown.insert(item.id, entry);
            }
        }
        Ok(shown)
    }

    async fn load_menu(
        &self,
        name: &str,
        deps: &mut DepKeys,
    ) -> Result<Option<MenuTree>, CoreError> {
        let menu = match MenuService::new(self.store.clone()).get(name).await {
            Ok(m) => m,
            Err(CoreError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let shown = self.entries(&menu.items, deps).await?;
        Ok(Some(MenuTree {
            items: tree(&menu.items, None, &shown, 0),
        }))
    }
}

/// Defence in depth for custom menu URLs: `MenuService` validates on write, but rows
/// can reach the store other ways (imports, older data). Allowed: `http(s)://…`,
/// `mailto:…`, root-relative paths (not `//…`) and `#fragments`; never whitespace,
/// control characters or backslashes. Everything else (`javascript:`, `data:`, …) is
/// dropped.
fn safe_menu_url(url: &str) -> bool {
    if url.is_empty()
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || url.contains('\\')
    {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p) && lower.len() > p.len())
        || url.starts_with('#')
        || (url.starts_with('/') && !url.starts_with("//"))
}

/// The children of `parent` in position order. An entry whose target is hidden is
/// dropped together with its children.
fn tree(
    items: &[MenuItem],
    parent: Option<MenuItemId>,
    shown: &HashMap<MenuItemId, (String, String)>,
    depth: usize,
) -> Vec<MenuNode> {
    if depth >= MAX_MENU_DEPTH {
        return Vec::new();
    }
    let mut level: Vec<&MenuItem> = items.iter().filter(|i| i.parent_id == parent).collect();
    level.sort_by_key(|i| i.position);
    level
        .into_iter()
        .filter_map(|i| {
            let (label, url) = shown.get(&i.id)?;
            Some(MenuNode {
                label: label.clone(),
                url: url.clone(),
                children: tree(items, Some(i.id), shown, depth + 1),
            })
        })
        .collect()
}

impl HelperBridge for BlockingBridge {
    fn related(&self, item: ContentId, n: u32) -> Bridged<Vec<PageItem>> {
        let mut deps = DepKeys::default();
        let items = self.run("related", async {
            let read = self.reader.by_id(item).await?;
            deps.extend(read.deps);
            let Some(p) = read.value else {
                return Ok(Vec::new());
            };
            let related = self.reader.related(&p, n as usize).await?;
            deps.extend(related.deps);
            Ok(related.value)
        });
        (items.unwrap_or_default(), deps)
    }

    /// With `term`, only posts can match (term archives list posts) and at most
    /// `reading.per_page` items come back: the first page of the term archive.
    fn query(&self, kind: &str, term: Option<&str>, limit: u32) -> Bridged<Vec<PageItem>> {
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        let items = self.run("query", async {
            match term {
                Some(term) => {
                    let Some((taxonomy, slug)) = term.split_once('/') else {
                        return Ok(Vec::new());
                    };
                    if kind != POST_KIND {
                        return Ok(Vec::new());
                    }
                    let read = self.reader.term_archive(taxonomy, slug, 1).await?;
                    deps.extend(read.deps);
                    Ok(read
                        .value
                        .map(|l| l.items.into_iter().take(limit as usize).collect())
                        .unwrap_or_default())
                }
                None => {
                    let read = self.reader.latest(kind, limit, 0).await?;
                    deps.extend(read.deps);
                    Ok(read.value)
                }
            }
        });
        (items.unwrap_or_default(), deps)
    }

    fn menu(&self, name: &str) -> Bridged<Option<MenuTree>> {
        let mut deps = DepKeys::default();
        deps.insert(menu_key(name));
        let tree = self.run("menu", self.load_menu(name, &mut deps));
        (tree.flatten(), deps)
    }

    fn image(&self, media: ContentId, size: &str) -> Option<ImageRef> {
        let read = self.run("image", self.reader.media(media))?;
        read.value.map(|m| ImageRef::from_media(&m, size))
    }
}

#[cfg(test)]
mod tests {
    use super::safe_menu_url;

    #[test]
    fn menu_urls_are_filtered() {
        for ok in [
            "/",
            "/about/",
            "#top",
            "https://example.com/x",
            "HTTP://a.b",
            "mailto:a@b.c",
        ] {
            assert!(safe_menu_url(ok), "{ok}");
        }
        for bad in [
            "",
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "java\tscript:x",
            "data:text/html,x",
            "vbscript:x",
            "//evil.com",
            "/\\evil.com",
            "https://",
            "mailto:",
            "x",
        ] {
            assert!(!safe_menu_url(bad), "{bad:?}");
        }
    }
}
