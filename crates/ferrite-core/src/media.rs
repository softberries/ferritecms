//! Media items (03 §4, M2a-2b P3/P5/P8): recording uploads and their variants, admin
//! reads and edits.
//!
//! A media item is a content item of kind `media` plus its `media` row. The bytes are
//! staged and their type decided before core is called (`ferrite-http`'s uploader,
//! outside any `Tx`, P3/P4); core records what was staged and never trusts the client's
//! file name: [`NewUpload::original_name`] keeps only its last path segment, is checked
//! for length and control characters, and is stored as text (never used as a path).
//!
//! **Rights.** Uploads need [`Capability::UploadMedia`] (role, then `media:write`).
//! Edits, trash and delete apply the content rules of the item's role checks
//! ([`can_edit`](crate::can_edit) and the lifecycle's ownership rules) with the
//! `media:write` scope in place of `content:write`; reads need `media:read`
//! ([`require_read`]`(Media)`) and [`can_read`] per item.
//!
//! **Events (M1c-2 R11).** Every write of a `media` row emits `post:<media id>` in the
//! same `Tx`: an upload through the create/publish events of the item, an edit through
//! the item's versioned update, [`record_variants`](MediaService::record_variants) with
//! its own `invalidate` event.

use std::{collections::HashMap, sync::Arc};

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentItem, ContentOrder, ContentQuery,
    Media, Page, ReadableBy, Timestamp, UserId,
};
use ferrite_store::Store;
use serde_json::Value;

use crate::{
    Actor, ContentDraft, ContentService, CoreError, Permalinks,
    actor::{Area, Capability, Scope, Scopes, can, can_read, require, require_read, require_scope},
    content::{Edit, WALKED},
    events,
    jobs::{MEDIA_VARIANTS_VERSION, MediaVariants, media_variants_job},
    site::MEDIA_KIND,
    tx::finish,
};

/// Longest media title, in characters.
pub const MAX_MEDIA_TITLE: usize = 200;
/// Longest alternative text, in characters.
pub const MAX_MEDIA_ALT: usize = 1_000;
/// Longest stored file name, in bytes (after dropping any path).
pub const MAX_ORIGINAL_NAME: usize = 255;
/// The only type whose bytes can carry the sanitiser's marker.
const SVG_MIME: &str = "image/svg+xml";

/// A staged upload to record ([`MediaService::record_upload`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NewUpload {
    /// The stored original (kind `media`); its row is inserted unless the hash exists.
    pub artifact: Artifact,
    /// The type decided from the content (never the client's claim).
    pub mime: String,
    pub bytes: u64,
    /// The client's file name; only its last `/` or `\` segment is kept.
    pub original_name: String,
    /// `None` or blank: the file name without its extension.
    pub title: Option<String>,
    pub alt: String,
    /// The bytes are the SVG sanitiser's output (only for `image/svg+xml`).
    pub sanitized: bool,
    /// A raster image: a `media.variants` job is enqueued.
    pub raster: bool,
}

/// A media item: the content item and its `media` row.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaItem {
    pub item: ContentItem,
    pub media: Media,
}

/// One page of media items ([`MediaService::list`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaQuery {
    /// 1 through [`ferrite_domain::MAX_PAGE`].
    pub limit: u32,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
    pub author: Option<UserId>,
}

/// The editable fields of a media item; `None` keeps the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPatch {
    pub title: Option<String>,
    pub alt: Option<String>,
}

/// Records uploads and variants and serves the admin's media reads and edits.
pub struct MediaService {
    store: Arc<dyn Store>,
    content: Arc<ContentService>,
    permalinks: Arc<dyn Permalinks>,
}

impl MediaService {
    pub fn new(
        store: Arc<dyn Store>,
        content: Arc<ContentService>,
        permalinks: Arc<dyn Permalinks>,
    ) -> Self {
        Self {
            store,
            content,
            permalinks,
        }
    }

    /// `Ok` if `actor` may upload (`upload_media`, then `media:write`). Handlers call it
    /// before reading an upload body; [`record_upload`](Self::record_upload) checks again.
    pub fn authorize_upload(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::UploadMedia)
    }

    /// Records a staged upload as a **published** media item authored by `actor` (P5),
    /// in one `Tx`: the artifact row (unless its hash exists), the item (kind `media`,
    /// title = `upload.title` or the file name without its extension, empty body; its
    /// revision, search entry, `invalidate` event with `post:<id>` and `post.saved`
    /// action jobs), the publish (redirect cleanup, event, `post.published` jobs), the
    /// `media` row (no dimensions, no variants) and, for a raster image, one
    /// [`media.variants`](crate::jobs::MEDIA_VARIANTS_KIND) job.
    ///
    /// Checked first, before anything is written: `upload_media` and `media:write`;
    /// `title` at most [`MAX_MEDIA_TITLE`] characters, `alt` at most [`MAX_MEDIA_ALT`],
    /// neither with control characters (`InvalidField` `title` / `alt`); the file name's
    /// last segment 1..=[`MAX_ORIGINAL_NAME`] bytes, not `.`/`..`, without control
    /// characters (`InvalidField` `file`); the artifact of kind `media`, `sanitized` only
    /// for `image/svg+xml` (`Invalid`). A failure inside the `Tx` leaves no row, event or
    /// job; the blob stays for M4's GC (M1b R2).
    pub async fn record_upload(
        &self,
        actor: &Actor,
        upload: NewUpload,
    ) -> Result<MediaItem, CoreError> {
        self.authorize_upload(actor)?;
        let original_name = check_file_name(&upload.original_name)?;
        let title = match upload.title.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => check_title(t)?,
            _ => title_from(&original_name),
        };
        let alt = check_alt(&upload.alt)?;
        if upload.artifact.kind != ArtifactKind::Media {
            return Err(CoreError::Invalid(
                "an upload's artifact must be of kind media".into(),
            ));
        }
        if upload.sanitized && upload.mime != SVG_MIME {
            return Err(CoreError::Invalid(
                "only an SVG can be marked sanitised".into(),
            ));
        }
        let bytes = i64::try_from(upload.bytes)
            .map_err(|_| CoreError::Invalid("upload size out of range".into()))?;
        let writer = content_writer(actor);
        let draft = ContentDraft {
            kind: MEDIA_KIND.into(),
            title,
            slug: None,
            parent_id: None,
            excerpt: Some(String::new()),
            body: serde_json::json!({ "type": "doc", "content": [] }),
            fields: serde_json::json!({}),
            menu_order: 0,
            terms: Vec::new(),
        };
        // One set of rules for the whole write (R10, F10).
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = async {
            tx.artifacts().insert_if_absent(&upload.artifact).await?;
            let item = self
                .content
                .create_in(tx.as_ref(), rules.as_ref(), &writer, draft)
                .await?;
            // Uploads are published at once (P5); the lifecycle's keys include
            // `post:<id>`, which covers the media row written next (R11).
            let item =
                ContentService::publish_in(tx.as_ref(), rules.as_ref(), &writer, item).await?;
            let media = Media {
                item_id: item.id,
                artifact_hash: upload.artifact.hash.clone(),
                mime: upload.mime,
                bytes,
                width: None,
                height: None,
                alt,
                original_name,
                variants: Value::Object(serde_json::Map::new()),
                sanitized: upload.sanitized,
            };
            tx.media().insert(&media).await?;
            if upload.raster {
                tx.jobs()
                    .enqueue(&media_variants_job(&MediaVariants {
                        v: MEDIA_VARIANTS_VERSION,
                        item: item.id,
                        artifact: upload.artifact.hash.to_string(),
                    }))
                    .await?;
            }
            Ok(MediaItem { item, media })
        }
        .await;
        finish(tx, done).await
    }

    /// Media item `id` in any status, if `actor` may read it (`media:read`, then
    /// [`can_read`]). Missing, not readable, not of kind `media` or without a `media`
    /// row is `NotFound`.
    pub async fn get(&self, actor: &Actor, id: ContentId) -> Result<MediaItem, CoreError> {
        require_read(actor, Area::Media)?;
        let repos = self.store.read();
        let now = Timestamp::now();
        let item = repos
            .content()
            .get(id)
            .await?
            .filter(|i| i.kind == MEDIA_KIND && can_read(actor, i, now))
            .ok_or(CoreError::NotFound)?;
        let media = repos.media().get(id).await?.ok_or(CoreError::NotFound)?;
        Ok(MediaItem { item, media })
    }

    /// One page of media items in every status but trash, newest update first
    /// (`media:read`). Without `edit_others` in its role, `actor` sees only items
    /// publicly visible now and its own, as [`ContentService::list`] does. Items
    /// without a `media` row are left out (the page can then be shorter than `limit`).
    pub async fn list(
        &self,
        actor: &Actor,
        query: MediaQuery,
    ) -> Result<Page<MediaItem>, CoreError> {
        require_read(actor, Area::Media)?;
        let now = Timestamp::now();
        let readable_by = (!can(actor.role, Capability::EditOthers)).then_some(ReadableBy {
            user: actor.user,
            now,
        });
        let repos = self.store.read();
        let page = repos
            .content()
            .list(&ContentQuery {
                kind: Some(MEDIA_KIND.into()),
                statuses: WALKED.to_vec(),
                author: query.author,
                readable_by,
                order: ContentOrder::UpdatedDesc,
                limit: query.limit,
                cursor: query.cursor,
                ..ContentQuery::default()
            })
            .await?;
        let ids: Vec<ContentId> = page.items.iter().map(|i| i.id).collect();
        // An empty id set short-circuits (M1c-1 R3); a page holds at most `MAX_PAGE`
        // (= `MAX_ID_FILTER`) ids.
        let mut rows: HashMap<ContentId, Media> = if ids.is_empty() {
            HashMap::new()
        } else {
            repos
                .media()
                .get_many(&ids)
                .await?
                .into_iter()
                .map(|m| (m.item_id, m))
                .collect()
        };
        let items = page
            .items
            .into_iter()
            .filter_map(|item| {
                let media = rows.remove(&item.id)?;
                Some(MediaItem { item, media })
            })
            .collect();
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
        })
    }

    /// Changes the title and/or alt text of media item `id`, which must still be at
    /// `expected_version` (else `Conflict`), in one `Tx`: the item through the content
    /// update (title only; status, slug and body stay; a revision and the `invalidate`
    /// event with `post:<id>` as for any content update) and the `media` row's alt
    /// text, read with a row lock. Needs `media:write` and the role rules of
    /// [`can_edit`](crate::can_edit). A trashed item is `Invalid("restore first")`; a
    /// missing or non-media item `NotFound`. Field limits as on upload.
    pub async fn update(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
        patch: MediaPatch,
    ) -> Result<MediaItem, CoreError> {
        require_scope(actor, Some(Scope::write(Area::Media)))?;
        let title = patch
            .title
            .as_deref()
            .map(|t| check_title(t.trim()))
            .transpose()?;
        let alt = patch.alt.as_deref().map(check_alt).transpose()?;
        let writer = content_writer(actor);
        // One set of rules for the whole write (R10).
        let rules = self.permalinks.pin();
        let tx = self.store.begin().await?;
        let done = async {
            let current = tx
                .content()
                .get(id)
                .await?
                .filter(|i| i.kind == MEDIA_KIND)
                .ok_or(CoreError::NotFound)?;
            let mut media = tx
                .media()
                .get_for_update(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            let terms = tx
                .terms()
                .for_item(id)
                .await?
                .into_iter()
                .map(|t| t.id)
                .collect();
            let draft = ContentDraft {
                kind: current.kind,
                title: title.unwrap_or(current.title),
                slug: None,
                parent_id: current.parent_id,
                excerpt: Some(current.excerpt),
                body: current.body,
                fields: current.fields,
                menu_order: current.menu_order,
                terms,
            };
            // Rights, version and trash checks; its event names `post:<id>` (R11).
            let item = self
                .content
                .update_in(
                    tx.as_ref(),
                    rules.as_ref(),
                    &writer,
                    Edit {
                        id,
                        expected_version,
                        draft,
                        note: None,
                    },
                )
                .await?;
            if let Some(alt) = alt {
                media.alt = alt;
                tx.media().update(&media).await?;
            }
            Ok(MediaItem { item, media })
        }
        .await;
        finish(tx, done).await
    }

    /// Moves media item `id` to trash ([`ContentService::trash`], with `media:write`
    /// in place of `content:write`). The file stays served only while the item is
    /// public, so a trashed item's URLs stop answering. A non-media item is `NotFound`.
    pub async fn trash(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<ContentItem, CoreError> {
        require_scope(actor, Some(Scope::write(Area::Media)))?;
        self.check_kind(id).await?;
        self.content
            .trash(&content_writer(actor), id, expected_version)
            .await
    }

    /// Permanently deletes trashed media item `id` ([`ContentService::delete`], with
    /// `media:write` in place of `content:write`). The `media` row goes with the item
    /// (cascade); the blobs stay for M4's GC (03 §6). A non-media item is `NotFound`.
    pub async fn delete(
        &self,
        actor: &Actor,
        id: ContentId,
        expected_version: i64,
    ) -> Result<(), CoreError> {
        require_scope(actor, Some(Scope::write(Area::Media)))?;
        self.check_kind(id).await?;
        self.content
            .delete(&content_writer(actor), id, expected_version)
            .await
    }

    /// Records the result of a `media.variants` job (a system path without an actor),
    /// in one `Tx`: when the `media` row of `item` still holds `artifact` as its
    /// original, the variant `artifacts` rows (unless their hashes exist), the row's
    /// `width`, `height` and `variants` (name → `{hash, w, h, mime}`), and an
    /// `invalidate` event with `post:<item>`; returns `true`. When the row is gone or
    /// holds another original, nothing is written and the result is `false`. The row
    /// is read with a row lock, so a concurrent alt edit is not lost. `variants` must
    /// be an object and the dimensions must fit the row (`Invalid`).
    pub async fn record_variants(
        &self,
        item: ContentId,
        artifact: &ArtifactHash,
        width: u32,
        height: u32,
        variants: Value,
        artifacts: &[Artifact],
    ) -> Result<bool, CoreError> {
        if !variants.is_object() {
            return Err(CoreError::Invalid("variants must be an object".into()));
        }
        let dimension = |v: u32| {
            i32::try_from(v).map_err(|_| CoreError::Invalid("image dimension out of range".into()))
        };
        let (width, height) = (dimension(width)?, dimension(height)?);
        let tx = self.store.begin().await?;
        let done = async {
            let Some(mut row) = tx.media().get_for_update(item).await? else {
                return Ok(false);
            };
            if row.artifact_hash != *artifact {
                return Ok(false);
            }
            for a in artifacts {
                tx.artifacts().insert_if_absent(a).await?;
            }
            row.width = Some(width);
            row.height = Some(height);
            row.variants = variants;
            tx.media().update(&row).await?;
            tx.events()
                .append(&events::invalidate([events::post_key(item)]))
                .await?;
            Ok(true)
        }
        .await;
        finish(tx, done).await
    }

    /// `NotFound` unless item `id` exists and is of kind `media`. The kind never
    /// changes, so reading it before the write's own `Tx` is safe.
    async fn check_kind(&self, id: ContentId) -> Result<(), CoreError> {
        match self.store.read().content().get(id).await? {
            Some(i) if i.kind == MEDIA_KIND => Ok(()),
            _ => Err(CoreError::NotFound),
        }
    }
}

/// `actor` as the content rules see it for a media write whose `media:write` scope was
/// checked: the same user and role, holding `content:write` alone, so the content
/// checks apply the role rules only.
fn content_writer(actor: &Actor) -> Actor {
    actor.with_scopes(Scopes::only(Scope::write(Area::Content)))
}

/// The last `/` or `\` segment of `raw`: 1..=[`MAX_ORIGINAL_NAME`] bytes, not `.` or
/// `..`, no control characters.
fn check_file_name(raw: &str) -> Result<String, CoreError> {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    if name.is_empty()
        || name.len() > MAX_ORIGINAL_NAME
        || name == "."
        || name == ".."
        || name.chars().any(char::is_control)
    {
        return Err(CoreError::invalid_field(
            "file",
            format!(
                "the file name must be 1 to {MAX_ORIGINAL_NAME} bytes without control characters"
            ),
        ));
    }
    Ok(name.to_owned())
}

/// A title of at most [`MAX_MEDIA_TITLE`] characters without control characters.
fn check_title(title: &str) -> Result<String, CoreError> {
    if title.chars().count() > MAX_MEDIA_TITLE || title.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "title",
            format!(
                "the title must be at most {MAX_MEDIA_TITLE} characters without control characters"
            ),
        ));
    }
    Ok(title.to_owned())
}

/// Alternative text of at most [`MAX_MEDIA_ALT`] characters without control characters.
fn check_alt(alt: &str) -> Result<String, CoreError> {
    if alt.chars().count() > MAX_MEDIA_ALT || alt.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "alt",
            format!(
                "the alt text must be at most {MAX_MEDIA_ALT} characters without control characters"
            ),
        ));
    }
    Ok(alt.to_owned())
}

/// The file name without its extension (a leading dot is not one), cut to
/// [`MAX_MEDIA_TITLE`] characters.
fn title_from(name: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    stem.chars().take(MAX_MEDIA_TITLE).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn file_names_keep_their_last_segment() {
        assert_eq!(check_file_name("a.png").unwrap(), "a.png");
        assert_eq!(check_file_name("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(check_file_name(r"C:\x\y.jpg").unwrap(), "y.jpg");
        for bad in ["", "a/", "a\\", ".", "x/..", "a\nb", "a\u{7f}"] {
            assert!(
                matches!(
                    check_file_name(bad),
                    Err(CoreError::InvalidField { field: "file", .. })
                ),
                "{bad:?}"
            );
        }
        assert!(check_file_name(&"é".repeat(127)).is_ok(), "254 bytes");
        assert!(check_file_name(&"é".repeat(128)).is_err(), "256 bytes");
    }

    #[test]
    fn titles_come_from_the_file_name() {
        assert_eq!(title_from("photo.png"), "photo");
        assert_eq!(title_from("archive.tar.gz"), "archive.tar");
        assert_eq!(title_from(".bashrc"), ".bashrc");
        assert_eq!(title_from("README"), "README");
        assert_eq!(title_from(&format!("{}.png", "n".repeat(251))).len(), 200);
    }

    #[test]
    fn content_writer_holds_content_write_only() {
        let actor = Actor::new(UserId::new(), ferrite_domain::Role::Author)
            .with_scopes(Scopes::from_strs(&["media:write".into()]).unwrap());
        let w = content_writer(&actor);
        assert_eq!((w.user, w.role), (actor.user, actor.role));
        assert_eq!(w.scopes.to_strings(), vec!["content:write".to_owned()]);
    }
}
