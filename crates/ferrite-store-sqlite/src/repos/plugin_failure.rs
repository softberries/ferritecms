//! Shared plugin failure counters (P20) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{FailureBucket, Timestamp};
use ferrite_store::{
    PluginFailureRepo, StoreError, check_failure_count, check_prune_limit, validate_text,
};

use super::SqliteRepos;
use crate::exec::run;

#[async_trait]
impl PluginFailureRepo for SqliteRepos {
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
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO plugin_failures (plugin, hook, bucket_start, count) \
             VALUES (?, ?, ?, ?) ON CONFLICT (plugin, hook, bucket_start) \
             DO UPDATE SET count = plugin_failures.count + excluded.count"
        )
        .bind(plugin)
        .bind(hook)
        .bind(bucket_start.as_micros())
        .bind(i64::from(count))
        .execute(e))?;
        Ok(())
    }

    async fn sum_since(&self, plugin: &str, since: Timestamp) -> Result<u64, StoreError> {
        validate_text("plugin failure plugin", plugin)?;
        let sum: i64 = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT COALESCE(SUM(count), 0) FROM plugin_failures \
             WHERE plugin = ? AND bucket_start >= ?"
        )
        .bind(plugin)
        .bind(since.as_micros())
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
        // Hours aligned to the epoch; failure buckets are never before 1970.
        let rows: Vec<(String, i64, i64)> = run!(&self.reader, |e| sqlx::query_as(
            "SELECT hook, bucket_start / 3600000000 * 3600000000 AS hour, SUM(count) \
             FROM plugin_failures WHERE plugin = ? AND bucket_start >= ? \
             GROUP BY hook, hour ORDER BY hour DESC, hook LIMIT ?"
        )
        .bind(plugin)
        .bind(since.as_micros())
        .bind(limit)
        .fetch_all(e))?;
        rows.into_iter()
            .map(|(hook, hour, count)| {
                Ok(FailureBucket {
                    hook,
                    bucket_start: Timestamp::from_micros(hour)?,
                    count: u64::try_from(count).map_err(|_| {
                        StoreError::Database(format!("corrupt failure sum {count}"))
                    })?,
                })
            })
            .collect()
    }

    async fn clear(&self, plugin: &str) -> Result<(), StoreError> {
        validate_text("plugin failure plugin", plugin)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugin_failures WHERE plugin = ?"
        )
        .bind(plugin)
        .execute(e))?;
        Ok(())
    }

    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM plugin_failures WHERE rowid IN \
             (SELECT rowid FROM plugin_failures WHERE bucket_start < ? LIMIT ?)"
        )
        .bind(before.as_micros())
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
