//! `media.variants` (M2a-2b P8): the size variants of one raster upload.

use std::sync::Arc;

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler};
use ferrite_core::{
    CoreError, MediaService,
    jobs::{MEDIA_VARIANTS_KIND, MEDIA_VARIANTS_VERSION, MediaVariants},
};
use ferrite_domain::{Artifact, ArtifactHash, ArtifactKind, Job, Timestamp};
use ferrite_http::media::MAX_PROCESS_BYTES;
use ferrite_media::{MediaError, MediaStore, Processed, VariantSpec, process_image};
use ferrite_store::Store;
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

/// What decodes an original into its variants: [`process_image`] in production; a
/// test seam (`NodeOptions::image_processor`, behind `test-seams`) replaces it.
pub type ImageProcessor =
    Arc<dyn Fn(&[u8], &[VariantSpec]) -> Result<Processed, MediaError> + Send + Sync>;

/// The sizes to make, read when a job runs (the node's active theme, P8).
pub type VariantSpecs = Arc<dyn Fn() -> Vec<VariantSpec> + Send + Sync>;

/// `media.variants`: makes and records the size variants of one raster upload.
///
/// 1. The payload (`{"v":1,"item","artifact"}`) must parse and name a valid hash, else
///    `Permanent`.
/// 2. One of the node's image slots (`media.image_workers`, M1b R4) is taken **without
///    waiting**; none free is [`JobError::Busy`] (the attempt is refunded and the job is
///    due again after 1–5 s), so image jobs never hold every runner worker (F17). It is
///    taken first, so a busy re-claim costs no store read and no (S3) `HEAD`.
/// 3. The item's `media` row is read: gone, or holding another original (the file was
///    replaced), the job is done without decoding anything.
/// 4. An original over [`MAX_PROCESS_BYTES`] (64 MiB, by `head`) gets no variants: done,
///    logged (the upload stays served as is; themes fall back to the original).
/// 5. The original is read and decoded on the blocking pool. The slot's permit moves
///    into the blocking closure: the runner may drop `run` at any await (lease loss,
///    timeout, shutdown) but cannot stop a blocking task, and the slot stays taken until
///    the decode itself ends, so at most `image_workers` images are ever decoded at once.
/// 6. Each variant is stored (`put_artifact`, kind `variant`), then
///    [`MediaService::record_variants`] writes the row (under a row lock, only while it
///    still holds this original) with its `post:<item>` event.
///
/// **Hostile images.** [`process_image`] sniffs the type, reads the header's dimensions
/// and refuses more than [`MAX_PIXELS`](ferrite_media::MAX_PIXELS) (100 MP) or 50 000 px
/// a side, and a decoded buffer (w × h × bytes per pixel) over 512 MiB (R12), **before**
/// any pixel buffer is allocated; decoder-internal allocations are capped at 512 MiB
/// (decompression bombs). EXIF orientation is applied; variants are re-encoded, so EXIF
/// (GPS included) and XMP are never copied into them (only a matching ICC profile
/// ≤ 1 MiB is kept). The stored original is kept byte for byte, its EXIF (GPS included)
/// with it, as WordPress does (R11).
///
/// **Memory per slot** (upper bounds, see [`process_image`]): the original (≤ 64 MiB),
/// the decoded buffer (≤ 512 MiB), a rotated or RGB copy of it, and per size the resize's
/// `f32` intermediate (≤ 16 B per source pixel; ≈ 1.3 GB at most with the core sizes) and
/// its output (≤ the decoded buffer): about 2.4 GB with the core sizes, up to about
/// 2.7 GB with wide theme sizes. Size `media.image_workers` against that (default 1). An image that is too large, does not decode, or makes the
/// decoder panic is `Permanent`: the job goes `dead` at once with the reason in
/// `last_error`, the worker carries on, and the original stays served. The image crate
/// offers no decode-time limit; a slow decode is bounded only by the image limits above
/// and by the slots: it holds one slot (and one blocking thread) until it finishes,
/// even past the handler timeout, and never more than `image_workers` run at once.
///
/// Store and media backend failures are `Retry` (backoff, then `dead` after the job's
/// attempts).
pub struct MediaVariantsHandler {
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
    service: Arc<MediaService>,
    slots: Arc<Semaphore>,
    specs: VariantSpecs,
    processor: ImageProcessor,
}

impl MediaVariantsHandler {
    /// A handler decoding at most `slots` images at once with the sizes `specs` returns.
    pub fn new(
        store: Arc<dyn Store>,
        media: Arc<MediaStore>,
        service: Arc<MediaService>,
        slots: Arc<Semaphore>,
        specs: VariantSpecs,
    ) -> Self {
        Self {
            store,
            media,
            service,
            slots,
            specs,
            processor: Arc::new(process_image),
        }
    }

    /// Test seam: decodes with `processor` instead of [`process_image`].
    #[cfg(feature = "test-seams")]
    pub fn with_processor(mut self, processor: ImageProcessor) -> Self {
        self.processor = processor;
        self
    }
}

#[async_trait]
impl JobHandler for MediaVariantsHandler {
    fn kind(&self) -> &'static str {
        MEDIA_VARIANTS_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[MEDIA_VARIANTS_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: MediaVariants = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let hash = ArtifactHash::parse(&payload.artifact)
            .map_err(|e| JobError::Permanent(format!("invalid payload: artifact: {e}")))?;
        let item = payload.item;
        // The slot first: a full node answers `Busy` before any store or media request.
        // Dropping `run` before the decode starts releases it.
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            return Err(JobError::Busy(
                "busy: every image slot of this node is in use".to_owned(),
            ));
        };
        let row = self
            .store
            .read()
            .media()
            .get(item)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        if row.is_none_or(|r| r.artifact_hash != hash) {
            tracing::info!(%item, %hash, "media row gone or replaced; variants not made");
            return Ok(());
        }
        let size = self
            .media
            .head(&hash)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?
            .ok_or_else(|| JobError::Retry("the original is not stored".to_owned()))?;
        if size > MAX_PROCESS_BYTES {
            tracing::info!(
                %item, %hash, bytes = size,
                "original larger than {MAX_PROCESS_BYTES} bytes; no variants made"
            );
            return Ok(());
        }
        // Read with the slot held, so buffered originals are bounded by the slots too.
        let data = self
            .media
            .get(&hash)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        let specs = (self.specs)();
        let processor = self.processor.clone();
        let processed = tokio::task::spawn_blocking(move || {
            // Released when the decode ends, not when `run` is dropped (F17).
            let _slot = slot;
            processor(&data, &specs)
        })
        .await
        .map_err(|e| {
            if e.is_panic() {
                JobError::Permanent("image processing panicked".to_owned())
            } else {
                JobError::Retry(format!("image processing was cancelled: {e}"))
            }
        })?
        .map_err(process_error)?;

        let mut variants = Map::new();
        let mut artifacts = Vec::with_capacity(processed.variants.len());
        for v in processed.variants {
            let stored = self
                .media
                .put_artifact(v.data)
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?;
            variants.insert(
                v.name,
                json!({ "hash": stored.hash.as_str(), "w": v.width, "h": v.height, "mime": v.mime }),
            );
            artifacts.push(variant_artifact(&stored)?);
        }
        let recorded = self
            .service
            .record_variants(
                item,
                &hash,
                processed.width,
                processed.height,
                Value::Object(variants),
                &artifacts,
            )
            .await
            .map_err(|e| match e {
                CoreError::Invalid(_) | CoreError::InvalidField { .. } => {
                    JobError::Permanent(e.to_string())
                }
                other => JobError::Retry(other.to_string()),
            })?;
        if recorded {
            tracing::info!(%item, variants = artifacts.len(), "media variants recorded");
        } else {
            tracing::info!(%item, %hash, "media row replaced while processing; variants dropped");
        }
        Ok(())
    }
}

/// Decoding failures that no retry can change are `Permanent`.
fn process_error(e: MediaError) -> JobError {
    match e {
        MediaError::ImageTooLarge { .. }
        | MediaError::UnsupportedImage(_)
        | MediaError::Config(_)
        | MediaError::Io(_)
        | MediaError::Unsupported { .. }
        | MediaError::Svg(_)
        | MediaError::Task(_) => JobError::Permanent(format!("image could not be processed: {e}")),
        MediaError::Backend(_)
        | MediaError::NotFound
        | MediaError::Range(_)
        | MediaError::TooLarge { .. } => JobError::Retry(e.to_string()),
    }
}

fn variant_artifact(stored: &ferrite_media::ArtifactRef) -> Result<Artifact, JobError> {
    Ok(Artifact {
        hash: stored.hash.clone(),
        kind: ArtifactKind::Variant,
        bytes: i64::try_from(stored.bytes)
            .map_err(|_| JobError::Permanent("variant size out of range".to_owned()))?,
        mime: stored.mime.clone(),
        created_at: Timestamp::now(),
    })
}
