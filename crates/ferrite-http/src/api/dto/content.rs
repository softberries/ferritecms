//! Bodies of the `/content/{type}` endpoints.
//!
//! Ids are UUID strings and times RFC 3339 UTC (`…Z`). `type` is the content type's
//! name (the item's `kind` in core). Input bodies ignore members they do not know (an
//! admin form may send back a whole [`ContentDto`]); a known member with the wrong type
//! is `400 malformed-json` naming it.

use chrono::{DateTime, Utc};
use ferrite_core::{
    ContentDraft,
    content::{
        AllowedActions, AuthorRef, AutosaveDraft, ContentRef, Robots, Seo, SeoPatch, StatusCounts,
    },
};
use ferrite_domain::{Autosave, ContentId, ContentItem, Revision, TermId, Timestamp};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A content item with its terms and public path (`GET`, and every write's result).
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentDto {
    pub id: Uuid,
    /// The content type (`post`, `page`, …).
    #[serde(rename = "type")]
    pub kind: String,
    /// `draft`, `pending`, `scheduled`, `published`, `private` or `trash`.
    pub status: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub author_id: Uuid,
    pub title: String,
    pub excerpt: String,
    /// The block tree.
    pub body: serde_json::Value,
    /// Custom fields.
    pub fields: serde_json::Value,
    pub menu_order: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// What `ETag` carries and `If-Match` must send back.
    pub version: i64,
    /// Term ids in their stored order.
    pub terms: Vec<Uuid>,
    /// The public path (whatever the status).
    pub permalink: String,
    /// Who wrote it (`author_id` resolved).
    pub author: AuthorRefDto,
    /// What the caller may do with it now.
    pub allowed: AllowedActionsDto,
    /// The SEO title and description (`content_meta` `seo.title`, `seo.description`).
    pub seo: SeoDto,
    /// The item's locale (a tag of `GET /meta`'s `locales`).
    pub locale: String,
    /// The id of the translation group's original (the item's own id when alone).
    pub translation_group: Uuid,
    /// The other members of the item's translation group the caller may read.
    pub translations: Vec<TranslationRefDto>,
}

/// Another item of a translation group.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranslationRefDto {
    pub id: Uuid,
    pub locale: String,
    /// `draft`, `pending`, `scheduled`, `published`, `private` or `trash`.
    pub status: String,
    pub title: String,
    /// The public path (whatever the status).
    pub permalink: String,
}

/// A list row's translation: its locale and status.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranslationStatusDto {
    pub id: Uuid,
    pub locale: String,
    pub status: String,
}

impl ContentDto {
    pub fn new(
        item: ContentItem,
        terms: Vec<TermId>,
        permalink: String,
        author: AuthorRef,
        allowed: AllowedActions,
        seo: Seo,
    ) -> Self {
        Self {
            locale: item.locale.clone(),
            translation_group: item.translation_group.as_uuid(),
            translations: Vec::new(),
            id: item.id.as_uuid(),
            kind: item.kind,
            status: item.status.as_str().to_owned(),
            slug: item.slug,
            parent_id: item.parent_id.map(|p| p.as_uuid()),
            author_id: item.author_id.as_uuid(),
            title: item.title,
            excerpt: item.excerpt,
            body: item.body,
            fields: item.fields,
            menu_order: item.menu_order,
            published_at: item.published_at.map(time),
            scheduled_at: item.scheduled_at.map(time),
            created_at: time(item.created_at),
            updated_at: time(item.updated_at),
            version: item.version,
            terms: terms.into_iter().map(|t| t.as_uuid()).collect(),
            permalink,
            author: author.into(),
            allowed: allowed.into(),
            seo: seo.into(),
        }
    }
}

/// A robots directive of an item's page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RobotsDto {
    NoArchive,
    NoFollow,
    NoImageIndex,
    NoIndex,
    NoSnippet,
}

impl From<Robots> for RobotsDto {
    fn from(r: Robots) -> Self {
        match r {
            Robots::NoArchive => Self::NoArchive,
            Robots::NoFollow => Self::NoFollow,
            Robots::NoImageIndex => Self::NoImageIndex,
            Robots::NoIndex => Self::NoIndex,
            Robots::NoSnippet => Self::NoSnippet,
        }
    }
}

impl From<RobotsDto> for Robots {
    fn from(r: RobotsDto) -> Self {
        match r {
            RobotsDto::NoArchive => Self::NoArchive,
            RobotsDto::NoFollow => Self::NoFollow,
            RobotsDto::NoImageIndex => Self::NoImageIndex,
            RobotsDto::NoIndex => Self::NoIndex,
            RobotsDto::NoSnippet => Self::NoSnippet,
        }
    }
}

/// An item's SEO meta. `title`, when set, is the whole `<title>` and `og:title` of the
/// item's page (no site suffix); `description` comes before the excerpt as the meta
/// description; `canonical` (an absolute `http(s)` URL) replaces the page's own URL in
/// the canonical link and `og:url`; `robots` (sorted, deduplicated; empty: none) is the
/// page's `<meta name="robots">`. `null`: not set.
#[derive(Debug, Serialize, ToSchema)]
pub struct SeoDto {
    #[schema(required = true)]
    pub title: Option<String>,
    #[schema(required = true)]
    pub description: Option<String>,
    #[schema(required = true)]
    pub canonical: Option<String>,
    pub robots: Vec<RobotsDto>,
}

impl From<Seo> for SeoDto {
    fn from(s: Seo) -> Self {
        Self {
            title: s.title,
            description: s.description,
            canonical: s.canonical,
            robots: s.robots.into_iter().map(Into::into).collect(),
        }
    }
}

/// A change to an item's SEO meta. Per member: absent keeps it, `null` or a string
/// that is blank after trimming deletes it, any other string sets it (trimmed). No NUL,
/// and no control characters once trimmed (an edge newline is trimmed, an inner one
/// refused); `title` at most 200 characters and `description` at most 500 (`422`,
/// `errors[0].field` = `seo.title` / `seo.description`). `canonical`, trimmed, must be
/// an absolute `http(s)` URL of at most 2048 characters (`seo.canonical`); `robots`
/// replaces the directives (`null` or `[]` deletes them; an unknown one is `400`, a malformed body).
#[derive(Debug, Deserialize, ToSchema)]
pub struct SeoPatchDto {
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<String>, required = false)]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<String>, required = false)]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<String>, required = false)]
    pub canonical: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Vec<RobotsDto>>, required = false)]
    pub robots: Option<Option<Vec<RobotsDto>>>,
}

impl From<SeoPatchDto> for SeoPatch {
    fn from(p: SeoPatchDto) -> Self {
        Self {
            title: p.title,
            description: p.description,
            canonical: p.canonical,
            robots: p
                .robots
                .map(|r| r.map(|v| v.into_iter().map(Into::into).collect())),
        }
    }
}

/// A list entry: the item's columns without `body`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentSummaryDto {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub slug: String,
    pub parent_id: Option<Uuid>,
    pub author_id: Uuid,
    pub title: String,
    pub excerpt: String,
    pub fields: serde_json::Value,
    pub menu_order: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
    /// Who wrote it (`author_id` resolved).
    pub author: AuthorRefDto,
    /// What the caller may do with it now.
    pub allowed: AllowedActionsDto,
    /// The item's locale.
    pub locale: String,
    /// The other members of its translation group the caller may read.
    pub translations: Vec<TranslationStatusDto>,
}

impl ContentSummaryDto {
    pub fn new(item: ContentItem, author: AuthorRef, allowed: AllowedActions) -> Self {
        Self {
            locale: item.locale.clone(),
            translations: Vec::new(),
            id: item.id.as_uuid(),
            kind: item.kind,
            status: item.status.as_str().to_owned(),
            slug: item.slug,
            parent_id: item.parent_id.map(|p| p.as_uuid()),
            author_id: item.author_id.as_uuid(),
            title: item.title,
            excerpt: item.excerpt,
            fields: item.fields,
            menu_order: item.menu_order,
            published_at: item.published_at.map(time),
            scheduled_at: item.scheduled_at.map(time),
            created_at: time(item.created_at),
            updated_at: time(item.updated_at),
            version: item.version,
            author: author.into(),
            allowed: allowed.into(),
        }
    }
}

/// A user as an item's or a revision's author: id, login and display name (public on
/// author archives).
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorRefDto {
    pub id: Uuid,
    pub login: String,
    pub display_name: String,
}

impl From<AuthorRef> for AuthorRefDto {
    fn from(a: AuthorRef) -> Self {
        Self {
            id: a.id.as_uuid(),
            login: a.login,
            display_name: a.display_name,
        }
    }
}

/// What the caller may do with an item now, as core decides it (role, token scopes and
/// status): the admin shows only these actions. `true` means the request passes its
/// permission and transition checks; it can still fail on a stale version, a schedule
/// time in the past, or (`delete`) children.
#[derive(Debug, Serialize, ToSchema)]
pub struct AllowedActionsDto {
    /// `PATCH` (and revision restore).
    pub edit: bool,
    pub publish: bool,
    pub unpublish: bool,
    pub schedule: bool,
    pub submit: bool,
    /// `DELETE` without `force`.
    pub trash: bool,
    pub restore: bool,
    /// `DELETE ?force=true`.
    pub delete: bool,
    /// `DELETE` is allowed and `restore` would be once the item is trashed (the admin's
    /// Undo after a trash).
    pub trash_undoable: bool,
}

impl From<AllowedActions> for AllowedActionsDto {
    fn from(a: AllowedActions) -> Self {
        Self {
            edit: a.edit,
            publish: a.publish,
            unpublish: a.unpublish,
            schedule: a.schedule,
            submit: a.submit,
            trash: a.trash,
            restore: a.restore,
            delete: a.delete,
            trash_undoable: a.trash_undoable,
        }
    }
}

/// `GET /content/{type}/counts`: items per status that the caller may read (the same
/// rule as the list: without `edit_others`, others' items only when publicly visible).
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentCountsDto {
    pub draft: u64,
    pub pending: u64,
    pub scheduled: u64,
    pub published: u64,
    pub private: u64,
    pub trash: u64,
}

impl From<StatusCounts> for ContentCountsDto {
    fn from(c: StatusCounts) -> Self {
        Self {
            draft: c.draft,
            pending: c.pending,
            scheduled: c.scheduled,
            published: c.published,
            private: c.private,
            trash: c.trash,
        }
    }
}

/// An item named by id (`GET /content/refs`).
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentRefDto {
    pub id: Uuid,
    /// The content type (`post`, `page`, `media`, …).
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    /// `draft`, `pending`, `scheduled`, `published`, `private` or `trash`.
    pub status: String,
    /// The public path; `null` unless the item has a public page now (published, not
    /// future-dated, not media).
    #[schema(required = true)]
    pub permalink: Option<String>,
}

impl From<ContentRef> for ContentRefDto {
    fn from(r: ContentRef) -> Self {
        Self {
            id: r.id.as_uuid(),
            kind: r.kind,
            title: r.title,
            status: r.status.as_str().to_owned(),
            permalink: r.permalink,
        }
    }
}

/// `GET /content/refs`: the requested items the caller may read, in no particular
/// order; unknown and unreadable ids are absent.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentRefList {
    pub items: Vec<ContentRefDto>,
}

/// `GET /content/{type}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentList {
    pub items: Vec<ContentSummaryDto>,
    /// Pass as `cursor` for the next page; `null` on the last page.
    pub next_cursor: Option<String>,
}

/// `POST /content/{type}`: a new draft authored by the caller.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContentInput {
    pub title: String,
    /// Generated from the title when absent; a taken slug gets `-2`, `-3`, ….
    #[serde(default)]
    pub slug: Option<String>,
    /// Only for hierarchical types (pages).
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// Derived from the body (160 characters) when absent.
    #[serde(default)]
    pub excerpt: Option<String>,
    /// The block tree; an empty document when absent.
    #[serde(default = "empty_doc", deserialize_with = "not_null")]
    #[schema(value_type = Object)]
    pub body: serde_json::Value,
    #[serde(default = "empty_object", deserialize_with = "not_null")]
    #[schema(value_type = Object)]
    pub fields: serde_json::Value,
    #[serde(default)]
    pub menu_order: i32,
    /// Term ids, in order; duplicates are ignored.
    #[serde(default)]
    pub terms: Vec<Uuid>,
    /// SEO title and description; absent or `null`: none.
    #[serde(default)]
    pub seo: Option<SeoPatchDto>,
    /// One of the site's locales (`GET /meta`); absent: the default.
    #[serde(default)]
    pub locale: Option<String>,
    /// An item of the same type whose translation group the new item joins (its locale
    /// must be free there, else `409`).
    #[serde(default)]
    pub translation_of: Option<Uuid>,
}

impl ContentInput {
    pub fn into_draft(self, kind: &str) -> ContentDraft {
        ContentDraft {
            kind: kind.to_owned(),
            title: self.title,
            slug: self.slug,
            parent_id: self.parent_id.map(ContentId::from_uuid),
            excerpt: self.excerpt,
            body: self.body,
            fields: self.fields,
            menu_order: self.menu_order,
            terms: term_ids(&self.terms),
            seo: self.seo.map(Into::into),
        }
    }
}

/// `PATCH /content/{type}/{id}`: every member is optional and an absent one keeps the
/// stored value. `null` is accepted only by `parent_id` (move to the top level) and
/// `excerpt` (derive it from the body again); elsewhere, `slug` included, it is
/// `400 malformed-json`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContentPatch {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub title: Option<String>,
    /// A taken slug gets `-2`, `-3`, ….
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub slug: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Uuid>, required = false)]
    pub parent_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<String>, required = false)]
    pub excerpt: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Object, required = false)]
    pub body: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Object, required = false)]
    pub fields: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = i32, required = false)]
    pub menu_order: Option<i32>,
    /// Replaces the item's terms; `[]` removes them all.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Vec<Uuid>, required = false)]
    pub terms: Option<Vec<Uuid>>,
    /// Changes the SEO meta member by member; a change to it alone is a full save
    /// (a new version and revision).
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = SeoPatchDto, required = false)]
    pub seo: Option<SeoPatchDto>,
    /// Moves the item to another of the site's locales (a published item's old path
    /// redirects; not for an item with children).
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub locale: Option<String>,
    /// Joins the translation group of this item; `null` leaves the group.
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Uuid>, required = false)]
    pub translation_of: Option<Option<Uuid>>,
}

/// `POST /content/{type}/{id}/translations`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TranslateInput {
    /// The target locale (one of the site's, not the item's).
    pub locale: String,
}

/// The new translation (`201`) and what it could not take over.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranslationCreated {
    pub item: ContentDto,
    /// `parent_untranslated`, `term_untranslated:<term id>`.
    pub notes: Vec<String>,
}

impl ContentPatch {
    /// `current` (with its `terms`) changed by this patch.
    pub fn apply(self, current: &ContentItem, terms: Vec<TermId>) -> ContentDraft {
        ContentDraft {
            kind: current.kind.clone(),
            title: self.title.unwrap_or_else(|| current.title.clone()),
            slug: self.slug,
            parent_id: match self.parent_id {
                None => current.parent_id,
                Some(p) => p.map(ContentId::from_uuid),
            },
            excerpt: match self.excerpt {
                None => Some(current.excerpt.clone()),
                Some(e) => e,
            },
            body: self.body.unwrap_or_else(|| current.body.clone()),
            fields: self.fields.unwrap_or_else(|| current.fields.clone()),
            menu_order: self.menu_order.unwrap_or(current.menu_order),
            terms: self.terms.map_or(terms, |t| term_ids(&t)),
            seo: self.seo.map(Into::into),
        }
    }
}

/// `POST /content/{type}/{id}/schedule`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ScheduleRequest {
    /// When to publish; must lie in the future.
    pub at: DateTime<Utc>,
}

/// A revision in a list: who saved it and when, without the content.
#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionSummaryDto {
    pub id: Uuid,
    pub author_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub title: String,
    /// `restored from <revision id>` for a restore; else `null`.
    pub note: Option<String>,
    /// Who saved it (`author_id` resolved).
    pub author: AuthorRefDto,
}

impl RevisionSummaryDto {
    pub fn new(r: Revision, author: AuthorRef) -> Self {
        Self {
            id: r.id.as_uuid(),
            author_id: r.author_id.as_uuid(),
            created_at: time(r.created_at),
            title: r.title,
            note: r.note,
            author: author.into(),
        }
    }
}

/// `GET /content/{type}/{id}/revisions`: newest first, at most 50 (all that are kept).
#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionList {
    pub items: Vec<RevisionSummaryDto>,
    /// Always `null`: one page holds every kept revision.
    pub next_cursor: Option<String>,
}

/// One revision: the saved title, excerpt, body and fields.
#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionDto {
    pub id: Uuid,
    pub author_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub title: String,
    pub excerpt: String,
    pub body: serde_json::Value,
    pub fields: serde_json::Value,
    pub note: Option<String>,
    /// Who saved it (`author_id` resolved).
    pub author: AuthorRefDto,
}

impl RevisionDto {
    pub fn new(r: Revision, author: AuthorRef) -> Self {
        Self {
            id: r.id.as_uuid(),
            author_id: r.author_id.as_uuid(),
            created_at: time(r.created_at),
            title: r.title,
            excerpt: r.excerpt,
            body: r.body,
            fields: r.fields,
            note: r.note,
            author: author.into(),
        }
    }
}

/// `GET`/`PUT /content/{type}/{id}/autosave`: the caller's own unsaved working copy of
/// the item (one per item and user). Nothing in it has been checked against the type's
/// fields, the parent or the terms.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutosaveDto {
    /// The item version the copy was made from.
    pub base_version: i64,
    pub title: String,
    /// `null`: derive from the body.
    pub excerpt: Option<String>,
    /// `null`: unchanged.
    pub slug: Option<String>,
    /// The working copy's parent; `null`: top level (not "unchanged").
    pub parent_id: Option<Uuid>,
    /// `null`: unchanged.
    pub menu_order: Option<i32>,
    /// Term ids in order; `null`: unchanged.
    pub terms: Option<Vec<Uuid>>,
    /// The block tree.
    pub body: serde_json::Value,
    pub fields: serde_json::Value,
    /// The SEO patch as sent; `null`: unchanged.
    #[schema(value_type = Option<SeoPatchDto>)]
    pub seo: Option<serde_json::Value>,
    pub saved_at: DateTime<Utc>,
}

impl From<Autosave> for AutosaveDto {
    fn from(a: Autosave) -> Self {
        Self {
            base_version: a.base_version,
            title: a.title,
            excerpt: a.excerpt,
            slug: a.slug,
            parent_id: a.parent_id.map(|p| p.as_uuid()),
            menu_order: a.menu_order,
            terms: a
                .terms
                .map(|t| t.into_iter().map(|id| id.as_uuid()).collect()),
            body: a.body,
            fields: a.fields,
            seo: a.seo,
            saved_at: time(a.saved_at),
        }
    }
}

/// `PUT /content/{type}/{id}/autosave`: replaces the caller's autosave (last write
/// wins; no `If-Match`). Text is checked as a content write checks it (no NUL; the slug
/// also no control characters; `seo` as a content write checks it, then stored as
/// sent), `body` must be a document (`type` `doc`) and `terms` holds at most 200 ids;
/// the field schema, the parent and the terms are not checked: a draft may be
/// incomplete.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AutosaveInput {
    /// The item version the working copy was made from.
    pub base_version: i64,
    pub title: String,
    /// Absent or `null`: derive from the body.
    #[serde(default)]
    pub excerpt: Option<String>,
    /// Absent or `null`: unchanged.
    #[serde(default)]
    pub slug: Option<String>,
    /// The working copy's parent; absent or `null`: top level (not "unchanged"), so
    /// always send it.
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// Absent or `null`: unchanged.
    #[serde(default)]
    pub menu_order: Option<i32>,
    /// Term ids in order (at most 200); absent or `null`: unchanged.
    #[serde(default)]
    pub terms: Option<Vec<Uuid>>,
    #[serde(deserialize_with = "not_null")]
    #[schema(value_type = Object)]
    pub body: serde_json::Value,
    #[serde(deserialize_with = "not_null")]
    #[schema(value_type = Object)]
    pub fields: serde_json::Value,
    /// The working copy's SEO patch; absent or `null`: unchanged.
    #[serde(default)]
    pub seo: Option<SeoPatchDto>,
}

impl From<AutosaveInput> for AutosaveDraft {
    fn from(i: AutosaveInput) -> Self {
        Self {
            base_version: i.base_version,
            title: i.title,
            excerpt: i.excerpt,
            slug: i.slug,
            parent_id: i.parent_id.map(ContentId::from_uuid),
            menu_order: i.menu_order,
            terms: i.terms.as_deref().map(term_ids),
            body: i.body,
            fields: i.fields,
            seo: i.seo.map(Into::into),
        }
    }
}

fn time(t: Timestamp) -> DateTime<Utc> {
    t.as_datetime()
}

fn term_ids(ids: &[Uuid]) -> Vec<TermId> {
    ids.iter().copied().map(TermId::from_uuid).collect()
}

fn empty_doc() -> serde_json::Value {
    serde_json::json!({"type": "doc", "content": []})
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

/// A member that may be absent (`#[serde(default)]` gives `None`) but not `null` (also
/// for `serde_json::Value`, which would take `null` as a value).
pub(crate) fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    match Option::<T>::deserialize(d)? {
        Some(v) => Ok(Some(v)),
        None => Err(serde::de::Error::custom(
            "null is not accepted here; leave the member out to keep the stored value",
        )),
    }
}

/// A JSON value other than `null`.
fn not_null<'de, D>(d: D) -> Result<serde_json::Value, D::Error>
where
    D: Deserializer<'de>,
{
    present(d)?.ok_or_else(|| serde::de::Error::custom("null is not accepted here"))
}

/// A member that may be absent (`None`) or `null` (`Some(None)`).
pub(crate) fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(v: serde_json::Value) -> Result<ContentPatch, serde_json::Error> {
        serde_json::from_value(v)
    }

    #[test]
    fn patch_absent_null_and_values() {
        let p = patch(json!({})).unwrap();
        assert!(p.title.is_none() && p.slug.is_none());
        assert_eq!(p.parent_id, None);
        assert_eq!(p.excerpt, None);
        let p = patch(json!({"parent_id": null, "excerpt": null})).unwrap();
        assert_eq!(p.parent_id, Some(None));
        assert_eq!(p.excerpt, Some(None));
        let id = Uuid::now_v7();
        let p = patch(json!({"parent_id": id, "excerpt": "e", "slug": "s"})).unwrap();
        assert_eq!(p.parent_id, Some(Some(id)));
        assert_eq!(p.excerpt, Some(Some("e".into())));
        assert_eq!(p.slug.as_deref(), Some("s"));
        for field in ["slug", "title", "body", "fields", "menu_order", "terms"] {
            assert!(patch(json!({ field: null })).is_err(), "{field}");
        }
        let input = |v| serde_json::from_value::<ContentInput>(v);
        assert!(input(json!({"title": "t", "body": null})).is_err());
        assert!(input(json!({"title": "t", "fields": null})).is_err());
        let i = input(json!({"title": "t"})).unwrap();
        assert_eq!(i.body, json!({"type": "doc", "content": []}));
        assert_eq!(i.fields, json!({}));
        // Unknown members (a whole DTO sent back) are ignored.
        assert!(patch(json!({"id": id, "version": 3, "status": "draft"})).is_ok());
    }
}
