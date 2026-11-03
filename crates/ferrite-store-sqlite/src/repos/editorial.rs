//! Releases, workflows, reviews and preview links (`0017_releases_workflows`) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{
    ContentId, Page, PreviewLink, PreviewLinkId, PreviewTarget, PreviewTargetKind, Release,
    ReleaseEntry, ReleaseId, ReleaseQuery, ReleaseStatus, Review, ReviewDecision, ReviewId,
    ReviewQuery, ReviewTarget, RevisionId, StagedChange, Timestamp, UserId, Workflow, WorkflowStep,
};
use ferrite_store::{
    MAX_ID_FILTER, MAX_RELEASES_OF, PreviewLinkRepo, RELEASE_CURSOR_TAG, REVIEW_CURSOR_TAG,
    ReleaseRepo, ReviewRepo, StoreError, WorkflowRepo, check_limit, check_prune_limit,
    check_release, check_review, check_steps, encode_release_cursor, encode_review_cursor,
    micros_cursor, validate_json, validate_text,
};
use sqlx::{AssertSqlSafe, Row, sqlite::SqliteRow};
use uuid::Uuid;

use super::SqliteRepos;
use crate::{
    codec::{json_from_db, json_to_db},
    exec::{map_db_err, run},
};

const RELEASE_COLUMNS: &str = "id, title, description, status, scheduled_at, scheduled_by, \
     override_reason, failure, created_by, created_at, updated_at, applied_at, applied_by, version";
const ENTRY_COLUMNS: &str = "release_id, item_id, position, transition, changes, base_version, \
     entry_version, added_by, updated_at";
const REVIEW_COLUMNS: &str = "r.id, r.target, r.item_id, r.release_id, r.revision_id, \
     r.entry_version, r.status, r.step, r.steps, r.note, r.submitted_by, r.submitted_at, \
     r.decided_at, r.version";
const REVIEW_RETURNING: &str = "id, target, item_id, release_id, revision_id, entry_version, \
     status, step, steps, note, submitted_by, submitted_at, decided_at, version";
const LINK_COLUMNS: &str =
    "id, target_kind, target_id, created_by, created_at, expires_at, last_used_at";
/// The open review statuses, as SQL.
const OPEN: &str = "('in_review', 'approved')";

fn ts(micros: i64) -> Result<Timestamp, StoreError> {
    Ok(Timestamp::from_micros(micros)?)
}

fn ts_col(row: &SqliteRow, col: &str) -> Result<Timestamp, StoreError> {
    ts(row.try_get(col).map_err(map_db_err)?)
}

fn ts_opt(row: &SqliteRow, col: &str) -> Result<Option<Timestamp>, StoreError> {
    let v: Option<i64> = row.try_get(col).map_err(map_db_err)?;
    v.map(ts).transpose()
}

fn micros(t: Option<Timestamp>) -> Option<i64> {
    t.map(|t| t.as_micros())
}

fn user_opt(row: &SqliteRow, col: &str) -> Result<Option<UserId>, StoreError> {
    let v: Option<Uuid> = row.try_get(col).map_err(map_db_err)?;
    Ok(v.map(UserId::from_uuid))
}

fn json_opt(row: &SqliteRow, col: &str) -> Result<Option<serde_json::Value>, StoreError> {
    let v: Option<String> = row.try_get(col).map_err(map_db_err)?;
    v.as_deref().map(json_from_db).transpose()
}

fn corrupt(what: &str) -> impl FnOnce(serde_json::Error) -> StoreError + '_ {
    move |e| StoreError::Database(format!("corrupt {what}: {e}"))
}

fn steps_from(text: &str) -> Result<Vec<WorkflowStep>, StoreError> {
    serde_json::from_value(json_from_db(text)?).map_err(corrupt("workflow steps"))
}

fn steps_to(steps: &[WorkflowStep]) -> Result<String, StoreError> {
    serde_json::to_string(steps).map_err(|e| StoreError::Invalid(e.to_string()))
}

fn change_to(c: &StagedChange) -> Result<String, StoreError> {
    let v = serde_json::to_value(c).map_err(|e| StoreError::Invalid(e.to_string()))?;
    validate_json(&v)?;
    json_to_db(&v)
}

fn failure_to(f: &serde_json::Value) -> Result<String, StoreError> {
    validate_json(f)?;
    json_to_db(f)
}

fn release(row: &SqliteRow) -> Result<Release, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    Ok(Release {
        id: ReleaseId::from_uuid(id),
        title: row.try_get("title").map_err(map_db_err)?,
        description: row.try_get("description").map_err(map_db_err)?,
        status: status.parse()?,
        scheduled_at: ts_opt(row, "scheduled_at")?,
        scheduled_by: user_opt(row, "scheduled_by")?,
        override_reason: row.try_get("override_reason").map_err(map_db_err)?,
        failure: json_opt(row, "failure")?,
        created_by: user_opt(row, "created_by")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
        applied_at: ts_opt(row, "applied_at")?,
        applied_by: user_opt(row, "applied_by")?,
        version: row.try_get("version").map_err(map_db_err)?,
    })
}

fn entry(row: &SqliteRow) -> Result<ReleaseEntry, StoreError> {
    let r: Uuid = row.try_get("release_id").map_err(map_db_err)?;
    let i: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let transition: String = row.try_get("transition").map_err(map_db_err)?;
    let changes = json_opt(row, "changes")?
        .map(|v| serde_json::from_value::<StagedChange>(v).map_err(corrupt("staged change")))
        .transpose()?;
    Ok(ReleaseEntry {
        release_id: ReleaseId::from_uuid(r),
        item_id: ContentId::from_uuid(i),
        position: row.try_get("position").map_err(map_db_err)?,
        transition: transition.parse()?,
        changes,
        base_version: row.try_get("base_version").map_err(map_db_err)?,
        entry_version: row.try_get("entry_version").map_err(map_db_err)?,
        added_by: user_opt(row, "added_by")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

fn workflow(row: &SqliteRow) -> Result<Workflow, StoreError> {
    let steps: String = row.try_get("steps").map_err(map_db_err)?;
    Ok(Workflow {
        type_name: row.try_get("type").map_err(map_db_err)?,
        steps: steps_from(&steps)?,
        updated_by: user_opt(row, "updated_by")?,
        updated_at: ts_col(row, "updated_at")?,
        version: row.try_get("version").map_err(map_db_err)?,
    })
}

fn review(row: &SqliteRow) -> Result<Review, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let target: String = row.try_get("target").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let rev: Option<Uuid> = row.try_get("revision_id").map_err(map_db_err)?;
    let steps: String = row.try_get("steps").map_err(map_db_err)?;
    let step: i64 = row.try_get("step").map_err(map_db_err)?;
    Ok(Review {
        id: ReviewId::from_uuid(id),
        target: ReviewTarget::parse(&target)?,
        revision_id: rev.map(RevisionId::from_uuid),
        entry_version: row.try_get("entry_version").map_err(map_db_err)?,
        status: status.parse()?,
        step: u32::try_from(step).map_err(|_| StoreError::Database("bad review step".into()))?,
        steps: steps_from(&steps)?,
        note: row.try_get("note").map_err(map_db_err)?,
        submitted_by: user_opt(row, "submitted_by")?,
        submitted_at: ts_col(row, "submitted_at")?,
        decided_at: ts_opt(row, "decided_at")?,
        version: row.try_get("version").map_err(map_db_err)?,
    })
}

fn link(row: &SqliteRow) -> Result<PreviewLink, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let kind: String = row.try_get("target_kind").map_err(map_db_err)?;
    let target: Uuid = row.try_get("target_id").map_err(map_db_err)?;
    let by: Uuid = row.try_get("created_by").map_err(map_db_err)?;
    Ok(PreviewLink {
        id: PreviewLinkId::from_uuid(id),
        target: PreviewTarget::from_parts(kind.parse::<PreviewTargetKind>()?, target),
        created_by: UserId::from_uuid(by),
        created_at: ts_col(row, "created_at")?,
        expires_at: ts_col(row, "expires_at")?,
        last_used_at: ts_opt(row, "last_used_at")?,
    })
}

/// The key of a versioned row.
enum Key {
    Id(Uuid),
    Text(String),
}

/// `Conflict` with the current version, or `NotFound`, for a versioned update that
/// matched no row.
async fn version_miss(repos: &SqliteRepos, table: &str, key_col: &str, key: Key) -> StoreError {
    let sql = format!("SELECT version FROM {table} WHERE {key_col} = ?");
    let q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql));
    let q = match key {
        Key::Id(id) => q.bind(id),
        Key::Text(t) => q.bind(t),
    };
    match run!(&repos.reader, |x| q.fetch_optional(x)) {
        Ok(Some(current_version)) => StoreError::Conflict { current_version },
        Ok(None) => StoreError::NotFound,
        Err(e) => e,
    }
}

#[async_trait]
impl ReleaseRepo for SqliteRepos {
    async fn insert(&self, r: &Release) -> Result<(), StoreError> {
        check_release(r)?;
        let failure = r.failure.as_ref().map(failure_to).transpose()?;
        run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO releases ({RELEASE_COLUMNS}) VALUES \
             (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )))
        .bind(r.id.as_uuid())
        .bind(&r.title)
        .bind(&r.description)
        .bind(r.status.as_str())
        .bind(micros(r.scheduled_at))
        .bind(r.scheduled_by.map(|u| u.as_uuid()))
        .bind(r.override_reason.as_deref())
        .bind(failure.as_deref())
        .bind(r.created_by.map(|u| u.as_uuid()))
        .bind(r.created_at.as_micros())
        .bind(r.updated_at.as_micros())
        .bind(micros(r.applied_at))
        .bind(r.applied_by.map(|u| u.as_uuid()))
        .bind(r.version)
        .execute(x))?;
        Ok(())
    }

    async fn get(&self, id: ReleaseId) -> Result<Option<Release>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RELEASE_COLUMNS} FROM releases WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(release).transpose()
    }

    async fn update(&self, r: &Release) -> Result<Release, StoreError> {
        check_release(r)?;
        let failure = r.failure.as_ref().map(failure_to).transpose()?;
        let row = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "UPDATE releases SET title = ?, description = ?, status = ?, scheduled_at = ?, \
             scheduled_by = ?, override_reason = ?, failure = ?, updated_at = ?, \
             applied_at = ?, applied_by = ?, version = version + 1 \
             WHERE id = ? AND version = ? RETURNING {RELEASE_COLUMNS}"
        )))
        .bind(&r.title)
        .bind(&r.description)
        .bind(r.status.as_str())
        .bind(micros(r.scheduled_at))
        .bind(r.scheduled_by.map(|u| u.as_uuid()))
        .bind(r.override_reason.as_deref())
        .bind(failure.as_deref())
        .bind(r.updated_at.as_micros())
        .bind(micros(r.applied_at))
        .bind(r.applied_by.map(|u| u.as_uuid()))
        .bind(r.id.as_uuid())
        .bind(r.version)
        .fetch_optional(x))?;
        match row {
            Some(row) => release(&row),
            None => Err(version_miss(self, "releases", "id", Key::Id(r.id.as_uuid())).await),
        }
    }

    async fn delete(&self, id: ReleaseId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM releases WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn list(&self, q: &ReleaseQuery) -> Result<Page<Release>, StoreError> {
        let limit = check_limit(q.limit)?;
        let after = q
            .cursor
            .as_deref()
            .map(|c| micros_cursor(c, RELEASE_CURSOR_TAG))
            .transpose()?;
        let mut sql = format!("SELECT {RELEASE_COLUMNS} FROM releases WHERE 1 = 1");
        if q.status.is_some() {
            sql.push_str(" AND status = ?");
        }
        if q.created_by.is_some() {
            sql.push_str(" AND created_by = ?");
        }
        if after.is_some() {
            sql.push_str(" AND (created_at, id) < (?, ?)");
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
        let mut query = sqlx::query(AssertSqlSafe(sql));
        if let Some(s) = q.status {
            query = query.bind(s.as_str());
        }
        if let Some(u) = q.created_by {
            query = query.bind(u.as_uuid());
        }
        if let Some((at, id)) = after {
            query = query.bind(at).bind(id);
        }
        let query = query.bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |x| query.fetch_all(x))?;
        let mut items = rows.iter().map(release).collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|l| encode_release_cursor(l.created_at, l.id))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn entry_counts(&self, ids: &[ReleaseId]) -> Result<Vec<(ReleaseId, u32)>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if ids.len() > MAX_ID_FILTER {
            return Err(StoreError::Invalid(format!(
                "at most {MAX_ID_FILTER} releases per call"
            )));
        }
        let marks = vec!["?"; ids.len()].join(", ");
        let mut query = sqlx::query(AssertSqlSafe(format!(
            "SELECT release_id, COUNT(*) AS n FROM release_items \
             WHERE release_id IN ({marks}) GROUP BY release_id"
        )));
        for id in ids {
            query = query.bind(id.as_uuid());
        }
        let rows = run!(&self.reader, |x| query.fetch_all(x))?;
        rows.iter()
            .map(|r| {
                let id: Uuid = r.try_get("release_id").map_err(map_db_err)?;
                let n: i64 = r.try_get("n").map_err(map_db_err)?;
                Ok((
                    ReleaseId::from_uuid(id),
                    u32::try_from(n).unwrap_or(u32::MAX),
                ))
            })
            .collect()
    }

    async fn entries(&self, release: ReleaseId) -> Result<Vec<ReleaseEntry>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENTRY_COLUMNS} FROM release_items WHERE release_id = ? \
             ORDER BY position, item_id"
        )))
        .bind(release.as_uuid())
        .fetch_all(x))?;
        rows.iter().map(entry).collect()
    }

    async fn entry(
        &self,
        release: ReleaseId,
        item: ContentId,
    ) -> Result<Option<ReleaseEntry>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENTRY_COLUMNS} FROM release_items WHERE release_id = ? AND item_id = ?"
        )))
        .bind(release.as_uuid())
        .bind(item.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(entry).transpose()
    }

    async fn put_entry(&self, e: &ReleaseEntry) -> Result<ReleaseEntry, StoreError> {
        let changes = e.changes.as_ref().map(change_to).transpose()?;
        let updated = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "UPDATE release_items SET transition = ?, changes = ?, base_version = ?, \
             updated_at = ?, entry_version = entry_version + 1 \
             WHERE release_id = ? AND item_id = ? RETURNING {ENTRY_COLUMNS}"
        )))
        .bind(e.transition.as_str())
        .bind(changes.as_deref())
        .bind(e.base_version)
        .bind(e.updated_at.as_micros())
        .bind(e.release_id.as_uuid())
        .bind(e.item_id.as_uuid())
        .fetch_optional(x))?;
        if let Some(row) = updated {
            return entry(&row);
        }
        let row = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO release_items ({ENTRY_COLUMNS}) VALUES (?, ?, \
             (SELECT COALESCE(MAX(position), 0) + 1 FROM release_items WHERE release_id = ?), \
             ?, ?, ?, 1, ?, ?) RETURNING {ENTRY_COLUMNS}"
        )))
        .bind(e.release_id.as_uuid())
        .bind(e.item_id.as_uuid())
        .bind(e.release_id.as_uuid())
        .bind(e.transition.as_str())
        .bind(changes.as_deref())
        .bind(e.base_version)
        .bind(e.added_by.map(|u| u.as_uuid()))
        .bind(e.updated_at.as_micros())
        .fetch_one(x))?;
        entry(&row)
    }

    async fn delete_entry(&self, release: ReleaseId, item: ContentId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM release_items WHERE release_id = ? AND item_id = ?"
        )
        .bind(release.as_uuid())
        .bind(item.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn releases_of(&self, item: ContentId) -> Result<Vec<Release>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RELEASE_COLUMNS} FROM releases WHERE status <> 'applied' AND id IN \
             (SELECT release_id FROM release_items WHERE item_id = ?) \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        )))
        .bind(item.as_uuid())
        .bind(i64::from(MAX_RELEASES_OF))
        .fetch_all(x))?;
        rows.iter().map(release).collect()
    }

    async fn lock(&self, item: ContentId, release: ReleaseId) -> Result<(), StoreError> {
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO release_locks (item_id, release_id) VALUES (?, ?)"
        )
        .bind(item.as_uuid())
        .bind(release.as_uuid())
        .execute(x))?;
        Ok(())
    }

    async fn unlock_all(&self, release: ReleaseId) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM release_locks WHERE release_id = ?"
        )
        .bind(release.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected())
    }

    async fn lock_of(&self, item: ContentId) -> Result<Option<ReleaseId>, StoreError> {
        let id: Option<Uuid> = run!(&self.reader, |x| sqlx::query_scalar(
            "SELECT release_id FROM release_locks WHERE item_id = ?"
        )
        .bind(item.as_uuid())
        .fetch_optional(x))?;
        Ok(id.map(ReleaseId::from_uuid))
    }

    async fn due(&self, now: Timestamp, limit: u32) -> Result<Vec<Release>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RELEASE_COLUMNS} FROM releases WHERE status = ? AND scheduled_at <= ? \
             ORDER BY scheduled_at, id LIMIT ?"
        )))
        .bind(ReleaseStatus::Scheduled.as_str())
        .bind(now.as_micros())
        .bind(i64::from(limit))
        .fetch_all(x))?;
        rows.iter().map(release).collect()
    }
}

#[async_trait]
impl WorkflowRepo for SqliteRepos {
    async fn get(&self, type_name: &str) -> Result<Option<Workflow>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(
            "SELECT type, steps, updated_by, updated_at, version FROM workflows WHERE type = ?"
        )
        .bind(type_name)
        .fetch_optional(x))?;
        row.as_ref().map(workflow).transpose()
    }

    async fn put(&self, w: &Workflow, expected: Option<i64>) -> Result<Workflow, StoreError> {
        validate_text("content type", &w.type_name)?;
        check_steps(&w.steps)?;
        let steps = steps_to(&w.steps)?;
        let row = match expected {
            None => run!(&self.writer, |x| sqlx::query(
                "INSERT INTO workflows (type, steps, updated_by, updated_at, version) \
                 VALUES (?, ?, ?, ?, 1) RETURNING type, steps, updated_by, updated_at, version"
            )
            .bind(&w.type_name)
            .bind(&steps)
            .bind(w.updated_by.map(|u| u.as_uuid()))
            .bind(w.updated_at.as_micros())
            .fetch_one(x))?,
            Some(v) => {
                let row = run!(&self.writer, |x| sqlx::query(
                    "UPDATE workflows SET steps = ?, updated_by = ?, updated_at = ?, \
                     version = version + 1 WHERE type = ? AND version = ? \
                     RETURNING type, steps, updated_by, updated_at, version"
                )
                .bind(&steps)
                .bind(w.updated_by.map(|u| u.as_uuid()))
                .bind(w.updated_at.as_micros())
                .bind(&w.type_name)
                .bind(v)
                .fetch_optional(x))?;
                match row {
                    Some(r) => r,
                    None => {
                        return Err(version_miss(
                            self,
                            "workflows",
                            "type",
                            Key::Text(w.type_name.clone()),
                        )
                        .await);
                    }
                }
            }
        };
        workflow(&row)
    }

    async fn delete(&self, type_name: &str) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM workflows WHERE type = ?"
        )
        .bind(type_name)
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn list(&self) -> Result<Vec<Workflow>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(
            "SELECT type, steps, updated_by, updated_at, version FROM workflows \
             ORDER BY CAST(type AS BLOB)"
        )
        .fetch_all(x))?;
        rows.iter().map(workflow).collect()
    }
}

#[async_trait]
impl ReviewRepo for SqliteRepos {
    async fn insert(&self, r: &Review) -> Result<(), StoreError> {
        check_review(r)?;
        let steps = steps_to(&r.steps)?;
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO reviews (id, target, item_id, release_id, revision_id, entry_version, \
             status, step, steps, note, submitted_by, submitted_at, decided_at, version) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id.as_uuid())
        .bind(r.target.key())
        .bind(r.target.item().as_uuid())
        .bind(r.target.release().map(|x| x.as_uuid()))
        .bind(r.revision_id.map(|x| x.as_uuid()))
        .bind(r.entry_version)
        .bind(r.status.as_str())
        .bind(i64::from(r.step))
        .bind(&steps)
        .bind(r.note.as_deref())
        .bind(r.submitted_by.map(|u| u.as_uuid()))
        .bind(r.submitted_at.as_micros())
        .bind(micros(r.decided_at))
        .bind(r.version)
        .execute(x))?;
        Ok(())
    }

    async fn get(&self, id: ReviewId) -> Result<Option<Review>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {REVIEW_COLUMNS} FROM reviews r WHERE r.id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(review).transpose()
    }

    async fn update(&self, r: &Review) -> Result<Review, StoreError> {
        check_review(r)?;
        let row = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "UPDATE reviews SET status = ?, step = ?, note = ?, decided_at = ?, \
             version = version + 1 WHERE id = ? AND version = ? RETURNING {REVIEW_RETURNING}"
        )))
        .bind(r.status.as_str())
        .bind(i64::from(r.step))
        .bind(r.note.as_deref())
        .bind(micros(r.decided_at))
        .bind(r.id.as_uuid())
        .bind(r.version)
        .fetch_optional(x))?;
        match row {
            Some(row) => review(&row),
            None => Err(version_miss(self, "reviews", "id", Key::Id(r.id.as_uuid())).await),
        }
    }

    async fn open_for(&self, target: &ReviewTarget) -> Result<Option<Review>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {REVIEW_COLUMNS} FROM reviews r WHERE r.target = ? AND r.status IN {OPEN}"
        )))
        .bind(target.key())
        .fetch_optional(x))?;
        row.as_ref().map(review).transpose()
    }

    async fn list(&self, q: &ReviewQuery) -> Result<Page<Review>, StoreError> {
        let limit = check_limit(q.limit)?;
        let after = q
            .cursor
            .as_deref()
            .map(|c| micros_cursor(c, REVIEW_CURSOR_TAG))
            .transpose()?;
        let mut sql = format!("SELECT {REVIEW_COLUMNS} FROM reviews r");
        if q.kind.is_some() {
            sql.push_str(" JOIN content_items c ON c.id = r.item_id");
        }
        sql.push_str(" WHERE 1 = 1");
        if q.status.is_some() {
            sql.push_str(" AND r.status = ?");
        }
        if q.kind.is_some() {
            sql.push_str(" AND c.type = ?");
        }
        if q.submitted_by.is_some() {
            sql.push_str(" AND r.submitted_by = ?");
        }
        if q.assignee.is_some() {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM review_assignees a WHERE a.review_id = r.id \
                 AND a.step = r.step AND a.user_id = ?)",
            );
        }
        if after.is_some() {
            sql.push_str(" AND (r.submitted_at, r.id) > (?, ?)");
        }
        sql.push_str(" ORDER BY r.submitted_at, r.id LIMIT ?");
        let mut query = sqlx::query(AssertSqlSafe(sql));
        if let Some(s) = q.status {
            query = query.bind(s.as_str());
        }
        if let Some(k) = &q.kind {
            query = query.bind(k.clone());
        }
        if let Some(u) = q.submitted_by {
            query = query.bind(u.as_uuid());
        }
        if let Some(u) = q.assignee {
            query = query.bind(u.as_uuid());
        }
        if let Some((at, id)) = after {
            query = query.bind(at).bind(id);
        }
        let query = query.bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |x| query.fetch_all(x))?;
        let mut items = rows.iter().map(review).collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|l| encode_review_cursor(l.submitted_at, l.id))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn mark_stale_item(&self, item: ContentId, at: Timestamp) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "UPDATE reviews SET status = 'stale', decided_at = ?, version = version + 1 \
             WHERE target = ? AND status IN {OPEN}"
        )))
        .bind(at.as_micros())
        .bind(ReviewTarget::Item(item).key())
        .execute(x))?;
        Ok(done.rows_affected())
    }

    async fn mark_stale_entry(
        &self,
        release: ReleaseId,
        item: ContentId,
        at: Timestamp,
    ) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "UPDATE reviews SET status = 'stale', decided_at = ?, version = version + 1 \
             WHERE target = ? AND status IN {OPEN}"
        )))
        .bind(at.as_micros())
        .bind(ReviewTarget::Entry(release, item).key())
        .execute(x))?;
        Ok(done.rows_affected())
    }

    async fn withdraw_for_type(&self, type_name: &str, at: Timestamp) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "UPDATE reviews SET status = 'withdrawn', decided_at = ?, version = version + 1 \
             WHERE status IN {OPEN} AND item_id IN \
             (SELECT id FROM content_items WHERE type = ?)"
        )))
        .bind(at.as_micros())
        .bind(type_name)
        .execute(x))?;
        Ok(done.rows_affected())
    }

    async fn decisions(&self, review_id: ReviewId) -> Result<Vec<ReviewDecision>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(
            "SELECT id, review_id, step, user_id, decision, comment, at FROM review_decisions \
             WHERE review_id = ? ORDER BY at, id"
        )
        .bind(review_id.as_uuid())
        .fetch_all(x))?;
        rows.iter()
            .map(|row| {
                let id: Uuid = row.try_get("id").map_err(map_db_err)?;
                let step: i64 = row.try_get("step").map_err(map_db_err)?;
                let d: String = row.try_get("decision").map_err(map_db_err)?;
                Ok(ReviewDecision {
                    id,
                    review_id,
                    step: u32::try_from(step)
                        .map_err(|_| StoreError::Database("bad decision step".into()))?,
                    user_id: user_opt(row, "user_id")?,
                    decision: d.parse()?,
                    comment: row.try_get("comment").map_err(map_db_err)?,
                    at: ts_col(row, "at")?,
                })
            })
            .collect()
    }

    async fn add_decision(&self, d: &ReviewDecision) -> Result<(), StoreError> {
        if let Some(c) = &d.comment {
            validate_text("review comment", c)?;
        }
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO review_decisions (id, review_id, step, user_id, decision, comment, at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(d.id)
        .bind(d.review_id.as_uuid())
        .bind(i64::from(d.step))
        .bind(d.user_id.map(|u| u.as_uuid()))
        .bind(d.decision.as_str())
        .bind(d.comment.as_deref())
        .bind(d.at.as_micros())
        .execute(x))?;
        Ok(())
    }

    async fn assignees(&self, review_id: ReviewId) -> Result<Vec<(u32, UserId)>, StoreError> {
        let rows = run!(&self.reader, |x| {
            sqlx::query(
            "SELECT step, user_id FROM review_assignees WHERE review_id = ? ORDER BY step, user_id"
        )
        .bind(review_id.as_uuid())
        .fetch_all(x)
        })?;
        rows.iter()
            .map(|row| {
                let step: i64 = row.try_get("step").map_err(map_db_err)?;
                let u: Uuid = row.try_get("user_id").map_err(map_db_err)?;
                Ok((
                    u32::try_from(step)
                        .map_err(|_| StoreError::Database("bad assignee step".into()))?,
                    UserId::from_uuid(u),
                ))
            })
            .collect()
    }

    async fn set_assignees(
        &self,
        review_id: ReviewId,
        step: u32,
        users: &[UserId],
    ) -> Result<(), StoreError> {
        if users.len() > MAX_ID_FILTER {
            return Err(StoreError::Invalid("too many assignees".into()));
        }
        run!(&self.writer, |x| sqlx::query(
            "DELETE FROM review_assignees WHERE review_id = ? AND step = ?"
        )
        .bind(review_id.as_uuid())
        .bind(i64::from(step))
        .execute(x))?;
        for u in users {
            run!(&self.writer, |x| sqlx::query(
                "INSERT INTO review_assignees (review_id, step, user_id) VALUES (?, ?, ?) \
                 ON CONFLICT DO NOTHING"
            )
            .bind(review_id.as_uuid())
            .bind(i64::from(step))
            .bind(u.as_uuid())
            .execute(x))?;
        }
        Ok(())
    }

    async fn counts_for(&self, user: UserId) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.reader, |x| sqlx::query_scalar(
            "SELECT COUNT(*) FROM reviews r WHERE r.status = 'in_review' AND EXISTS \
             (SELECT 1 FROM review_assignees a WHERE a.review_id = r.id AND a.step = r.step \
             AND a.user_id = ?)"
        )
        .bind(user.as_uuid())
        .fetch_one(x))?;
        Ok(n.max(0) as u64)
    }
}

#[async_trait]
impl PreviewLinkRepo for SqliteRepos {
    async fn insert(&self, l: &PreviewLink, token_hash: &str) -> Result<(), StoreError> {
        validate_text("token hash", token_hash)?;
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO preview_links (id, token_hash, target_kind, target_id, created_by, \
             created_at, expires_at, last_used_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(l.id.as_uuid())
        .bind(token_hash)
        .bind(l.target.kind().as_str())
        .bind(l.target.id())
        .bind(l.created_by.as_uuid())
        .bind(l.created_at.as_micros())
        .bind(l.expires_at.as_micros())
        .bind(micros(l.last_used_at))
        .execute(x))?;
        Ok(())
    }

    async fn get(&self, id: PreviewLinkId) -> Result<Option<PreviewLink>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {LINK_COLUMNS} FROM preview_links WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(link).transpose()
    }

    async fn get_by_hash(&self, token_hash: &str) -> Result<Option<PreviewLink>, StoreError> {
        validate_text("token hash", token_hash)?;
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {LINK_COLUMNS} FROM preview_links WHERE token_hash = ?"
        )))
        .bind(token_hash)
        .fetch_optional(x))?;
        row.as_ref().map(link).transpose()
    }

    async fn touch(&self, id: PreviewLinkId, at: Timestamp) -> Result<(), StoreError> {
        run!(&self.writer, |x| sqlx::query(
            "UPDATE preview_links SET last_used_at = ? WHERE id = ?"
        )
        .bind(at.as_micros())
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(())
    }

    async fn list_for(&self, target: &PreviewTarget) -> Result<Vec<PreviewLink>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {LINK_COLUMNS} FROM preview_links WHERE target_kind = ? AND target_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT 200"
        )))
        .bind(target.kind().as_str())
        .bind(target.id())
        .fetch_all(x))?;
        rows.iter().map(link).collect()
    }

    async fn delete(&self, id: PreviewLinkId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM preview_links WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete_for_target(&self, target: &PreviewTarget) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM preview_links WHERE target_kind = ? AND target_id = ?"
        )
        .bind(target.kind().as_str())
        .bind(target.id())
        .execute(x))?;
        Ok(done.rows_affected())
    }

    async fn prune(&self, now: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM preview_links WHERE id IN \
             (SELECT id FROM preview_links WHERE expires_at < ? ORDER BY expires_at LIMIT ?)"
        )
        .bind(now.as_micros())
        .bind(limit)
        .execute(x))?;
        Ok(done.rows_affected())
    }
}
