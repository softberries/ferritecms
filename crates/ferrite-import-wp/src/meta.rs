//! WordPress post meta → `wp.<key>` item meta.
//!
//! Every key the import does not read itself (SEO, ACF values, the featured image) is kept
//! as `wp.<key>`. Its value is a JSON string, or a JSON array of strings when WordPress
//! repeated the key (every value is kept), except WordPress's bookkeeping, which is dropped:
//! `_edit_lock`, `_edit_last`, `_pingme`, `_encloseme`, `_wp_old_slug`, `_wp_old_date`,
//! `_wp_trash_meta_*`, `_wp_desired_post_slug`, `_oembed_*`, `_menu_item_*` and ACF's
//! `_<name>` reference keys (whose value is a field key, `field_…`). `_wp_page_template` is
//! kept as `wp.page_template`.

use std::collections::HashMap;

use serde_json::Value;

use crate::model::Item;

/// The prefix of kept meta keys.
pub const PREFIX: &str = "wp.";
/// Keys dropped as they are.
const DROPPED: [&str; 7] = [
    "_edit_lock",
    "_edit_last",
    "_pingme",
    "_encloseme",
    "_wp_old_slug",
    "_wp_old_date",
    "_wp_desired_post_slug",
];
/// Key prefixes dropped.
const DROPPED_PREFIXES: [&str; 3] = ["_wp_trash_meta_", "_oembed_", "_menu_item_"];
/// The page template key and the name it is kept under.
const PAGE_TEMPLATE: (&str, &str) = ("_wp_page_template", "wp.page_template");

/// Whether WordPress meta `key` with `value` is bookkeeping the import drops.
pub fn is_dropped(key: &str, value: &str) -> bool {
    DROPPED.contains(&key)
        || DROPPED_PREFIXES.iter().any(|p| key.starts_with(p))
        || (key.starts_with('_') && value.starts_with("field_"))
}

/// The `wp.*` meta of `item`: every key that is neither dropped nor `consumed` (read by
/// the import into a field of its own), in first-seen order.
pub fn wp_meta(item: &Item, consumed: impl Fn(&str) -> bool) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (key, value) in &item.meta {
        if key.is_empty() || is_dropped(key, value) || consumed(key) {
            continue;
        }
        let name = if key == PAGE_TEMPLATE.0 {
            PAGE_TEMPLATE.1.to_owned()
        } else {
            format!("{PREFIX}{key}")
        };
        let text = Value::String(value.replace('\0', ""));
        match seen.get(&name) {
            Some(&i) => match &mut out[i].1 {
                Value::Array(values) => values.push(text),
                first => *first = Value::Array(vec![first.take(), text]),
            },
            None => {
                seen.insert(name.clone(), out.len());
                out.push((name, text));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn bookkeeping_is_dropped_and_the_rest_kept() {
        let item = Item {
            meta: [
                ("_edit_lock", "1"),
                ("_pingme", "1"),
                ("_wp_trash_meta_status", "publish"),
                ("_oembed_time_abc", "1"),
                ("_menu_item_type", "custom"),
                ("_isbn", "field_isbn"),
                ("_wp_page_template", "default"),
                ("_yoast_wpseo_focuskw", "x"),
                ("color", "red"),
                ("color", "blue"),
                ("isbn", "123"),
                ("nul", "a\0b"),
            ]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
            ..Item::default()
        };
        assert_eq!(
            wp_meta(&item, |k| k == "isbn"),
            [
                ("wp.page_template".to_owned(), json!("default")),
                ("wp._yoast_wpseo_focuskw".to_owned(), json!("x")),
                ("wp.color".to_owned(), json!(["red", "blue"])),
                ("wp.nul".to_owned(), json!("ab")),
            ]
        );
    }
}
