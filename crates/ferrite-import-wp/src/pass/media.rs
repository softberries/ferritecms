//! WXR attachments → media items (pass 2, P13, P14).
//!
//! Attachments go in chunks of `batch_items`. A first batch per chunk reads the import
//! map: an attachment mapped with the same record hash is unchanged, so its file is not
//! read again (its media row gives the public URL). For the others, the file
//! (`_wp_attached_file`) is opened from the run's [`MediaSource`](crate::run::MediaSource)
//! and staged as an upload is (type from the content, SVG sanitised), with no batch
//! open; then a batch writes them as published media items: the title from the item,
//! the alt text from `_wp_attachment_image_alt` and the author the attachment's (else
//! the importing admin). WP attachment parents are not imported (M3-1 ruling: nothing
//! in core reads them; the "uploaded to" link is lost). A missing file is reported and
//! skipped, and so is one whose bytes could not be read to the end (a download cut off
//! or timed out), with the reason noted; a file the upload rules refuse (too large, of
//! no accepted type) is counted as failed and noted. The [`MediaIndex`] of every imported attachment's URL forms is
//! stored in the IR at the end.
//!
//! A dry run stages into a throwaway store under the run folder, deleted when the pass
//! ends, so it leaves no files in the site's media store.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ferrite_core::import::{ImportBatch, ImportMedia, WP_ID_META, Written};
use ferrite_core::{MAX_MEDIA_ALT, MAX_MEDIA_TITLE, NewUpload, check_file_name};
use ferrite_domain::{Artifact, ArtifactKind, ContentId, ImportMapKind, Timestamp, UserId};
use ferrite_media::ingest::stage_stream;
use ferrite_media::{Backend, MediaConfig, MediaError, MediaStore, PublicUrlMode};
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
/// The dry run's throwaway media store, in the run folder.
const DRY_MEDIA: &str = "dry-run-media";

/// One attachment whose file is staged.
struct Staged {
    item: Item,
    hash: String,
    upload: NewUpload,
    public_url: String,
}

/// Imports every attachment and stores the URL index in the IR. A dry run stages into
/// a throwaway store and deletes it afterwards, whatever the outcome.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    if !session.opts().dry_run {
        return import_into(session, &session.ctx().media_store, report).await;
    }
    let root = session.run_dir().join(DRY_MEDIA);
    let store = MediaStore::open(MediaConfig {
        backend: Backend::Local { root: root.clone() },
        prefix: String::new(),
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".into(),
        },
    })?;
    let done = import_into(session, &store, report).await;
    drop(store);
    remove_dir(&root).await;
    done
}

async fn remove_dir(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "removing the dry run's media failed")
        }
    }
}

/// [`import`], staging files into `staging`.
async fn import_into(
    session: &Session<'_>,
    staging: &MediaStore,
    report: &mut Report,
) -> Result<(), ImportError> {
    let authors: HashMap<String, i64> = session
        .ir()
        .authors()
        .await?
        .into_iter()
        .map(|a| (a.login, a.wp_id))
        .collect();
    let mut index = MediaIndex::default();
    let mut chunk = Vec::new();
    let mut items = session.ir().items(&[ATTACHMENT]);
    loop {
        let next = items.try_next().await?;
        let end = next.is_none();
        if let Some(item) = next {
            chunk.push(item);
        }
        if chunk.len() >= session.batch_items() || (end && !chunk.is_empty()) {
            let todo = unchanged(session, std::mem::take(&mut chunk), &mut index, report).await?;
            let mut staged = Vec::with_capacity(todo.len());
            let mut beat = Instant::now();
            for (item, hash) in todo {
                if let Some(s) = stage(session, staging, item, hash, report).await? {
                    staged.push(s);
                }
                if beat.elapsed() >= HEARTBEAT_EVERY {
                    session.heartbeat().await?;
                    beat = Instant::now();
                }
            }
            if !staged.is_empty() {
                write(session, staged, &authors, &mut index, report).await?;
            }
        }
        if end {
            break;
        }
    }
    drop(items);
    index.save(session.ir()).await
}

/// Reads the import map for `chunk` in one batch: an attachment mapped with the same
/// record hash whose media row still exists is counted unchanged and indexed without
/// reading its file. The rest come back with their record hashes.
async fn unchanged(
    session: &Session<'_>,
    chunk: Vec<Item>,
    index: &mut MediaIndex,
    report: &mut Report,
) -> Result<Vec<(Item, String)>, ImportError> {
    let mut batch = session.batch().await?;
    let found = check_map(&mut batch, chunk).await;
    let (todo, same) = session.finish_batch(batch, found).await?;
    let store = &session.ctx().media_store;
    for (item, id, row) in same {
        report.counts_of("media").unchanged += 1;
        let url = store.public_url(&row.artifact_hash, &row.original_name);
        add_to_index(index, &item, id, url);
    }
    Ok(todo)
}

/// Per item: (to stage, with its hash) or (unchanged, with its id and media row).
type MapCheck = (
    Vec<(Item, String)>,
    Vec<(Item, ContentId, ferrite_domain::Media)>,
);

async fn check_map(b: &mut ImportBatch<'_>, chunk: Vec<Item>) -> Result<MapCheck, ImportError> {
    let (mut todo, mut same) = (Vec::new(), Vec::new());
    for item in chunk {
        let hash = record_hash(&item)?;
        let mapped = b.lookup(ImportMapKind::Media, item.wp_id).await?;
        if let Some(m) = mapped.filter(|m| m.hash == hash) {
            let id = ContentId::from_uuid(m.id);
            if let Some(row) = b.media_row(id).await? {
                same.push((item, id, row));
                continue;
            }
        }
        todo.push((item, hash));
    }
    Ok((todo, same))
}

/// The URL an attachment is known by: `<wp:attachment_url>`, else its guid.
fn attachment_url(item: &Item) -> String {
    item.attachment_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| item.guid.clone())
}

/// Adds every URL form of `item` (its URL and the sizes of its metadata) to `index`.
fn add_to_index(index: &mut MediaIndex, item: &Item, id: ContentId, public_url: String) {
    let metadata = meta(item, METADATA).and_then(|m| unserialize(m).ok());
    index.insert(
        &attachment_url(item),
        metadata
            .as_ref()
            .filter(|m| matches!(m, PhpValue::Array(_))),
        MediaRef {
            id: id.as_uuid(),
            url: public_url,
        },
    );
}

/// Opens and stages one attachment's file; `None` (reported) when it is missing or
/// refused.
async fn stage(
    session: &Session<'_>,
    staging: &MediaStore,
    item: Item,
    hash: String,
    report: &mut Report,
) -> Result<Option<Staged>, ImportError> {
    let url = attachment_url(&item);
    let path = meta(&item, ATTACHED_FILE)
        .map(str::to_owned)
        .or_else(|| uploads_path(&url))
        .unwrap_or_default();
    let missing = |report: &mut Report| {
        report.counts_of("media").skipped += 1;
        report.add_missing_media(MissingMedia {
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
    let refused = |report: &mut Report, e: &MediaError| {
        report.counts_of("media").failed += 1;
        report.note(format!(
            "attachment {} ({path}) was refused: {e}",
            item.wp_id
        ));
    };
    let stream = match ctx.media_source.open(&path, &url, max).await {
        Ok(Some(stream)) => stream,
        Ok(None) => {
            missing(report);
            return Ok(None);
        }
        // A source may refuse a file it knows is too large before reading it.
        Err(ImportError::Media(e @ MediaError::TooLarge { .. })) => {
            refused(report, &e);
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let staged = match stage_stream(
        staging,
        &file_name,
        stream,
        max,
        session.opts().strip_metadata,
    )
    .await
    {
        Ok(staged) => staged,
        Err(
            e @ (MediaError::TooLarge { .. }
            | MediaError::Unsupported { .. }
            | MediaError::Svg(_)
            | MediaError::Metadata(_)),
        ) => {
            refused(report, &e);
            return Ok(None);
        }
        // Reading the file failed part-way (a download cut off, a timeout, a body
        // shorter than announced): the file is missing, the run goes on.
        Err(MediaError::Io(why)) => {
            missing(report);
            report.note(format!(
                "attachment {} ({path}) could not be read from {url}: {why}",
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
            orphaned_at: None,
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
        hash,
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
        add_to_index(index, &s.item, id, s.public_url);
    }
    Ok(())
}

/// Writes `chunk` in one batch: each attachment's author (the importing admin when it
/// is not mapped), then its media item.
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
        let outcome = b
            .media(ImportMedia {
                wp_id: s.item.wp_id,
                id_meta: WP_ID_META,
                hash: s.hash.clone(),
                upload: s.upload.clone(),
                author,
                created_at: wp_time(&s.item.date_gmt, &s.item.date_local)
                    .unwrap_or_else(Timestamp::now),
                parent: None,
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
