//! Outbox events and the cache dependency-key contract (06 §6, R9/I5).
//!
//! The page cache stores each rendered page together with the set of dependency keys the
//! renderer recorded while building it. Write paths append an `invalidate` event whose
//! keys name everything the write may have changed; the cache drops every page whose key
//! set intersects an event's keys. The two sides only work if they agree, so the whole
//! contract lives here.
//!
//! # Key grammar
//!
//! | key | meaning |
//! |-----|---------|
//! | `post:<content id>` | one content item: its fields, URL, body, status. Also used for media items (a media item is a content item of kind `media`) |
//! | `term:<term id>` | one term: its name, slug, description, and the list of items in it |
//! | `archive:<kind>` | the listing of a content type (home, feeds, type archives, term archives of posts, date archives, search) |
//! | `menu:<name>` | a menu's items |
//! | `option:<key>` | one option value |
//! | `user:<user id>` | the public face of a user (login, display name) shown as an author |
//! | `redirect:<source path>` | a redirect (informational, see below) |
//! | `theme` | the active theme or its files; recorded by every page |
//! | `plugins` | the set of enabled plugins; recorded by every page |
//! | `all` | flush everything; the cache honours it by dropping every entry |
//!
//! # Emitter side (write paths in this crate)
//!
//! * Content `update`: `post:<id>`, `archive:<kind>`, `term:<t>` for the union of the old
//!   and the new terms, `post:<p>` for the old and the new parent (whichever are set),
//!   and `post:<d>` for each published descendant whose URL moved (the descendant walk
//!   of the redirect bookkeeping). Create emits the same set with no old terms.
//! * Every lifecycle transition (publish, unpublish, schedule, submit, publish_due,
//!   trash, restore, delete): `post:<id>`, `archive:<kind>`, `term:<t>` for all current
//!   terms (read before the links are removed on delete) and `post:<parent>` if any.
//! * Term create: `term:<id>` and `archive:<type>` for each content type of the taxonomy.
//!   Term update/delete: the same, plus `post:<item>` for every published item linked to
//!   the term (its pages show the term name), because the cached pages of those items
//!   embed the term. Past [`MAX_TERM_ITEMS`] items the item keys are replaced by `all`.
//! * Menu writes: `menu:<name>`. Option writes: kind `options.changed`, key
//!   `option:<key>`.
//! * Redirect writes (manual upsert/delete, and the 301s recorded when a published item
//!   moves): kind `invalidate`, keys `redirect:<source>` for each source touched and its
//!   trailing-slash variant. **Informational**: the router checks redirects before the
//!   cache, so no cached page depends on them; the keys exist so other nodes and
//!   observers can follow the changes.
//! * Users: pages show `user:<author id>`. There is no user-update service in core yet;
//!   the user service added in M2 must emit `user:<id>` (via [`user_key`]) whenever a
//!   login or display name changes or a user is deleted.
//! * Theme and plugin changes emit `theme` / `plugins` (not written by core yet).
//!
//! # Renderer side ([`SiteReader`](crate::SiteReader))
//!
//! A read records `post:<id>` for every item it shows (the single item, each listed
//! item, each ancestor whose slug is part of a URL, each published media item shown as
//! featured or inline image, and each content item referenced from fields), `term:<id>`
//! for every term shown, `user:<id>` for every author shown, `archive:<kind>` for
//! listings, and `option:<key>` for each option read (front page, per-page). Menus must
//! record `menu:<name>` and `post:<target>` for every target they link to. The HTTP
//! layer adds `theme` and `plugins` to every page.
//!
//! Because every item shown is recorded as `post:<id>`, a write that changes how an item
//! appears needs only to emit that item's key; the extra keys above cover pages that
//! show the item indirectly (listings, term pages, parents).

use ferrite_domain::{ContentId, NewEvent, TermId, UserId};

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

/// Dependency key of a menu.
pub fn menu_key(name: &str) -> String {
    format!("menu:{name}")
}

/// Dependency key of a user shown as an author.
pub fn user_key(id: UserId) -> String {
    format!("user:{id}")
}

/// Dependency key of a redirect source path.
pub fn redirect_key(source: &str) -> String {
    format!("redirect:{source}")
}

/// Dependency key recorded by every page: the active theme.
pub const THEME_KEY: &str = "theme";

/// Dependency key recorded by every page: the enabled plugins.
pub const PLUGINS_KEY: &str = "plugins";

/// Flush-everything key: the cache drops every entry.
pub const ALL_KEY: &str = "all";

/// Most items a term write names individually; past it the event carries [`ALL_KEY`].
pub const MAX_TERM_ITEMS: usize = 1000;

/// Dependency key of an option.
pub fn option_key(key: &str) -> String {
    format!("option:{key}")
}

/// Event kind for option changes.
pub const OPTIONS_CHANGED: &str = "options.changed";

/// An `options.changed` event for the given option key (not the dependency key).
pub fn options_changed(key: &str) -> NewEvent {
    NewEvent {
        kind: OPTIONS_CHANGED.to_owned(),
        keys: vec![option_key(key)],
        origin_node: None,
    }
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
        let u = UserId::new();
        assert_eq!(user_key(u), format!("user:{u}"));
        assert_eq!(redirect_key("/a/"), "redirect:/a/");
    }
}
