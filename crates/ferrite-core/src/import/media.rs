//! Imported attachments (P13): a published `media` item with the dates and author of
//! the WP attachment, recorded like an upload.

use ferrite_domain::{
    ContentId, ContentStatus, ImportMapKind, MEDIA_KIND, Media, Timestamp, UserId,
};
use serde_json::json;

use super::{ContentOutcome, ImportBatch, ImportContent, Written};
use crate::{
    CoreError, NewUpload,
    media::{check_upload, record_upload_in},
};

/// One WXR attachment whose file is staged; its parent is already mapped.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportMedia {
    pub wp_id: i64,
    /// The meta key `wp_id` is stored under, as [`ImportContent::id_meta`].
    pub id_meta: &'static str,
    /// SHA-256 of the attachment's IR record (P3, P4).
    pub hash: String,
    /// The staged original, checked as an upload is
    /// ([`MediaService::record_upload`](crate::MediaService::record_upload)).
    pub upload: NewUpload,
    pub author: UserId,
    /// `post_date_gmt`: the item's `created_at` and `published_at`, and its `updated_at`.
    pub created_at: Timestamp,
    /// The post the attachment is attached to.
    pub parent: Option<ContentId>,
}

impl ImportBatch<'_> {
    /// The `media` row of item `id` (its artifact, type and file name), as this batch
    /// sees it: what a re-run needs about an unchanged attachment without its file.
    pub async fn media_row(&self, id: ContentId) -> Result<Option<Media>, CoreError> {
        Ok(self.tx.media().get(id).await?)
    }

    /// Writes one imported attachment as a **published** `media` item (P4, P13):
    ///
    /// - the upload is checked first as `record_upload` checks it (`InvalidField`
    ///   `title` / `alt` / `file`, `Invalid`), before anything is written;
    /// - the item follows [`content`](Self::content)'s rules (map kind `media`): created,
    ///   [`Unchanged`](Written::Unchanged) on the same `hash`, [`Updated`](Written::Updated)
    ///   when the hash changed and nobody edited it here (title, dates, author, parent
    ///   and the alt text follow; the stored file, type and size do not),
    ///   [`KeptEdited`](Written::KeptEdited) otherwise;
    /// - a new item gets its artifact row (unless the hash exists), its `media` row and,
    ///   for a raster image, one `media.variants` job (left to the next `serve`);
    /// - no plugin actions are enqueued (P6).
    ///
    /// **Errors.** As for [`content`](Self::content): any `Err` leaves the batch
    /// unusable, and the caller must [`rollback`](Self::rollback) it.
    pub async fn media(&mut self, m: ImportMedia) -> Result<ContentOutcome, CoreError> {
        let checked = check_upload(&m.upload)?;
        let item = ImportContent {
            wp_id: m.wp_id,
            id_meta: m.id_meta,
            hash: m.hash,
            kind: MEDIA_KIND.into(),
            title: checked.title.clone(),
            slug: None,
            parent: m.parent,
            author: m.author,
            status: ContentStatus::Published,
            published_at: Some(m.created_at),
            created_at: m.created_at,
            updated_at: m.created_at,
            excerpt: Some(String::new()),
            body: json!({ "type": "doc", "content": [] }),
            fields: json!({}),
            menu_order: 0,
            terms: Vec::new(),
            seo: None,
            meta: Vec::new(),
            locale: None,
            translation_group: None,
        };
        let out = self.write_content(ImportMapKind::Media, item).await?;
        let repos = self.tx.as_ref();
        match out.result {
            Written::Created => {
                record_upload_in(repos, out.id, &checked, &m.upload).await?;
            }
            Written::Updated => {
                let mut row = repos
                    .media()
                    .get_for_update(out.id)
                    .await?
                    .ok_or(CoreError::NotFound)?;
                if row.alt != checked.alt {
                    row.alt = checked.alt;
                    repos.media().update(&row).await?;
                }
            }
            Written::Unchanged | Written::KeptEdited => {}
        }
        Ok(out)
    }
}
