//! Backend-shared menu tree and redirect checks, so both stores behave identically.

use std::collections::HashMap;

use ferrite_domain::{Menu, MenuItem, MenuItemId, REDIRECT_CODES};

use crate::StoreError;

/// Rejects a redirect status code outside [`REDIRECT_CODES`].
pub fn check_redirect_code(code: u16) -> Result<(), StoreError> {
    if REDIRECT_CODES.contains(&code) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!(
            "redirect code must be one of {REDIRECT_CODES:?}, got {code}"
        )))
    }
}

/// The items of `menu` ordered parents-before-children (by depth, then input order), so a
/// row's `parent_id` foreign key is satisfied at insert time without deferring it.
/// A parent that is not an item of the same menu, or a cycle, is `Invalid`.
pub fn insertion_order(menu: &Menu) -> Result<Vec<&MenuItem>, StoreError> {
    let parents: HashMap<MenuItemId, Option<MenuItemId>> =
        menu.items.iter().map(|i| (i.id, i.parent_id)).collect();
    let mut keyed = Vec::with_capacity(menu.items.len());
    for item in &menu.items {
        let mut depth = 0_usize;
        let mut at = item.parent_id;
        while let Some(parent) = at {
            depth += 1;
            if depth > parents.len() {
                return Err(StoreError::Invalid(format!(
                    "menu item {} is in a parent cycle",
                    item.id
                )));
            }
            at = *parents.get(&parent).ok_or_else(|| {
                StoreError::Invalid(format!(
                    "menu item {} has parent {parent} outside this menu",
                    item.id
                ))
            })?;
        }
        keyed.push((depth, item));
    }
    keyed.sort_by_key(|(depth, _)| *depth); // stable
    Ok(keyed.into_iter().map(|(_, item)| item).collect())
}

/// Orders stored items depth-first: roots first, siblings by `position` then id.
pub fn tree_order(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut children: HashMap<Option<MenuItemId>, Vec<MenuItem>> = HashMap::new();
    for item in items {
        children.entry(item.parent_id).or_default().push(item);
    }
    for siblings in children.values_mut() {
        // Reverse order so `pop` yields the smallest first.
        siblings.sort_by_key(|a| std::cmp::Reverse((a.position, a.id)));
    }
    let mut out = Vec::new();
    let mut stack: Vec<MenuItem> = children.remove(&None).unwrap_or_default();
    while let Some(item) = stack.pop() {
        if let Some(kids) = children.remove(&Some(item.id)) {
            stack.extend(kids);
        }
        out.push(item);
    }
    out
}

#[cfg(test)]
mod tests {
    use ferrite_domain::MenuItemKind;

    use super::*;

    fn item(id: MenuItemId, parent: Option<MenuItemId>, position: i32) -> MenuItem {
        MenuItem {
            id,
            parent_id: parent,
            position,
            kind: MenuItemKind::Url,
            target_id: None,
            url: Some("/".into()),
            label: format!("{position}"),
        }
    }

    fn menu(items: Vec<MenuItem>) -> Menu {
        Menu {
            name: "m".into(),
            label: "M".into(),
            items,
        }
    }

    #[test]
    fn parents_come_first_and_bad_parents_are_rejected() {
        let (a, b, c) = (MenuItemId::new(), MenuItemId::new(), MenuItemId::new());
        let m = menu(vec![
            item(c, Some(b), 0),
            item(b, Some(a), 0),
            item(a, None, 0),
        ]);
        let ids: Vec<_> = insertion_order(&m).unwrap().iter().map(|i| i.id).collect();
        assert_eq!(ids, [a, b, c]);

        let orphan = menu(vec![item(a, Some(MenuItemId::new()), 0)]);
        assert!(insertion_order(&orphan).is_err());
        let cycle = menu(vec![item(a, Some(b), 0), item(b, Some(a), 0)]);
        assert!(insertion_order(&cycle).is_err());
        let own = menu(vec![item(a, Some(a), 0)]);
        assert!(insertion_order(&own).is_err());
    }

    #[test]
    fn tree_order_is_depth_first_by_position() {
        let (a, a1, a2, b) = (
            MenuItemId::new(),
            MenuItemId::new(),
            MenuItemId::new(),
            MenuItemId::new(),
        );
        let items = vec![
            item(a2, Some(a), 1),
            item(b, None, 1),
            item(a1, Some(a), 0),
            item(a, None, 0),
        ];
        let ids: Vec<_> = tree_order(items).iter().map(|i| i.id).collect();
        assert_eq!(ids, [a, a1, a2, b]);
    }
}
