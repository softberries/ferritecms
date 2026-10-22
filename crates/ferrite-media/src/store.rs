use std::sync::Arc;
use std::time::Duration;

use std::ops::Range;

use bytes::Bytes;
use ferrite_domain::ArtifactHash;
use futures::{Stream, StreamExt};
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, RetryConfig, WriteMultipart};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::config::{Backend, MediaConfig, PublicUrlMode, S3Config};
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

pub struct MediaStore {
    inner: Arc<dyn ObjectStore>,
    prefix: String,
    public: PublicUrlMode,
}

impl MediaStore {
    pub fn open(cfg: MediaConfig) -> Result<Self, MediaError> {
        keys::validate_prefix(&cfg.prefix)?;
        let inner: Arc<dyn ObjectStore> = match &cfg.backend {
            Backend::Local { root } => {
                std::fs::create_dir_all(root).map_err(|e| {
                    MediaError::Io(format!("create media root {}: {e}", root.display()))
                })?;
                Arc::new(LocalFileSystem::new_with_prefix(root)?)
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
            return self.put_stream(Box::pin(stream)).await;
        }
        self.put_small(data).await
    }

    /// Stores a stream under its SHA-256 without holding it in memory.
    ///
    /// The first 8 MiB are buffered: a stream ending below that takes the direct small path.
    /// Otherwise the payload is uploaded as a multipart upload to `<prefix>tmp/<uuid>` (8 MiB
    /// parts), hashed on the way, then placed at the final key with `copy_if_not_exists`
    /// (falling back to head + copy where unsupported) and the staging object is deleted.
    /// On any error the multipart upload is aborted / the staging object deleted (best effort).
    pub async fn put_stream<S>(&self, mut stream: S) -> Result<ArtifactRef, MediaError>
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
        let mut writer = WriteMultipart::new_with_chunk_size(upload, PART_SIZE);
        let result = async {
            writer.wait_for_capacity(PART_CONCURRENCY).await?;
            writer.write(&buf);
            drop(std::mem::take(&mut buf));
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| MediaError::Io(e.to_string()))?;
                hasher.update(&chunk);
                total += chunk.len() as u64;
                writer.write(&chunk);
                writer.wait_for_capacity(PART_CONCURRENCY).await?;
            }
            Ok::<(), MediaError>(())
        }
        .await;
        if let Err(e) = result {
            if let Err(abort_err) = writer.abort().await {
                tracing::warn!(key = %tmp, error = %abort_err, "failed to abort multipart upload");
            }
            self.cleanup_tmp(&tmp).await;
            return Err(e);
        }
        if let Err(e) = writer.finish().await {
            self.cleanup_tmp(&tmp).await;
            return Err(e.into());
        }
        let hash = finish_hash(hasher)?;
        let placed = self.place_staged(&tmp, &hash).await;
        self.cleanup_tmp(&tmp).await;
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

    async fn cleanup_tmp(&self, tmp: &Path) {
        match self.inner.delete(tmp).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => tracing::warn!(key = %tmp, error = %e, "failed to delete staging object"),
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
    pub async fn get_range(
        &self,
        hash: &ArtifactHash,
        range: Range<u64>,
    ) -> Result<Bytes, MediaError> {
        let size = self.head(hash).await?.ok_or(MediaError::NotFound)?;
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
        if range.is_empty() {
            return Ok(Bytes::new());
        }
        let key = keys::artifact_key(&self.prefix, hash);
        Ok(self.inner.get_range(&key, range).await?)
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

    pub fn public_url(&self, hash: &ArtifactHash, filename: &str) -> String {
        match &self.public {
            PublicUrlMode::ServeViaApp { path_prefix } => {
                // "", "." and ".." would be normalised away by clients and drop the hash segment.
                let filename = match filename {
                    "" | "." | ".." => "file",
                    other => other,
                };
                let name = utf8_percent_encode(filename, SEGMENT);
                format!("{}/{hash}/{name}", path_prefix.trim_end_matches('/'))
            }
            PublicUrlMode::Direct { base_url } => {
                let base = base_url.as_str().trim_end_matches('/');
                format!("{base}/{}", keys::artifact_rel(&self.prefix, hash))
            }
        }
    }
}

fn finish_hash(hasher: Sha256) -> Result<ArtifactHash, MediaError> {
    use std::fmt::Write;
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest.iter() {
        // Writing to a String cannot fail.
        let _ = write!(hex, "{b:02x}");
    }
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
    Ok(builder.build()?)
}
