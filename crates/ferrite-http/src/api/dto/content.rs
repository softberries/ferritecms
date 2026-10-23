//! Bodies of the `/content/{type}` endpoints.
//!
//! Ids are UUID strings and times RFC 3339 UTC (`…Z`). `type` is the content type's
//! name (the item's `kind` in core). Input bodies ignore members they do not know (an
//! admin form may send back a whole [`ContentDto`]); a known member with the wrong type
//! is `400 malformed-json` naming it.

use chrono::{DateTime, Utc};
use ferrite_core::ContentDraft;
use ferrite_domain::{ContentId, ContentItem, Revision, TermId, Timestamp};
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
}

impl ContentDto {
    pub fn new(item: ContentItem, terms: Vec<TermId>, permalink: String) -> Self {
        Self {
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
}

impl From<ContentItem> for ContentSummaryDto {
    fn from(item: ContentItem) -> Self {
        Self {
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
        }
    }
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
}

impl From<Revision> for RevisionSummaryDto {
    fn from(r: Revision) -> Self {
        Self {
            id: r.id.as_uuid(),
            author_id: r.author_id.as_uuid(),
            created_at: time(r.created_at),
            title: r.title,
            note: r.note,
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
}

impl From<Revision> for RevisionDto {
    fn from(r: Revision) -> Self {
        Self {
            id: r.id.as_uuid(),
            author_id: r.author_id.as_uuid(),
            created_at: time(r.created_at),
            title: r.title,
            excerpt: r.excerpt,
            body: r.body,
            fields: r.fields,
            note: r.note,
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
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
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
fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
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
