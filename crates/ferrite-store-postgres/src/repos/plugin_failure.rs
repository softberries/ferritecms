//! Shared plugin failure counters (P20) on Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{FailureBucket, Timestamp};
use ferrite_store::{
    PluginFailureRepo, StoreError, check_failure_count, check_prune_limit, validate_text,
};

use super::PgRepos;
use crate::exec::run;

#[async_trait]
impl PluginFailureRepo for PgRepos {
    async fn add(
        &self,
        plugin: &str,
        hook: &str,
        bucket_start: Timestamp,
        count: u32,
    ) -> Result<(), StoreError> {
        validate_text("plugin failure plugin", plugin)?;
        validate_text("plugin failure hook", hook)?;
        check_failure_count(count)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO plugin_failures (plugin, hook, bucket_start, count) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (plugin, hook, bucket_start) \
             DO UPDATE SET count = plugin_failures.count + excluded.count"
        )
        .bind(plugin)
        .bind(hook)
        .bind(DateTime::<Utc>::from(bucket_start))
        .bind(i64::from(count))
        .execute(e))?;
        Ok(())
    }

    async fn sum_since(&self, plugin: &str, since: Timestamp) -> Result<u64, StoreError> {
        validate_text("plugin failure plugin", plugin)?;
        let sum: i64 = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT COALESCE(SUM(count), 0)::bigint FROM plugin_failures \
             WHERE plugin = $1 AND bucket_start >= $2"
        )
        .bind(plugin)
        .bind(DateTime::<Utc>::from(since))
        .fetch_one(e))?;
        u64::try_from(sum).map_err(|_| StoreError::Database(format!("corrupt failure sum {sum}")))
    }

    async fn buckets(
        &self,
        plugin: &str,
        since: Timestamp,
        limit: u32,
    ) -> Result<Vec<FailureBucket>, StoreError> {
        validate_text("plugin failure plugin", plugin)?;
        let limit = check_prune_limit(limit)?;
        // `date_bin` from the epoch: hours independent of the session time zone.
        let rows: Vec<(String, DateTime<Utc>, i64)> = run!(&self.exec, |e| {
            sqlx::query_as(
            "SELECT hook, date_bin('1 hour', bucket_start, TIMESTAMPTZ '1970-01-01 00:00:00+00') \
               AS hour, SUM(count)::bigint \
             FROM plugin_failures WHERE plugin = $1 AND bucket_start >= $2 \
             GROUP BY hook, hour ORDER BY hour DESC, hook LIMIT $3"
        )
        .bind(plugin)
        .bind(DateTime::<Utc>::from(since))
        .bind(limit)
        .fetch_all(e)
        })?;
        rows.into_iter()
            .map(|(hook, hour, count)| {
                Ok(FailureBucket {
                    hook,
                    bucket_start: Timestamp::from(hour),
                    count: u64::try_from(count).map_err(|_| {
                        StoreError::Database(format!("corrupt failure sum {count}"))
                    })?,
                })
            })
            .collect()
    }

    async fn clear(&self, plugin: &str) -> Result<(), StoreError> {
        validate_text("plugin failure plugin", plugin)?;
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM plugin_failures WHERE plugin = $1"
        )
        .bind(plugin)
        .execute(e))?;
        Ok(())
    }

    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM plugin_failures WHERE (plugin, hook, bucket_start) IN \
             (SELECT plugin, hook, bucket_start FROM plugin_failures \
              WHERE bucket_start < $1 LIMIT $2)"
        )
        .bind(DateTime::<Utc>::from(before))
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
