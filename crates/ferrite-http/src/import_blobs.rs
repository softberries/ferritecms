//! Import report storage (M3-2 P29): [`MediaArtifacts`], core's
//! [`ArtifactBlobs`](ferrite_core::import::ArtifactBlobs) over the site's [`MediaStore`], so
//! a report written by the node that ran the job can be read from any node (local or S3).

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_core::CoreError;
use ferrite_core::import::ArtifactBlobs;
use ferrite_domain::ArtifactHash;
use ferrite_media::MediaStore;

/// [`ArtifactBlobs`] over a [`MediaStore`]: content-addressed, idempotent writes; a read
/// checks the size first and never reads more than `max` bytes.
pub struct MediaArtifacts(Arc<MediaStore>);

impl MediaArtifacts {
    pub fn new(media: Arc<MediaStore>) -> Self {
        Self(media)
    }
}

#[async_trait]
impl ArtifactBlobs for MediaArtifacts {
    async fn put(&self, data: Bytes) -> Result<ArtifactHash, CoreError> {
        match self.0.put_artifact(data).await {
            Ok(stored) => Ok(stored.hash),
            Err(e) => {
                tracing::warn!(error = %e, "storing an import artifact failed");
                Err(CoreError::Internal("media store"))
            }
        }
    }

    async fn get(&self, hash: &ArtifactHash, max: u64) -> Result<Bytes, CoreError> {
        let size = match self.0.head(hash).await {
            Ok(size) => size,
            Err(e) => {
                tracing::warn!(error = %e, "reading an import artifact's size failed");
                return Err(CoreError::Internal("media store"));
            }
        };
        match size {
            None => Err(CoreError::NotFound),
            Some(size) if size > max => Err(CoreError::Internal("import artifact too large")),
            Some(_) => self.0.get(hash).await.map_err(|e| {
                tracing::warn!(error = %e, "reading an import artifact failed");
                CoreError::Internal("media store")
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use ferrite_media::{Backend, MediaConfig, PublicUrlMode};

    use super::*;

    #[tokio::test]
    async fn round_trips_and_bounds_reads() {
        let dir = tempfile::tempdir().unwrap();
        let media = MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: dir.path().join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap();
        let blobs = MediaArtifacts::new(Arc::new(media));
        let hash = blobs.put(Bytes::from_static(b"{\"v\":1}")).await.unwrap();
        assert_eq!(blobs.get(&hash, 64).await.unwrap(), &b"{\"v\":1}"[..]);
        assert!(matches!(
            blobs.get(&hash, 3).await,
            Err(CoreError::Internal(_))
        ));
        let missing = blobs.put(Bytes::from_static(b"other")).await.unwrap();
        blobs.0.delete(&missing).await.unwrap();
        assert!(matches!(
            blobs.get(&missing, 64).await,
            Err(CoreError::NotFound)
        ));
    }
}
