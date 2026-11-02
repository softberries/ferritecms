//! Menu tools (design 12 §5.2): `menus_list`, `menu_get` and `menu_set`.
//!
//! Adapters over [`MenuService`](ferrite_core::MenuService) and the REST layer's menu list
//! (`api::menus::menu_list`). The REST API keeps a menu as a flat list with
//! `parent_id` and `position`; the tools show and take a **nested tree** (`children`),
//! which is what a model can write without inventing ids: `menu_set` numbers the
//! siblings itself and keeps an item's `id` when it is given. Core checks every
//! permission (`menus:read`, `manage_menus` with `menus:write`) and validates the tree
//! (at most 500 items; labels, URLs, targets) inside the write's transaction, as for
//! `PUT /menus/{name}`.

use ferrite_domain::{Menu, MenuItem, MenuItemId, MenuItemKind};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    api::{
        dto::site::{MenuItemKindDto, MenuLocationDto, MenuSummary},
        menus::menu_list,
    },
    mcp::{Caller, FerriteMcp, results},
};

// ---- inputs ------------------------------------------------------------------------------

/// A menu named by its name (`menu_get`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameInput {
    /// The menu's name, for example `primary` or `footer` (from menus_list).
    pub name: String,
}

/// What a menu entry links to.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A content item (`target_id` is its id).
    Item,
    /// A term's archive page (`target_id` is the term id).
    Term,
    /// An address (`url`): `http(s)://...` or a local path such as `/contact/`.
    Url,
}

impl From<Kind> for MenuItemKind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Item => Self::Item,
            Kind::Term => Self::Term,
            Kind::Url => Self::Url,
        }
    }
}

/// One entry of the menu tree.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemIn {
    /// Keep the `id` menu_get returned for an entry you keep; leave out for a new one.
    #[serde(default)]
    pub id: Option<Uuid>,
    /// What the entry links to.
    pub kind: Kind,
    /// The text shown, at most 200 characters. Leave out to show the target's title.
    #[serde(default)]
    pub label: String,
    /// The content item or term id, for `item` and `term`.
    #[serde(default)]
    pub target_id: Option<Uuid>,
    /// The address, for `url`: `http(s)://...` or a path starting with `/`.
    #[serde(default)]
    pub url: Option<String>,
    /// Sub-entries, shown in this order.
    #[serde(default)]
    pub children: Vec<ItemIn>,
}

/// The input of `menu_set`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetInput {
    /// The menu's name: lowercase, no spaces or `/`. A theme shows the menu named like
    /// one of its locations (see menus_list `locations`), for example `primary`. A new
    /// name creates the menu.
    pub name: String,
    /// The menu's label, at most 200 characters. Leave out to keep the current one.
    #[serde(default)]
    pub label: Option<String>,
    /// The complete new tree, in display order, at most 500 entries in all. It replaces
    /// every entry the menu has now: `[]` empties the menu.
    pub items: Vec<ItemIn>,
}

// ---- outputs -----------------------------------------------------------------------------

/// `menus_list`.
#[derive(Debug, Serialize)]
pub struct MenusOut {
    pub items: Vec<MenuSummary>,
    /// The menu locations of the active theme, and whether a menu fills each.
    pub locations: Vec<MenuLocationDto>,
}

/// One entry of the menu tree as returned.
#[derive(Debug, Serialize)]
pub struct ItemOut {
    pub id: Uuid,
    pub kind: MenuItemKindDto,
    pub label: String,
    pub target_id: Option<Uuid>,
    pub url: Option<String>,
    pub children: Vec<ItemOut>,
}

/// A menu with its nested entries.
#[derive(Debug, Serialize)]
pub struct MenuOut {
    pub name: String,
    pub label: String,
    pub items: Vec<ItemOut>,
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = menu_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists the site's navigation menus (name and label) and the menu locations the
    /// active theme declares (for example `primary`, `footer`), each with whether a menu
    /// of that name exists. A theme shows the menu whose name equals a location. Use
    /// menu_get for the entries.
    #[tool(annotations(title = "List menus", read_only_hint = true, open_world_hint = false))]
    pub async fn menus_list(&self, caller: Caller) -> CallToolResult {
        match menu_list(&self.api, caller.actor()).await {
            Ok(list) => {
                let summary = format!(
                    "{} menu{}, {} theme location{}.",
                    list.items.len(),
                    if list.items.len() == 1 { "" } else { "s" },
                    list.locations.len(),
                    if list.locations.len() == 1 { "" } else { "s" },
                );
                results::ok(
                    MenusOut {
                        items: list.items,
                        locations: list.locations,
                    },
                    summary,
                )
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Returns one menu with its entries as a nested tree: each entry has its `id`,
    /// `kind` (`item`, `term` or `url`), `label`, `target_id` or `url`, and `children`,
    /// in display order. Read it before changing it with menu_set.
    #[tool(annotations(title = "Get menu", read_only_hint = true, open_world_hint = false))]
    pub async fn menu_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        match self.api.menus.get_for(caller.actor(), &input.name).await {
            Ok(menu) => {
                let out = menu_out(menu);
                let summary = format!("{:?}: {} entries.", out.name, count(&out.items));
                results::ok(out, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Replaces a menu's whole item tree with `items` (creating the menu when the name is
    /// new). Everything the menu has now is replaced, so call menu_get first and send
    /// the complete new tree, keeping the `id` of entries you keep. Entries link to a
    /// content item or term by id, or to an address; the theme shows the menu named like
    /// one of its locations. The live site changes at once, so confirm the new structure
    /// with the user. Returns the saved menu.
    #[tool(annotations(
        title = "Set menu",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn menu_set(
        &self,
        caller: Caller,
        Parameters(input): Parameters<SetInput>,
    ) -> CallToolResult {
        let actor = caller.actor();
        let label = match input.label {
            Some(l) => l,
            // Keep the current label; a menu that is new or cannot be read has none.
            None => self
                .api
                .menus
                .get_for(actor, &input.name)
                .await
                .map(|m| m.label)
                .unwrap_or_default(),
        };
        let mut items = Vec::new();
        flatten(input.items, None, &mut items);
        if let Err(p) = results::within_json_cap("items", &items) {
            return results::from_problem(p);
        }
        let menu = Menu {
            name: input.name,
            label,
            items,
        };
        if let Err(e) = self.api.menus.put(actor, &menu).await {
            return results::from_core(e);
        }
        self.audit(
            &caller,
            "menu_set",
            "menu",
            Some(menu.name.clone()),
            json!({"items": menu.items.len()}),
        )
        .await;
        let summary = format!(
            "Saved the menu {:?} with {} entries.",
            menu.name,
            menu.items.len()
        );
        // The saved menu as stored; if that read fails the write is still done.
        let saved = match self.api.menus.get_for(actor, &menu.name).await {
            Ok(m) => menu_out(m),
            Err(e) => {
                tracing::warn!(error = %e, menu = %menu.name, "menu not read after a done write");
                return results::ok(
                    json!({
                        "name": menu.name,
                        "note": "The menu was saved, but could not be read back; call menu_get.",
                    }),
                    summary,
                );
            }
        };
        results::ok(saved, summary)
    }
}

/// The input tree as the flat entries core stores: `parent_id` and sibling `position`.
fn flatten(items: Vec<ItemIn>, parent: Option<MenuItemId>, out: &mut Vec<MenuItem>) {
    for (position, item) in items.into_iter().enumerate() {
        let id = item.id.map_or_else(MenuItemId::new, MenuItemId::from_uuid);
        out.push(MenuItem {
            id,
            parent_id: parent,
            position: i32::try_from(position).unwrap_or(i32::MAX),
            kind: item.kind.into(),
            target_id: item.target_id,
            url: item.url,
            label: item.label,
        });
        flatten(item.children, Some(id), out);
    }
}

/// `menu` with its entries nested. Entries whose parent is not in the menu are left out
/// (core does not store such a menu).
fn menu_out(menu: Menu) -> MenuOut {
    let Menu { name, label, items } = menu;
    let mut sorted: Vec<&MenuItem> = items.iter().collect();
    sorted.sort_by_key(|i| (i.position, i.id.as_uuid()));
    MenuOut {
        name,
        label,
        items: nest(&sorted, None, 0),
    }
}

/// The nesting depth after which a (corrupt) tree is cut.
const MAX_DEPTH: usize = 64;

fn nest(items: &[&MenuItem], parent: Option<MenuItemId>, depth: usize) -> Vec<ItemOut> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    items
        .iter()
        .filter(|i| i.parent_id == parent)
        .map(|i| ItemOut {
            id: i.id.as_uuid(),
            kind: i.kind.into(),
            label: i.label.clone(),
            target_id: i.target_id,
            url: i.url.clone(),
            children: nest(items, Some(i.id), depth + 1),
        })
        .collect()
}

/// How many entries the tree holds.
fn count(items: &[ItemOut]) -> usize {
    items.iter().map(|i| 1 + count(&i.children)).sum()
}
