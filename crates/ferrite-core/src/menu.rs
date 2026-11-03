//! Navigation menus: validated, replaced as a whole tree.

use std::sync::Arc;

use ferrite_domain::{ContentId, ContentStatus, Menu, MenuItem, MenuItemKind, TermId};
use ferrite_store::{MenuSummary, Repos, Store};

use crate::{
    Actor, CoreError,
    actor::{Area, Capability, require, require_read},
    events, slug,
    tx::finish,
};

/// Longest menu item URL, in characters.
pub const MAX_MENU_URL: usize = 2048;
/// Longest menu or menu item label, in characters.
pub const MAX_MENU_LABEL: usize = 200;
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
/// `menus` (from [`MenuService::list_for`]) has its name. Core knows no
/// theme: the caller passes the locations of the theme it serves.
pub fn menu_locations<'a>(
    declared: impl IntoIterator<Item = (&'a str, &'a str)>,
    menus: &[MenuSummary],
) -> Vec<MenuLocation> {
    declared
        .into_iter()
        .map(|(name, label)| MenuLocation {
            name: name.to_owned(),
            label: label.to_owned(),
            menu: menus.iter().any(|m| m.name == name),
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

    /// Lists every menu (name, label, locale, `translation_of`), by name.
    pub async fn list(&self) -> Result<Vec<MenuSummary>, CoreError> {
        Ok(self.store.read().menus().list().await?)
    }

    /// The menu a theme location `name` shows in `locale` (13 §2.6, D11): the menu that
    /// translates `name` into `locale`, else `name` itself (`None` when neither exists).
    pub async fn for_location(&self, name: &str, locale: &str) -> Result<Option<Menu>, CoreError> {
        let menus = self.store.read().menus();
        if let Some(m) = menus.get_translation(name, locale).await? {
            return Ok(Some(m));
        }
        Ok(menus.get(name).await?)
    }

    /// [`get`](Self::get) for the admin (`require_read(Menus)`, F16).
    pub async fn get_for(&self, actor: &Actor, name: &str) -> Result<Menu, CoreError> {
        require_read(actor, Area::Menus)?;
        self.get(name).await
    }

    /// [`list`](Self::list) for the admin (`require_read(Menus)`, F16).
    pub async fn list_for(&self, actor: &Actor) -> Result<Vec<MenuSummary>, CoreError> {
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
        let tx = self.store.begin().await?;
        let done = async {
            let keys = put_in(tx.as_ref(), menu).await?;
            tx.events().append(&events::invalidate(keys)).await?;
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
            let Some(stored) = tx.menus().get(name).await? else {
                return Err(CoreError::NotFound);
            };
            tx.menus().delete(name).await?;
            let keys = menu_keys(name, [stored.translation_of.as_deref()]);
            tx.events().append(&events::invalidate(keys)).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }
}

/// The checks of [`MenuService::put`] that need no store: name, label, item count.
fn check_menu(menu: &Menu) -> Result<(), CoreError> {
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
    Ok(())
}

/// [`MenuService::put`] without the capability check, the transaction and the event:
/// validates the menu and every item against `repos` and replaces the menu. The caller
/// emits `menu:<name>` (the WordPress import collects it into its batch's event).
///
/// An empty `menu.locale` keeps the stored menu's locale (a new menu takes the site's
/// default locale).
///
/// Returns the invalidation keys of the write: `menu:<name>`, and `menu:<of>` for the
/// menu it translates now and the one it translated before (pages in that locale showed
/// the default menu at that location until now, and depend on its key).
pub(crate) async fn put_in(repos: &dyn Repos, menu: &Menu) -> Result<Vec<String>, CoreError> {
    check_menu(menu)?;
    for item in &menu.items {
        check_item(repos, item).await?;
    }
    let locales = crate::locales::read(repos).await?;
    let stored = repos.menus().get(&menu.name).await?;
    let mut menu = menu.clone();
    if menu.locale.is_empty() {
        menu.locale = match &stored {
            Some(stored) if !stored.locale.is_empty() => stored.locale.clone(),
            _ => locales.default.tag.clone(),
        };
    }
    let keys = menu_keys(
        &menu.name,
        [
            menu.translation_of.as_deref(),
            stored.as_ref().and_then(|s| s.translation_of.as_deref()),
        ],
    );
    if !locales.contains(&menu.locale) {
        return Err(CoreError::invalid_field(
            "locale",
            format!("{:?} is not one of the site's locales", menu.locale),
        ));
    }
    if let Some(of) = &menu.translation_of {
        if of == &menu.name {
            return Err(CoreError::invalid_field(
                "translation_of",
                "a menu cannot translate itself",
            ));
        }
        match repos.menus().get(of).await? {
            Some(original) if original.locale == locales.default.tag => {}
            _ => {
                return Err(CoreError::invalid_field(
                    "translation_of",
                    "translation_of must name a menu in the default locale",
                ));
            }
        }
        if menu.locale == locales.default.tag {
            return Err(CoreError::invalid_field(
                "translation_of",
                "a translation is in another locale than the default",
            ));
        }
        if let Some(taken) = repos.menus().get_translation(of, &menu.locale).await?
            && taken.name != menu.name
        {
            return Err(CoreError::InUse(format!(
                "the translation of {of} into {} already exists: {}",
                menu.locale, taken.name
            )));
        }
    }
    repos.menus().put(&menu).await?;
    Ok(keys)
}

/// `menu:<name>` and `menu:<of>` for each translated menu named (deduplicated).
fn menu_keys<'a>(name: &str, of: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut keys = vec![events::menu_key(name)];
    for of in of.into_iter().flatten() {
        let key = events::menu_key(of);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// `InvalidField { field, .. }` when `label` is longer than [`MAX_MENU_LABEL`].
fn check_label(label: &str, field: &'static str, what: &str) -> Result<(), CoreError> {
    if label.chars().count() > MAX_MENU_LABEL {
        return Err(CoreError::invalid_field(
            field,
            format!("{what} longer than {MAX_MENU_LABEL} characters"),
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
            if url.chars().count() > MAX_MENU_URL {
                return Err(CoreError::Invalid(format!(
                    "menu url longer than {MAX_MENU_URL} characters"
                )));
            }
            if !is_allowed_menu_url(url) {
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
pub fn is_allowed_menu_url(url: &str) -> bool {
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
    use super::{MenuLocation, is_allowed_menu_url, menu_locations};

    #[test]
    fn locations_mark_existing_menus() {
        let summary = |name: &str, label: &str| ferrite_store::MenuSummary {
            name: name.into(),
            label: label.into(),
            locale: "en".into(),
            translation_of: None,
        };
        let menus = [summary("primary", "Main"), summary("sidebar", "Side")];
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
            assert!(is_allowed_menu_url(ok), "{ok}");
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
            assert!(!is_allowed_menu_url(bad), "{bad}");
        }
    }
}
