//! `artifacts.gc` (M4-1 P12): the daily artifacts GC pass ([`crate::gc::run`]). It lives
//! here, not in `ferrite-cluster`'s built-ins, because it needs the media store.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_domain::{Job, Timestamp};
use ferrite_media::MediaStore;
use ferrite_store::Store;
use serde_json::json;

use crate::gc::{self, GcOptions};

/// The job kind.
pub const ARTIFACTS_GC_KIND: &str = "artifacts.gc";
/// How often the pass is scheduled.
pub const ARTIFACTS_GC_EVERY: Duration = Duration::from_secs(86_400);
/// Longest one pass may take; a pass cut short continues the next day.
pub const ARTIFACTS_GC_TIMEOUT: Duration = Duration::from_secs(3_600);

/// `artifacts.gc`: one GC pass with the node's grace period (`media.gc_grace_days`).
/// Another pass holding the `artifacts.gc` lock (a `ferrite media gc`, or this job on
/// another node) is [`JobError::Busy`]; a store or media error is `Retry`.
pub struct ArtifactsGcHandler {
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
    grace: Duration,
}

impl ArtifactsGcHandler {
    pub fn new(store: Arc<dyn Store>, media: Arc<MediaStore>, grace: Duration) -> Self {
        Self {
            store,
            media,
            grace,
        }
    }
}

#[async_trait]
impl JobHandler for ArtifactsGcHandler {
    fn kind(&self) -> &'static str {
        ARTIFACTS_GC_KIND
    }

    fn timeout(&self) -> Duration {
        ARTIFACTS_GC_TIMEOUT
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let opts = GcOptions {
            grace: self.grace,
            dry_run: false,
            now: Timestamp::now(),
        };
        match gc::run(self.store.as_ref(), &self.media, &opts).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(JobError::Busy("another artifacts GC is running".into())),
            Err(e) => Err(JobError::Retry(format!("{e:#}"))),
        }
    }
}

/// The [`ARTIFACTS_GC_KIND`] periodic, every [`ARTIFACTS_GC_EVERY`].
pub fn artifacts_gc_periodic() -> Periodic {
    Periodic {
        kind: ARTIFACTS_GC_KIND,
        every: ARTIFACTS_GC_EVERY,
        payload: json!({"v": 1}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
    }
}
