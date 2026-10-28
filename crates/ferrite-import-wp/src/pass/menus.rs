//! `nav_menu_item` items → menus (pass 4, P19).
//!
//! One menu per `nav_menu` term: name = the term slug, label = its name. Items follow
//! `menu_order`, nested by `_menu_item_menu_item_parent`. Kinds: `custom` → a URL (an old
//! site URL becomes the new path), `post_type` → the item, `taxonomy` → the term; an empty
//! title takes the target's. Core's menu items have no link target, so `_blank` is
//! dropped and counted. An entry whose target was not imported (or is trashed), or whose
//! URL core would refuse, is left out and noted; its children move up to its parent.
//! Theme locations are not in WXR, so none are assigned.
//!
//! A menu is written whole. Its import-map row (kind `menu`, the term's WP id) keeps a
//! hash of its WXR records: the same hash on a re-run leaves the menu as it is.

use std::collections::HashMap;

use ferrite_core::MAX_MENU_ITEMS;
use ferrite_core::import::ImportBatch;
use ferrite_core::slug;
use ferrite_domain::{ImportMapKind, Menu, MenuItem, MenuItemId, MenuItemKind};
use futures::TryStreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::content::{Links, parent_first};
use super::meta;
use super::types::taxonomy_name;
use crate::ImportError;
use crate::ir::Ir;
use crate::model::{Item, Term};
use crate::run::{Report, Session};
use crate::urls::percent_decode;

/// The report kind of menus.
const KIND: &str = "menus";
/// Core's longest menu and item label (`menu::MAX_LABEL_LEN`).
const MAX_LABEL: usize = 200;
/// Core's longest menu URL (`menu::MAX_URL_LEN`).
const MAX_URL: usize = 2048;

/// One menu's WXR records, which its hash covers.
#[derive(Serialize)]
struct Source<'a> {
    term: &'a Term,
    items: &'a [Item],
}

/// Imports every menu.
pub(crate) async fn import(
    session: &Session<'_>,
    links: &Links,
    report: &mut Report,
) -> Result<(), ImportError> {
    let ir = session.ir();
    let menus: Vec<Term> = ir
        .terms()
        .await?
        .into_iter()
        .filter(|t| t.taxonomy == "nav_menu")
        .collect();
    if menus.is_empty() {
        return Ok(());
    }
    let mut entries: HashMap<String, Vec<Item>> = HashMap::new();
    let mut items = ir.items(&["nav_menu_item"]);
    while let Some(item) = items.try_next().await? {
        if let Some(menu) = item.terms.iter().find(|t| t.taxonomy == "nav_menu") {
            entries
                .entry(percent_decode(&menu.slug))
                .or_default()
                .push(item);
        }
    }
    drop(items);
    for term in &menus {
        let name = percent_decode(&term.slug);
        let mut menu_items = entries.remove(&name).unwrap_or_default();
        menu_items.sort_by_key(|i| i.menu_order);
        let hash = hash(&Source {
            term,
            items: &menu_items,
        })?;
        let menu = MenuIn {
            term,
            name: &name,
            items: &menu_items,
            hash: &hash,
        };
        let mut batch = session.batch().await?;
        let mut part = Report::default();
        let written = write(&mut batch, ir, links, &menu, &mut part).await;
        session.finish_batch(batch, written).await?;
        let (counts, c) = (report.counts_of(KIND), part.counts_of(KIND));
        counts.created += c.created;
        counts.updated += c.updated;
        counts.unchanged += c.unchanged;
        counts.skipped += c.skipped;
        report.notes.extend(part.notes);
    }
    Ok(())
}

/// The SHA-256 (hex) of a menu's records.
fn hash(source: &Source<'_>) -> Result<String, ImportError> {
    let json = serde_json::to_vec(source)?;
    Ok(Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// One menu to write: its term, name, entries (in `menu_order`) and records' hash.
struct MenuIn<'a> {
    term: &'a Term,
    name: &'a str,
    items: &'a [Item],
    hash: &'a str,
}

/// Writes one menu (unless its hash is the mapped one).
async fn write(
    b: &mut ImportBatch<'_>,
    ir: &Ir,
    links: &Links,
    menu: &MenuIn<'_>,
    report: &mut Report,
) -> Result<(), ImportError> {
    let MenuIn {
        term,
        name,
        items,
        hash,
    } = *menu;
    if slug::validate(name).ok().as_deref() != Some(name) {
        report.counts_of(KIND).skipped += 1;
        report.note(format!(
            "menu {name:?} skipped: its name is not a valid menu name"
        ));
        return Ok(());
    }
    let mapped = b.lookup(ImportMapKind::Menu, term.wp_id).await?;
    if mapped.as_ref().is_some_and(|m| m.hash == hash) {
        report.counts_of(KIND).unchanged += 1;
        return Ok(());
    }
    // Parents before children; a dropped entry's children move up to its parent.
    let nodes: Vec<(i64, i64)> = items.iter().map(|i| (i.wp_id, parent_of(i))).collect();
    let by_wp: HashMap<i64, &Item> = items.iter().map(|i| (i.wp_id, i)).collect();
    let mut kept: HashMap<i64, Option<MenuItemId>> = HashMap::new();
    let mut out: Vec<MenuItem> = Vec::new();
    let mut positions: HashMap<Option<MenuItemId>, i32> = HashMap::new();
    let mut blank = 0u64;
    for wp_id in parent_first(&nodes) {
        let item = by_wp[&wp_id];
        let mut parent_wp = parent_of(item);
        let parent = loop {
            match kept.get(&parent_wp) {
                Some(Some(id)) => break Some(*id),
                Some(None) => parent_wp = by_wp.get(&parent_wp).map_or(0, |p| parent_of(p)),
                None => break None,
            }
        };
        if meta(item, "_menu_item_target") == Some("_blank") {
            blank += 1;
        }
        let entry = match entry(b, ir, links, item).await? {
            Ok(entry) if out.len() < MAX_MENU_ITEMS => entry,
            Ok(_) => {
                report.note(format!(
                    "menu {name:?}: more than {MAX_MENU_ITEMS} entries; entry {wp_id} left out"
                ));
                kept.insert(wp_id, None);
                continue;
            }
            Err(why) => {
                report.note(format!("menu {name:?}: entry {wp_id} left out: {why}"));
                kept.insert(wp_id, None);
                continue;
            }
        };
        let id = MenuItemId::new();
        let position = positions.entry(parent).or_insert(0);
        out.push(MenuItem {
            id,
            parent_id: parent,
            position: *position,
            ..entry
        });
        *position += 1;
        kept.insert(wp_id, Some(id));
    }
    if blank > 0 {
        report.note(format!(
            "menu {name:?}: {blank} entries opened in a new tab; menu items have no target here"
        ));
    }
    let label: String = term.name.trim().chars().take(MAX_LABEL).collect();
    b.menu(&Menu {
        name: name.to_owned(),
        label,
        items: out,
    })
    .await?;
    b.remember(ImportMapKind::Menu, term.wp_id, Uuid::nil(), hash, 0)
        .await?;
    let counts = report.counts_of(KIND);
    if mapped.is_some() {
        counts.updated += 1;
    } else {
        counts.created += 1;
    }
    Ok(())
}

/// The WP id of an entry's parent entry (0 for none).
fn parent_of(item: &Item) -> i64 {
    meta(item, "_menu_item_menu_item_parent")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// One entry without its id, parent and position; `Err` with why it is left out.
async fn entry(
    b: &mut ImportBatch<'_>,
    ir: &Ir,
    links: &Links,
    item: &Item,
) -> Result<Result<MenuItem, String>, ImportError> {
    let kind = meta(item, "_menu_item_type").unwrap_or_default();
    let object = meta(item, "_menu_item_object").unwrap_or_default();
    let object_id: i64 = meta(item, "_menu_item_object_id")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let mut label = item.title.trim().to_owned();
    let base = MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position: 0,
        kind: MenuItemKind::Url,
        target_id: None,
        url: None,
        label: String::new(),
    };
    let entry = match kind {
        "custom" => {
            let url = links.local_url(meta(item, "_menu_item_url").unwrap_or_default());
            if !allowed_url(&url) {
                return Ok(Err(format!(
                    "its URL {url:?} is not http(s) or a local path"
                )));
            }
            MenuItem {
                url: Some(url),
                ..base
            }
        }
        "post_type" => {
            let trashed = links
                .item(object_id)
                .is_none_or(|(_, status)| status == "trash");
            let target = match trashed {
                true => None,
                false => b.lookup(ImportMapKind::Content, object_id).await?,
            };
            let Some(target) = target else {
                return Ok(Err(format!("its {object} {object_id} was not imported")));
            };
            if label.is_empty() {
                label = ir
                    .item(object_id)
                    .await?
                    .map(|i| i.title)
                    .unwrap_or_default();
            }
            MenuItem {
                kind: MenuItemKind::Item,
                target_id: Some(target.id),
                ..base
            }
        }
        "taxonomy" if taxonomy_name(object).is_some() => {
            let Some(target) = b.lookup(ImportMapKind::Term, object_id).await? else {
                return Ok(Err(format!(
                    "its {object} term {object_id} was not imported"
                )));
            };
            if label.is_empty() {
                label = ir
                    .terms()
                    .await?
                    .into_iter()
                    .find(|t| t.wp_id == object_id)
                    .map(|t| t.name)
                    .unwrap_or_default();
            }
            MenuItem {
                kind: MenuItemKind::Term,
                target_id: Some(target.id),
                ..base
            }
        }
        other => {
            return Ok(Err(format!(
                "entries of type {other:?} ({object}) are not imported"
            )));
        }
    };
    Ok(Ok(MenuItem {
        label: label
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_LABEL)
            .collect(),
        ..entry
    }))
}

/// Core's rule for a menu URL: `http(s)://…` or an absolute local path, no whitespace,
/// control characters or backslashes, at most 2 048 characters.
fn allowed_url(url: &str) -> bool {
    if url.chars().count() > MAX_URL
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || url.contains('\\')
    {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    let scheme = |p: &str| lower.strip_prefix(p).is_some_and(|rest| !rest.is_empty());
    scheme("http://") || scheme("https://") || (url.starts_with('/') && !url.starts_with("//"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_urls_follow_core_rules() {
        for ok in ["/", "/about/", "https://example.org/x", "HTTP://a"] {
            assert!(allowed_url(ok), "{ok}");
        }
        for bad in [
            "mailto:a@b.test",
            "#top",
            "//cdn.test/x",
            "http://",
            "/a b",
            "javascript:x",
        ] {
            assert!(!allowed_url(bad), "{bad}");
        }
    }
}
