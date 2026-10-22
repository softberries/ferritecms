use std::sync::Arc;
use std::time::Duration;

use std::ops::Range;

use bytes::Bytes;
use ferrite_domain::ArtifactHash;
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, RetryConfig};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::config::{Backend, MediaConfig, PublicUrlMode, S3Config};
use crate::error::MediaError;
use crate::keys;
use crate::sniff::sniff_mime;

/// Payloads at or above this size must go through the streaming path.
const SMALL_LIMIT: usize = 8 * 1024 * 1024;
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

    /// Stores a small payload (< 8 MiB) under its SHA-256. Existing artifacts are not rewritten.
    ///
    /// `created` is decided by a head check followed by a conditional create
    /// (`PutMode::Create`, i.e. `If-None-Match: *` on S3, link-based on local disk), so two
    /// racing writers of the same bytes cannot both report `created == true`. Backends without
    /// conditional put fall back to a plain put after the head check; there the race window
    /// between head and put can yield `created == true` twice for identical content (harmless:
    /// the bytes are identical).
    pub async fn put_artifact(&self, data: Bytes) -> Result<ArtifactRef, MediaError> {
        if data.len() >= SMALL_LIMIT {
            return Err(MediaError::Config(
                "payload too large for put_artifact; use put_stream".into(),
            ));
        }
        let hash = hash_bytes(&data)?;
        let mime = sniff_mime(&data[..data.len().min(SNIFF_LEN)]);
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

fn hash_bytes(data: &[u8]) -> Result<ArtifactHash, MediaError> {
    use std::fmt::Write;
    let digest = Sha256::digest(data);
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
