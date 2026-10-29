//! The job queue (design 04 §6) on Postgres: claims race across nodes under
//! `FOR UPDATE SKIP LOCKED` (P5), every time comparison uses the database clock
//! (`now()`), and lease-holder writes are conditional on the lease token (P6).

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{Job, JobId, JobLease, JobQuery, JobStatus, NewJob, NodeId, Page, Timestamp};
use ferrite_store::{
    Channel, JobRepo, StoreError, check_claim, check_job_duration, check_job_query, check_new_job,
    check_prune_limit, encode_job_cursor, sanitize_job_error, sort_claimed,
};
use sqlx::{AssertSqlSafe, Postgres, QueryBuilder, types::Json};

use super::PgRepos;
use crate::{
    codec::{self, JOB_COLUMNS, JOB_COLUMNS_QUALIFIED},
    exec::run,
};

/// The lease-token condition every lease-holder write carries (P6); `$1` id, `$2` node,
/// `$3` attempt.
const HELD: &str = "id = $1 AND status = 'running' AND locked_by = $2 AND attempts = $3";

/// A validated lease or delay in seconds, for `make_interval(secs => ...)`.
fn seconds(what: &str, d: Duration, positive: bool) -> Result<f64, StoreError> {
    let micros = check_job_duration(what, d, positive)?;
    // Exact for every value up to MAX_JOB_DURATION (well below 2^53 microseconds).
    Ok(micros as f64 / 1_000_000.0)
}

#[async_trait]
impl JobRepo for PgRepos {
    async fn enqueue(&self, job: &NewJob) -> Result<Option<JobId>, StoreError> {
        check_new_job(job)?;
        // P2: notify the jobs channel (payload: the kind) only when a row was inserted;
        // delivered when the insert commits.
        let id: Option<uuid::Uuid> = run!(&self.exec, |e| sqlx::query_scalar(
            "WITH ins AS (INSERT INTO jobs (id, kind, payload, unique_key, run_at, priority, \
               status, attempts, max_attempts, created_at) \
               VALUES ($1, $2, $3, $4, COALESCE($5, now()), $6, 'queued', 0, $7, now()) \
               ON CONFLICT (unique_key) DO NOTHING RETURNING id, kind) \
             SELECT id, pg_notify($8, kind) FROM ins"
        )
        .bind(JobId::new().as_uuid())
        .bind(&job.kind)
        .bind(Json(&job.payload))
        .bind(job.unique_key.as_deref())
        .bind(job.run_at.map(DateTime::<Utc>::from))
        .bind(job.priority)
        .bind(job.max_attempts)
        .bind(self.channels.name(Channel::Jobs))
        .fetch_optional(e))?;
        Ok(id.map(JobId::from_uuid))
    }

    async fn claim(
        &self,
        node: NodeId,
        kinds: &[String],
        limit: u32,
        lease: Duration,
    ) -> Result<Vec<Job>, StoreError> {
        check_claim(kinds, limit)?;
        let lease = seconds("lease", lease, true)?;
        // P7: an expired lease at max_attempts goes dead instead of being reclaimed.
        run!(&self.exec, |e| sqlx::query(
            "UPDATE jobs SET status = 'dead', \
             last_error = 'lease expired after attempt ' || attempts || ' on node ' \
               || COALESCE(locked_by, '?'), \
             finished_at = now(), locked_by = NULL, locked_until = NULL \
             WHERE status = 'running' AND locked_until < now() \
             AND attempts >= max_attempts AND kind = ANY($1)"
        )
        .bind(kinds)
        .execute(e))?;
        // P5: the MATERIALIZED CTE evaluates the locking subquery exactly once.
        // P31: two branches, each bounded by LIMIT on its own index (`jobs_claim_idx` for
        // due jobs, `jobs_lease_idx` for expired leases), as the SQLite claim does. One
        // OR-filtered scan of `jobs_claim_idx` would read every future-dated job. Postgres
        // forbids FOR UPDATE on a UNION leaf, so each branch locks inside a derived table; the outer
        // select only picks the best LIMIT of the (at most 2 x LIMIT) locked ids, and the
        // rest are released when the statement ends.
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "WITH next AS MATERIALIZED ( \
               SELECT id FROM ( \
                 SELECT id, priority, run_at FROM ( \
                   SELECT id, priority, run_at FROM jobs \
                   WHERE kind = ANY($1) AND status = 'queued' AND run_at <= now() \
                   ORDER BY priority DESC, run_at, id \
                   LIMIT $2 \
                   FOR UPDATE SKIP LOCKED) due \
                 UNION ALL \
                 SELECT id, priority, run_at FROM ( \
                   SELECT id, priority, run_at FROM jobs \
                   WHERE kind = ANY($1) AND status = 'running' AND locked_until < now() \
                     AND attempts < max_attempts \
                   ORDER BY priority DESC, run_at, id \
                   LIMIT $2 \
                   FOR UPDATE SKIP LOCKED) expired) u \
               ORDER BY priority DESC, run_at, id \
               LIMIT $2) \
             UPDATE jobs SET status = 'running', locked_by = $3, \
               locked_until = now() + make_interval(secs => $4), attempts = attempts + 1 \
             FROM next WHERE jobs.id = next.id \
             RETURNING {JOB_COLUMNS_QUALIFIED}"
        )))
        .bind(kinds)
        .bind(i64::from(limit))
        .bind(node.to_string())
        .bind(lease)
        .fetch_all(e))?;
        let mut jobs = rows.iter().map(codec::job).collect::<Result<Vec<_>, _>>()?;
        sort_claimed(&mut jobs);
        Ok(jobs)
    }

    async fn extend(&self, lease: &JobLease, duration: Duration) -> Result<bool, StoreError> {
        let secs = seconds("lease", duration, true)?;
        let done = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET locked_until = now() + make_interval(secs => $4) WHERE {HELD}"
        )))
        .bind(lease.id.as_uuid())
        .bind(lease.node.to_string())
        .bind(lease.attempt)
        .bind(secs)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn complete(&self, lease: &JobLease) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET status = 'done', finished_at = now(), locked_by = NULL, \
             locked_until = NULL WHERE {HELD}"
        )))
        .bind(lease.id.as_uuid())
        .bind(lease.node.to_string())
        .bind(lease.attempt)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        permanent: bool,
    ) -> Result<Option<JobStatus>, StoreError> {
        let error = sanitize_job_error(error);
        // The backoff is `job_backoff(attempts)`, computed in SQL.
        let status: Option<String> =
            run!(&self.exec, |e| sqlx::query_scalar(AssertSqlSafe(format!(
                "UPDATE jobs SET \
                 status = CASE WHEN $4 OR attempts >= max_attempts \
                   THEN 'dead' ELSE 'queued' END, \
                 run_at = CASE WHEN $4 OR attempts >= max_attempts THEN run_at \
                   ELSE now() + make_interval(secs => \
                     LEAST(power(2, LEAST(GREATEST(attempts, 0), 12)) * 10, 3600)) END, \
                 finished_at = CASE WHEN $4 OR attempts >= max_attempts \
                   THEN now() ELSE NULL END, \
                 last_error = $5, locked_by = NULL, locked_until = NULL \
                 WHERE {HELD} RETURNING status"
            )))
            .bind(lease.id.as_uuid())
            .bind(lease.node.to_string())
            .bind(lease.attempt)
            .bind(permanent)
            .bind(error)
            .fetch_optional(e))?;
        status
            .map(|s| s.parse().map_err(StoreError::from))
            .transpose()
    }

    async fn release(
        &self,
        lease: &JobLease,
        delay: Duration,
        count_attempt: bool,
    ) -> Result<bool, StoreError> {
        let secs = seconds("release delay", delay, false)?;
        // Counting the final attempt ends the job: `dead`, like a failure would.
        let done = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET \
             status = CASE WHEN $5 AND attempts >= max_attempts \
               THEN 'dead' ELSE 'queued' END, \
             run_at = CASE WHEN $5 AND attempts >= max_attempts THEN run_at \
               ELSE now() + make_interval(secs => $4) END, \
             finished_at = CASE WHEN $5 AND attempts >= max_attempts THEN now() ELSE NULL END, \
             last_error = CASE WHEN $5 AND attempts >= max_attempts \
               THEN 'released after final attempt' ELSE last_error END, \
             locked_by = NULL, locked_until = NULL, \
             attempts = CASE WHEN $5 THEN attempts ELSE attempts - 1 END \
             WHERE {HELD}"
        )))
        .bind(lease.id.as_uuid())
        .bind(lease.node.to_string())
        .bind(lease.attempt)
        .bind(secs)
        .bind(count_attempt)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::job).transpose()
    }

    async fn list(&self, query: &JobQuery) -> Result<Page<Job>, StoreError> {
        let (limit, after) = check_job_query(query)?;
        // Only the filters given are in the SQL: with `status` the planner walks
        // `jobs_status_created_idx`, without it `jobs_created_idx` (0006, F26).
        let mut qb =
            QueryBuilder::<Postgres>::new(format!("SELECT {JOB_COLUMNS} FROM jobs WHERE true"));
        if let Some(status) = query.status {
            qb.push(" AND status = ").push_bind(status.as_str());
        }
        if let Some(kind) = &query.kind {
            qb.push(" AND kind = ").push_bind(kind.as_str());
        }
        if let Some((at, id)) = after {
            qb.push(" AND (created_at, id) < (")
                .push_bind(DateTime::<Utc>::from(Timestamp::from_micros(at)?))
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        let mut items = rows.iter().map(codec::job).collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|j| encode_job_cursor(j.created_at, j.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn retry_dead(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET status = 'queued', attempts = 0, run_at = now(), \
             finished_at = NULL, locked_by = NULL, locked_until = NULL \
             WHERE id = $1 AND status = 'dead' RETURNING {JOB_COLUMNS}"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::job).transpose()
    }

    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError> {
        let rows: Vec<(String, i64)> = run!(&self.exec, |e| sqlx::query_as(
            "SELECT status::text, COUNT(*) FROM jobs GROUP BY status"
        )
        .fetch_all(e))?;
        let mut counts: BTreeMap<JobStatus, u64> = JobStatus::ALL.iter().map(|s| (*s, 0)).collect();
        for (status, n) in rows {
            counts.insert(status.parse()?, u64::try_from(n).unwrap_or(0));
        }
        Ok(counts)
    }

    async fn prune(
        &self,
        done_before: Timestamp,
        dead_before: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        // The outer predicate repeats the subquery's: under READ COMMITTED a row picked
        // here and changed meanwhile (a dead job retried, `retry_dead`) is re-checked
        // against the outer `WHERE` only, and must then be kept.
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM jobs WHERE id IN (SELECT id FROM jobs \
             WHERE (status = 'done' AND finished_at < $1) \
             OR (status = 'dead' AND finished_at < $2) LIMIT $3) \
             AND ((status = 'done' AND finished_at < $1) OR (status = 'dead' AND finished_at < $2))"
        )
        .bind(DateTime::<Utc>::from(done_before))
        .bind(DateTime::<Utc>::from(dead_before))
        .bind(limit)
        .execute(e))
        .map(|done| done.rows_affected())
    }
}
