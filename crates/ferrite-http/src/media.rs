//! Staging uploaded media files (M2a-2b P3/P4/P7/P9, R1/F13): the node's [`Uploader`].
//!
//! The `file` part of `POST /api/v1/media` is streamed into the media store, outside any
//! `Tx`, and hashed while it streams; core then records what was staged
//! ([`MediaService::record_upload`](ferrite_core::MediaService::record_upload)). The
//! type is decided from the content's first [`HEAD_BYTES`] before anything is stored,
//! never from the client's `Content-Type`, by [`stage_stream`] (its module docs have
//! the table; imported files go through it too).
//!
//! Documents are served download-only by `/_media` (R1). At most [`MEDIA_UPLOAD_SLOTS`]
//! uploads stream at once per node; one more is refused `Busy` before any of its file is
//! read. A body over `max_upload` is cut while it streams; a cut, a client that goes
//! away and a refused type leave nothing stored (the store aborts and removes its
//! staging object).
//! The MCP `media_upload` tool stages through the same pipeline and slots:
//! [`Uploader::stage_bytes`] for base64 data and [`Uploader::stage_url`] for a file
//! fetched with the importer's SSRF-checked client ([`HttpSource::for_upload`]: `https`
//! only, public addresses, 30 s). The download runs under its own two fetch slots and is
//! buffered up to the cap; an upload slot is taken only to stage the bytes.
//! An upload that receives no bytes for the idle timeout (`media.upload_idle_timeout`,
//! default [`DEFAULT_IDLE_TIMEOUT`]) is aborted the same way and frees its slot, so slow
//! clients cannot hold the node's slots.
//! A failure after a blob was stored never deletes it (M1b R2: another upload may share
//! it); orphans wait for M4's GC. File contents are never logged.

use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::extract::multipart::{Field, MultipartError};
use bytes::{Bytes, BytesMut};
use ferrite_domain::{Artifact, ArtifactKind, Timestamp};
use ferrite_import_wp::{ImportError, run::MediaSource};
use ferrite_media::{
    MediaError, MediaStore, VariantSpec, default_variants,
    ingest::{StagedFile, stage_stream},
    svg::SvgError,
};
#[cfg(doc)]
use ferrite_media::{ingest::HEAD_BYTES, svg::MAX_SVG_BYTES};
use ferrite_theme::Theme;
use futures::StreamExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{import_fetch::HttpSource, plugins::egress};

/// Uploads streaming at once per node (P9); one more is [`UploadError::Busy`].
pub const MEDIA_UPLOAD_SLOTS: usize = 8;

/// Downloads of files named by URL at once per node: a slow server holds one of these,
/// never an upload slot.
pub const URL_FETCH_SLOTS: usize = 2;

/// How long a download waits for a fetch slot before the call is `Busy`.
pub const FETCH_WAIT: Duration = Duration::from_secs(2);

/// Largest raster original whose variants are made (P8): 64 MiB.
pub const MAX_PROCESS_BYTES: u64 = 64 * 1024 * 1024;

/// Default for how long an upload may receive nothing before it is aborted
/// (`media.upload_idle_timeout`).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// `media.strip_metadata` is on and the image's metadata cannot be removed (too large
    /// to strip, or not parseable); nothing was stored. The reason is a fixed text.
    #[error("metadata could not be removed: {0}")]
    Metadata(String),
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
    /// A file named by URL could not be fetched: a refused address or scheme, an
    /// unreachable host, a status other than 2xx, too many redirects, a timeout or a
    /// body that broke. Nothing was stored.
    #[error("the URL could not be fetched")]
    Unreachable,
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
    strip_metadata: bool,
    idle: Duration,
    slots: Arc<Semaphore>,
    /// Downloads of files named by URL, apart from the upload slots.
    fetches: Arc<Semaphore>,
    started: AtomicU64,
    /// The client for files named by URL, built on first use.
    remote: OnceLock<Result<HttpSource, String>>,
    /// Test seam: the client allows loopback and plain `http`.
    #[cfg(feature = "test-seams")]
    private_fetch: bool,
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
            strip_metadata: false,
            idle: DEFAULT_IDLE_TIMEOUT,
            slots: Arc::new(Semaphore::new(MEDIA_UPLOAD_SLOTS)),
            fetches: Arc::new(Semaphore::new(URL_FETCH_SLOTS)),
            started: AtomicU64::new(0),
            remote: OnceLock::new(),
            #[cfg(feature = "test-seams")]
            private_fetch: false,
        }
    }

    /// Test seam (`test-seams`): files named by URL may come from loopback addresses and
    /// over plain `http`, so tests can serve them from 127.0.0.1.
    #[cfg(feature = "test-seams")]
    pub fn with_private_fetch(mut self) -> Self {
        self.private_fetch = true;
        self
    }

    /// Aborts an upload that receives nothing for `idle` (`media.upload_idle_timeout`).
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    /// Strips EXIF and XMP from JPEG, PNG and WebP uploads before they are stored
    /// (`media.strip_metadata`, default off).
    pub fn with_strip_metadata(mut self, strip: bool) -> Self {
        self.strip_metadata = strip;
        self
    }

    /// `media.max_upload`. Test seam (`test-seams`).
    #[cfg(feature = "test-seams")]
    pub fn max_upload(&self) -> u64 {
        self.max_upload
    }

    /// How long an upload may receive nothing (`media.upload_idle_timeout`); import
    /// uploads use it too.
    pub fn idle_timeout(&self) -> Duration {
        self.idle
    }

    fn idle_error(&self) -> UploadError {
        UploadError::Idle {
            secs: self.idle.as_secs().max(1),
        }
    }

    /// Uploads holding a slot now. Test seam (`test-seams`).
    #[cfg(feature = "test-seams")]
    pub fn in_flight(&self) -> usize {
        MEDIA_UPLOAD_SLOTS.saturating_sub(self.slots.available_permits())
    }

    /// Uploads that got a slot since this uploader was made. Test seam (`test-seams`).
    #[cfg(feature = "test-seams")]
    pub fn started(&self) -> u64 {
        self.started.load(Ordering::Relaxed)
    }

    /// Takes an upload slot (else `Busy`).
    fn take_slot(&self) -> Result<OwnedSemaphorePermit, UploadError> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| UploadError::Busy)?;
        self.started.fetch_add(1, Ordering::Relaxed);
        Ok(slot)
    }

    /// [`stage_stream`]s `stream` as file `original_name` (at most `cap` bytes) and
    /// keeps `slot` with the result.
    async fn stage_slot<S>(
        &self,
        slot: OwnedSemaphorePermit,
        original_name: &str,
        stream: S,
        cap: u64,
    ) -> Result<StagedUpload, MediaError>
    where
        S: futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin,
    {
        let file =
            stage_stream(&self.media, original_name, stream, cap, self.strip_metadata).await?;
        Ok(staged(file, slot))
    }

    /// Stages the `file` part `field`: takes a slot (else `Busy`), then
    /// [`stage_stream`]s the field with the idle timeout applied to every chunk.
    pub async fn stage(&self, field: Field<'_>) -> Result<StagedUpload, UploadError> {
        let slot = self.take_slot()?;
        let original_name = field.file_name().unwrap_or_default().to_owned();
        // The stager sees an I/O error and aborts; why the stream failed is kept for the
        // answer.
        let mut failure: Option<Failure> = None;
        let idle = self.idle;
        let stream = futures::stream::unfold(
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
        let result = self
            .stage_slot(slot, &original_name, Box::pin(stream), self.max_upload)
            .await;
        match (result, failure) {
            (Ok(file), _) => Ok(file),
            (Err(_), Some(Failure::Body(body))) => Err(UploadError::Body(body)),
            (Err(_), Some(Failure::Idle)) => Err(self.idle_error()),
            (Err(e), None) => Err(e.into()),
        }
    }

    /// Stages `bytes` already in memory as file `original_name`: the same slots, type
    /// sniffing, SVG sanitising and metadata stripping as [`stage`](Self::stage), with
    /// the size cap `limit.min(max_upload)`.
    pub async fn stage_bytes(
        &self,
        bytes: Bytes,
        original_name: &str,
        limit: u64,
    ) -> Result<StagedUpload, UploadError> {
        self.stage_decoded(original_name, limit, || Ok(bytes)).await
    }

    /// [`stage_bytes`](Self::stage_bytes) for bytes that still have to be made (decoded
    /// from base64): `make` runs only after a slot is taken, so a node whose slots are
    /// all busy answers `Busy` without allocating the file.
    pub async fn stage_decoded<E>(
        &self,
        original_name: &str,
        limit: u64,
        make: impl FnOnce() -> Result<Bytes, E>,
    ) -> Result<StagedUpload, E>
    where
        E: From<UploadError>,
    {
        let slot = self.take_slot()?;
        let cap = limit.min(self.max_upload);
        let bytes = make()?;
        let stream = futures::stream::iter([Ok::<_, std::io::Error>(bytes)]);
        self.stage_slot(slot, original_name, stream, cap)
            .await
            .map_err(|e| E::from(UploadError::from(e)))
    }

    /// Fetches `url` (`https` only, public addresses, redirects bounded, 30 s in all;
    /// see [`HttpSource::for_upload`]) and stages the body as file `original_name`, with
    /// the size cap `limit.min(max_upload)`. The download runs under one of the node's
    /// [`URL_FETCH_SLOTS`] fetch slots (else `Busy` after [`FETCH_WAIT`]) and is buffered
    /// up to the cap; only then is an upload slot taken to stage the bytes, so a slow
    /// server holds no upload slot. A fetch that fails for any reason is
    /// [`UploadError::Unreachable`].
    pub async fn stage_url(
        &self,
        url: &str,
        original_name: &str,
        limit: u64,
    ) -> Result<StagedUpload, UploadError> {
        let cap = limit.min(self.max_upload);
        let bytes = self.fetch_url(url, original_name, cap).await?;
        self.stage_bytes(bytes, original_name, cap).await
    }

    /// The body of `url`, at most `cap` bytes (see [`stage_url`](Self::stage_url)).
    async fn fetch_url(&self, url: &str, name: &str, cap: u64) -> Result<Bytes, UploadError> {
        let _permit = tokio::time::timeout(FETCH_WAIT, self.fetches.clone().acquire_owned())
            .await
            .map_err(|_| UploadError::Busy)?
            .map_err(|_| UploadError::Busy)?;
        let source = self.remote()?;
        let download = async {
            let mut body = match source.open(name, url, cap).await {
                Ok(Some(body)) => body,
                Ok(None) => return Err(UploadError::Unreachable),
                Err(ImportError::Media(e)) => return Err(e.into()),
                Err(_) => return Err(UploadError::Unreachable),
            };
            let mut buf = BytesMut::new();
            while let Some(chunk) = body.next().await {
                // The body breaking mid-way is the fetch failing, not storage.
                let chunk = chunk.map_err(|_| UploadError::Unreachable)?;
                if (buf.len() + chunk.len()) as u64 > cap {
                    return Err(UploadError::TooLarge { limit: cap });
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(buf.freeze())
        };
        // The client's own timeout covers one request; this bounds the whole download.
        tokio::time::timeout(egress::MEDIA_TIMEOUT, download)
            .await
            .unwrap_or(Err(UploadError::Unreachable))
    }

    /// Parses `raw` as a URL to fetch: absolute, `https` (plain `http` too under the
    /// `test-seams` private-fetch seam), no credentials. The address rules are the
    /// client's, applied when it connects. The error is a sentence for the caller.
    pub fn check_fetch_url(&self, raw: &str) -> Result<url::Url, &'static str> {
        let url = url::Url::parse(raw.trim()).map_err(|_| "not an absolute URL")?;
        #[cfg(feature = "test-seams")]
        let plain_ok = self.private_fetch;
        #[cfg(not(feature = "test-seams"))]
        let plain_ok = false;
        match url.scheme() {
            "https" => {}
            "http" if plain_ok => {}
            "http" => return Err("only https URLs are fetched; http is refused"),
            _ => return Err("only https URLs are fetched"),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("URLs with credentials are refused");
        }
        if url.host().is_none() {
            return Err("the URL has no host");
        }
        Ok(url)
    }

    /// The client for files named by URL.
    fn remote(&self) -> Result<&HttpSource, UploadError> {
        self.remote
            .get_or_init(|| {
                let version = env!("CARGO_PKG_VERSION");
                #[cfg(feature = "test-seams")]
                if self.private_fetch {
                    return HttpSource::allowing_private(version)
                        .map(HttpSource::with_host_only_logs);
                }
                HttpSource::for_upload(version)
            })
            .as_ref()
            .map_err(|e| UploadError::Task(e.clone()))
    }
}

impl From<MediaError> for UploadError {
    fn from(e: MediaError) -> Self {
        match e {
            MediaError::TooLarge { limit } => Self::TooLarge { limit },
            MediaError::Unsupported { mime } => Self::Unsupported { mime },
            MediaError::Svg(svg) => Self::Svg(svg),
            MediaError::Metadata(reason) => Self::Metadata(reason),
            MediaError::Task(task) => Self::Task(task),
            other => Self::Media(other),
        }
    }
}

fn staged(file: StagedFile, slot: OwnedSemaphorePermit) -> StagedUpload {
    StagedUpload {
        artifact: Artifact {
            hash: file.artifact.hash,
            kind: ArtifactKind::Media,
            bytes: i64::try_from(file.bytes).unwrap_or(i64::MAX),
            mime: file.mime.clone(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        },
        mime: file.mime,
        bytes: file.bytes,
        original_name: file.original_name,
        sanitized: file.sanitized,
        raster: file.raster,
        _slot: slot,
    }
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
}
