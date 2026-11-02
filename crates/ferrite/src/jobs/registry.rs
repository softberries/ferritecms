//! `plugins.registry_check` (M6-2, design 14 §8; ruling P5): fetches and verifies the
//! configured registry's index, stores it and applies its revocations. Off by default:
//! the periodic is scheduled only with `[plugins] registry_check = "daily"` and a
//! registry; the Plugins screen and the CLI refresh on demand. In a cluster the queue
//! runs it once.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_domain::Job;
use ferrite_http::extensions::{ExtensionError, Installer};
use serde_json::json;

/// The job kind.
pub const REGISTRY_CHECK_KIND: &str = "plugins.registry_check";
/// How often the check is scheduled when it is on.
pub const REGISTRY_CHECK_EVERY: Duration = Duration::from_secs(86_400);
/// When in the (UTC) day it is due: 04:00, after the artifacts GC.
pub const REGISTRY_CHECK_OFFSET: Duration = Duration::from_secs(4 * 3_600);
/// Longest one check may take: the index and its signature, 30 s each.
pub const REGISTRY_CHECK_TIMEOUT: Duration = Duration::from_secs(120);

/// See the module docs. No registry: done. A network failure: `Retry` (within the job's
/// attempts; the next day's check runs anyway). An index that does not verify or
/// validate, or a rollback: `Permanent` after the error is recorded on the cache row.
pub struct RegistryCheckHandler {
    pub installer: Arc<Installer>,
}

#[async_trait]
impl JobHandler for RegistryCheckHandler {
    fn kind(&self) -> &'static str {
        REGISTRY_CHECK_KIND
    }

    fn timeout(&self) -> Duration {
        REGISTRY_CHECK_TIMEOUT
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        match self.installer.refresh_registry().await {
            Ok(r) => {
                if !r.disabled.is_empty() {
                    tracing::warn!(plugins = ?r.disabled, "a revoked publisher key disabled plugins");
                }
                Ok(())
            }
            Err(ExtensionError::NoRegistry) => Ok(()),
            Err(e @ ExtensionError::RegistryInvalid(_)) => Err(JobError::Permanent(e.to_string())),
            Err(e) => Err(JobError::Retry(e.to_string())),
        }
    }
}

/// The [`REGISTRY_CHECK_KIND`] periodic, every [`REGISTRY_CHECK_EVERY`], due
/// [`REGISTRY_CHECK_OFFSET`] into the day.
pub fn registry_check_periodic() -> Periodic {
    Periodic {
        kind: REGISTRY_CHECK_KIND,
        every: REGISTRY_CHECK_EVERY,
        payload: json!({"v": 1}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
        offset: REGISTRY_CHECK_OFFSET,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_is_daily() {
        let p = registry_check_periodic();
        assert_eq!(p.kind, "plugins.registry_check");
        assert_eq!(p.every, Duration::from_secs(86_400));
        // Not one of the always-on periodics (ruling P5).
        assert!(
            !crate::jobs::core_periodics()
                .iter()
                .any(|c| c.kind == REGISTRY_CHECK_KIND)
        );
    }
}
