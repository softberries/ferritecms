//! Per-node minute metrics (M4-2 P35) on Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{MetricCounts, NodeId, SeriesBucket, SeriesStep, Timestamp};
use ferrite_store::{
    NodeMetricsRepo, StoreError, check_node_metrics, check_prune_limit, check_series_range,
    series_bucket_from_db,
};

use super::PgRepos;
use crate::exec::run;

/// A bucket row: start, the nine sums, the largest p95.
type BucketRow = (
    DateTime<Utc>,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

/// The series select over `[$1, $2)`, in `date_bin` buckets of `$stride` from the
/// epoch (independent of the session time zone), then the node filter (`$3`) when
/// given; sums are cast back from `numeric`.
macro_rules! series_sql {
    ($stride:literal $(, $filter:literal)?) => {
        concat!(
            "SELECT date_bin('", $stride,
            "', minute, TIMESTAMPTZ '1970-01-01 00:00:00+00') AS bucket, ",
            "SUM(requests)::bigint, SUM(errors_5xx)::bigint, SUM(page_views)::bigint, ",
            "SUM(cache_hits)::bigint, SUM(cache_misses)::bigint, SUM(plugin_calls)::bigint, ",
            "SUM(plugin_failures)::bigint, SUM(jobs_done)::bigint, SUM(jobs_failed)::bigint, ",
            "MAX(p95_ms)::bigint FROM node_metrics WHERE minute >= $1 AND minute < $2",
            $($filter,)?
            " GROUP BY bucket ORDER BY bucket"
        )
    };
}

/// The series statement of `step`, with the node filter when `by_node`.
fn series_sql(step: SeriesStep, by_node: bool) -> &'static str {
    match (step, by_node) {
        (SeriesStep::Minute, false) => series_sql!("1 minute"),
        (SeriesStep::Minute, true) => series_sql!("1 minute", " AND node_id = $3"),
        (SeriesStep::Hour, false) => series_sql!("1 hour"),
        (SeriesStep::Hour, true) => series_sql!("1 hour", " AND node_id = $3"),
        (SeriesStep::Day, false) => series_sql!("1 day"),
        (SeriesStep::Day, true) => series_sql!("1 day", " AND node_id = $3"),
    }
}

#[async_trait]
impl NodeMetricsRepo for PgRepos {
    async fn insert(
        &self,
        node: NodeId,
        minute: Timestamp,
        counts: &MetricCounts,
    ) -> Result<(), StoreError> {
        let (c, p95) = check_node_metrics(minute, counts)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO node_metrics (node_id, minute, requests, errors_5xx, page_views, \
             cache_hits, cache_misses, plugin_calls, plugin_failures, jobs_done, \
             jobs_failed, p95_ms) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
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
             p95_ms = GREATEST(node_metrics.p95_ms, excluded.p95_ms)"
        )
        .bind(node.as_uuid())
        .bind(DateTime::<Utc>::from(minute))
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
        let sql = series_sql(step, node.is_some());
        let rows: Vec<BucketRow> = run!(&self.exec, |e| {
            let q = sqlx::query_as::<_, BucketRow>(sql)
                .bind(DateTime::<Utc>::from(from))
                .bind(DateTime::<Utc>::from(to));
            match node {
                Some(n) => q.bind(n.as_uuid()),
                None => q,
            }
            .fetch_all(e)
        })?;
        rows.into_iter()
            .map(|(start, a, b, c, d, f, g, h, i, j, p95)| {
                series_bucket_from_db(Timestamp::from(start), [a, b, c, d, f, g, h, i, j], p95)
            })
            .collect()
    }

    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM node_metrics WHERE (node_id, minute) IN \
             (SELECT node_id, minute FROM node_metrics WHERE minute < $1 LIMIT $2)"
        )
        .bind(DateTime::<Utc>::from(before))
        .bind(limit)
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
