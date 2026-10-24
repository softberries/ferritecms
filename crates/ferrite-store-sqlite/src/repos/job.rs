//! The job queue (design 04 §6) on SQLite: the single writer connection makes the claim
//! atomic (P1), so there is no `SKIP LOCKED`. The process clock is the database clock
//! (one process owns the file), so `now` is bound from `Timestamp::now()`.

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use ferrite_domain::{Job, JobId, JobLease, JobStatus, NewJob, NodeId, Timestamp};
use ferrite_store::{
    JobRepo, StoreError, check_claim, check_job_duration, check_new_job, check_prune_limit,
    job_backoff, sanitize_job_error, sort_claimed,
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

/// Claim pre-check on the reader: anything due or reapable? Two `EXISTS`, one per
/// branch, each pinned to its partial index (`jobs_due_idx`, `jobs_lease_idx`) with
/// `INDEXED BY`: without statistics the planner would otherwise prefer the equality on
/// `jobs_status_created_idx` (0006) and walk every queued or running row. Binds now,
/// kinds, now, kinds.
pub(crate) const PRECHECK_SQL: &str = "SELECT EXISTS (SELECT 1 FROM jobs INDEXED BY jobs_due_idx \
       WHERE status = 'queued' AND run_at <= ? AND kind IN (SELECT value FROM json_each(?))) \
     OR EXISTS (SELECT 1 FROM jobs INDEXED BY jobs_lease_idx \
       WHERE status = 'running' AND locked_until < ? \
       AND kind IN (SELECT value FROM json_each(?)))";

/// P7: an expired lease at max_attempts goes dead instead of being reclaimed (on
/// `jobs_lease_idx`, pinned as in [`PRECHECK_SQL`]). Binds now (finished_at), now, kinds.
pub(crate) const REAPER_SQL: &str = "UPDATE jobs INDEXED BY jobs_lease_idx SET status = 'dead', \
     last_error = 'lease expired after attempt ' || attempts || ' on node ' \
       || COALESCE(locked_by, '?'), \
     finished_at = ?, locked_by = NULL, locked_until = NULL \
     WHERE status = 'running' AND locked_until < ? AND attempts >= max_attempts \
     AND kind IN (SELECT value FROM json_each(?))";

/// The claim: due queued jobs (`jobs_due_idx`) and expired leases with attempts left
/// (`jobs_lease_idx`), each branch pinned to its partial index as in [`PRECHECK_SQL`],
/// then the (few) due rows sorted. Binds node, locked_until, kinds, now, now, limit.
pub(crate) fn claim_sql() -> String {
    format!(
        "UPDATE jobs SET status = 'running', locked_by = ?1, locked_until = ?2, \
         attempts = attempts + 1 \
         WHERE id IN (SELECT id FROM ( \
           SELECT id, priority, run_at FROM jobs INDEXED BY jobs_due_idx \
             WHERE status = 'queued' AND run_at <= ?4 \
             AND kind IN (SELECT value FROM json_each(?3)) \
           UNION ALL \
           SELECT id, priority, run_at FROM jobs INDEXED BY jobs_lease_idx \
             WHERE status = 'running' AND locked_until < ?5 AND attempts < max_attempts \
             AND kind IN (SELECT value FROM json_each(?3))) \
           ORDER BY priority DESC, run_at, id LIMIT ?6) \
         RETURNING {JOB_COLUMNS}"
    )
}

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
        let due: bool = run!(&self.reader, |e| sqlx::query_scalar(PRECHECK_SQL)
            .bind(now.as_micros())
            .bind(&kinds)
            .bind(now.as_micros())
            .bind(&kinds)
            .fetch_one(e))?;
        if !due {
            return Ok(Vec::new());
        }
        let node = node.to_string();
        let mut jobs = atomic!(&self.writer, |conn| async {
            // Re-read the clock once the writer is ours (waiting for it can take seconds).
            let now = Timestamp::now();
            let until = now.as_micros().saturating_add(lease);
            sqlx::query(REAPER_SQL)
                .bind(now.as_micros())
                .bind(now.as_micros())
                .bind(&kinds)
                .execute(&mut *conn)
                .await
                .map_err(map_db_err)?;
            let rows = sqlx::query(AssertSqlSafe(claim_sql()))
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
        // Counting the final attempt ends the job: `dead`, like a failure would.
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE jobs SET \
             status = CASE WHEN ?1 AND attempts >= max_attempts THEN 'dead' ELSE 'queued' END, \
             run_at = CASE WHEN ?1 AND attempts >= max_attempts THEN run_at ELSE ?2 END, \
             finished_at = CASE WHEN ?1 AND attempts >= max_attempts THEN ?3 ELSE NULL END, \
             last_error = CASE WHEN ?1 AND attempts >= max_attempts \
               THEN 'released after final attempt' ELSE last_error END, \
             locked_by = NULL, locked_until = NULL, \
             attempts = CASE WHEN ?1 THEN attempts ELSE attempts - 1 END \
             WHERE id = ?4 AND status = 'running' AND locked_by = ?5 AND attempts = ?6"
        )
        .bind(count_attempt)
        .bind(run_at)
        .bind(Timestamp::now().as_micros())
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
        limit: u32,
    ) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM jobs WHERE rowid IN (SELECT rowid FROM jobs \
             WHERE (status = 'done' AND finished_at < ?) \
             OR (status = 'dead' AND finished_at < ?) LIMIT ?)"
        )
        .bind(done_before.as_micros())
        .bind(dead_before.as_micros())
        .bind(limit)
        .execute(e))
        .map(|done| done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use ferrite_store::Store;
    use sqlx::AssertSqlSafe;

    use super::*;
    use crate::{SqliteOptions, SqliteStore};

    async fn plan(store: &SqliteStore, sql: &str, binds: &[&str]) -> String {
        let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {sql}"
        )));
        for b in binds {
            q = match *b {
                "kinds" => q.bind(r#"["a","b"]"#),
                "node" => q.bind("n"),
                _ => q.bind(2_000_000_i64),
            };
        }
        let rows = q.fetch_all(&store.reader).await.unwrap();
        rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
    }

    /// The claim pre-check, the claim and the reaper search the partial indexes (no
    /// table scan), before and after ANALYZE on 20k mostly finished rows.
    #[tokio::test]
    async fn claim_statements_use_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
            .await
            .unwrap();
        store.migrate_up(None).await.unwrap();
        sqlx::raw_sql(
            "WITH RECURSIVE g(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM g WHERE n < 20000) \
             INSERT INTO jobs (id, kind, payload, run_at, priority, status, attempts, \
               max_attempts, locked_by, locked_until, created_at, finished_at) \
             SELECT randomblob(16), char(97 + n % 3), '{}', 1000000 + (n % 2) * 2000000, \
               n % 5, \
               CASE WHEN n <= 18000 THEN 'done' WHEN n <= 19600 THEN 'queued' \
                 WHEN n <= 19800 THEN 'running' ELSE 'dead' END, \
               1, 5, CASE WHEN n BETWEEN 19601 AND 19800 THEN 'n' END, \
               CASE WHEN n BETWEEN 19601 AND 19800 THEN 1000000 + (n % 2) * 2000000 END, \
               0, CASE WHEN n <= 18000 OR n > 19800 THEN 1 END \
             FROM g",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        let claim = claim_sql();
        for analyzed in [false, true] {
            if analyzed {
                sqlx::raw_sql("ANALYZE")
                    .execute(&store.writer)
                    .await
                    .unwrap();
            }
            let pre = plan(&store, PRECHECK_SQL, &["now", "kinds", "now", "kinds"]).await;
            let reaper = plan(&store, REAPER_SQL, &["now", "now", "kinds"]).await;
            let claim = plan(
                &store,
                &claim,
                &["node", "now", "kinds", "now", "now", "now"],
            )
            .await;
            eprintln!(
                "analyzed={analyzed}\n-- precheck\n{pre}\n-- reaper\n{reaper}\n-- claim\n{claim}"
            );
            for (name, plan) in [("precheck", &pre), ("claim", &claim)] {
                assert!(plan.contains("jobs_due_idx"), "{name}: {plan}");
                assert!(plan.contains("jobs_lease_idx"), "{name}: {plan}");
            }
            assert!(reaper.contains("jobs_lease_idx"), "reaper: {reaper}");
            for plan in [&pre, &reaper, &claim] {
                assert!(!plan.contains("SCAN jobs"), "{plan}");
            }
        }
    }
}
