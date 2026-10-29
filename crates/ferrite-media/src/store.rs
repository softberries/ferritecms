use std::sync::Arc;
use std::time::Duration;

use std::ops::Range;

use bytes::Bytes;
use ferrite_domain::ArtifactHash;
use ferrite_domain::Timestamp;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt, TryStreamExt};
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{
    ClientOptions, GetOptions, GetRange, ObjectStore, ObjectStoreExt, PutMode, PutOptions,
    RetryConfig, WriteMultipart,
};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::config::{Backend, ConditionalPut, MediaConfig, PublicUrlMode, S3Config};
use crate::error::MediaError;
use crate::keys;
use crate::sniff::sniff_mime;

/// Payloads at or above this size must go through the streaming path.
const SMALL_LIMIT: usize = 8 * 1024 * 1024;
/// Multipart part size.
const PART_SIZE: usize = 8 * 1024 * 1024;
/// Parts in flight at once (bounds memory to roughly this many parts).
const PART_CONCURRENCY: usize = 2;
/// How much of the payload is inspected for MIME sniffing.
const SNIFF_LEN: usize = 8 * 1024;

/// Everything except RFC 3986 unreserved characters is escaped (valid for a path segment).
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'\\')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'|')
    .add(b'^')
    .add(b'[')
    .add(b']')
    .add(b':')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=');

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    pub hash: ArtifactHash,
    pub bytes: u64,
    /// Sniffed from content, never from a file name.
    pub mime: String,
    /// `false` if the artifact already existed.
    pub created: bool,
}

/// A streaming read opened by [`MediaStore::open_read`].
pub struct ReadStream {
    /// Total size of the object.
    pub size: u64,
    /// The byte range this stream serves (the whole object when none was requested).
    pub range: Range<u64>,
    pub stream: BoxStream<'static, Result<Bytes, MediaError>>,
}

/// One object under `artifacts/`, as listed by [`MediaStore::list_artifacts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    /// The artifact hash named by the key; `None` when the key is not
    /// `<prefix>artifacts/<first two hex chars>/<64 hex chars>`.
    pub hash: Option<ArtifactHash>,
    /// The full object key, including the store's prefix.
    pub key: String,
    pub size: u64,
    /// When the backend last modified the object (a blob's age, for a grace period).
    pub last_modified: Timestamp,
}

pub struct MediaStore {
    inner: Arc<dyn ObjectStore>,
    prefix: String,
    public: PublicUrlMode,
}

impl MediaStore {
    pub fn open(cfg: MediaConfig) -> Result<Self, MediaError> {
        keys::validate_prefix(&cfg.prefix)?;
        if let PublicUrlMode::ServeViaApp { path_prefix } = &cfg.public
            && (!path_prefix.starts_with('/') || path_prefix.ends_with('/'))
        {
            return Err(MediaError::Config(format!(
                "serve-via-app path_prefix {path_prefix:?} must start with '/' and not end with '/'"
            )));
        }
        let inner: Arc<dyn ObjectStore> = match &cfg.backend {
            Backend::Local { root } => {
                std::fs::create_dir_all(root).map_err(|e| {
                    MediaError::Io(format!("create media root {}: {e}", root.display()))
                })?;
                Arc::new(LocalFileSystem::new_with_prefix(root).map_err(|e| {
                    MediaError::Config(format!("local media root {}: {e}", root.display()))
                })?)
            }
            Backend::S3(s3) => Arc::new(build_s3(s3)?),
        };
        Ok(Self {
            inner,
            prefix: cfg.prefix,
            public: cfg.public,
        })
    }

    /// Writes, reads back and deletes a probe object under `<prefix>tmp/`.
    /// The probe is always deleted (best effort), even when the read fails.
    pub async fn check(&self) -> Result<(), MediaError> {
        let key = keys::probe_key(&self.prefix);
        let payload = Bytes::from_static(b"ferrite-media-probe");
        // Nothing to clean up if the put itself failed.
        self.inner.put(&key, payload.clone().into()).await?;
        let result = async {
            let read = self.inner.get(&key).await?.bytes().await?;
            if read != payload {
                return Err(MediaError::Backend(
                    "probe object read back with different content".into(),
                ));
            }
            Ok(())
        }
        .await;
        if let Err(e) = self.inner.delete(&key).await {
            tracing::warn!(key = %key, error = %e, "failed to delete media probe object");
        }
        result
    }

    /// Stores a payload under its SHA-256. Existing artifacts are not rewritten.
    ///
    /// Payloads of 8 MiB or more are delegated to [`MediaStore::put_stream`].
    pub async fn put_artifact(&self, data: Bytes) -> Result<ArtifactRef, MediaError> {
        if data.len() >= SMALL_LIMIT {
            let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(data) });
            return self.put_stream(Box::pin(stream), u64::MAX).await;
        }
        self.put_small(data).await
    }

    /// Stores a stream under its SHA-256 without holding it in memory.
    ///
    /// The first 8 MiB are buffered: a stream ending below that takes the direct small path.
    /// Otherwise the payload is uploaded as a multipart upload to `<prefix>tmp/<uuid>` (8 MiB
    /// parts), hashed on the way, then placed at the final key with `copy_if_not_exists`
    /// (falling back to head + copy where unsupported) and the staging object is deleted.
    ///
    /// `max_bytes` caps the payload: as soon as the running total exceeds it the call fails
    /// with [`MediaError::TooLarge`] (exactly `max_bytes` is accepted) and nothing is kept.
    /// On any error the multipart upload is aborted / the staging object deleted (best effort).
    pub async fn put_stream<S>(
        &self,
        mut stream: S,
        max_bytes: u64,
    ) -> Result<ArtifactRef, MediaError>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin,
    {
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut buf: Vec<u8> = Vec::new();
        let mut ended = false;
        while buf.len() < SMALL_LIMIT {
            match stream.next().await {
                Some(chunk) => {
                    let chunk = chunk.map_err(|e| MediaError::Io(e.to_string()))?;
                    if buf.len() as u64 + chunk.len() as u64 > max_bytes {
                        return Err(MediaError::TooLarge { limit: max_bytes });
                    }
                    hasher.update(&chunk);
                    buf.extend_from_slice(&chunk);
                }
                None => {
                    ended = true;
                    break;
                }
            }
        }
        total += buf.len() as u64;
        let mime = sniff_mime(&buf[..buf.len().min(SNIFF_LEN)]);
        if ended {
            let hash = finish_hash(hasher)?;
            return self.put_small_hashed(Bytes::from(buf), hash, mime).await;
        }

        let tmp = keys::tmp_key(&self.prefix);
        let upload = self.inner.put_multipart(&tmp).await?;
        let mut guard = StagingGuard {
            store: self.inner.clone(),
            tmp,
            writer: Some(WriteMultipart::new_with_chunk_size(upload, PART_SIZE)),
            armed: true,
        };
        let result = async {
            let writer = guard.writer_mut()?;
            writer.wait_for_capacity(PART_CONCURRENCY).await?;
            writer.write(&buf);
            drop(std::mem::take(&mut buf));
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| MediaError::Io(e.to_string()))?;
                if total.saturating_add(chunk.len() as u64) > max_bytes {
                    return Err(MediaError::TooLarge { limit: max_bytes });
                }
                hasher.update(&chunk);
                total += chunk.len() as u64;
                let writer = guard.writer_mut()?;
                writer.write(&chunk);
                writer.wait_for_capacity(PART_CONCURRENCY).await?;
            }
            // Hash before finishing so a failure here still aborts the upload.
            finish_hash(hasher)
        }
        .await;
        let hash = match result {
            Ok(hash) => hash,
            Err(e) => {
                guard.abort_and_cleanup().await;
                return Err(e);
            }
        };
        if let Some(writer) = guard.writer.take()
            && let Err(e) = writer.finish().await
        {
            guard.abort_and_cleanup().await;
            return Err(e.into());
        }
        let placed = self.place_staged(&guard.tmp, &hash).await;
        guard.abort_and_cleanup().await;
        Ok(ArtifactRef {
            hash,
            bytes: total,
            mime,
            created: placed?,
        })
    }

    /// Moves the staged object to its final key. Returns `true` if this call created it.
    async fn place_staged(&self, tmp: &Path, hash: &ArtifactHash) -> Result<bool, MediaError> {
        let key = keys::artifact_key(&self.prefix, hash);
        match self.inner.copy_if_not_exists(tmp, &key).await {
            Ok(()) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
            Err(
                object_store::Error::NotImplemented { .. }
                | object_store::Error::NotSupported { .. },
            ) => {
                // Content-addressed: an existing object under this key has identical bytes.
                if self.head(hash).await?.is_some() {
                    return Ok(false);
                }
                match self.inner.copy(tmp, &key).await {
                    Ok(()) => Ok(true),
                    Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `created` is decided by a head check followed by a conditional create
    /// (`PutMode::Create`, i.e. `If-None-Match: *` on S3, link-based on local disk), so two
    /// racing writers of the same bytes cannot both report `created == true`. Backends without
    /// conditional put fall back to a plain put after the head check; there the race window
    /// between head and put can yield `created == true` twice for identical content (harmless:
    /// the bytes are identical).
    async fn put_small(&self, data: Bytes) -> Result<ArtifactRef, MediaError> {
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hash = finish_hash(hasher)?;
        let mime = sniff_mime(&data[..data.len().min(SNIFF_LEN)]);
        self.put_small_hashed(data, hash, mime).await
    }

    async fn put_small_hashed(
        &self,
        data: Bytes,
        hash: ArtifactHash,
        mime: String,
    ) -> Result<ArtifactRef, MediaError> {
        let bytes = data.len() as u64;
        let key = keys::artifact_key(&self.prefix, &hash);
        let created = if self.head(&hash).await?.is_some() {
            false
        } else {
            let opts = PutOptions::from(PutMode::Create);
            match self.inner.put_opts(&key, data.clone().into(), opts).await {
                Ok(_) => true,
                Err(object_store::Error::AlreadyExists { .. }) => false,
                Err(object_store::Error::NotImplemented { .. }) => {
                    self.inner.put(&key, data.into()).await?;
                    true
                }
                Err(e) => return Err(e.into()),
            }
        };
        Ok(ArtifactRef {
            hash,
            bytes,
            mime,
            created,
        })
    }

    pub async fn get(&self, hash: &ArtifactHash) -> Result<Bytes, MediaError> {
        let key = keys::artifact_key(&self.prefix, hash);
        Ok(self.inner.get(&key).await?.bytes().await?)
    }

    /// Reads `range` (end exclusive). Inverted ranges and ranges past the end are `Range` errors.
    /// Buffered: for small reads. Use [`MediaStore::open_read`] to serve large objects.
    pub async fn get_range(
        &self,
        hash: &ArtifactHash,
        range: Range<u64>,
    ) -> Result<Bytes, MediaError> {
        let size = self.head(hash).await?.ok_or(MediaError::NotFound)?;
        check_range(&range, size)?;
        if range.is_empty() {
            return Ok(Bytes::new());
        }
        let key = keys::artifact_key(&self.prefix, hash);
        Ok(self.inner.get_range(&key, range).await?)
    }

    /// Opens a streaming read of the whole object (`None`) or of `range` (end exclusive).
    ///
    /// One `get_opts` request (with a bounded `GetRange` when a range is given); the total size
    /// comes from the response metadata, not from a separate `head`. Validation matches
    /// [`MediaStore::get_range`]: a missing artifact is `NotFound`, an inverted range or one
    /// past the end is `Range`. Inverted and empty ranges are answered with a metadata-only
    /// request (no body is transferred). The body is not buffered; dropping the stream aborts
    /// the transfer.
    pub async fn open_read(
        &self,
        hash: &ArtifactHash,
        range: Option<Range<u64>>,
    ) -> Result<ReadStream, MediaError> {
        let key = keys::artifact_key(&self.prefix, hash);
        let metadata_only = range.as_ref().is_some_and(|r| r.start >= r.end);
        let mut opts = GetOptions::default();
        if metadata_only {
            opts.head = true;
        } else if let Some(r) = &range {
            opts.range = Some(GetRange::Bounded(r.clone()));
        }
        let res = match self.inner.get_opts(&key, opts).await {
            Ok(res) => res,
            Err(object_store::Error::NotFound { .. }) => return Err(MediaError::NotFound),
            Err(e) => {
                // A backend may reject an unsatisfiable range with its own error; report
                // that as `Range`, like `get_range`, when the object exists and is smaller.
                if let Some(r) = &range
                    && let Ok(Some(size)) = self.head(hash).await
                {
                    check_range(r, size)?;
                }
                return Err(e.into());
            }
        };
        let size = res.meta.size;
        if let Some(r) = &range {
            check_range(r, size)?;
        }
        let served = range.unwrap_or(0..size);
        if metadata_only {
            return Ok(ReadStream {
                size,
                range: served,
                stream: futures::stream::empty().boxed(),
            });
        }
        Ok(ReadStream {
            size,
            range: served,
            stream: res.into_stream().map_err(MediaError::from).boxed(),
        })
    }

    pub async fn head(&self, hash: &ArtifactHash) -> Result<Option<u64>, MediaError> {
        let key = keys::artifact_key(&self.prefix, hash);
        match self.inner.head(&key).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Idempotent: deleting a missing artifact succeeds.
    pub async fn delete(&self, hash: &ArtifactHash) -> Result<(), MediaError> {
        let key = keys::artifact_key(&self.prefix, hash);
        match self.inner.delete(&key).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Lists every object under `<prefix>artifacts/`, recorded in a database or not.
    ///
    /// Staging objects under `tmp/` are not listed. Keys that do not name an artifact (a stray
    /// file in the folder) come back with `hash: None` so callers can report or ignore them.
    /// The stream is lazy and pages through the backend; the order is the backend's.
    pub fn list_artifacts(&self) -> BoxStream<'static, Result<StoredObject, MediaError>> {
        let dir = Path::from(format!("{}artifacts", self.prefix));
        self.inner
            .list(Some(&dir))
            .map_err(MediaError::from)
            .map_ok(|meta| {
                let key = meta.location.to_string();
                StoredObject {
                    hash: hash_of_key(&key),
                    key,
                    size: meta.size,
                    last_modified: Timestamp::from(meta.last_modified),
                }
            })
            .boxed()
    }

    /// Copies one artifact to `dest` under the same key (the same hash): streams the object
    /// out of this store into [`MediaStore::put_stream`] of `dest`, which hashes it on the way,
    /// then checks the size at `dest`. Returns the size in bytes.
    ///
    /// Fails with [`MediaError::NotFound`] when the artifact is missing here, and with
    /// [`MediaError::Backend`] when the bytes do not hash to `hash` (a corrupt source) or the
    /// copy has a different size. In the corrupt case `dest` holds the bytes under their own
    /// (valid) hash, never under `hash`. An object already at `dest` is not rewritten.
    pub async fn copy_to(&self, dest: &MediaStore, hash: &ArtifactHash) -> Result<u64, MediaError> {
        let read = self.open_read(hash, None).await?;
        let size = read.size;
        let stream = read
            .stream
            .map_err(|e| std::io::Error::other(e.to_string()));
        let placed = dest.put_stream(Box::pin(stream), u64::MAX).await?;
        if placed.hash != *hash {
            return Err(MediaError::Backend(format!(
                "source object {hash} holds content with hash {}",
                placed.hash
            )));
        }
        match dest.head(hash).await? {
            Some(copied) if copied == size => Ok(size),
            Some(copied) => Err(MediaError::Backend(format!(
                "copy of {hash} has {copied} bytes, the source {size}"
            ))),
            None => Err(MediaError::Backend(format!(
                "copy of {hash} is missing at the destination"
            ))),
        }
    }

    pub fn public_url(&self, hash: &ArtifactHash, filename: &str) -> String {
        match &self.public {
            PublicUrlMode::ServeViaApp { path_prefix } => {
                // Routable: see `sanitize_filename` (shared with the HTTP layer).
                let filename = crate::sanitize_filename(filename);
                let name = utf8_percent_encode(&filename, SEGMENT);
                format!("{}/{hash}/{name}", path_prefix.trim_end_matches('/'))
            }
            PublicUrlMode::Direct { base_url } => {
                let base = base_url.as_str().trim_end_matches('/');
                format!("{base}/{}", keys::artifact_rel(&self.prefix, hash))
            }
        }
    }
}

/// Owns a staged multipart upload. Dropping it while armed (e.g. the caller cancelled the
/// `put_stream` future) aborts the upload and deletes the staging object in a spawned task.
///
/// Known limitation: if no tokio runtime is available at drop time, or the abort/delete
/// request fails, the staging object / orphan S3 parts stay behind and a warning is logged.
/// An S3 lifecycle rule (abort incomplete multipart uploads, expire `tmp/`) is the backstop.
struct StagingGuard {
    store: Arc<dyn ObjectStore>,
    tmp: Path,
    writer: Option<WriteMultipart>,
    armed: bool,
}

impl StagingGuard {
    fn writer_mut(&mut self) -> Result<&mut WriteMultipart, MediaError> {
        self.writer
            .as_mut()
            .ok_or_else(|| MediaError::Backend("multipart writer already consumed".into()))
    }

    /// Aborts the upload if still open, deletes the staging object, disarms the guard.
    async fn abort_and_cleanup(&mut self) {
        self.armed = false;
        Self::cleanup(self.store.clone(), self.tmp.clone(), self.writer.take()).await;
    }

    async fn cleanup(store: Arc<dyn ObjectStore>, tmp: Path, writer: Option<WriteMultipart>) {
        if let Some(writer) = writer
            && let Err(e) = writer.abort().await
        {
            tracing::warn!(key = %tmp, error = %e, "failed to abort multipart upload");
        }
        match store.delete(&tmp).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => tracing::warn!(key = %tmp, error = %e, "failed to delete staging object"),
        }
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let store = self.store.clone();
        let tmp = self.tmp.clone();
        let writer = self.writer.take();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(Self::cleanup(store, tmp, writer));
            }
            Err(_) => tracing::warn!(
                key = %tmp,
                "upload dropped outside a tokio runtime; staging object left behind"
            ),
        }
    }
}

fn check_range(range: &Range<u64>, size: u64) -> Result<(), MediaError> {
    if range.start > range.end {
        return Err(MediaError::Range(format!(
            "inverted range {}..{}",
            range.start, range.end
        )));
    }
    if range.end > size {
        return Err(MediaError::Range(format!(
            "range {}..{} exceeds object size {size}",
            range.start, range.end
        )));
    }
    Ok(())
}

/// The hash a key names, when it is `…artifacts/<xx>/<64 hex>` with `xx` the hash's first
/// two characters.
fn hash_of_key(key: &str) -> Option<ArtifactHash> {
    let mut parts = key.rsplit('/');
    let name = parts.next()?;
    let dir = parts.next()?;
    if parts.next()? != "artifacts" {
        return None;
    }
    let hash = ArtifactHash::parse(name).ok()?;
    (name.get(..2) == Some(dir)).then_some(hash)
}

/// Checks a `copy_if_not_exists` value with object_store's own parser:
/// `header:<name>:<value>`, `header-with-status:<name>:<value>:<status>` or `multipart`.
/// The `Err` text is fit for a configuration message.
pub fn validate_copy_if_not_exists(value: &str) -> Result<(), String> {
    // The parser is private to object_store and runs when the builder is built.
    AmazonS3Builder::new()
        .with_bucket_name("check")
        .with_region("us-east-1")
        .with_config(AmazonS3ConfigKey::CopyIfNotExists, value)
        .build()
        .map(drop)
        .map_err(|_| {
            "must be \"header:<name>:<value>\", \"header-with-status:<name>:<value>:<status>\" or \"multipart\"".to_owned()
        })
}

fn finish_hash(hasher: Sha256) -> Result<ArtifactHash, MediaError> {
    let hex = ferrite_domain::to_hex(&hasher.finalize());
    ArtifactHash::parse(&hex)
        .map_err(|e| MediaError::Backend(format!("computed hash invalid: {e}")))
}

fn build_s3(cfg: &S3Config) -> Result<object_store::aws::AmazonS3, MediaError> {
    let mut builder = match (&cfg.access_key_id, &cfg.secret_access_key) {
        (None, None) => AmazonS3Builder::from_env(),
        (Some(id), Some(secret)) => AmazonS3Builder::new()
            .with_access_key_id(id)
            .with_secret_access_key(secret.expose()),
        _ => {
            return Err(MediaError::Config(
                "s3 access_key_id and secret_access_key must be set together".into(),
            ));
        }
    };
    builder = builder
        .with_bucket_name(&cfg.bucket)
        .with_region(&cfg.region)
        // Must come before `with_allow_http`: it replaces the whole `ClientOptions`.
        .with_client_options(
            // No total-request timeout: large artifacts stream for as long as they need.
            // Connects must succeed within 5 s; a body read that stalls for 30 s fails.
            ClientOptions::new()
                .with_timeout_disabled()
                .with_connect_timeout(Duration::from_secs(5))
                .with_read_timeout(Duration::from_secs(30)),
        )
        .with_allow_http(cfg.allow_http)
        .with_virtual_hosted_style_request(cfg.virtual_hosted_style)
        .with_retry(RetryConfig {
            max_retries: 2,
            retry_timeout: Duration::from_secs(20),
            ..RetryConfig::default()
        });
    if let Some(endpoint) = &cfg.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    // Only what is set overrides the environment path's `AWS_COPY_IF_NOT_EXISTS` /
    // `AWS_CONDITIONAL_PUT`; the builder's own conditional-put default is `etag`.
    if let Some(mode) = &cfg.copy_if_not_exists {
        builder = builder.with_config(AmazonS3ConfigKey::CopyIfNotExists, mode);
    }
    if cfg.conditional_put == ConditionalPut::Disabled {
        builder = builder.with_config(AmazonS3ConfigKey::ConditionalPut, "disabled");
    }
    builder
        .build()
        .map_err(|e| MediaError::Config(format!("s3 backend: {e}")))
}
