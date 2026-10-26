//! Navigation menus: validated, replaced as a whole tree.

use std::sync::Arc;

use ferrite_domain::{ContentId, ContentStatus, Menu, MenuItem, MenuItemKind, TermId};
use ferrite_store::{Repos, Store};

use crate::{
    Actor, CoreError,
    actor::{Area, Capability, require, require_read},
    events, slug,
    tx::finish,
};

const MAX_URL_LEN: usize = 2048;
const MAX_LABEL_LEN: usize = 200;
/// Most items one menu holds (each is checked inside the write's transaction).
pub const MAX_MENU_ITEMS: usize = 500;

/// A menu location a theme declares (its `[menus]` table), and whether a menu fills it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLocation {
    pub name: String,
    pub label: String,
    /// A menu named `name` exists (the theme shows it there).
    pub menu: bool,
}

/// The `declared` locations (name, label) in the given order, each with whether one of
/// `menus` (name, label; from [`MenuService::list_for`]) has its name. Core knows no
/// theme: the caller passes the locations of the theme it serves.
pub fn menu_locations<'a>(
    declared: impl IntoIterator<Item = (&'a str, &'a str)>,
    menus: &[(String, String)],
) -> Vec<MenuLocation> {
    declared
        .into_iter()
        .map(|(name, label)| MenuLocation {
            name: name.to_owned(),
            label: label.to_owned(),
            menu: menus.iter().any(|(m, _)| m == name),
        })
        .collect()
}

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

    /// [`get`](Self::get) for the admin (`require_read(Menus)`, F16).
    pub async fn get_for(&self, actor: &Actor, name: &str) -> Result<Menu, CoreError> {
        require_read(actor, Area::Menus)?;
        self.get(name).await
    }

    /// [`list`](Self::list) for the admin (`require_read(Menus)`, F16).
    pub async fn list_for(&self, actor: &Actor) -> Result<Vec<(String, String)>, CoreError> {
        require_read(actor, Area::Menus)?;
        self.list().await
    }

    /// Creates the menu or replaces its whole item tree. Every item is validated:
    /// - `url`: needs a URL that is `http(s)://…` or a local path (`/…`, not `//…`);
    /// - `item`: `target_id` must be an existing, non-trashed content item;
    /// - `term`: `target_id` must be an existing term.
    ///
    /// The menu label and item labels are at most 200 characters, and a menu holds at
    /// most [`MAX_MENU_ITEMS`] items. Emits `menu:<name>`.
    pub async fn put(&self, actor: &Actor, menu: &Menu) -> Result<(), CoreError> {
        require(actor, Capability::ManageMenus)?;
        if slug::validate(&menu.name)? != menu.name {
            return Err(CoreError::Invalid(
                "menu name must be lowercase without whitespace or '/'".into(),
            ));
        }
        check_label(&menu.label, "label", "menu label")?;
        if menu.items.len() > MAX_MENU_ITEMS {
            return Err(CoreError::invalid_field(
                "items",
                format!("a menu holds at most {MAX_MENU_ITEMS} items"),
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

/// `InvalidField { field, .. }` when `label` is longer than [`MAX_LABEL_LEN`].
fn check_label(label: &str, field: &'static str, what: &str) -> Result<(), CoreError> {
    if label.chars().count() > MAX_LABEL_LEN {
        return Err(CoreError::invalid_field(
            field,
            format!("{what} longer than {MAX_LABEL_LEN} characters"),
        ));
    }
    Ok(())
}

async fn check_item(repos: &dyn Repos, item: &MenuItem) -> Result<(), CoreError> {
    check_label(&item.label, "items", "menu item label")?;
    match item.kind {
        MenuItemKind::Url => {
            if item.target_id.is_some() {
                return Err(CoreError::Invalid(
                    "url menu item must not have a target_id".into(),
                ));
            }
            let url = item
                .url
                .as_deref()
                .ok_or_else(|| CoreError::Invalid("url menu item needs a url".into()))?;
            if url.chars().count() > MAX_URL_LEN {
                return Err(CoreError::Invalid(format!(
                    "menu url longer than {MAX_URL_LEN} characters"
                )));
            }
            if !is_allowed_url(url) {
                return Err(CoreError::Invalid(format!(
                    "menu url must be http(s) or a local path: {url:?}"
                )));
            }
        }
        MenuItemKind::Item => {
            if item.url.is_some() {
                return Err(CoreError::Invalid(
                    "item menu entry must not have a url".into(),
                ));
            }
            let id = item
                .target_id
                .ok_or_else(|| CoreError::Invalid("item menu entry needs a target_id".into()))?;
            match repos.content().get(ContentId::from_uuid(id)).await? {
                Some(c) if c.status != ContentStatus::Trash => {}
                _ => return Err(CoreError::Invalid("menu target item not found".into())),
            }
        }
        MenuItemKind::Term => {
            if item.url.is_some() {
                return Err(CoreError::Invalid(
                    "term menu entry must not have a url".into(),
                ));
            }
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
    use super::{MenuLocation, is_allowed_url, menu_locations};

    #[test]
    fn locations_mark_existing_menus() {
        let menus = [
            ("primary".to_owned(), "Main".to_owned()),
            ("sidebar".to_owned(), "Side".to_owned()),
        ];
        let got = menu_locations([("footer", "Footer"), ("primary", "Primary")], &menus);
        let loc = |name: &str, label: &str, menu| MenuLocation {
            name: name.into(),
            label: label.into(),
            menu,
        };
        assert_eq!(
            got,
            [
                loc("footer", "Footer", false),
                loc("primary", "Primary", true)
            ]
        );
        assert!(menu_locations([], &menus).is_empty());
    }

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
