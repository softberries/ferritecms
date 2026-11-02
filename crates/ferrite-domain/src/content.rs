use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ContentId, RevisionId, TermId, Timestamp, UserId, string_enum};

string_enum!(
    /// Lifecycle state of a content item.
    ContentStatus {
        Draft => "draft",
        Pending => "pending",
        Scheduled => "scheduled",
        Published => "published",
        Private => "private",
        Trash => "trash",
    }
);

/// The content type of media items (built in, like `post` and `page`).
pub const MEDIA_KIND: &str = "media";

string_enum!(
    /// The kind of a media item, from its `media.mime`: `image/…`, `video/…`, `audio/…`,
    /// and `document` for every other type ([`ContentQuery::media_kind`]).
    MediaKind {
        Image => "image",
        Video => "video",
        Audio => "audio",
        Document => "document",
    }
);

impl MediaKind {
    /// Whether a media item of type `mime` is of this kind (the stores' filter, in Rust).
    pub fn matches(self, mime: &str) -> bool {
        let top = |p: &str| mime.starts_with(p);
        match self {
            MediaKind::Image => top("image/"),
            MediaKind::Video => top("video/"),
            MediaKind::Audio => top("audio/"),
            MediaKind::Document => !(top("image/") || top("video/") || top("audio/")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentType {
    pub name: String,
    pub label: String,
    pub hierarchical: bool,
    pub has_archive: bool,
    /// JSON schema of the type's custom fields.
    pub fields: Value,
    pub built_in: bool,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentItem {
    pub id: ContentId,
    /// Content type name (column `type`).
    pub kind: String,
    pub status: ContentStatus,
    pub slug: String,
    pub parent_id: Option<ContentId>,
    pub author_id: UserId,
    pub title: String,
    pub excerpt: String,
    /// Block tree.
    pub body: Value,
    /// Cached rendered HTML.
    pub body_html: Option<String>,
    /// Custom fields.
    pub fields: Value,
    pub menu_order: i32,
    pub published_at: Option<Timestamp>,
    pub scheduled_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Optimistic-lock counter.
    pub version: i64,
    /// The item's locale (a BCP 47 tag of the site's list; 13 §2.3).
    pub locale: String,
    /// The id of the translation group's original; the item's own id when it is alone.
    pub translation_group: ContentId,
}

/// Insert input: a [`ContentItem`] without storage-managed columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewContent {
    pub id: ContentId,
    pub kind: String,
    pub status: ContentStatus,
    pub slug: String,
    pub parent_id: Option<ContentId>,
    pub author_id: UserId,
    pub title: String,
    pub excerpt: String,
    pub body: Value,
    pub body_html: Option<String>,
    pub fields: Value,
    pub menu_order: i32,
    pub published_at: Option<Timestamp>,
    pub scheduled_at: Option<Timestamp>,
    /// `None` = now. The WordPress import sets both (M3-1 P7).
    pub created_at: Option<Timestamp>,
    /// `None` = now.
    pub updated_at: Option<Timestamp>,
    /// The item's locale.
    pub locale: String,
    /// The translation group to join; `None` = the item's own id (a new group).
    pub translation_group: Option<ContentId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    pub id: RevisionId,
    pub item_id: ContentId,
    pub author_id: UserId,
    pub created_at: Timestamp,
    pub title: String,
    pub excerpt: String,
    pub body: Value,
    pub fields: Value,
    /// Snapshot of the item's meta at save time.
    pub meta: Value,
    pub note: Option<String>,
}

/// A user's unsaved working copy of an item (the autosave slot, M2b P15): at most one per
/// (item, user), never a revision. `base_version` is the item version the copy was made
/// from. `excerpt` `None` means "derive from the body"; `parent_id` is the working
/// copy's parent, `None` = top level (ruling R4); `slug`, `menu_order`, `terms` and
/// `seo` `None` mean "unchanged" (F11). `seo` is the SEO patch as the editor sent it (a
/// JSON object with optional `title` and `description`, each a string or `null`; M2b-2
/// Task 1). Nothing in it is checked
/// against the type's field schema, the parent or the terms until it is really saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Autosave {
    pub item_id: ContentId,
    pub user_id: UserId,
    pub base_version: i64,
    pub title: String,
    pub excerpt: Option<String>,
    pub slug: Option<String>,
    pub parent_id: Option<ContentId>,
    pub menu_order: Option<i32>,
    pub terms: Option<Vec<TermId>>,
    pub body: Value,
    pub fields: Value,
    pub seo: Option<Value>,
    pub saved_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParentFilter {
    #[default]
    Any,
    Root,
    Of(ContentId),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentOrder {
    #[default]
    PublishedDesc,
    UpdatedDesc,
    MenuOrderAsc,
    TitleAsc,
}

/// The reader of a [`ContentQuery::readable_by`] / `SearchQuery::readable_by` filter:
/// who reads, and the instant "publicly visible" is judged at (core's clock, so the SQL
/// filter agrees with core's `is_public` re-check).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadableBy {
    pub user: UserId,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentQuery {
    pub kind: Option<String>,
    /// `type IN (kinds)` when non-empty; empty means no filter. At most 20 (`Invalid`
    /// beyond), and `Invalid` together with [`ContentQuery::kind`].
    pub kinds: Vec<String>,
    /// Empty means any status.
    pub statuses: Vec<ContentStatus>,
    pub parent: ParentFilter,
    pub term: Option<TermId>,
    pub author: Option<UserId>,
    /// Only items [`ReadableBy::user`] may read in the admin without `edit_others`:
    /// items publicly visible at [`ReadableBy::now`] (`status = published` and
    /// `published_at <= now`; a NULL `published_at` never matches), or items authored by
    /// this user (any status). `None`: no such filter.
    pub readable_by: Option<ReadableBy>,
    /// Only these items. **Empty means no filter, not "match nothing"**: a caller that
    /// computed an empty id set must short-circuit instead of querying. At most 200
    /// (`Invalid` beyond).
    pub ids: Vec<ContentId>,
    /// Never these items; empty means no filter. At most 200 (`Invalid` beyond).
    pub exclude: Vec<ContentId>,
    /// `published_at >= t` (inclusive); items without a publish time never match.
    pub published_after: Option<Timestamp>,
    /// `published_at < t` (exclusive); items without a publish time never match.
    pub published_before: Option<Timestamp>,
    /// Only media items of this kind (a sub-select on `media`, so items without a
    /// `media` row never match); `Invalid` together with a [`ContentQuery::kind`] or a
    /// [`ContentQuery::kinds`] entry other than `media`.
    pub media_kind: Option<MediaKind>,
    /// Only items in this locale; `None`: any. Media items are never filtered by it
    /// (they are shared across locales, 13 §2.3).
    pub locale: Option<String>,
    pub order: ContentOrder,
    /// Capped at [`crate::MAX_PAGE`] by the stores.
    pub limit: u32,
    pub cursor: Option<String>,
}

impl Default for ContentQuery {
    fn default() -> Self {
        Self {
            kind: None,
            kinds: Vec::new(),
            statuses: Vec::new(),
            parent: ParentFilter::Any,
            term: None,
            author: None,
            readable_by: None,
            ids: Vec::new(),
            exclude: Vec::new(),
            published_after: None,
            published_before: None,
            media_kind: None,
            locale: None,
            order: ContentOrder::default(),
            limit: 20,
            cursor: None,
        }
    }
}
