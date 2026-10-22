//! Outbox events and dependency keys (06 §6).

use ferrite_domain::{ContentId, NewEvent, TermId};

/// Event kind for cache invalidation.
pub const INVALIDATE: &str = "invalidate";

/// Dependency key of a content item.
pub fn post_key(id: ContentId) -> String {
    format!("post:{id}")
}

/// Dependency key of a content type's archive.
pub fn archive_key(kind: &str) -> String {
    format!("archive:{kind}")
}

/// Dependency key of a term.
pub fn term_key(id: TermId) -> String {
    format!("term:{id}")
}

/// An `invalidate` event with `keys` deduplicated (first occurrence wins the position).
pub fn invalidate(keys: impl IntoIterator<Item = String>) -> NewEvent {
    let mut out: Vec<String> = Vec::new();
    for key in keys {
        if !out.contains(&key) {
            out.push(key);
        }
    }
    NewEvent {
        kind: INVALIDATE.to_owned(),
        keys: out,
        origin_node: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_dedupes_in_order() {
        let e = invalidate(["b".to_owned(), "a".to_owned(), "b".to_owned()]);
        assert_eq!(e.kind, "invalidate");
        assert_eq!(e.keys, ["b", "a"]);
        assert_eq!(e.origin_node, None);
    }

    #[test]
    fn key_formats() {
        let id = ContentId::new();
        assert_eq!(post_key(id), format!("post:{id}"));
        assert_eq!(archive_key("page"), "archive:page");
        let t = TermId::new();
        assert_eq!(term_key(t), format!("term:{t}"));
    }
}
