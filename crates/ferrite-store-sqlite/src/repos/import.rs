//! WordPress import runs and the source-id map (`0009_import`, `0010_import_jobs`) on
//! SQLite. The single writer serialises a batch's fence against a takeover, so the fence
//! is a plain UPDATE.

use async_trait::async_trait;
use ferrite_domain::{
    ImportMapKind, ImportMapRow, ImportRun, ImportRunId, ImportRunState, JobId, Timestamp, UserId,
};
use ferrite_store::{
    ImportRepo, StoreError, check_import_map, check_import_run, check_limit, token_from_db,
    token_to_db, validate_json, validate_text,
};
use sqlx::{AssertSqlSafe, Row, sqlite::SqliteRow};
use uuid::Uuid;

use super::SqliteRepos;
use crate::{
    codec,
    exec::{map_db_err, run},
};

const RUN_COLUMNS: &str = "id, source, token, state, started_by, started_at, heartbeat_at, \
     finished_at, stats, dry_run, job_id, step, report_hash";
const MAP_COLUMNS: &str = "source, kind, wp_id, ferrite_id, hash, version";

fn ts(micros: i64) -> Result<Timestamp, StoreError> {
    Ok(Timestamp::from_micros(micros)?)
}

fn corrupt<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Database(format!("corrupt import row: {e}"))
}

fn import_run(row: &SqliteRow) -> Result<ImportRun, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let token: String = row.try_get("token").map_err(map_db_err)?;
    let state: String = row.try_get("state").map_err(map_db_err)?;
    let by: Uuid = row.try_get("started_by").map_err(map_db_err)?;
    let job: Option<Uuid> = row.try_get("job_id").map_err(map_db_err)?;
    let finished: Option<i64> = row.try_get("finished_at").map_err(map_db_err)?;
    let stats: String = row.try_get("stats").map_err(map_db_err)?;
    Ok(ImportRun {
        id: ImportRunId::from_uuid(id),
        source: row.try_get("source").map_err(map_db_err)?,
        token: token_from_db(&token)?,
        state: state.parse().map_err(corrupt)?,
        started_by: UserId::from_uuid(by),
        started_at: ts(row.try_get("started_at").map_err(map_db_err)?)?,
        heartbeat_at: ts(row.try_get("heartbeat_at").map_err(map_db_err)?)?,
        finished_at: finished.map(ts).transpose()?,
        stats: codec::json_from_db(&stats)?,
        dry_run: row.try_get("dry_run").map_err(map_db_err)?,
        job_id: job.map(JobId::from_uuid),
        step: row.try_get("step").map_err(map_db_err)?,
        report_hash: row.try_get("report_hash").map_err(map_db_err)?,
    })
}

fn import_map(row: &SqliteRow) -> Result<ImportMapRow, StoreError> {
    let kind: String = row.try_get("kind").map_err(map_db_err)?;
    Ok(ImportMapRow {
        source: row.try_get("source").map_err(map_db_err)?,
        kind: kind.parse().map_err(corrupt)?,
        wp_id: row.try_get("wp_id").map_err(map_db_err)?,
        ferrite_id: row.try_get("ferrite_id").map_err(map_db_err)?,
        hash: row.try_get("hash").map_err(map_db_err)?,
        version: row.try_get("version").map_err(map_db_err)?,
    })
}

#[async_trait]
impl ImportRepo for SqliteRepos {
    async fn create_run(&self, r: &ImportRun) -> Result<(), StoreError> {
        check_import_run(r)?;
        run!(&self.writer, |e| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO import_runs ({RUN_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )))
        .bind(r.id.as_uuid())
        .bind(&r.source)
        .bind(token_to_db(&r.token))
        .bind(r.state.as_str())
        .bind(r.started_by.as_uuid())
        .bind(r.started_at.as_micros())
        .bind(r.heartbeat_at.as_micros())
        .bind(r.finished_at.map(|t| t.as_micros()))
        .bind(codec::json_to_db(&r.stats)?)
        .bind(r.dry_run)
        .bind(r.job_id.map(|j| j.as_uuid()))
        .bind(&r.step)
        .bind(&r.report_hash)
        .execute(e))
        .map(|_| ())
    }

    async fn fence(
        &self,
        id: ImportRunId,
        token: &[u8; 16],
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE import_runs SET heartbeat_at = ? \
             WHERE id = ? AND token = ? AND state = 'running'"
        )
        .bind(now.as_micros())
        .bind(id.as_uuid())
        .bind(token_to_db(token))
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn abandon_stale(
        &self,
        id: ImportRunId,
        stale_before: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE import_runs SET state = 'abandoned', finished_at = ? \
             WHERE id = ? AND state = 'running' AND heartbeat_at < ?"
        )
        .bind(now.as_micros())
        .bind(id.as_uuid())
        .bind(stale_before.as_micros())
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn abandon_stale_for_job(
        &self,
        id: ImportRunId,
        stale_before: Timestamp,
        now: Timestamp,
        job: JobId,
    ) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE import_runs SET state = 'abandoned', finished_at = ? \
             WHERE id = ? AND state = 'running' AND heartbeat_at < ? \
             AND (job_id IS NULL OR job_id = ? OR NOT EXISTS \
             (SELECT 1 FROM jobs WHERE jobs.id = import_runs.job_id AND jobs.status = 'running' \
             AND (jobs.locked_until IS NULL OR jobs.locked_until >= ?)))"
        )
        .bind(now.as_micros())
        .bind(id.as_uuid())
        .bind(stale_before.as_micros())
        .bind(job.as_uuid())
        .bind(now.as_micros())
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn get_run(&self, id: ImportRunId) -> Result<Option<ImportRun>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(import_run).transpose()
    }

    async fn running(&self) -> Result<Option<ImportRun>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs WHERE state = 'running'"
        )))
        .fetch_optional(e))?;
        row.as_ref().map(import_run).transpose()
    }

    async fn list_runs(&self, limit: u32) -> Result<Vec<ImportRun>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs ORDER BY started_at DESC, id DESC LIMIT ?"
        )))
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(import_run).collect()
    }

    async fn list_runs_page(
        &self,
        limit: u32,
        cursor: Option<(Timestamp, Uuid)>,
    ) -> Result<Vec<ImportRun>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = match cursor {
            None => run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM import_runs \
                     ORDER BY started_at DESC, id DESC LIMIT ?"
            )))
            .bind(i64::from(limit))
            .fetch_all(e))?,
            Some((at, id)) => run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM import_runs WHERE (started_at, id) < (?, ?) \
                     ORDER BY started_at DESC, id DESC LIMIT ?"
            )))
            .bind(at.as_micros())
            .bind(id)
            .bind(i64::from(limit))
            .fetch_all(e))?,
        };
        rows.iter().map(import_run).collect()
    }

    async fn run_by_job(&self, job: JobId) -> Result<Option<ImportRun>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs WHERE job_id = ? \
             ORDER BY started_at DESC, id DESC LIMIT 1"
        )))
        .bind(job.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(import_run).transpose()
    }

    async fn set_step(
        &self,
        id: ImportRunId,
        token: &[u8; 16],
        step: &str,
    ) -> Result<bool, StoreError> {
        validate_text("import step", step)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE import_runs SET step = ? \
             WHERE id = ? AND token = ? AND state = 'running'"
        )
        .bind(step)
        .bind(id.as_uuid())
        .bind(token_to_db(token))
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn set_report(&self, id: ImportRunId, hash: &str) -> Result<(), StoreError> {
        validate_text("import report hash", hash)?;
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE import_runs SET report_hash = ? WHERE id = ?"
        )
        .bind(hash)
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn set_state(
        &self,
        id: ImportRunId,
        state: ImportRunState,
        stats: &serde_json::Value,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        validate_json(stats)?;
        let finished = (state != ImportRunState::Running).then(|| now.as_micros());
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE import_runs SET state = ?, stats = ?, finished_at = ? WHERE id = ?"
        )
        .bind(state.as_str())
        .bind(codec::json_to_db(stats)?)
        .bind(finished)
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn map_get(
        &self,
        source: &str,
        kind: ImportMapKind,
        wp_id: i64,
    ) -> Result<Option<ImportMapRow>, StoreError> {
        validate_text("import source", source)?;
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {MAP_COLUMNS} FROM import_map WHERE source = ? AND kind = ? AND wp_id = ?"
        )))
        .bind(source)
        .bind(kind.as_str())
        .bind(wp_id)
        .fetch_optional(e))?;
        row.as_ref().map(import_map).transpose()
    }

    async fn map_put(&self, row: &ImportMapRow) -> Result<(), StoreError> {
        check_import_map(row)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO import_map (source, kind, wp_id, ferrite_id, hash, version) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (source, kind, wp_id) DO UPDATE SET \
               ferrite_id = excluded.ferrite_id, hash = excluded.hash, \
               version = excluded.version"
        )
        .bind(&row.source)
        .bind(row.kind.as_str())
        .bind(row.wp_id)
        .bind(row.ferrite_id)
        .bind(&row.hash)
        .bind(row.version)
        .execute(e))
        .map(|_| ())
    }

    async fn map_list(
        &self,
        source: &str,
        kind: ImportMapKind,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<ImportMapRow>, StoreError> {
        validate_text("import source", source)?;
        let limit = check_limit(limit)?;
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {MAP_COLUMNS} FROM import_map \
             WHERE source = ? AND kind = ? AND (? IS NULL OR wp_id > ?) \
             ORDER BY wp_id LIMIT ?"
        )))
        .bind(source)
        .bind(kind.as_str())
        .bind(after)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(import_map).collect()
    }
}
