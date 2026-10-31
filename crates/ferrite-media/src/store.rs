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

/// One listed object: under `artifacts/` ([`MediaStore::list_artifacts`]) or `tmp/`
/// ([`MediaStore::list_tmp`], always with `hash: None`).
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

/// The name of the owner marker under the store's prefix (M4-1 R16).
pub const OWNER_MARKER: &str = "ferrite-owner.json";
/// The largest owner marker read (it holds two ids and a time).
const MAX_OWNER_BYTES: u64 = 4096;

/// Which site installation owns a media store (M4-1 R16), kept at
/// `<prefix>ferrite-owner.json`. The artifacts GC deletes from a store only on a pass that
/// reads this naming its own install (`schema_meta.install_id`), so a site that does not
/// own the store never deletes from it. The owner still deletes whatever its own database
/// does not reference, a copy's uploads included: a second site needs its own store.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnerMarker {
    /// The owning database's `schema_meta.install_id`.
    pub install_id: String,
    /// Its `schema_meta.site_id` (informative: copies of one site share it).
    pub site_id: String,
    /// When the marker was written, RFC 3339.
    pub claimed_at: String,
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
    ///
    /// An `AlreadyExists` answer is checked with a `head` of the final key (R18): a wrong
    /// `copy_if_not_exists` header can make a store answer 412 for a reason other than an
    /// existing destination, and taking that as "already stored" would lose the upload.
    async fn place_staged(&self, tmp: &Path, hash: &ArtifactHash) -> Result<bool, MediaError> {
        let key = keys::artifact_key(&self.prefix, hash);
        match self.inner.copy_if_not_exists(tmp, &key).await {
            Ok(()) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. }) => self.placed_by_another(hash).await,
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
                    Err(object_store::Error::AlreadyExists { .. }) => {
                        self.placed_by_another(hash).await
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// After the store answered that the final key exists: `Ok(false)` when it does, an
    /// error when it does not (the answer was a precondition failure of something else).
    async fn placed_by_another(&self, hash: &ArtifactHash) -> Result<bool, MediaError> {
        match self.head(hash).await? {
            Some(_) => Ok(false),
            None => Err(MediaError::Backend(format!(
                "the store refused to place {hash} as already stored, but there is no object \
                 at its key; check media.s3.copy_if_not_exists (09 §4a)"
            ))),
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
        self.list_dir("artifacts", hash_of_key)
    }

    /// Lists every staging object under `<prefix>tmp/` (streaming uploads and `check`
    /// probes left behind), each with `hash: None`. Lazy, in the backend's order. The
    /// artifacts GC deletes the old ones with [`MediaStore::delete_key`].
    pub fn list_tmp(&self) -> BoxStream<'static, Result<StoredObject, MediaError>> {
        self.list_dir("tmp", |_| None)
    }

    fn list_dir(
        &self,
        dir: &str,
        hash: fn(&str) -> Option<ArtifactHash>,
    ) -> BoxStream<'static, Result<StoredObject, MediaError>> {
        let dir = Path::from(format!("{}{dir}", self.prefix));
        self.inner
            .list(Some(&dir))
            .map_err(MediaError::from)
            .map_ok(move |meta| {
                let key = meta.location.to_string();
                StoredObject {
                    hash: hash(&key),
                    key,
                    size: meta.size,
                    last_modified: Timestamp::from(meta.last_modified),
                }
            })
            .boxed()
    }

    /// Deletes one staging object by its full key (as [`MediaStore::list_tmp`] gives it).
    /// Idempotent: a missing object succeeds. Only keys `<prefix>tmp/<…>` (a valid object
    /// path, no `.`/`..` segments) are accepted; any other key is [`MediaError::Config`],
    /// so artifacts are only ever deleted by hash ([`MediaStore::delete`]).
    pub async fn delete_key(&self, key: &str) -> Result<(), MediaError> {
        let tmp = format!("{}tmp/", self.prefix);
        let path = match key.strip_prefix(tmp.as_str()) {
            Some(rest) if !rest.is_empty() => Path::parse(key).ok(),
            _ => None,
        };
        let Some(path) = path.filter(|p| p.as_ref() == key) else {
            return Err(MediaError::Config(format!(
                "{key:?} is not a staging key under {tmp:?}"
            )));
        };
        match self.inner.delete(&path).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
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

    /// Reads the owner marker ([`OwnerMarker`]); `Ok(None)` when there is none. A marker
    /// that is not a marker (bad JSON, over 4 KiB) is [`MediaError::Backend`].
    pub async fn read_owner(&self) -> Result<Option<OwnerMarker>, MediaError> {
        let key = keys::owner_key(&self.prefix);
        let res = match self.inner.get(&key).await {
            Ok(res) => res,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if res.meta.size > MAX_OWNER_BYTES {
            return Err(MediaError::Backend(format!(
                "{key} is {} bytes, not an owner marker",
                res.meta.size
            )));
        }
        let bytes = res.bytes().await?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| MediaError::Backend(format!("{key} is not an owner marker: {e}")))
    }

    /// Writes the owner marker. With `replace` it overwrites any marker and returns `true`;
    /// without, it writes only when there is none (a conditional create; a backend without
    /// conditional put falls back to a check followed by a plain put) and returns `false`
    /// when a marker is already there.
    pub async fn write_owner(
        &self,
        marker: &OwnerMarker,
        replace: bool,
    ) -> Result<bool, MediaError> {
        let key = keys::owner_key(&self.prefix);
        let body = Bytes::from(
            serde_json::to_vec(marker).map_err(|e| MediaError::Backend(e.to_string()))?,
        );
        if replace {
            self.inner.put(&key, body.into()).await?;
            return Ok(true);
        }
        let opts = PutOptions::from(PutMode::Create);
        match self.inner.put_opts(&key, body.clone().into(), opts).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
            Err(object_store::Error::NotImplemented { .. }) => {
                match self.inner.head(&key).await {
                    Ok(_) => return Ok(false),
                    Err(object_store::Error::NotFound { .. }) => {}
                    Err(e) => return Err(e.into()),
                }
                self.inner.put(&key, body.into()).await?;
                Ok(true)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The object store under this media store, for tools that keep other objects next to
    /// the artifacts (a backup repository's snapshots). Keys built from it must start with
    /// [`MediaStore::prefix`].
    pub fn object_store(&self) -> Arc<dyn ObjectStore> {
        self.inner.clone()
    }

    /// The key prefix (empty, or ending with `/`).
    pub fn prefix(&self) -> &str {
        &self.prefix
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
    install_crypto_provider();
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

/// Installs ring as the process's rustls provider unless one is set already. object_store's
/// reqwest client is built without a provider of its own (M4-2 P28: no aws-lc) and panics
/// when the process has none.
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// The HTTP client options of the S3 store. `env` is the environment of the
/// no-credentials path (empty with explicit keys, which read no environment): its
/// `AWS_*` client keys (a proxy, `AWS_ALLOW_HTTP`, HTTP/1 only, pool settings), parsed
/// as `AmazonS3Builder::from_env` parses them, are kept, because
/// `with_client_options` replaces everything `from_env` read. Then our timeouts, which
/// win over the environment's: no total-request timeout (large artifacts stream for as
/// long as they need), connects within 5 s, a body read that stalls for 30 s fails.
/// `allow_http` (`[media.s3] allow_http`) only turns plain HTTP on; `false` leaves the
/// environment's `AWS_ALLOW_HTTP`.
fn s3_client_options(
    env: impl IntoIterator<Item = (String, String)>,
    allow_http: bool,
) -> ClientOptions {
    let mut opts = ClientOptions::new();
    for (key, value) in env {
        if !key.starts_with("AWS_") {
            continue;
        }
        if let Ok(AmazonS3ConfigKey::Client(key)) = key.to_ascii_lowercase().parse() {
            opts = opts.with_config(key, value);
        }
    }
    if allow_http {
        opts = opts.with_allow_http(true);
    }
    opts.with_timeout_disabled()
        .with_connect_timeout(Duration::from_secs(5))
        .with_read_timeout(Duration::from_secs(30))
}

/// The process environment as `AmazonS3Builder::from_env` sees it (UTF-8 pairs only).
fn utf8_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

fn build_s3(cfg: &S3Config) -> Result<object_store::aws::AmazonS3, MediaError> {
    install_crypto_provider();
    let (mut builder, env) = match (&cfg.access_key_id, &cfg.secret_access_key) {
        (None, None) => (AmazonS3Builder::from_env(), utf8_env()),
        (Some(id), Some(secret)) => (
            AmazonS3Builder::new()
                .with_access_key_id(id)
                .with_secret_access_key(secret.expose()),
            Vec::new(),
        ),
        _ => {
            return Err(MediaError::Config(
                "s3 access_key_id and secret_access_key must be set together".into(),
            ));
        }
    };
    builder = builder
        .with_bucket_name(&cfg.bucket)
        .with_region(&cfg.region)
        .with_client_options(s3_client_options(env, cfg.allow_http))
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
    // `AWS_CONDITIONAL_PUT`; unset, the builder's default (`etag`) stands.
    if let Some(mode) = &cfg.copy_if_not_exists {
        builder = builder.with_config(AmazonS3ConfigKey::CopyIfNotExists, mode);
    }
    if let Some(put) = cfg.conditional_put {
        let value = match put {
            ConditionalPut::Etag => "etag",
            ConditionalPut::Disabled => "disabled",
        };
        builder = builder.with_config(AmazonS3ConfigKey::ConditionalPut, value);
    }
    builder
        .build()
        .map_err(|e| MediaError::Config(format!("s3 backend: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use object_store::{
        CopyMode, CopyOptions, GetResult, ListResult, MultipartUpload, ObjectMeta,
        PutMultipartOptions, PutPayload, PutResult,
    };

    /// An in-memory store whose conditional copy always answers `AlreadyExists`, as a
    /// store does that turns a wrong `copy_if_not_exists` header into a 412.
    #[derive(Debug)]
    struct RefusingCopy(InMemory);

    impl std::fmt::Display for RefusingCopy {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("RefusingCopy")
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for RefusingCopy {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            opts: PutOptions,
        ) -> object_store::Result<PutResult> {
            self.0.put_opts(location, payload, opts).await
        }

        async fn put_multipart_opts(
            &self,
            location: &Path,
            opts: PutMultipartOptions,
        ) -> object_store::Result<Box<dyn MultipartUpload>> {
            self.0.put_multipart_opts(location, opts).await
        }

        async fn get_opts(
            &self,
            location: &Path,
            options: GetOptions,
        ) -> object_store::Result<GetResult> {
            self.0.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<Path>>,
        ) -> BoxStream<'static, object_store::Result<Path>> {
            self.0.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
            self.0.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
            self.0.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: CopyOptions,
        ) -> object_store::Result<()> {
            if matches!(options.mode, CopyMode::Create) {
                return Err(object_store::Error::AlreadyExists {
                    path: to.to_string(),
                    source: "412 Precondition Failed".into(),
                });
            }
            self.0.copy_opts(from, to, options).await
        }
    }

    fn refusing() -> MediaStore {
        MediaStore {
            inner: Arc::new(RefusingCopy(InMemory::new())),
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        }
    }

    /// R18: a placement the store refuses although the final key is absent fails loudly
    /// (and leaves no staging object); one whose final key exists is "already stored".
    #[tokio::test]
    async fn refused_placement_without_an_object_fails() {
        let store = refusing();
        let data = Bytes::from(vec![5u8; SMALL_LIMIT + 1]);
        let err = store.put_artifact(data.clone()).await.unwrap_err();
        assert!(
            matches!(&err, MediaError::Backend(m) if m.contains("copy_if_not_exists")),
            "{err:?}"
        );
        let tmp: Vec<StoredObject> = store.list_tmp().try_collect().await.unwrap();
        assert!(tmp.is_empty(), "{tmp:?}");

        // The same bytes already at the final key (stored by another writer): not created.
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hash = finish_hash(hasher).unwrap();
        store
            .inner
            .put(&keys::artifact_key("", &hash), data.clone().into())
            .await
            .unwrap();
        let placed = store.put_artifact(data).await.unwrap();
        assert_eq!(placed.hash, hash);
        assert!(!placed.created);
    }

    /// The S3 client options keep what the environment's `AWS_*` client keys set (as
    /// `AmazonS3Builder::from_env` reads them: a proxy, `AWS_ALLOW_HTTP`, HTTP/1 only),
    /// with our timeouts on top (no total timeout, whatever `AWS_TIMEOUT` says), and
    /// `allow_http` from the config only turns plain HTTP on.
    #[test]
    fn build_s3_keeps_environment_client_options() {
        use object_store::ClientConfigKey as K;
        let env = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let opts = s3_client_options(
            env(&[
                ("AWS_PROXY_URL", "http://proxy.test:3128"),
                ("AWS_ALLOW_HTTP", "true"),
                ("AWS_HTTP1_ONLY", "true"),
                ("AWS_TIMEOUT", "5 seconds"),
                ("AWS_CONNECT_TIMEOUT", "1 minute"),
                ("AWS_BUCKET", "elsewhere"),
                ("PROXY_URL", "http://not-aws.test"),
                ("aws_user_agent", "lower-case keys are not read"),
            ]),
            false,
        );
        let ours = ClientOptions::new()
            .with_connect_timeout(Duration::from_secs(5))
            .with_read_timeout(Duration::from_secs(30));
        assert_eq!(
            opts.get_config_value(&K::ProxyUrl).as_deref(),
            Some("http://proxy.test:3128")
        );
        assert_eq!(
            opts.get_config_value(&K::AllowHttp).as_deref(),
            Some("true")
        );
        assert_eq!(
            opts.get_config_value(&K::Http1Only).as_deref(),
            Some("true")
        );
        assert_eq!(opts.get_config_value(&K::Timeout), None);
        assert_eq!(
            opts.get_config_value(&K::ConnectTimeout),
            ours.get_config_value(&K::ConnectTimeout)
        );
        assert_eq!(
            opts.get_config_value(&K::ReadTimeout),
            ours.get_config_value(&K::ReadTimeout)
        );
        assert_eq!(
            opts.get_config_value(&K::UserAgent),
            ClientOptions::new().get_config_value(&K::UserAgent)
        );

        let plain = s3_client_options(Vec::new(), false);
        assert_eq!(
            plain.get_config_value(&K::AllowHttp).as_deref(),
            Some("false")
        );
        assert_eq!(plain.get_config_value(&K::ProxyUrl), None);
        let http = s3_client_options(env(&[("AWS_ALLOW_HTTP", "false")]), true);
        assert_eq!(
            http.get_config_value(&K::AllowHttp).as_deref(),
            Some("true")
        );
    }
}
