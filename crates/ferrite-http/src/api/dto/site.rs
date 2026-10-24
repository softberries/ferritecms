//! Bodies of the `/menus`, `/redirects` and `/settings` endpoints.
//!
//! Input bodies ignore members they do not know; in a `PATCH` an absent member keeps the
//! stored value and `null` is refused (`400 malformed-json`).

use chrono::{DateTime, Utc};
use ferrite_core::ImportReport;
use ferrite_domain::{Menu, MenuItem, MenuItemId, MenuItemKind, Redirect};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;
use uuid::Uuid;

use super::content::present;

/// What a menu entry links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MenuItemKindDto {
    /// A content item (`target_id`).
    Item,
    /// A term archive (`target_id`).
    Term,
    /// A URL (`url`): `http(s)://…` or a local path.
    Url,
}

impl From<MenuItemKind> for MenuItemKindDto {
    fn from(k: MenuItemKind) -> Self {
        match k {
            MenuItemKind::Item => Self::Item,
            MenuItemKind::Term => Self::Term,
            MenuItemKind::Url => Self::Url,
        }
    }
}

impl From<MenuItemKindDto> for MenuItemKind {
    fn from(k: MenuItemKindDto) -> Self {
        match k {
            MenuItemKindDto::Item => Self::Item,
            MenuItemKindDto::Term => Self::Term,
            MenuItemKindDto::Url => Self::Url,
        }
    }
}

/// One menu entry. The tree is flat: `parent_id` names another entry of the same menu
/// (`null` for a top-level one), and siblings are ordered by `position`, then `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MenuItemDto {
    /// Chosen by the client (a new UUID for a new entry); unique across all menus.
    pub id: Uuid,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    pub position: i32,
    pub kind: MenuItemKindDto,
    /// The content item or term (`item`, `term`); `null` for `url`.
    #[serde(default)]
    pub target_id: Option<Uuid>,
    /// Only for `url`.
    #[serde(default)]
    pub url: Option<String>,
    /// At most 200 characters.
    #[serde(default)]
    pub label: String,
}

impl From<MenuItem> for MenuItemDto {
    fn from(i: MenuItem) -> Self {
        Self {
            id: i.id.as_uuid(),
            parent_id: i.parent_id.map(|p| p.as_uuid()),
            position: i.position,
            kind: i.kind.into(),
            target_id: i.target_id,
            url: i.url,
            label: i.label,
        }
    }
}

impl From<MenuItemDto> for MenuItem {
    fn from(i: MenuItemDto) -> Self {
        Self {
            id: MenuItemId::from_uuid(i.id),
            parent_id: i.parent_id.map(MenuItemId::from_uuid),
            position: i.position,
            kind: i.kind.into(),
            target_id: i.target_id,
            url: i.url,
            label: i.label,
        }
    }
}

/// A menu with its entries in tree order (roots first, each followed by its children).
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuDto {
    /// Lowercase, no whitespace or `/` (a theme's menu location, e.g. `primary`).
    pub name: String,
    pub label: String,
    pub items: Vec<MenuItemDto>,
}

impl From<Menu> for MenuDto {
    fn from(m: Menu) -> Self {
        Self {
            name: m.name,
            label: m.label,
            items: m.items.into_iter().map(MenuItemDto::from).collect(),
        }
    }
}

/// `PUT /menus/{name}`: the whole menu (the path names it; a `name` member, if sent,
/// must match).
#[derive(Debug, Deserialize, ToSchema)]
pub struct MenuInput {
    #[serde(default)]
    pub name: Option<String>,
    /// At most 200 characters.
    #[serde(default)]
    pub label: String,
    /// At most 500 entries.
    #[serde(default)]
    pub items: Vec<MenuItemDto>,
}

/// A menu's name and label.
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuSummary {
    pub name: String,
    pub label: String,
}

/// `GET /menus`: every menu, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuList {
    pub items: Vec<MenuSummary>,
    /// Always `null`: one page holds every menu.
    pub next_cursor: Option<String>,
}

/// How a redirect came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RedirectOriginDto {
    Manual,
    Import,
    /// Created by core when a published item's path changed.
    SlugChange,
}

impl From<ferrite_domain::RedirectOrigin> for RedirectOriginDto {
    fn from(o: ferrite_domain::RedirectOrigin) -> Self {
        use ferrite_domain::RedirectOrigin as O;
        match o {
            O::Manual => Self::Manual,
            O::Import => Self::Import,
            O::SlugChange => Self::SlugChange,
        }
    }
}

/// A redirect.
#[derive(Debug, Serialize, ToSchema)]
pub struct RedirectDto {
    pub id: Uuid,
    /// The path it matches (exactly, or with the trailing slash toggled).
    pub source: String,
    /// A local path or an absolute http(s) URL.
    pub target: String,
    /// 301, 302, 307, 308 or 410.
    pub code: u16,
    pub hits: i64,
    pub origin: RedirectOriginDto,
    pub created_at: DateTime<Utc>,
    pub last_hit_at: Option<DateTime<Utc>>,
}

impl From<Redirect> for RedirectDto {
    fn from(r: Redirect) -> Self {
        Self {
            id: r.id.as_uuid(),
            source: r.source_path,
            target: r.target,
            code: r.code,
            hits: r.hits,
            origin: r.origin.into(),
            created_at: r.created_at.as_datetime(),
            last_hit_at: r.last_hit_at.map(|t| t.as_datetime()),
        }
    }
}

/// `GET /redirects`: one page, by source.
#[derive(Debug, Serialize, ToSchema)]
pub struct RedirectList {
    pub items: Vec<RedirectDto>,
    pub next_cursor: Option<String>,
}

/// The default redirect code.
fn moved_permanently() -> u16 {
    301
}

/// `POST /redirects`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RedirectInput {
    pub source: String,
    pub target: String,
    /// Default 301.
    #[serde(default = "moved_permanently")]
    pub code: u16,
}

/// `PATCH /redirects/{id}`: merged into the current redirect.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RedirectPatch {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub source: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub target: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = u16, required = false)]
    pub code: Option<u16>,
}

/// `POST /redirects/import`: what the import did.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportReportDto {
    /// Rows whose source had no redirect.
    pub created: u32,
    /// Rows that replaced the target and code of an existing redirect.
    pub updated: u32,
}

impl From<ImportReport> for ImportReportDto {
    fn from(r: ImportReport) -> Self {
        Self {
            created: r.created,
            updated: r.updated,
        }
    }
}

/// The site settings (`GET /settings`, and the result of `PATCH /settings`): every key,
/// `null` when unset (the default applies). A `PATCH` body is an object with any of
/// these keys (`null` resets one to its default); an unknown key is `422`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SettingsDto {
    /// At most 200 characters.
    #[serde(rename = "site.title")]
    #[schema(value_type = Option<String>)]
    pub site_title: Value,
    /// At most 1000 characters.
    #[serde(rename = "site.description")]
    #[schema(value_type = Option<String>)]
    pub site_description: Value,
    /// 1 to 35 ASCII letters, digits, `-` or `_` (e.g. `en`, `pt-BR`).
    #[serde(rename = "site.locale")]
    #[schema(value_type = Option<String>)]
    pub site_locale: Value,
    /// 1..=100.
    #[serde(rename = "reading.per_page")]
    #[schema(value_type = Option<i64>)]
    pub reading_per_page: Value,
    /// The id of an existing page shown at `/`, or `null` for the post listing.
    #[serde(rename = "reading.front_page")]
    #[schema(value_type = Option<Uuid>)]
    pub reading_front_page: Value,
    /// Ask search engines not to index the site (`robots.txt` disallows everything).
    #[serde(rename = "reading.discourage_indexing")]
    #[schema(value_type = Option<bool>)]
    pub reading_discourage_indexing: Value,
    /// The post permalink structure (default `/%slug%/`).
    #[serde(rename = "permalink.post")]
    #[schema(value_type = Option<String>)]
    pub permalink_post: Value,
    /// The page permalink structure (default `/%parent_path%/%slug%/`; needs
    /// `%parent_path%`).
    #[serde(rename = "permalink.page")]
    #[schema(value_type = Option<String>)]
    pub permalink_page: Value,
    /// Whether posts take comments (default `true`).
    #[serde(rename = "comments.enabled")]
    #[schema(value_type = Option<bool>)]
    pub comments_enabled: Value,
    /// `"all"` (every guest comment is held) or `"known"` (default: a commenter with an
    /// approved comment, by the same name and e-mail, posts directly). Under `known`
    /// anyone who knows a regular guest's exact name and e-mail posts unmoderated as
    /// them (as in WordPress); `"all"` closes that.
    #[serde(rename = "comments.moderation")]
    #[schema(value_type = Option<String>)]
    pub comments_moderation: Value,
}

impl SettingsDto {
    /// From [`ferrite_core::settings::read_all`]'s map (a key it lacks is `null`).
    pub fn from_map(mut map: Map<String, Value>) -> Self {
        let mut take = |key: &str| map.remove(key).unwrap_or(Value::Null);
        Self {
            site_title: take("site.title"),
            site_description: take("site.description"),
            site_locale: take("site.locale"),
            reading_per_page: take("reading.per_page"),
            reading_front_page: take("reading.front_page"),
            reading_discourage_indexing: take("reading.discourage_indexing"),
            permalink_post: take("permalink.post"),
            permalink_page: take("permalink.page"),
            comments_enabled: take("comments.enabled"),
            comments_moderation: take("comments.moderation"),
        }
    }
}

#[cfg(test)]
mod tests {
    use ferrite_core::settings::SETTINGS;

    use super::*;

    #[test]
    fn settings_dto_has_every_allowlisted_key() {
        let v = serde_json::to_value(SettingsDto::from_map(Map::new())).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let mut want: Vec<&str> = SETTINGS.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        want.sort_unstable();
        assert_eq!(keys, want);
    }
}
