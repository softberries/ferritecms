//! The job queue (design 04 §6) on SQLite: the single writer connection makes the claim
//! atomic (P1), so there is no `SKIP LOCKED`. The process clock is the database clock
//! (one process owns the file), so `now` is bound from `Timestamp::now()`.

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use ferrite_domain::{Job, JobId, JobLease, JobStatus, NewJob, NodeId, Timestamp};
use ferrite_store::{
    JobRepo, StoreError, check_claim, check_job_duration, check_new_job, job_backoff,
    sanitize_job_error, sort_claimed,
};
use sqlx::AssertSqlSafe;

use super::SqliteRepos;
use crate::{
    codec::{self, JOB_COLUMNS},
    exec::{atomic, map_db_err, run},
};

/// The lease-token condition every lease-holder write carries (P6); binds id, node,
/// attempt.
const HELD: &str = "id = ? AND status = 'running' AND locked_by = ? AND attempts = ?";

/// Jobs a claim may take; binds kinds (JSON array), now, now.
const DUE: &str = "kind IN (SELECT value FROM json_each(?)) \
     AND ((status = 'queued' AND run_at <= ?) \
       OR (status = 'running' AND locked_until < ? AND attempts < max_attempts))";

fn micros(now: Timestamp, d: Duration, what: &str, positive: bool) -> Result<i64, StoreError> {
    let d = check_job_duration(what, d, positive)?;
    Ok(now.as_micros().saturating_add(d))
}

fn kinds_json(kinds: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(kinds).map_err(|e| StoreError::Invalid(e.to_string()))
}

#[async_trait]
impl JobRepo for SqliteRepos {
    async fn enqueue(&self, job: &NewJob) -> Result<Option<JobId>, StoreError> {
        check_new_job(job)?;
        let payload = codec::json_to_db(&job.payload)?;
        let now = Timestamp::now();
        let id: Option<uuid::Uuid> = run!(&self.writer, |e| sqlx::query_scalar(
            "INSERT INTO jobs (id, kind, payload, unique_key, run_at, priority, status, \
             attempts, max_attempts, created_at) VALUES (?, ?, ?, ?, ?, ?, 'queued', 0, ?, ?) \
             ON CONFLICT (unique_key) DO NOTHING RETURNING id"
        )
        .bind(JobId::new().as_uuid())
        .bind(&job.kind)
        .bind(payload)
        .bind(job.unique_key.as_deref())
        .bind(job.run_at.unwrap_or(now).as_micros())
        .bind(job.priority)
        .bind(job.max_attempts)
        .bind(now.as_micros())
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
        let lease = check_job_duration("lease", lease, true)?;
        let kinds = kinds_json(kinds)?;
        let now = Timestamp::now();
        // An idle node must not take the write lock every poll (C7): look on the reader
        // first. This also covers expired leases at max_attempts, which the reaper needs.
        let due: bool = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind IN (SELECT value FROM json_each(?)) \
             AND ((status = 'queued' AND run_at <= ?) \
               OR (status = 'running' AND locked_until < ?)))"
        )
        .bind(&kinds)
        .bind(now.as_micros())
        .bind(now.as_micros())
        .fetch_one(e))?;
        if !due {
            return Ok(Vec::new());
        }
        let node = node.to_string();
        let mut jobs = atomic!(&self.writer, |conn| async {
            // Re-read the clock once the writer is ours (waiting for it can take seconds).
            let now = Timestamp::now();
            let until = now.as_micros().saturating_add(lease);
            // P7: an expired lease at max_attempts goes dead instead of being reclaimed.
            sqlx::query(
                "UPDATE jobs SET status = 'dead', \
                 last_error = 'lease expired after attempt ' || attempts || ' on node ' \
                   || COALESCE(locked_by, '?'), \
                 finished_at = ?, locked_by = NULL, locked_until = NULL \
                 WHERE status = 'running' AND locked_until < ? AND attempts >= max_attempts \
                 AND kind IN (SELECT value FROM json_each(?))",
            )
            .bind(now.as_micros())
            .bind(now.as_micros())
            .bind(&kinds)
            .execute(&mut *conn)
            .await
            .map_err(map_db_err)?;
            let rows = sqlx::query(AssertSqlSafe(format!(
                "UPDATE jobs SET status = 'running', locked_by = ?, locked_until = ?, \
                 attempts = attempts + 1 \
                 WHERE id IN (SELECT id FROM jobs WHERE {DUE} \
                   ORDER BY priority DESC, run_at, id LIMIT ?) \
                 RETURNING {JOB_COLUMNS}"
            )))
            .bind(&node)
            .bind(until)
            .bind(&kinds)
            .bind(now.as_micros())
            .bind(now.as_micros())
            .bind(i64::from(limit))
            .fetch_all(&mut *conn)
            .await
            .map_err(map_db_err)?;
            rows.iter().map(codec::job).collect::<Result<Vec<_>, _>>()
        })?;
        sort_claimed(&mut jobs);
        Ok(jobs)
    }

    async fn extend(&self, lease: &JobLease, duration: Duration) -> Result<bool, StoreError> {
        let until = micros(Timestamp::now(), duration, "lease", true)?;
        let done = run!(&self.writer, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET locked_until = ? WHERE {HELD}"
        )))
        .bind(until)
        .bind(lease.id.as_uuid())
        .bind(lease.node.to_string())
        .bind(lease.attempt)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn complete(&self, lease: &JobLease) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET status = 'done', finished_at = ?, locked_by = NULL, \
             locked_until = NULL WHERE {HELD}"
        )))
        .bind(Timestamp::now().as_micros())
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
        let now = Timestamp::now();
        // `attempts = lease.attempt` is part of the condition, so the backoff is known.
        let retry_at = micros(now, job_backoff(lease.attempt), "backoff", false)?;
        let status: Option<String> = run!(&self.writer, |e| sqlx::query_scalar(AssertSqlSafe(
            format!(
                "UPDATE jobs SET \
                 status = CASE WHEN ? OR attempts >= max_attempts THEN 'dead' ELSE 'queued' END, \
                 run_at = CASE WHEN ? OR attempts >= max_attempts THEN run_at ELSE ? END, \
                 finished_at = CASE WHEN ? OR attempts >= max_attempts THEN ? ELSE NULL END, \
                 last_error = ?, locked_by = NULL, locked_until = NULL \
                 WHERE {HELD} RETURNING status"
            )
        ))
        .bind(permanent)
        .bind(permanent)
        .bind(retry_at)
        .bind(permanent)
        .bind(now.as_micros())
        .bind(error)
        .bind(lease.id.as_uuid())
        .bind(lease.node.to_string())
        .bind(lease.attempt)
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
        let run_at = micros(Timestamp::now(), delay, "release delay", false)?;
        let done = run!(&self.writer, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE jobs SET status = 'queued', run_at = ?, locked_by = NULL, \
             locked_until = NULL, attempts = CASE WHEN ? THEN attempts ELSE attempts - 1 END \
             WHERE {HELD}"
        )))
        .bind(run_at)
        .bind(count_attempt)
        .bind(lease.id.as_uuid())
        .bind(lease.node.to_string())
        .bind(lease.attempt)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::job).transpose()
    }

    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError> {
        let rows: Vec<(String, i64)> = run!(&self.reader, |e| sqlx::query_as(
            "SELECT status, COUNT(*) FROM jobs GROUP BY status"
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
    ) -> Result<u64, StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM jobs WHERE (status = 'done' AND finished_at < ?) \
             OR (status = 'dead' AND finished_at < ?)"
        )
        .bind(done_before.as_micros())
        .bind(dead_before.as_micros())
        .execute(e))
        .map(|done| done.rows_affected())
    }
}
