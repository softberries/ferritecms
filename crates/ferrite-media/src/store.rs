use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::{ObjectStore, ObjectStoreExt, RetryConfig};

use crate::config::{Backend, MediaConfig, PublicUrlMode, S3Config};
use crate::error::MediaError;
use crate::keys;

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
    pub async fn check(&self) -> Result<(), MediaError> {
        let key = keys::probe_key(&self.prefix);
        let payload = Bytes::from_static(b"ferrite-media-probe");
        self.inner.put(&key, payload.clone().into()).await?;
        let read = self.inner.get(&key).await?.bytes().await?;
        self.inner.delete(&key).await?;
        if read != payload {
            return Err(MediaError::Backend(
                "probe object read back with different content".into(),
            ));
        }
        Ok(())
    }

    pub fn public_mode(&self) -> &PublicUrlMode {
        &self.public
    }
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
