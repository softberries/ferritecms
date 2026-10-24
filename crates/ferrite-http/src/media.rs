//! Staging uploaded media files (M2a-2b P3/P4/P7/P9, R1/F13): the node's [`Uploader`].
//!
//! The `file` part of `POST /api/v1/media` is streamed into the media store, outside any
//! `Tx`, and hashed while it streams; core then records what was staged
//! ([`MediaService::record_upload`](ferrite_core::MediaService::record_upload)). The
//! type is decided from the content's first [`HEAD_BYTES`] before anything is stored,
//! never from the client's `Content-Type`:
//!
//! | Head | Stored as |
//! |---|---|
//! | looks like SVG ([`looks_like_svg`]) | the sanitiser's output, `image/svg+xml`, marked sanitised (at most [`MAX_SVG_BYTES`] read) |
//! | sniffs as `audio/mpeg`, named `.txt`/`.csv`, and the head passes the text rule below | `text/plain` / `text/csv` (infer's MP3 matcher reads only `ID3` or `FF Fx`; a note starting "ID3" is text) |
//! | sniffs as an [`inline_safe`] type (JPEG, PNG, GIF, WebP, MP3/Ogg/WAV, MP4/WebM, PDF) | that type |
//! | a ZIP (`PK\x03\x04`) named `.docx .xlsx .pptx .odt .ods .odp .zip` | the type of that extension |
//! | no known magic, named `.txt`/`.csv`, UTF-8 (BOM allowed) without NUL, first non-space character not `<` | `text/plain` / `text/csv` |
//! | anything else | refused (`415`) |
//!
//! The file name only picks within the ZIP and text classes (F13); documents are served
//! download-only by `/_media` (R1). At most [`MEDIA_UPLOAD_SLOTS`] uploads stream at
//! once per node; one more is refused `Busy` before any of its file is read. A body
//! over `max_upload` is cut while it streams; a cut, a client that goes away and a
//! refused type leave nothing stored (the store aborts and removes its staging object).
//! An upload that receives no bytes for the idle timeout (`media.upload_idle_timeout`,
//! default [`DEFAULT_IDLE_TIMEOUT`]) is aborted the same way and frees its slot, so slow
//! clients cannot hold the node's slots.
//! A failure after a blob was stored never deletes it (M1b R2: another upload may share
//! it); orphans wait for M4's GC. File contents are never logged.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::extract::multipart::{Field, MultipartError};
use bytes::{Bytes, BytesMut};
use ferrite_domain::{Artifact, ArtifactKind, Timestamp};
use ferrite_media::{
    ArtifactRef, MediaError, MediaStore, VariantSpec, default_variants, inline_safe,
    is_raster_image, sniff_mime,
    svg::{MAX_SVG_BYTES, SVG_MIME, SvgError, looks_like_svg, sanitize_svg},
};
use ferrite_theme::Theme;
use futures::StreamExt as _;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Uploads streaming at once per node (P9); one more is [`UploadError::Busy`].
pub const MEDIA_UPLOAD_SLOTS: usize = 8;

/// Largest raster original whose variants are made (P8): 64 MiB.
pub const MAX_PROCESS_BYTES: u64 = 64 * 1024 * 1024;

/// How much of the file decides its type (P4): 8 KiB.
pub const HEAD_BYTES: usize = 8 * 1024;

/// Default for how long an upload may receive nothing before it is aborted
/// (`media.upload_idle_timeout`).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The ZIP-based document classes by extension (R1/F13).
const ZIP_TYPES: [(&str, &str); 7] = [
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("zip", "application/zip"),
];

/// The text classes by extension (R1/F13).
const TEXT_TYPES: [(&str, &str); 2] = [("txt", "text/plain"), ("csv", "text/csv")];

const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";
/// What `sniff_mime` answers for MP3 (a weak, 2–3 byte match).
const MP3: &str = "audio/mpeg";
/// What `sniff_mime` answers for content without known magic.
const UNKNOWN: &str = "application/octet-stream";

/// Why a file could not be staged.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// The file is larger than `limit` bytes (`max_upload`, or [`MAX_SVG_BYTES`] for an
    /// SVG); nothing was stored.
    #[error("the file is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    /// The content is of no accepted type (`mime` as sniffed); nothing was stored.
    #[error("files of type {mime} are not accepted")]
    Unsupported { mime: String },
    /// The SVG cannot be made safe; nothing was stored.
    #[error("the SVG could not be made safe")]
    Svg(SvgError),
    /// Every upload slot of this node is taken; nothing was read.
    #[error("every upload slot of this node is taken")]
    Busy,
    /// The request body failed while the file was read (malformed, over the route
    /// limit, or the client went away); nothing was stored.
    #[error("the request body failed: {0}")]
    Body(MultipartError),
    /// No bytes arrived for `secs` seconds; the upload was aborted, nothing was stored.
    #[error("no bytes arrived for {secs} s")]
    Idle { secs: u64 },
    #[error("media storage: {0}")]
    Media(MediaError),
    /// The sanitiser's blocking task panicked or was cancelled.
    #[error("internal error: {0}")]
    Task(String),
}

/// A staged file, ready for [`NewUpload`](ferrite_core::NewUpload).
#[derive(Debug)]
pub struct StagedUpload {
    /// The stored original (kind `media`, `mime` and `bytes` as below).
    pub artifact: Artifact,
    /// Decided from the content (see the module docs).
    pub mime: String,
    pub bytes: u64,
    /// The part's file name as sent (core keeps its last segment).
    pub original_name: String,
    /// The bytes are the SVG sanitiser's output.
    pub sanitized: bool,
    /// A raster image (`is_raster_image`): core enqueues its variants job.
    pub raster: bool,
    /// Held until the upload is recorded or dropped.
    _slot: OwnedSemaphorePermit,
}

/// The node's one upload pipeline: its slots bound the node (P9).
pub struct Uploader {
    media: Arc<MediaStore>,
    max_upload: u64,
    idle: Duration,
    slots: Arc<Semaphore>,
    started: AtomicU64,
}

/// Why the file stream given to the store ended early.
enum Failure {
    Body(MultipartError),
    Idle,
}

impl Uploader {
    /// An uploader storing into `media`, refusing files over `max_upload` bytes, with
    /// the [`DEFAULT_IDLE_TIMEOUT`].
    pub fn new(media: Arc<MediaStore>, max_upload: u64) -> Self {
        Self {
            media,
            max_upload,
            idle: DEFAULT_IDLE_TIMEOUT,
            slots: Arc::new(Semaphore::new(MEDIA_UPLOAD_SLOTS)),
            started: AtomicU64::new(0),
        }
    }

    /// Aborts an upload that receives nothing for `idle` (`media.upload_idle_timeout`).
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    /// `media.max_upload`.
    pub fn max_upload(&self) -> u64 {
        self.max_upload
    }

    /// The next chunk of `field`, or `Idle` when none arrives within the idle timeout.
    async fn chunk(&self, field: &mut Field<'_>) -> Result<Option<Bytes>, UploadError> {
        match tokio::time::timeout(self.idle, field.chunk()).await {
            Ok(chunk) => chunk.map_err(UploadError::Body),
            Err(_) => Err(self.idle_error()),
        }
    }

    fn idle_error(&self) -> UploadError {
        UploadError::Idle {
            secs: self.idle.as_secs().max(1),
        }
    }

    /// Uploads holding a slot now.
    pub fn in_flight(&self) -> usize {
        MEDIA_UPLOAD_SLOTS.saturating_sub(self.slots.available_permits())
    }

    /// Uploads that got a slot since this uploader was made.
    pub fn started(&self) -> u64 {
        self.started.load(Ordering::Relaxed)
    }

    /// Stages the `file` part `field` (see the module docs): takes a slot (else
    /// `Busy`), reads the head, decides the type and stores the file, streamed and
    /// hashed as it arrives (an SVG is read whole, at most [`MAX_SVG_BYTES`], and its
    /// sanitised form stored).
    pub async fn stage(&self, mut field: Field<'_>) -> Result<StagedUpload, UploadError> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| UploadError::Busy)?;
        self.started.fetch_add(1, Ordering::Relaxed);
        let original_name = field.file_name().unwrap_or_default().to_owned();
        let svg_limit = self.max_upload.min(MAX_SVG_BYTES as u64);

        let mut head = BytesMut::new();
        let mut ended = false;
        while head.len() < HEAD_BYTES {
            match self.chunk(&mut field).await? {
                Some(chunk) => {
                    if (head.len() + chunk.len()) as u64 > self.max_upload {
                        return Err(UploadError::TooLarge {
                            limit: self.max_upload,
                        });
                    }
                    head.extend_from_slice(&chunk);
                }
                None => {
                    ended = true;
                    break;
                }
            }
        }
        let sniff = &head[..head.len().min(HEAD_BYTES)];

        if looks_like_svg(sniff) {
            let mut all = head;
            while !ended {
                match self.chunk(&mut field).await? {
                    Some(chunk) => {
                        if (all.len() + chunk.len()) as u64 > svg_limit {
                            return Err(UploadError::TooLarge { limit: svg_limit });
                        }
                        all.extend_from_slice(&chunk);
                    }
                    None => ended = true,
                }
            }
            if all.len() as u64 > svg_limit {
                return Err(UploadError::TooLarge { limit: svg_limit });
            }
            let input = all.freeze();
            let clean = tokio::task::spawn_blocking(move || sanitize_svg(&input))
                .await
                .map_err(|e| UploadError::Task(e.to_string()))?
                .map_err(UploadError::Svg)?;
            let stored = self
                .media
                .put_artifact(Bytes::from(clean))
                .await
                .map_err(UploadError::Media)?;
            return Ok(staged(
                stored,
                SVG_MIME.to_owned(),
                original_name,
                true,
                slot,
            ));
        }

        // The head decides; it is cut when the file goes on past it.
        let cut = !ended || head.len() > HEAD_BYTES;
        let mime = classify(sniff, cut, &original_name)?;
        // The store sees an I/O error and aborts; why the stream failed is kept for the
        // answer.
        let mut failure: Option<Failure> = None;
        let idle = self.idle;
        let rest = futures::stream::unfold(
            (field, &mut failure),
            move |(mut field, failure)| async move {
                let failed = match tokio::time::timeout(idle, field.chunk()).await {
                    Ok(Ok(Some(chunk))) => return Some((Ok(chunk), (field, failure))),
                    Ok(Ok(None)) => return None,
                    Ok(Err(e)) => Failure::Body(e),
                    Err(_) => Failure::Idle,
                };
                *failure = Some(failed);
                let io = std::io::Error::other("the request body failed");
                Some((Err(io), (field, failure)))
            },
        );
        let stream = futures::stream::once(async move { Ok(head.freeze()) }).chain(rest);
        let stored = self
            .media
            .put_stream(Box::pin(stream), self.max_upload)
            .await;
        let stored = match stored {
            Ok(stored) => stored,
            Err(MediaError::TooLarge { limit }) => return Err(UploadError::TooLarge { limit }),
            Err(e) => {
                return Err(match failure {
                    Some(Failure::Body(body)) => UploadError::Body(body),
                    Some(Failure::Idle) => self.idle_error(),
                    None => UploadError::Media(e),
                });
            }
        };
        Ok(staged(stored, mime, original_name, false, slot))
    }
}

fn staged(
    stored: ArtifactRef,
    mime: String,
    original_name: String,
    sanitized: bool,
    slot: OwnedSemaphorePermit,
) -> StagedUpload {
    let bytes = stored.bytes;
    StagedUpload {
        artifact: Artifact {
            hash: stored.hash,
            kind: ArtifactKind::Media,
            bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
            mime: mime.clone(),
            created_at: Timestamp::now(),
        },
        raster: is_raster_image(&mime),
        mime,
        bytes,
        original_name,
        sanitized,
        _slot: slot,
    }
}

/// The type of a non-SVG file from its `head` (the first [`HEAD_BYTES`]; `cut` when
/// the file goes on) and, within the ZIP and text classes, its file name's extension.
fn classify(head: &[u8], cut: bool, file_name: &str) -> Result<String, UploadError> {
    let sniffed = sniff_mime(head);
    let ext = extension(file_name);
    let by_ext = |table: &[(&str, &str)]| {
        ext.as_deref()
            .and_then(|e| table.iter().find(|(x, _)| *x == e))
            .map(|(_, mime)| (*mime).to_owned())
    };
    if inline_safe(&sniffed) {
        // infer's MP3 matcher is 2–3 bytes (`ID3`, `FF FB/F3/F2`): a `.txt`/`.csv` whose
        // head is plain text is text. `FF` is never UTF-8, so only an `ID3`-led note
        // qualifies; a real MP3 head has NULs or invalid UTF-8 and stays audio.
        if sniffed == MP3
            && is_plain_text(head, cut)
            && let Some(mime) = by_ext(&TEXT_TYPES)
        {
            return Ok(mime);
        }
        return Ok(sniffed);
    }
    if head.starts_with(ZIP_MAGIC)
        && let Some(mime) = by_ext(&ZIP_TYPES)
    {
        return Ok(mime);
    }
    if sniffed == UNKNOWN
        && is_plain_text(head, cut)
        && let Some(mime) = by_ext(&TEXT_TYPES)
    {
        return Ok(mime);
    }
    Err(UploadError::Unsupported { mime: sniffed })
}

/// The lowercase extension of the last `/` or `\` segment of `name`.
fn extension(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = last.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// `head` is UTF-8 (a UTF-8 BOM allowed; when `cut`, a character split at the end is
/// allowed) without NUL, and its first non-whitespace character is not `<`.
fn is_plain_text(head: &[u8], cut: bool) -> bool {
    let body = head.strip_prefix(UTF8_BOM).unwrap_or(head);
    if body.contains(&0) {
        return false;
    }
    let text = match std::str::from_utf8(body) {
        Ok(text) => text,
        // Only an incomplete last character of a head that was cut.
        Err(e) if cut && e.error_len().is_none() => {
            match std::str::from_utf8(&body[..e.valid_up_to()]) {
                Ok(text) => text,
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };
    !text.trim_start().starts_with('<')
}

/// The sizes a `media.variants` job makes on this node (M2a-2b P8): the core sizes
/// ([`default_variants`]: `thumbnail`, `medium`, `large`) plus the `[images]` of
/// `theme`, in name order. A theme size whose name is already taken, with a height but
/// no crop (a bounding box, which the processor does not implement) or with a zero
/// dimension is skipped, so a theme cannot make every job fail.
pub fn variant_specs(theme: &Theme) -> Vec<VariantSpec> {
    let mut specs = default_variants();
    for (name, size) in &theme.manifest().images {
        let unusable = (size.height.is_some() && !size.crop)
            || size.width == 0
            || size.height == Some(0)
            || specs.iter().any(|s| &s.name == name);
        if unusable {
            tracing::debug!(theme = %theme.name(), size = %name, "theme image size skipped");
            continue;
        }
        specs.push(VariantSpec {
            name: name.clone(),
            width: size.width,
            height: size.height,
            crop: size.crop,
        });
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_specs_add_the_theme_sizes() {
        let blog = Theme::load_embedded("blog").expect("embedded blog theme");
        let names: Vec<String> = variant_specs(&blog).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["thumbnail", "medium", "large", "card", "hero"]);
        let specs = variant_specs(&blog);
        assert_eq!(
            specs[3],
            VariantSpec {
                name: "card".into(),
                width: 640,
                height: Some(360),
                crop: true,
            }
        );
    }

    #[test]
    fn classes_follow_content_then_extension() {
        let ok = |head: &[u8], name: &str| classify(head, false, name).ok();
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(ok(png, "x.svg").as_deref(), Some("image/png"));
        assert_eq!(ok(png, "x.txt").as_deref(), Some("image/png"));
        assert_eq!(
            ok(b"%PDF-1.7\n", "x.docx").as_deref(),
            Some("application/pdf")
        );
        let zip = b"PK\x03\x04\x14\0\0\0\x08\0";
        assert_eq!(
            ok(zip, "A.DOCX").as_deref(),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        assert_eq!(ok(zip, "x.zip").as_deref(), Some("application/zip"));
        assert_eq!(ok(zip, "x.txt"), None);
        assert_eq!(ok(zip, "x.exe"), None);
        assert_eq!(ok(zip, "docx"), None);
        assert_eq!(ok(b"hello", "notes.txt").as_deref(), Some("text/plain"));
        assert_eq!(ok(b"a,b\n", "d.CSV").as_deref(), Some("text/csv"));
        assert_eq!(ok(b"", "e.txt").as_deref(), Some("text/plain"));
        assert_eq!(
            ok(b"\xEF\xBB\xBFhi", "b.txt").as_deref(),
            Some("text/plain")
        );
        assert_eq!(ok(b"hello", "notes.md"), None);
        assert_eq!(ok(b"hello", ".txt"), None);
        assert_eq!(ok(b" \n<html>", "x.txt"), None);
        assert_eq!(ok(b"\xEF\xBB\xBF<p>", "x.txt"), None);
        assert_eq!(ok(b"\xFF\xFEh\0i\0", "x.txt"), None);
        assert_eq!(ok(b"bad \xC3\x28 utf8", "x.txt"), None);
        // The weak MP3 match yields to text only for a text name and a text head.
        assert_eq!(
            ok(b"ID3 tags, explained", "n.txt").as_deref(),
            Some("text/plain")
        );
        assert_eq!(
            ok(b"ID3 tags, explained", "n.mp3").as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(
            ok(b"ID3\x04\0\0\0\0", "n.txt").as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(ok(b"\xFF\xFBrest", "n.txt").as_deref(), Some("audio/mpeg"));
        assert!(matches!(
            classify(b"\x7fELF\x02\x01\x01\0", false, "x.txt"),
            Err(UploadError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_character_split_by_the_head_cut_is_text() {
        // "é" is C3 A9; the head ends after C3.
        let head = b"caf\xC3";
        assert!(is_plain_text(head, true));
        assert!(!is_plain_text(head, false));
    }

    #[test]
    fn extensions_come_from_the_last_segment() {
        assert_eq!(extension("a/b.c/report.PDF").as_deref(), Some("pdf"));
        assert_eq!(extension("a\\b\\x.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension("dir.txt/noext"), None);
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("trailing."), None);
    }
}
