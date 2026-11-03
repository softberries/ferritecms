//! `nav_menu_item` items → menus (pass 4, P19).
//!
//! One menu per `nav_menu` term: name = the term slug, label = its name. Items follow
//! `menu_order`, nested by `_menu_item_menu_item_parent`. Kinds: `custom` → a URL (an old
//! site URL becomes the new path), `post_type` → the item, `taxonomy` → the term; an empty
//! title takes the target's. Core's menu items have no link target, so `_blank` is
//! dropped and counted. A data problem never fails the run: an entry whose target was
//! not imported or is trashed here, or whose URL core would refuse, is left out and noted
//! (its children move up to its parent), and labels are cut to core's limit. Theme
//! locations are not in WXR, so none are assigned.
//!
//! Re-runs follow P4. The map row (kind `menu`, the term's WP id) keeps, as JSON in its
//! `hash`, a digest of the menu as written (its entries resolved to items, terms and
//! URLs) and the name it was written under:
//! - not mapped: written; when the site already has a menu of that name (one of its
//!   own), the import's goes under `<name>-wp` (or `-wp-2`, …), noted;
//! - mapped, but no menu of that name any more (deleted here): written again;
//! - mapped, and the stored menu differs from what the import wrote (edited here): kept,
//!   noted;
//! - mapped, and the menu built now has the same digest: unchanged; else rewritten (also
//!   when only its targets resolve differently, e.g. an item imported only now).

use std::collections::HashMap;

use ferrite_core::import::ImportBatch;
use ferrite_core::{MAX_MENU_ITEMS, MAX_MENU_LABEL, MAX_MENU_URL, is_allowed_menu_url, slug};
use ferrite_domain::{
    ContentId, ContentStatus, ImportMapKind, Menu, MenuItem, MenuItemId, MenuItemKind,
};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
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
/// Names tried for an imported menu beside a site menu of the same name.
const NAME_ATTEMPTS: usize = 100;

/// What the import map keeps about a menu it wrote (JSON in the row's `hash`).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct State {
    /// The digest of the menu as written ([`digest`]).
    written: String,
    /// The name it was written under.
    name: String,
}

/// What every menu entry reads.
struct Lookups<'a> {
    ir: &'a Ir,
    links: &'a Links,
    /// The export's terms by WP id.
    terms: HashMap<i64, &'a Term>,
    dry_run: bool,
}

/// One menu to write: its term, name and entries (in `menu_order`).
struct MenuIn<'a> {
    term: &'a Term,
    name: &'a str,
    items: &'a [Item],
}

/// Imports every menu.
pub(crate) async fn import(
    session: &Session<'_>,
    links: &Links,
    report: &mut Report,
) -> Result<(), ImportError> {
    let ir = session.ir();
    let terms = ir.terms().await?;
    let menus: Vec<&Term> = terms.iter().filter(|t| t.taxonomy == "nav_menu").collect();
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
    let lookups = Lookups {
        ir,
        links,
        terms: terms.iter().map(|t| (t.wp_id, t)).collect(),
        dry_run: session.opts().dry_run,
    };
    for term in menus {
        let name = percent_decode(&term.slug);
        let mut menu_items = entries.remove(&name).unwrap_or_default();
        menu_items.sort_by_key(|i| i.menu_order);
        let menu = MenuIn {
            term,
            name: &name,
            items: &menu_items,
        };
        let mut batch = session.batch().await?;
        let mut part = Report::default();
        let written = write(&mut batch, &lookups, &menu, &mut part).await;
        session.finish_batch(batch, written).await?;
        report.absorb(part);
    }
    Ok(())
}

/// The SHA-256 (hex) of `value` as JSON.
fn hash(value: &impl Serialize) -> Result<String, ImportError> {
    let json = serde_json::to_vec(value)?;
    Ok(Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A menu's content without its entry ids: its label, and per entry the label, kind,
/// target, URL, parent (as an index) and position.
fn digest(menu: &Menu) -> Result<String, ImportError> {
    let index: HashMap<MenuItemId, usize> = menu
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id, i))
        .collect();
    let entries: Vec<_> = menu
        .items
        .iter()
        .map(|i| {
            (
                &i.label,
                i.kind.as_str(),
                i.target_id,
                &i.url,
                i.parent_id.and_then(|p| index.get(&p).copied()),
                i.position,
            )
        })
        .collect();
    hash(&(&menu.label, entries))
}

/// Writes one menu as the module docs say.
async fn write(
    b: &mut ImportBatch<'_>,
    cx: &Lookups<'_>,
    menu: &MenuIn<'_>,
    report: &mut Report,
) -> Result<(), ImportError> {
    let MenuIn { term, name, items } = *menu;
    if slug::validate(name).ok().as_deref() != Some(name) {
        report.counts_of(KIND).skipped += 1;
        report.note(format!(
            "menu {name:?} skipped: its name is not a valid menu name"
        ));
        return Ok(());
    }
    let state = match b.lookup(ImportMapKind::Menu, term.wp_id).await? {
        Some(m) => serde_json::from_str::<State>(&m.hash).ok(),
        None => None,
    };
    let (target, created) = match &state {
        Some(state) => match b.get_menu(&state.name).await? {
            None => (state.name.clone(), true),
            Some(stored) if digest(&stored)? != state.written => {
                report.counts_of(KIND).kept += 1;
                report.note(format!(
                    "menu {:?} was edited here since the last import; kept",
                    state.name
                ));
                return Ok(());
            }
            Some(_) => (state.name.clone(), false),
        },
        None => match free_name(b, name).await? {
            Some(free) => {
                if free != name {
                    report.note(format!(
                        "the site already has a menu {name:?}; the imported one is {free:?}"
                    ));
                }
                (free, true)
            }
            None => {
                report.counts_of(KIND).skipped += 1;
                report.note(format!("menu {name:?} skipped: no free name for it"));
                return Ok(());
            }
        },
    };
    let out = build(b, cx, &target, term, items, report).await?;
    let written = digest(&out)?;
    if !created && state.as_ref().is_some_and(|s| s.written == written) {
        report.counts_of(KIND).unchanged += 1;
        return Ok(());
    }
    b.menu(&out).await?;
    let state = State {
        written,
        name: target,
    };
    b.remember(
        ImportMapKind::Menu,
        term.wp_id,
        Uuid::nil(),
        &serde_json::to_string(&state)?,
        0,
    )
    .await?;
    let counts = report.counts_of(KIND);
    if created {
        counts.created += 1;
    } else {
        counts.updated += 1;
    }
    Ok(())
}

/// `name`, or `<name>-wp`, `<name>-wp-2`, … when the site has a menu of that name; each
/// candidate cut before its suffix to fit core's slug limit and checked with its rule.
/// `None` when no valid name is free.
async fn free_name(b: &mut ImportBatch<'_>, name: &str) -> Result<Option<String>, ImportError> {
    for n in 0..NAME_ATTEMPTS {
        let Some(candidate) = candidate(name, n) else {
            continue;
        };
        if b.get_menu(&candidate).await?.is_none() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// The `n`th name to try for menu `name` (see [`free_name`]), if it is a valid one: the
/// name itself, then the name cut to make room for the suffix.
fn candidate(name: &str, n: usize) -> Option<String> {
    let suffix = match n {
        0 => String::new(),
        1 => "-wp".to_owned(),
        n => format!("-wp-{n}"),
    };
    let candidate = if suffix.is_empty() {
        name.to_owned()
    } else {
        let keep = slug::MAX_LEN.saturating_sub(suffix.chars().count());
        let head: String = name.chars().take(keep).collect();
        format!("{}{suffix}", head.trim_end_matches('-'))
    };
    (slug::validate(&candidate).ok().as_deref() == Some(candidate.as_str())).then_some(candidate)
}

/// The menu `name` from the WXR entries. Entries left out are noted, except on a dry
/// run, where the rows of earlier batches are gone (the dry-run note says so).
async fn build(
    b: &mut ImportBatch<'_>,
    cx: &Lookups<'_>,
    name: &str,
    term: &Term,
    items: &[Item],
    report: &mut Report,
) -> Result<Menu, ImportError> {
    let nodes: Vec<(i64, i64)> = items.iter().map(|i| (i.wp_id, parent_of(i))).collect();
    let by_wp: HashMap<i64, &Item> = items.iter().map(|i| (i.wp_id, i)).collect();
    let mut kept: HashMap<i64, Option<MenuItemId>> = HashMap::new();
    let mut out: Vec<MenuItem> = Vec::new();
    let mut positions: HashMap<Option<MenuItemId>, i32> = HashMap::new();
    let mut blank = 0u64;
    let mut notes = Vec::new();
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
        let entry = match entry(b, cx, item).await? {
            Ok(entry) if out.len() < MAX_MENU_ITEMS => entry,
            Ok(_) => {
                notes.push(format!(
                    "menu {name:?}: more than {MAX_MENU_ITEMS} entries; entry {wp_id} left out"
                ));
                kept.insert(wp_id, None);
                continue;
            }
            Err(why) => {
                notes.push(format!("menu {name:?}: entry {wp_id} left out: {why}"));
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
        notes.push(format!(
            "menu {name:?}: {blank} entries opened in a new tab; menu items have no target here"
        ));
    }
    if !cx.dry_run {
        for note in notes {
            report.note(note);
        }
    }
    Ok(Menu {
        name: name.to_owned(),
        label: label(&term.name),
        items: out,
        locale: String::new(),
        translation_of: None,
    })
}

/// A label as core takes it: no control characters, at most [`MAX_MENU_LABEL`].
fn label(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_MENU_LABEL)
        .collect()
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
    cx: &Lookups<'_>,
    item: &Item,
) -> Result<Result<MenuItem, String>, ImportError> {
    let kind = meta(item, "_menu_item_type").unwrap_or_default();
    let object = meta(item, "_menu_item_object").unwrap_or_default();
    let object_id: i64 = meta(item, "_menu_item_object_id")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let mut text = item.title.trim().to_owned();
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
            let url = cx
                .links
                .local_url(meta(item, "_menu_item_url").unwrap_or_default());
            if !is_allowed_menu_url(&url) || url.chars().count() > MAX_MENU_URL {
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
            let Some(target) = b.lookup(ImportMapKind::Content, object_id).await? else {
                return Ok(Err(format!("its {object} {object_id} was not imported")));
            };
            let usable = b
                .get_item(ContentId::from_uuid(target.id))
                .await?
                .is_some_and(|i| i.status != ContentStatus::Trash);
            if !usable {
                return Ok(Err(format!(
                    "its {object} {object_id} is in the trash here"
                )));
            }
            if text.is_empty() {
                text = cx
                    .ir
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
            if text.is_empty() {
                text = cx
                    .terms
                    .get(&object_id)
                    .map(|t| t.name.clone())
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
        label: label(&text),
        ..entry
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_fit_the_slug_limit() {
        assert_eq!(candidate("main", 0).as_deref(), Some("main"));
        assert_eq!(candidate("main", 1).as_deref(), Some("main-wp"));
        assert_eq!(candidate("main", 3).as_deref(), Some("main-wp-3"));
        let long = "m".repeat(199);
        assert_eq!(candidate(&long, 0).as_deref(), Some(long.as_str()));
        let free = candidate(&long, 1).unwrap();
        assert_eq!(free, format!("{}-wp", "m".repeat(197)));
        assert_eq!(candidate(&long, 12).unwrap().chars().count(), slug::MAX_LEN);
        assert_eq!(candidate(&"m".repeat(201), 0), None);
    }
}
