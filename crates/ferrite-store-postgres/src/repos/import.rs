//! WordPress import runs and the source-id map (`0009_import`, `0010_import_jobs`) on
//! Postgres. The fence's UPDATE takes the run row's lock, so inside a `Tx` a takeover's
//! `set_state` waits for the batch's commit.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    ImportMapKind, ImportMapRow, ImportRun, ImportRunId, ImportRunState, JobId, Timestamp, UserId,
};
use ferrite_store::{
    ImportRepo, StoreError, check_import_map, check_import_run, check_limit, token_from_db,
    token_to_db, validate_json, validate_text,
};
use sqlx::{AssertSqlSafe, Row, postgres::PgRow, types::Json};
use uuid::Uuid;

use super::PgRepos;
use crate::exec::{map_db_err, run};

const RUN_COLUMNS: &str = "id, source, token, state, started_by, started_at, heartbeat_at, \
     finished_at, stats, dry_run, job_id, step, report_hash";
const MAP_COLUMNS: &str = "source, kind, wp_id, ferrite_id, hash, version";

fn dt(t: Timestamp) -> DateTime<Utc> {
    t.into()
}

fn corrupt<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Database(format!("corrupt import row: {e}"))
}

fn import_run(row: &PgRow) -> Result<ImportRun, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let token: String = row.try_get("token").map_err(map_db_err)?;
    let state: String = row.try_get("state").map_err(map_db_err)?;
    let by: Uuid = row.try_get("started_by").map_err(map_db_err)?;
    let job: Option<Uuid> = row.try_get("job_id").map_err(map_db_err)?;
    let started: DateTime<Utc> = row.try_get("started_at").map_err(map_db_err)?;
    let beat: DateTime<Utc> = row.try_get("heartbeat_at").map_err(map_db_err)?;
    let finished: Option<DateTime<Utc>> = row.try_get("finished_at").map_err(map_db_err)?;
    let stats: Json<serde_json::Value> = row.try_get("stats").map_err(map_db_err)?;
    Ok(ImportRun {
        id: ImportRunId::from_uuid(id),
        source: row.try_get("source").map_err(map_db_err)?,
        token: token_from_db(&token)?,
        state: state.parse().map_err(corrupt)?,
        started_by: UserId::from_uuid(by),
        started_at: started.into(),
        heartbeat_at: beat.into(),
        finished_at: finished.map(Into::into),
        stats: stats.0,
        dry_run: row.try_get("dry_run").map_err(map_db_err)?,
        job_id: job.map(JobId::from_uuid),
        step: row.try_get("step").map_err(map_db_err)?,
        report_hash: row.try_get("report_hash").map_err(map_db_err)?,
    })
}

fn import_map(row: &PgRow) -> Result<ImportMapRow, StoreError> {
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
impl ImportRepo for PgRepos {
    async fn create_run(&self, r: &ImportRun) -> Result<(), StoreError> {
        check_import_run(r)?;
        run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO import_runs ({RUN_COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"
        )))
        .bind(r.id.as_uuid())
        .bind(&r.source)
        .bind(token_to_db(&r.token))
        .bind(r.state.as_str())
        .bind(r.started_by.as_uuid())
        .bind(dt(r.started_at))
        .bind(dt(r.heartbeat_at))
        .bind(r.finished_at.map(dt))
        .bind(Json(&r.stats))
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
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE import_runs SET heartbeat_at = $1 \
             WHERE id = $2 AND token = $3 AND state = 'running'"
        )
        .bind(dt(now))
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
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE import_runs SET state = 'abandoned', finished_at = $1 \
             WHERE id = $2 AND state = 'running' AND heartbeat_at < $3"
        )
        .bind(dt(now))
        .bind(id.as_uuid())
        .bind(dt(stale_before))
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn get_run(&self, id: ImportRunId) -> Result<Option<ImportRun>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(import_run).transpose()
    }

    async fn running(&self) -> Result<Option<ImportRun>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs WHERE state = 'running'"
        )))
        .fetch_optional(e))?;
        row.as_ref().map(import_run).transpose()
    }

    async fn list_runs(&self, limit: u32) -> Result<Vec<ImportRun>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs ORDER BY started_at DESC, id DESC LIMIT $1"
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
            None => run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM import_runs \
                     ORDER BY started_at DESC, id DESC LIMIT $1"
            )))
            .bind(i64::from(limit))
            .fetch_all(e))?,
            Some((at, id)) => run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
                "SELECT {RUN_COLUMNS} FROM import_runs WHERE (started_at, id) < ($1, $2) \
                     ORDER BY started_at DESC, id DESC LIMIT $3"
            )))
            .bind(dt(at))
            .bind(id)
            .bind(i64::from(limit))
            .fetch_all(e))?,
        };
        rows.iter().map(import_run).collect()
    }

    async fn run_by_job(&self, job: JobId) -> Result<Option<ImportRun>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM import_runs WHERE job_id = $1 \
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
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE import_runs SET step = $1 \
             WHERE id = $2 AND token = $3 AND state = 'running'"
        )
        .bind(step)
        .bind(id.as_uuid())
        .bind(token_to_db(token))
        .execute(e))?;
        Ok(done.rows_affected() == 1)
    }

    async fn set_report(&self, id: ImportRunId, hash: &str) -> Result<(), StoreError> {
        validate_text("import report hash", hash)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE import_runs SET report_hash = $1 WHERE id = $2"
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
        let finished = (state != ImportRunState::Running).then(|| dt(now));
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE import_runs SET state = $1, stats = $2, finished_at = $3 WHERE id = $4"
        )
        .bind(state.as_str())
        .bind(Json(stats))
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
        let row = run!(&self.exec, |e| {
            sqlx::query(AssertSqlSafe(format!(
            "SELECT {MAP_COLUMNS} FROM import_map WHERE source = $1 AND kind = $2 AND wp_id = $3"
        )))
        .bind(source)
        .bind(kind.as_str())
        .bind(wp_id)
        .fetch_optional(e)
        })?;
        row.as_ref().map(import_map).transpose()
    }

    async fn map_put(&self, row: &ImportMapRow) -> Result<(), StoreError> {
        check_import_map(row)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO import_map (source, kind, wp_id, ferrite_id, hash, version) \
             VALUES ($1, $2, $3, $4, $5, $6) \
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
}
