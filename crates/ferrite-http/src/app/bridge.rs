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
    events::{archive_key, menu_key, term_key},
    permalink::term_path,
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

    /// Label and URL of a menu entry; `None` when its target is not public (or gone).
    /// Records the target's key whether or not it is shown.
    async fn entry(
        &self,
        item: &MenuItem,
        deps: &mut DepKeys,
    ) -> Result<Option<(String, String)>, CoreError> {
        let label = |fallback: &str| {
            if item.label.trim().is_empty() {
                fallback.to_owned()
            } else {
                item.label.clone()
            }
        };
        Ok(match item.kind {
            MenuItemKind::Url => item.url.as_ref().map(|u| (label(u), u.clone())),
            MenuItemKind::Item => {
                let Some(id) = item.target_id else {
                    return Ok(None);
                };
                let read = self.reader.by_id(ContentId::from_uuid(id)).await?;
                deps.extend(read.deps);
                read.value.map(|p| (label(&p.item.title), p.url))
            }
            MenuItemKind::Term => {
                let Some(id) = item.target_id else {
                    return Ok(None);
                };
                let id = TermId::from_uuid(id);
                deps.insert(term_key(id));
                self.store
                    .read()
                    .terms()
                    .get(id)
                    .await?
                    .map(|t| (label(&t.name), term_path(&t.taxonomy, &t.slug)))
            }
        })
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
        let mut shown = HashMap::new();
        for item in &menu.items {
            if let Some(entry) = self.entry(item, deps).await? {
                shown.insert(item.id, entry);
            }
        }
        Ok(Some(MenuTree {
            items: tree(&menu.items, None, &shown, 0),
        }))
    }
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
