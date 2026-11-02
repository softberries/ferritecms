use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{MenuItemId, string_enum};

string_enum!(
    MenuItemKind {
        Item => "item",
        Term => "term",
        Url => "url",
    }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    pub id: MenuItemId,
    pub parent_id: Option<MenuItemId>,
    pub position: i32,
    pub kind: MenuItemKind,
    /// Content item or term id, depending on `kind`.
    pub target_id: Option<Uuid>,
    pub url: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Menu {
    pub name: String,
    pub label: String,
    pub items: Vec<MenuItem>,
    /// The menu's locale (13 §2.6).
    pub locale: String,
    /// The name of the default-locale menu this one translates; `None` for a menu of its
    /// own.
    pub translation_of: Option<String>,
}
