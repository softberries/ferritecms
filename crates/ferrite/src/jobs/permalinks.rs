//! `permalink.redirects` (M3-2 P30): redirects for a `permalink.*` structure change.

use std::sync::Arc;

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler};
use ferrite_core::{
    CoreError, RedirectService,
    jobs::{PERMALINK_REDIRECTS_KIND, PERMALINK_REDIRECTS_VERSION, PermalinkRedirects},
};
use ferrite_domain::Job;

/// `permalink.redirects`: redirects every published post and page from its path under
/// the payload's previous structures to its path under the stored ones
/// ([`RedirectService::structure_change`], 500 items per transaction). A payload that
/// does not parse, or whose previous structures are invalid, is `Permanent`; a store
/// error is retried, and the pages done before it are simply rewritten by the retry.
pub struct PermalinkRedirectsHandler {
    pub redirects: Arc<RedirectService>,
}

#[async_trait]
impl JobHandler for PermalinkRedirectsHandler {
    fn kind(&self) -> &'static str {
        PERMALINK_REDIRECTS_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[PERMALINK_REDIRECTS_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: PermalinkRedirects = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let report = self
            .redirects
            .structure_change(&payload)
            .await
            .map_err(|e| match e {
                CoreError::Invalid(_) | CoreError::InvalidField { .. } => {
                    JobError::Permanent(e.to_string())
                }
                other => JobError::Retry(other.to_string()),
            })?;
        tracing::info!(
            version = payload.version,
            moved = report.moved,
            kept = report.kept,
            refused = report.refused,
            "permalink structure redirects written"
        );
        Ok(())
    }
}
