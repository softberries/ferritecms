//! WXR attachments → media items (pass 2, P13, P14).
//!
//! Each attachment's file (`_wp_attached_file`) is opened from the run's
//! [`MediaSource`](crate::run::MediaSource) and staged as an upload is (type from the
//! content, SVG sanitised), with no batch open; then a batch writes up to `batch_items`
//! of them as published media items: the title from the item, the alt text from
//! `_wp_attachment_image_alt`, the author the attachment's (else the importing admin),
//! and the parent post when it is already mapped. A missing file is reported and
//! skipped; a file the upload rules refuse (too large, of no accepted type) is counted
//! as failed and noted. The [`MediaIndex`] of every imported attachment's URL forms is
//! stored in the IR at the end.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ferrite_core::import::{ImportBatch, ImportMedia, Written};
use ferrite_core::{MAX_MEDIA_ALT, MAX_MEDIA_TITLE, NewUpload, check_file_name};
use ferrite_domain::{Artifact, ArtifactKind, ContentId, ImportMapKind, Timestamp, UserId};
use ferrite_media::MediaError;
use ferrite_media::ingest::stage_stream;
use futures::TryStreamExt;

use super::{clean_text, meta, record_hash, wp_time};
use crate::ImportError;
use crate::model::Item;
use crate::php::{PhpValue, unserialize};
use crate::run::{MissingMedia, Report, Session};
use crate::urls::{MediaIndex, MediaRef, key};

/// The post type of attachments.
const ATTACHMENT: &str = "attachment";
/// The meta holding an attachment's path under `wp-content/uploads`.
const ATTACHED_FILE: &str = "_wp_attached_file";
/// The meta holding an image's alt text.
const IMAGE_ALT: &str = "_wp_attachment_image_alt";
/// The meta holding an attachment's sizes (PHP-serialised).
const METADATA: &str = "_wp_attachment_metadata";
/// How often a long run of file reads proves the run is alive (stale after 120 s).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

/// One attachment whose file is staged.
struct Staged {
    item: Item,
    url: String,
    upload: NewUpload,
    public_url: String,
}

/// Imports every attachment and stores the URL index in the IR.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    let authors: HashMap<String, i64> = session
        .ir()
        .authors()
        .await?
        .into_iter()
        .map(|a| (a.login, a.wp_id))
        .collect();
    let mut index = MediaIndex::default();
    let mut beat = Instant::now();
    let mut chunk = Vec::new();
    let mut items = session.ir().items(&[ATTACHMENT]);
    loop {
        let next = items.try_next().await?;
        let end = next.is_none();
        if let Some(item) = next {
            if let Some(staged) = stage(session, item, report).await? {
                chunk.push(staged);
            }
            if beat.elapsed() >= HEARTBEAT_EVERY {
                session.heartbeat().await?;
                beat = Instant::now();
            }
        }
        if chunk.len() >= session.batch_items() || (end && !chunk.is_empty()) {
            write(
                session,
                std::mem::take(&mut chunk),
                &authors,
                &mut index,
                report,
            )
            .await?;
            beat = Instant::now();
        }
        if end {
            break;
        }
    }
    drop(items);
    index.save(session.ir()).await
}

/// Opens and stages one attachment's file; `None` (reported) when it is missing or
/// refused.
async fn stage(
    session: &Session<'_>,
    item: Item,
    report: &mut Report,
) -> Result<Option<Staged>, ImportError> {
    let url = item
        .attachment_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| item.guid.clone());
    let path = meta(&item, ATTACHED_FILE)
        .map(str::to_owned)
        .or_else(|| uploads_path(&url))
        .unwrap_or_default();
    let missing = |report: &mut Report| {
        report.counts_of("media").skipped += 1;
        report.missing_media.push(MissingMedia {
            wp_id: item.wp_id,
            path: path.clone(),
            url: url.clone(),
        });
    };
    let Ok(file_name) = check_file_name(&path) else {
        missing(report);
        return Ok(None);
    };
    let ctx = session.ctx();
    let max = session.opts().max_upload;
    let Some(stream) = ctx.media_source.open(&path, &url, max).await? else {
        missing(report);
        return Ok(None);
    };
    let staged = match stage_stream(&ctx.media_store, &file_name, stream, max).await {
        Ok(staged) => staged,
        Err(
            e @ (MediaError::TooLarge { .. } | MediaError::Unsupported { .. } | MediaError::Svg(_)),
        ) => {
            report.counts_of("media").failed += 1;
            report.note(format!(
                "attachment {} ({path}) was refused: {e}",
                item.wp_id
            ));
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let public_url = ctx
        .media_store
        .public_url(&staged.artifact.hash, &file_name);
    let title = clean_text(item.title.trim(), MAX_MEDIA_TITLE);
    let upload = NewUpload {
        artifact: Artifact {
            hash: staged.artifact.hash,
            kind: ArtifactKind::Media,
            bytes: i64::try_from(staged.bytes).unwrap_or(i64::MAX),
            mime: staged.mime.clone(),
            created_at: Timestamp::now(),
        },
        mime: staged.mime,
        bytes: staged.bytes,
        original_name: file_name,
        title: Some(title).filter(|t| !t.is_empty()),
        alt: clean_text(
            meta(&item, IMAGE_ALT).unwrap_or_default().trim(),
            MAX_MEDIA_ALT,
        ),
        sanitized: staged.sanitized,
        raster: staged.raster,
    };
    Ok(Some(Staged {
        item,
        url,
        upload,
        public_url,
    }))
}

/// Writes one batch of staged attachments, counts them and adds them to `index`.
async fn write(
    session: &Session<'_>,
    chunk: Vec<Staged>,
    authors: &HashMap<String, i64>,
    index: &mut MediaIndex,
    report: &mut Report,
) -> Result<(), ImportError> {
    let admin = session.ctx().actor.user;
    let mut batch = session.batch().await?;
    let result = write_batch(&mut batch, chunk, authors, admin).await;
    let written = session.finish_batch(batch, result).await?;
    for (s, id, result) in written {
        let counts = report.counts_of("media");
        match result {
            Written::Created => counts.created += 1,
            Written::Updated => counts.updated += 1,
            Written::Unchanged => counts.unchanged += 1,
            Written::KeptEdited => counts.kept += 1,
        }
        let metadata = meta(&s.item, METADATA).and_then(|m| unserialize(m).ok());
        index.insert(
            &s.url,
            metadata
                .as_ref()
                .filter(|m| matches!(m, PhpValue::Array(_))),
            MediaRef {
                id: id.as_uuid(),
                url: s.public_url,
            },
        );
    }
    Ok(())
}

/// Writes `chunk` in one batch: each attachment's author (the importing admin when it
/// is not mapped) and parent (when mapped), then its media item.
async fn write_batch(
    b: &mut ImportBatch<'_>,
    chunk: Vec<Staged>,
    authors: &HashMap<String, i64>,
    admin: UserId,
) -> Result<Vec<(Staged, ContentId, Written)>, ImportError> {
    let mut out = Vec::with_capacity(chunk.len());
    for s in chunk {
        let author = match authors.get(&s.item.creator_login) {
            Some(wp) => b
                .lookup(ImportMapKind::User, *wp)
                .await?
                .map_or(admin, |m| UserId::from_uuid(m.id)),
            None => admin,
        };
        let parent = match s.item.parent_wp_id {
            0 => None,
            wp => b
                .lookup(ImportMapKind::Content, wp)
                .await?
                .map(|m| ContentId::from_uuid(m.id)),
        };
        let outcome = b
            .media(ImportMedia {
                wp_id: s.item.wp_id,
                hash: record_hash(&s.item)?,
                upload: s.upload.clone(),
                author,
                created_at: wp_time(&s.item.date_gmt, &s.item.date_local)
                    .unwrap_or_else(Timestamp::now),
                parent,
            })
            .await?;
        out.push((s, outcome.id, outcome.result));
    }
    Ok(out)
}

/// The path under `wp-content/uploads/` of an attachment URL, for an attachment without
/// `_wp_attached_file`.
fn uploads_path(url: &str) -> Option<String> {
    key(url)?
        .strip_prefix("/wp-content/uploads/")
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_fall_back_to_the_url() {
        assert_eq!(
            uploads_path("http://wp.test/wp-content/uploads/2024/03/a%20b.png").as_deref(),
            Some("2024/03/a b.png")
        );
        assert_eq!(uploads_path("http://wp.test/files/a.png"), None);
    }
}
