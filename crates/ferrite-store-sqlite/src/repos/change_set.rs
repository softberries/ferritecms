//! AI change sets and their changes (`0018_change_sets`) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{
    AgentKind, Change, ChangeSet, ChangeSetId, ChangeSetQuery, ChangeSummary, Image, ImageState,
    NewChange, NewChangeSet, Page, Timestamp, UserId,
};
use ferrite_store::{
    ChangeSetRepo, MAX_CHANGES_READ, StoreError, change_set_cursor, check_limit, check_new_change,
    check_new_change_set, check_prune_limit, encode_change_set_cursor,
};
use sqlx::{AssertSqlSafe, Row, sqlite::SqliteRow};
use uuid::Uuid;

use super::SqliteRepos;
use crate::exec::{map_db_err, run};

const SET_COLUMNS: &str = "s.id, s.user_id, s.agent_kind, s.agent_id, s.client, \
     s.open_key IS NOT NULL AS open, s.started_at, s.last_at, s.changes, s.reverted_at, \
     s.reverted_by, (SELECT COUNT(*) FROM change_set_changes c WHERE c.set_id = s.id \
     AND c.reverted_at IS NOT NULL) AS reverted_changes";

const CHANGE_HEAD: &str = "set_id, seq, at, op, object_kind, object_id, label, before_hash, \
     after_hash, reverted_at, reverted_by, revert_forced";

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

fn user_opt(row: &SqliteRow, col: &str) -> Result<Option<UserId>, StoreError> {
    let v: Option<Uuid> = row.try_get(col).map_err(map_db_err)?;
    Ok(v.map(UserId::from_uuid))
}

fn count(row: &SqliteRow, col: &str) -> Result<u32, StoreError> {
    let v: i64 = row.try_get(col).map_err(map_db_err)?;
    u32::try_from(v).map_err(|_| StoreError::Database(format!("corrupt {col}: {v}")))
}

fn set(row: &SqliteRow) -> Result<ChangeSet, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let kind: String = row.try_get("agent_kind").map_err(map_db_err)?;
    Ok(ChangeSet {
        id: ChangeSetId::from_uuid(id),
        user_id: user_opt(row, "user_id")?,
        agent_kind: kind.parse::<AgentKind>()?,
        agent_id: row.try_get("agent_id").map_err(map_db_err)?,
        client: row.try_get("client").map_err(map_db_err)?,
        open: row.try_get("open").map_err(map_db_err)?,
        started_at: ts_col(row, "started_at")?,
        last_at: ts_col(row, "last_at")?,
        changes: count(row, "changes")?,
        reverted_changes: count(row, "reverted_changes")?,
        reverted_at: ts_opt(row, "reverted_at")?,
        reverted_by: user_opt(row, "reverted_by")?,
    })
}

fn state(hash: &Option<String>, has_bytes: bool) -> ImageState {
    match (hash, has_bytes) {
        (None, _) => ImageState::Absent,
        (Some(_), true) => ImageState::Present,
        (Some(_), false) => ImageState::Skipped,
    }
}

fn summary(row: &SqliteRow) -> Result<ChangeSummary, StoreError> {
    let id: Uuid = row.try_get("set_id").map_err(map_db_err)?;
    let before_hash: Option<String> = row.try_get("before_hash").map_err(map_db_err)?;
    let after_hash: Option<String> = row.try_get("after_hash").map_err(map_db_err)?;
    let has_before: bool = row.try_get("has_before").map_err(map_db_err)?;
    let has_after: bool = row.try_get("has_after").map_err(map_db_err)?;
    Ok(ChangeSummary {
        set_id: ChangeSetId::from_uuid(id),
        seq: count(row, "seq")?,
        at: ts_col(row, "at")?,
        op: row.try_get("op").map_err(map_db_err)?,
        object_kind: row.try_get("object_kind").map_err(map_db_err)?,
        object_id: row.try_get("object_id").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
        before_state: state(&before_hash, has_before),
        before_hash,
        after_state: state(&after_hash, has_after),
        after_hash,
        reverted_at: ts_opt(row, "reverted_at")?,
        reverted_by: user_opt(row, "reverted_by")?,
        revert_forced: row.try_get("revert_forced").map_err(map_db_err)?,
    })
}

fn change(row: &SqliteRow) -> Result<Change, StoreError> {
    let id: Uuid = row.try_get("set_id").map_err(map_db_err)?;
    let before_hash: Option<String> = row.try_get("before_hash").map_err(map_db_err)?;
    let after_hash: Option<String> = row.try_get("after_hash").map_err(map_db_err)?;
    let before: Option<Vec<u8>> = row.try_get("before_image").map_err(map_db_err)?;
    let after: Option<Vec<u8>> = row.try_get("after_image").map_err(map_db_err)?;
    Ok(Change {
        set_id: ChangeSetId::from_uuid(id),
        seq: count(row, "seq")?,
        at: ts_col(row, "at")?,
        op: row.try_get("op").map_err(map_db_err)?,
        object_kind: row.try_get("object_kind").map_err(map_db_err)?,
        object_id: row.try_get("object_id").map_err(map_db_err)?,
        label: row.try_get("label").map_err(map_db_err)?,
        before: Image::from_columns(before_hash, before),
        after: Image::from_columns(after_hash, after),
        reverted_at: ts_opt(row, "reverted_at")?,
        reverted_by: user_opt(row, "reverted_by")?,
        revert_forced: row.try_get("revert_forced").map_err(map_db_err)?,
    })
}

/// The first `seq` of `n` just reserved, from the count after the update.
fn first_seq(changes: i64, n: u32) -> Result<u32, StoreError> {
    u32::try_from(changes - i64::from(n) + 1)
        .map_err(|_| StoreError::Database(format!("corrupt change count {changes}")))
}

#[async_trait]
impl ChangeSetRepo for SqliteRepos {
    async fn extend_open(
        &self,
        open_key: &str,
        now: Timestamp,
        idle_since: Timestamp,
        max_changes: u32,
        n: u32,
    ) -> Result<Option<(ChangeSetId, u32)>, StoreError> {
        ferrite_store::validate_text("change set open key", open_key)?;
        let row: Option<(Uuid, i64)> = run!(&self.writer, |x| sqlx::query_as(
            "UPDATE change_sets SET changes = changes + ?, last_at = MAX(last_at, ?) \
             WHERE open_key = ? AND last_at >= ? AND changes < ? RETURNING id, changes"
        )
        .bind(i64::from(n))
        .bind(now.as_micros())
        .bind(open_key)
        .bind(idle_since.as_micros())
        .bind(i64::from(max_changes))
        .fetch_optional(x))?;
        row.map(|(id, c)| Ok((ChangeSetId::from_uuid(id), first_seq(c, n)?)))
            .transpose()
    }

    async fn open(&self, s: &NewChangeSet, n: u32) -> Result<(ChangeSetId, u32), StoreError> {
        check_new_change_set(s)?;
        run!(&self.writer, |x| sqlx::query(
            "UPDATE change_sets SET open_key = NULL \
             WHERE open_key = ? AND (last_at < ? OR changes >= ?)"
        )
        .bind(&s.open_key)
        .bind(s.idle_since.as_micros())
        .bind(i64::from(s.max_changes))
        .execute(x))?;
        let (id, c): (Uuid, i64) = run!(&self.writer, |x| sqlx::query_as(
            "INSERT INTO change_sets (id, user_id, agent_kind, agent_id, client, open_key, \
             started_at, last_at, changes) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (open_key) DO UPDATE SET changes = change_sets.changes + ?, \
             last_at = MAX(change_sets.last_at, excluded.last_at) RETURNING id, changes"
        )
        .bind(s.id.as_uuid())
        .bind(s.user_id.map(|u| u.as_uuid()))
        .bind(s.agent_kind.as_str())
        .bind(s.agent_id)
        .bind(&s.client)
        .bind(&s.open_key)
        .bind(s.now.as_micros())
        .bind(s.now.as_micros())
        .bind(i64::from(n))
        .bind(i64::from(n))
        .fetch_one(x))?;
        Ok((ChangeSetId::from_uuid(id), first_seq(c, n)?))
    }

    async fn insert_change(&self, c: &NewChange) -> Result<(), StoreError> {
        check_new_change(c)?;
        let (before_hash, before) = c.before.columns();
        let (after_hash, after) = c.after.columns();
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO change_set_changes (set_id, seq, at, op, object_kind, object_id, \
             label, before_image, before_hash, after_image, after_hash) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(c.set_id.as_uuid())
        .bind(i64::from(c.seq))
        .bind(c.at.as_micros())
        .bind(&c.op)
        .bind(&c.object_kind)
        .bind(&c.object_id)
        .bind(&c.label)
        .bind(before)
        .bind(before_hash)
        .bind(after)
        .bind(after_hash)
        .execute(x))?;
        Ok(())
    }

    async fn get(&self, id: ChangeSetId) -> Result<Option<ChangeSet>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {SET_COLUMNS} FROM change_sets s WHERE s.id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(set).transpose()
    }

    async fn list(&self, q: &ChangeSetQuery) -> Result<Page<ChangeSet>, StoreError> {
        let limit = check_limit(q.limit)?;
        let after = q.cursor.as_deref().map(change_set_cursor).transpose()?;
        let mut sql = format!("SELECT {SET_COLUMNS} FROM change_sets s WHERE 1 = 1");
        if q.user.is_some() {
            sql.push_str(" AND s.user_id = ?");
        }
        if q.agent_id.is_some() {
            sql.push_str(" AND s.agent_id = ?");
        }
        match q.open {
            Some(true) => sql.push_str(" AND s.open_key IS NOT NULL AND s.last_at >= ?"),
            Some(false) => sql.push_str(" AND (s.open_key IS NULL OR s.last_at < ?)"),
            None => {}
        }
        if after.is_some() {
            sql.push_str(" AND (s.last_at, s.id) < (?, ?)");
        }
        sql.push_str(" ORDER BY s.last_at DESC, s.id DESC LIMIT ?");
        let mut query = sqlx::query(AssertSqlSafe(sql));
        if let Some(u) = q.user {
            query = query.bind(u.as_uuid());
        }
        if let Some(a) = q.agent_id {
            query = query.bind(a);
        }
        if q.open.is_some() {
            query = query.bind(q.idle_since.as_micros());
        }
        if let Some((at, id)) = after {
            query = query.bind(at).bind(id);
        }
        let query = query.bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |x| query.fetch_all(x))?;
        let mut items = rows.iter().map(set).collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|s| encode_change_set_cursor(s.last_at, s.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn summaries(&self, id: ChangeSetId) -> Result<Vec<ChangeSummary>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {CHANGE_HEAD}, before_image IS NOT NULL AS has_before, \
             after_image IS NOT NULL AS has_after FROM change_set_changes \
             WHERE set_id = ? ORDER BY seq LIMIT ?"
        )))
        .bind(id.as_uuid())
        .bind(i64::from(MAX_CHANGES_READ))
        .fetch_all(x))?;
        rows.iter().map(summary).collect()
    }

    async fn change(&self, id: ChangeSetId, seq: u32) -> Result<Option<Change>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {CHANGE_HEAD}, before_image, after_image FROM change_set_changes \
             WHERE set_id = ? AND seq = ?"
        )))
        .bind(id.as_uuid())
        .bind(i64::from(seq))
        .fetch_optional(x))?;
        row.as_ref().map(change).transpose()
    }

    async fn changes(&self, id: ChangeSetId) -> Result<Vec<Change>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {CHANGE_HEAD}, before_image, after_image FROM change_set_changes \
             WHERE set_id = ? ORDER BY seq LIMIT ?"
        )))
        .bind(id.as_uuid())
        .bind(i64::from(MAX_CHANGES_READ))
        .fetch_all(x))?;
        rows.iter().map(change).collect()
    }

    async fn mark_reverted(
        &self,
        id: ChangeSetId,
        seqs: &[u32],
        by: UserId,
        at: Timestamp,
        forced: &[u32],
    ) -> Result<(), StoreError> {
        for seq in seqs {
            run!(&self.writer, |x| sqlx::query(
                "UPDATE change_set_changes SET reverted_at = ?, reverted_by = ?, \
                 revert_forced = ? WHERE set_id = ? AND seq = ?"
            )
            .bind(at.as_micros())
            .bind(by.as_uuid())
            .bind(forced.contains(seq))
            .bind(id.as_uuid())
            .bind(i64::from(*seq))
            .execute(x))?;
        }
        run!(&self.writer, |x| sqlx::query(
            "UPDATE change_sets SET reverted_at = ?, reverted_by = ? WHERE id = ? \
             AND NOT EXISTS (SELECT 1 FROM change_set_changes \
             WHERE set_id = ? AND reverted_at IS NULL)"
        )
        .bind(at.as_micros())
        .bind(by.as_uuid())
        .bind(id.as_uuid())
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(())
    }

    async fn close(&self, id: ChangeSetId) -> Result<(), StoreError> {
        run!(&self.writer, |x| sqlx::query(
            "UPDATE change_sets SET open_key = NULL WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(())
    }

    async fn changes_since(
        &self,
        user: Option<UserId>,
        since: Timestamp,
    ) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.reader, |x| sqlx::query_scalar(
            "SELECT COUNT(*) FROM change_set_changes c JOIN change_sets s ON s.id = c.set_id \
             WHERE c.at >= ? AND (? IS NULL OR s.user_id = ?)"
        )
        .bind(since.as_micros())
        .bind(user.map(|u| u.as_uuid()))
        .bind(user.map(|u| u.as_uuid()))
        .fetch_one(x))?;
        Ok(n.max(0) as u64)
    }

    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM change_sets WHERE id IN (SELECT id FROM change_sets \
             WHERE last_at < ? ORDER BY last_at LIMIT ?)"
        )
        .bind(before.as_micros())
        .bind(limit)
        .execute(x))?;
        Ok(done.rows_affected())
    }
}
