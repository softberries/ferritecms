//! `content.health` (M6-4, design 16 §6.1): one content health run
//! ([`HealthRunner::run`]), daily at 04:00 UTC and on "Run now". Exclusive through the
//! `content.health` lock: a run that finds it held goes back to the queue
//! ([`JobError::Busy`], no attempt used).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_core::{
    CoreError,
    jobs::{
        CONTENT_HEALTH_EVERY, CONTENT_HEALTH_KIND, CONTENT_HEALTH_OFFSET, CONTENT_HEALTH_TIMEOUT,
        CONTENT_HEALTH_VERSION, ContentHealth,
    },
    quality::HealthRunner,
};
use ferrite_domain::{HealthTrigger, Job};
use ferrite_store::{LOCK_HEALTH, Store};
use serde_json::json;

/// See the module docs.
pub struct ContentHealthHandler {
    pub runner: Arc<HealthRunner>,
    pub store: Arc<dyn Store>,
}

#[async_trait]
impl JobHandler for ContentHealthHandler {
    fn kind(&self) -> &'static str {
        CONTENT_HEALTH_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[CONTENT_HEALTH_VERSION]
    }

    fn timeout(&self) -> Duration {
        CONTENT_HEALTH_TIMEOUT
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: ContentHealth = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let Some(guard) = self
            .store
            .try_lock(LOCK_HEALTH)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?
        else {
            return Err(JobError::Busy(
                "another content health run is going on".into(),
            ));
        };
        // A little before the job's own timeout, so the run ends as `done`.
        let deadline = Instant::now() + CONTENT_HEALTH_TIMEOUT - Duration::from_secs(60);
        let result = self
            .runner
            .run(payload.trigger, payload.user, deadline)
            .await;
        drop(guard);
        match result {
            Ok(run) => {
                tracing::info!(
                    run = %run.id,
                    items = run.counters.items,
                    truncated = run.truncated,
                    "content health run done"
                );
                Ok(())
            }
            Err(e @ (CoreError::Invalid(_) | CoreError::InvalidField { .. })) => {
                Err(JobError::Permanent(e.to_string()))
            }
            Err(e) => Err(JobError::Retry(e.to_string())),
        }
    }
}

/// The daily periodic run.
pub fn content_health_periodic() -> Periodic {
    Periodic {
        kind: CONTENT_HEALTH_KIND,
        every: CONTENT_HEALTH_EVERY,
        payload: json!({"v": CONTENT_HEALTH_VERSION, "trigger": HealthTrigger::Schedule.as_str(), "user": null}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
        offset: CONTENT_HEALTH_OFFSET,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn periodic_is_daily_at_0400_and_fits() {
        let p = content_health_periodic();
        assert_eq!(p.every, Duration::from_secs(86_400));
        assert_eq!(p.offset, Duration::from_secs(4 * 3600));
        let payload: ContentHealth = serde_json::from_value(p.payload).unwrap();
        assert_eq!(payload.trigger, HealthTrigger::Schedule);
        assert_eq!(payload.user, None);
        assert!(CONTENT_HEALTH_TIMEOUT < p.every);
    }

    /// A run that finds the lock held goes back to the queue without an attempt.
    #[tokio::test]
    async fn busy_lock_requeues_without_attempt() {
        use ferrite_core::quality::{PathProbe, PathStatus};
        struct NoProbe;
        #[async_trait]
        impl PathProbe for NoProbe {
            async fn probe(&self, _: &str) -> PathStatus {
                PathStatus::Unchecked
            }
        }
        let db = ferrite_store_tests::TestDb::sqlite().await;
        let store = db.shared_store();
        let handler = ContentHealthHandler {
            runner: Arc::new(HealthRunner::new(
                store.clone(),
                "https://example.com".into(),
                Arc::new(NoProbe),
                None,
                "n".into(),
            )),
            store: store.clone(),
        };
        let job = ferrite_domain::Job {
            payload: json!({"v": 1, "trigger": "manual", "user": null}),
            ..test_job()
        };
        let held = store.try_lock(LOCK_HEALTH).await.unwrap().unwrap();
        assert!(matches!(handler.run(&job).await, Err(JobError::Busy(_))));
        drop(held);
        handler.run(&job).await.unwrap();
        assert!(
            store
                .read()
                .quality()
                .latest_run(ferrite_domain::HealthRunStatus::Done)
                .await
                .unwrap()
                .is_some()
        );
        let bad = ferrite_domain::Job {
            payload: json!({"v": 1, "trigger": "sometimes"}),
            ..test_job()
        };
        assert!(matches!(
            handler.run(&bad).await,
            Err(JobError::Permanent(_))
        ));
        db.teardown().await;
    }

    fn test_job() -> ferrite_domain::Job {
        let now = ferrite_domain::Timestamp::now();
        ferrite_domain::Job {
            id: ferrite_domain::JobId::new(),
            kind: CONTENT_HEALTH_KIND.into(),
            payload: json!({}),
            unique_key: None,
            run_at: now,
            priority: 0,
            status: ferrite_domain::JobStatus::Running,
            attempts: 1,
            max_attempts: 3,
            locked_by: None,
            locked_until: None,
            last_error: None,
            created_at: now,
            finished_at: None,
        }
    }
}
