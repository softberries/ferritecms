//! Navigation menus: validated, replaced as a whole tree.

use std::sync::Arc;

use ferrite_domain::{ContentId, ContentStatus, Menu, MenuItem, MenuItemKind, TermId};
use ferrite_store::{Repos, Store};

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    events, slug,
    tx::finish,
};

/// Reads and edits menus (writes need `manage_menus`).
pub struct MenuService {
    store: Arc<dyn Store>,
}

impl MenuService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Fetches a menu with its item tree; a missing one is `NotFound`.
    pub async fn get(&self, name: &str) -> Result<Menu, CoreError> {
        self.store
            .read()
            .menus()
            .get(name)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Lists menus as (name, label) pairs.
    pub async fn list(&self) -> Result<Vec<(String, String)>, CoreError> {
        Ok(self.store.read().menus().list().await?)
    }

    /// Creates the menu or replaces its whole item tree. Every item is validated:
    /// - `url`: needs a URL that is `http(s)://…` or a local path (`/…`, not `//…`);
    /// - `item`: `target_id` must be an existing, non-trashed content item;
    /// - `term`: `target_id` must be an existing term.
    ///
    /// Emits `menu:<name>`.
    pub async fn put(&self, actor: &Actor, menu: &Menu) -> Result<(), CoreError> {
        require(actor, Capability::ManageMenus)?;
        if slug::validate(&menu.name)? != menu.name {
            return Err(CoreError::Invalid(
                "menu name must be lowercase without whitespace or '/'".into(),
            ));
        }
        let tx = self.store.begin().await?;
        let done = async {
            for item in &menu.items {
                check_item(tx.as_ref(), item).await?;
            }
            tx.menus().put(menu).await?;
            tx.events()
                .append(&events::invalidate([events::menu_key(&menu.name)]))
                .await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes a menu and its items (a missing menu is `NotFound`). Emits `menu:<name>`.
    pub async fn delete(&self, actor: &Actor, name: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageMenus)?;
        let tx = self.store.begin().await?;
        let done = async {
            if tx.menus().get(name).await?.is_none() {
                return Err(CoreError::NotFound);
            }
            tx.menus().delete(name).await?;
            tx.events()
                .append(&events::invalidate([events::menu_key(name)]))
                .await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }
}

async fn check_item(repos: &dyn Repos, item: &MenuItem) -> Result<(), CoreError> {
    match item.kind {
        MenuItemKind::Url => {
            let url = item
                .url
                .as_deref()
                .ok_or_else(|| CoreError::Invalid("url menu item needs a url".into()))?;
            if !is_allowed_url(url) {
                return Err(CoreError::Invalid(format!(
                    "menu url must be http(s) or a local path: {url:?}"
                )));
            }
        }
        MenuItemKind::Item => {
            let id = item
                .target_id
                .ok_or_else(|| CoreError::Invalid("item menu entry needs a target_id".into()))?;
            match repos.content().get(ContentId::from_uuid(id)).await? {
                Some(c) if c.status != ContentStatus::Trash => {}
                _ => return Err(CoreError::Invalid("menu target item not found".into())),
            }
        }
        MenuItemKind::Term => {
            let id = item
                .target_id
                .ok_or_else(|| CoreError::Invalid("term menu entry needs a target_id".into()))?;
            if repos.terms().get(TermId::from_uuid(id)).await?.is_none() {
                return Err(CoreError::Invalid("menu target term not found".into()));
            }
        }
    }
    Ok(())
}

/// `http://`/`https://` (any case) or an absolute local path; no control characters,
/// whitespace or backslashes.
fn is_allowed_url(url: &str) -> bool {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) || url.contains('\\') {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") {
        return lower.len() > "http://".len();
    }
    if lower.starts_with("https://") {
        return lower.len() > "https://".len();
    }
    url.starts_with('/') && !url.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::is_allowed_url;

    #[test]
    fn url_rules() {
        for ok in ["https://a.b/c", "HTTP://a.b", "/x", "/"] {
            assert!(is_allowed_url(ok), "{ok}");
        }
        for bad in [
            "",
            "//evil.com",
            "javascript:alert(1)",
            "ftp://a",
            "x",
            "http://",
            "/a b",
            "/\\evil",
            "mailto:a@b.c",
        ] {
            assert!(!is_allowed_url(bad), "{bad}");
        }
    }
}
