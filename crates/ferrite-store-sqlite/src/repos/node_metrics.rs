//! Per-node minute metrics (M4-2 P35) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{MetricCounts, NodeId, SeriesBucket, SeriesStep, Timestamp};
use ferrite_store::{
    NodeMetricsRepo, StoreError, check_node_metrics, check_prune_limit, check_series_range,
    series_bucket_from_db,
};

use super::SqliteRepos;
use crate::exec::run;

/// A bucket row: start (micros), the nine sums, the largest p95.
type BucketRow = (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64);

/// The series select over `[?2, ?3)` in buckets of `?1` micros, then the node filter
/// (`?4`) when given, then the grouping.
macro_rules! series_sql {
    ($($filter:literal)?) => {
        concat!(
            "SELECT minute / ?1 * ?1 AS bucket, SUM(requests), SUM(errors_5xx), ",
            "SUM(page_views), SUM(cache_hits), SUM(cache_misses), SUM(plugin_calls), ",
            "SUM(plugin_failures), SUM(jobs_done), SUM(jobs_failed), MAX(p95_ms) ",
            "FROM node_metrics WHERE minute >= ?2 AND minute < ?3",
            $($filter,)?
            " GROUP BY bucket ORDER BY bucket"
        )
    };
}

const SERIES_SQL: &str = series_sql!();
const SERIES_BY_NODE_SQL: &str = series_sql!(" AND node_id = ?4");

#[async_trait]
impl NodeMetricsRepo for SqliteRepos {
    async fn insert(
        &self,
        node: NodeId,
        minute: Timestamp,
        counts: &MetricCounts,
    ) -> Result<(), StoreError> {
        let (c, p95) = check_node_metrics(minute, counts)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO node_metrics (node_id, minute, requests, errors_5xx, page_views, \
             cache_hits, cache_misses, plugin_calls, plugin_failures, jobs_done, \
             jobs_failed, p95_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (node_id, minute) DO UPDATE SET \
             requests = node_metrics.requests + excluded.requests, \
             errors_5xx = node_metrics.errors_5xx + excluded.errors_5xx, \
             page_views = node_metrics.page_views + excluded.page_views, \
             cache_hits = node_metrics.cache_hits + excluded.cache_hits, \
             cache_misses = node_metrics.cache_misses + excluded.cache_misses, \
             plugin_calls = node_metrics.plugin_calls + excluded.plugin_calls, \
             plugin_failures = node_metrics.plugin_failures + excluded.plugin_failures, \
             jobs_done = node_metrics.jobs_done + excluded.jobs_done, \
             jobs_failed = node_metrics.jobs_failed + excluded.jobs_failed, \
             p95_ms = MAX(node_metrics.p95_ms, excluded.p95_ms)"
        )
        .bind(node.as_uuid())
        .bind(minute.as_micros())
        .bind(c[0])
        .bind(c[1])
        .bind(c[2])
        .bind(c[3])
        .bind(c[4])
        .bind(c[5])
        .bind(c[6])
        .bind(c[7])
        .bind(c[8])
        .bind(p95)
        .execute(e))?;
        Ok(())
    }

    async fn series(
        &self,
        from: Timestamp,
        to: Timestamp,
        step: SeriesStep,
        node: Option<NodeId>,
    ) -> Result<Vec<SeriesBucket>, StoreError> {
        check_series_range(from, to)?;
        // Minutes are never before 1970 (`check_node_metrics` refuses them), so integer division
        // floors.
        let sql = if node.is_some() {
            SERIES_BY_NODE_SQL
        } else {
            SERIES_SQL
        };
        let rows: Vec<BucketRow> = run!(&self.reader, |e| {
            let q = sqlx::query_as::<_, BucketRow>(sql)
                .bind(step.micros())
                .bind(from.as_micros())
                .bind(to.as_micros());
            match node {
                Some(n) => q.bind(n.as_uuid()),
                None => q,
            }
            .fetch_all(e)
        })?;
        rows.into_iter()
            .map(|(start, a, b, c, d, f, g, h, i, j, p95)| {
                series_bucket_from_db(
                    Timestamp::from_micros(start)?,
                    [a, b, c, d, f, g, h, i, j],
                    p95,
                )
            })
            .collect()
    }

    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM node_metrics WHERE rowid IN \
             (SELECT rowid FROM node_metrics WHERE minute < ? LIMIT ?)"
        )
        .bind(before.as_micros())
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
