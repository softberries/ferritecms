//! How a [`PageItem`] looks to a template (`post`, `page`, listing entries).

use std::sync::Arc;

use ferrite_core::PageItem;
use minijinja::value::{Enumerator, Object, Serde, Value};

/// Template view of a [`PageItem`]: the narrow public item (ruling R8) plus its URL,
/// rendered body (marked safe: it was produced and sanitised by core), author, terms
/// and featured image. Timestamps are RFC 3339 strings (UTC).
#[derive(Debug)]
pub(crate) struct ItemObject(pub(crate) PageItem);

const KEYS: &[&str] = &[
    "id",
    "kind",
    "slug",
    "title",
    "excerpt",
    "url",
    "body",
    "date",
    "updated",
    "menu_order",
    "parent_id",
    "fields",
    "author",
    "terms",
    "featured",
];

impl Object for ItemObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.get_value_by_str(key.as_str()?)
    }

    fn get_value_by_str(self: &Arc<Self>, key: &str) -> Option<Value> {
        let p = &self.0;
        let i = &p.item;
        Some(match key {
            "id" => Value::from(i.id.to_string()),
            "kind" => Value::from(i.kind.as_str()),
            "slug" => Value::from(i.slug.as_str()),
            "title" => Value::from(i.title.as_str()),
            "excerpt" => Value::from(i.excerpt.as_str()),
            "url" => Value::from(p.url.as_str()),
            "body" => Value::from_safe_string(p.html.clone()),
            "date" => i
                .published_at
                .map_or(Value::from(()), |t| Value::from(t.to_rfc3339())),
            "updated" => Value::from(i.updated_at.to_rfc3339()),
            "menu_order" => Value::from(i.menu_order),
            "parent_id" => i
                .parent_id
                .map_or(Value::from(()), |id| Value::from(id.to_string())),
            "fields" => Value::from(Serde(&i.fields)),
            "author" => Value::from(Serde(&p.author)),
            "terms" => Value::from(Serde(&p.terms)),
            "featured" => Value::from(Serde(&p.featured)),
            _ => return None,
        })
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(KEYS)
    }
}
