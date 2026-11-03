//! Suppressions, health runs, findings and the link cache (`0019_content_health`) on
//! Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    ContentId, ContentItem, FindingQuery, HealthCounters, HealthRun, HealthRunId, HealthRunStatus,
    LinkCheck, NewFinding, Page, RedirectId, StoredFinding, Suppression, Timestamp, UserId,
};
use ferrite_store::{
    MAX_FINDING_BATCH, MAX_LINK_BATCH, QualityRepo, RunEnd, StoreError, check_finding,
    check_finding_query, check_id_list, check_len, check_limit, check_link, check_run,
    check_suppression, encode_finding_cursor, link_url_hash, validate_text,
};
use sqlx::{AssertSqlSafe, Postgres, QueryBuilder, Row, postgres::PgRow, types::Json};
use uuid::Uuid;

use super::PgRepos;
use crate::{
    codec::{self, CONTENT_COLUMNS},
    exec::{map_db_err, run},
};

const RUN_COLUMNS: &str = "id, status, trigger, requested_by, node, started_at, finished_at, \
     items_checked, paths_checked, urls_checked, totals, truncated, error";

const FINDING_COLUMNS: &str = "run_id, seq, rule, severity, category, item_id, redirect_id, \
     item_type, locale, key, context, detail";

const LINK_COLUMNS: &str = "url, host, status, http_status, detail, failures, first_failed_at, \
     checked_at, last_seen_at";

fn pg(t: Timestamp) -> DateTime<Utc> {
    t.into()
}

fn ts_col(row: &PgRow, col: &str) -> Result<Timestamp, StoreError> {
    let at: DateTime<Utc> = row.try_get(col).map_err(map_db_err)?;
    Ok(at.into())
}

fn ts_opt(row: &PgRow, col: &str) -> Result<Option<Timestamp>, StoreError> {
    let at: Option<DateTime<Utc>> = row.try_get(col).map_err(map_db_err)?;
    Ok(at.map(Into::into))
}

fn suppression(row: &PgRow) -> Result<Suppression, StoreError> {
    let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let user: Option<Uuid> = row.try_get("user_id").map_err(map_db_err)?;
    Ok(Suppression {
        item_id: ContentId::from_uuid(item),
        rule: row.try_get("rule").map_err(map_db_err)?,
        key: row.try_get("key").map_err(map_db_err)?,
        reason: row.try_get("reason").map_err(map_db_err)?,
        user_id: user.map(UserId::from_uuid),
        created_at: ts_col(row, "created_at")?,
    })
}

fn health_run(row: &PgRow) -> Result<HealthRun, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let by: Option<Uuid> = row.try_get("requested_by").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let trigger: String = row.try_get("trigger").map_err(map_db_err)?;
    let totals: Json<serde_json::Value> = row.try_get("totals").map_err(map_db_err)?;
    Ok(HealthRun {
        id: HealthRunId::from_uuid(id),
        status: status.parse()?,
        trigger: trigger.parse()?,
        requested_by: by.map(UserId::from_uuid),
        node: row.try_get("node").map_err(map_db_err)?,
        started_at: ts_col(row, "started_at")?,
        finished_at: ts_opt(row, "finished_at")?,
        counters: HealthCounters {
            items: row.try_get("items_checked").map_err(map_db_err)?,
            paths: row.try_get("paths_checked").map_err(map_db_err)?,
            urls: row.try_get("urls_checked").map_err(map_db_err)?,
        },
        totals: totals.0,
        truncated: row.try_get("truncated").map_err(map_db_err)?,
        error: row.try_get("error").map_err(map_db_err)?,
    })
}

fn finding(row: &PgRow) -> Result<StoredFinding, StoreError> {
    let run: Uuid = row.try_get("run_id").map_err(map_db_err)?;
    let item: Option<Uuid> = row.try_get("item_id").map_err(map_db_err)?;
    let redirect: Option<Uuid> = row.try_get("redirect_id").map_err(map_db_err)?;
    let detail: Json<serde_json::Value> = row.try_get("detail").map_err(map_db_err)?;
    Ok(StoredFinding {
        run_id: HealthRunId::from_uuid(run),
        seq: row.try_get("seq").map_err(map_db_err)?,
        finding: NewFinding {
            rule: row.try_get("rule").map_err(map_db_err)?,
            severity: row.try_get("severity").map_err(map_db_err)?,
            category: row.try_get("category").map_err(map_db_err)?,
            item_id: item.map(ContentId::from_uuid),
            redirect_id: redirect.map(RedirectId::from_uuid),
            item_type: row.try_get("item_type").map_err(map_db_err)?,
            locale: row.try_get("locale").map_err(map_db_err)?,
            key: row.try_get("key").map_err(map_db_err)?,
            context: row.try_get("context").map_err(map_db_err)?,
            detail: detail.0,
        },
    })
}

fn link(row: &PgRow) -> Result<LinkCheck, StoreError> {
    let status: String = row.try_get("status").map_err(map_db_err)?;
    Ok(LinkCheck {
        url: row.try_get("url").map_err(map_db_err)?,
        host: row.try_get("host").map_err(map_db_err)?,
        status: status.parse()?,
        http_status: row.try_get("http_status").map_err(map_db_err)?,
        detail: row.try_get("detail").map_err(map_db_err)?,
        failures: row.try_get("failures").map_err(map_db_err)?,
        first_failed_at: ts_opt(row, "first_failed_at")?,
        checked_at: ts_col(row, "checked_at")?,
        last_seen_at: ts_col(row, "last_seen_at")?,
    })
}

#[async_trait]
impl QualityRepo for PgRepos {
    async fn suppressions_for(&self, items: &[ContentId]) -> Result<Vec<Suppression>, StoreError> {
        check_id_list("items", items)?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT item_id, rule, key, reason, user_id, created_at FROM quality_suppressions \
             WHERE item_id IN (",
        );
        let mut list = qb.separated(", ");
        for id in items {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(") ORDER BY item_id, rule, key");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        rows.iter().map(suppression).collect()
    }

    async fn suppress(&self, s: &Suppression) -> Result<(), StoreError> {
        check_suppression(s)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO quality_suppressions (item_id, rule, key, reason, user_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (item_id, rule, key) DO UPDATE SET \
             reason = excluded.reason, user_id = excluded.user_id, \
             created_at = excluded.created_at"
        )
        .bind(s.item_id.as_uuid())
        .bind(&s.rule)
        .bind(&s.key)
        .bind(&s.reason)
        .bind(s.user_id.map(|u| u.as_uuid()))
        .bind(pg(s.created_at))
        .execute(e))
        .map(|_| ())
    }

    async fn unsuppress(&self, item: ContentId, rule: &str, key: &str) -> Result<bool, StoreError> {
        validate_text("suppression rule", rule)?;
        validate_text("suppression key", key)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM quality_suppressions WHERE item_id = $1 AND rule = $2 AND key = $3"
        )
        .bind(item.as_uuid())
        .bind(rule)
        .bind(key)
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn published_page(
        &self,
        after: Option<ContentId>,
        limit: u32,
    ) -> Result<Vec<ContentItem>, StoreError> {
        let limit = check_limit(limit)?;
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items \
             WHERE status IN ('published', 'scheduled') AND type <> 'media'"
        ));
        if let Some(after) = after {
            qb.push(" AND id > ").push_bind(after.as_uuid());
        }
        qb.push(" ORDER BY id LIMIT ").push_bind(i64::from(limit));
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        rows.iter().map(codec::content_item).collect()
    }

    async fn last_revision_at(
        &self,
        items: &[ContentId],
    ) -> Result<Vec<(ContentId, Timestamp)>, StoreError> {
        check_id_list("items", items)?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT item_id, MAX(created_at) AS at FROM revisions WHERE item_id IN (",
        );
        let mut list = qb.separated(", ");
        for id in items {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(") GROUP BY item_id ORDER BY item_id");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        rows.iter()
            .map(|r| {
                let id: Uuid = r.try_get("item_id").map_err(map_db_err)?;
                Ok((ContentId::from_uuid(id), ts_col(r, "at")?))
            })
            .collect()
    }

    async fn insert_run(&self, r: &HealthRun) -> Result<(), StoreError> {
        check_run(r)?;
        run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO health_runs ({RUN_COLUMNS}) VALUES \
             ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"
        )))
        .bind(r.id.as_uuid())
        .bind(r.status.as_str())
        .bind(r.trigger.as_str())
        .bind(r.requested_by.map(|u| u.as_uuid()))
        .bind(&r.node)
        .bind(pg(r.started_at))
        .bind(r.finished_at.map(pg))
        .bind(r.counters.items)
        .bind(r.counters.paths)
        .bind(r.counters.urls)
        .bind(Json(&r.totals))
        .bind(r.truncated)
        .bind(&r.error)
        .execute(e))
        .map(|_| ())
    }

    async fn finish_run(&self, id: HealthRunId, end: &RunEnd) -> Result<(), StoreError> {
        if let Some(e) = &end.error {
            validate_text("health run error", e)?;
        }
        ferrite_store::validate_json(&end.totals)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE health_runs SET status = $1, finished_at = $2, items_checked = $3, \
             paths_checked = $4, urls_checked = $5, totals = $6, truncated = $7, error = $8 \
             WHERE id = $9"
        )
        .bind(end.status.as_str())
        .bind(pg(end.finished_at))
        .bind(end.counters.items)
        .bind(end.counters.paths)
        .bind(end.counters.urls)
        .bind(Json(&end.totals))
        .bind(end.truncated)
        .bind(&end.error)
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn abandon_running(&self, now: Timestamp) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE health_runs SET status = 'abandoned', finished_at = $1 \
             WHERE status = 'running'"
        )
        .bind(pg(now))
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn get_run(&self, id: HealthRunId) -> Result<Option<HealthRun>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM health_runs WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(health_run).transpose()
    }

    async fn latest_run(&self, status: HealthRunStatus) -> Result<Option<HealthRun>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM health_runs WHERE status = $1 \
             ORDER BY started_at DESC, id DESC LIMIT 1"
        )))
        .bind(status.as_str())
        .fetch_optional(e))?;
        row.as_ref().map(health_run).transpose()
    }

    async fn prune_runs(
        &self,
        before: Timestamp,
        keep: Option<HealthRunId>,
    ) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM health_runs WHERE started_at < $1 AND id IS DISTINCT FROM $2"
        )
        .bind(pg(before))
        .bind(keep.map(|k| k.as_uuid()))
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn insert_findings(
        &self,
        run_id: HealthRunId,
        first_seq: i64,
        findings: &[NewFinding],
    ) -> Result<(), StoreError> {
        check_len("findings", findings.len(), MAX_FINDING_BATCH)?;
        if findings.is_empty() {
            return Ok(());
        }
        let mut rows = Vec::with_capacity(findings.len());
        for (i, f) in findings.iter().enumerate() {
            check_finding(f)?;
            let seq = first_seq + i64::try_from(i).unwrap_or(i64::MAX);
            rows.push((seq, f, Json(&f.detail)));
        }
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "INSERT INTO health_findings ({FINDING_COLUMNS}) "
        ));
        qb.push_values(rows.iter(), |mut b, (seq, f, detail)| {
            b.push_bind(run_id.as_uuid())
                .push_bind(*seq)
                .push_bind(&f.rule)
                .push_bind(&f.severity)
                .push_bind(&f.category)
                .push_bind(f.item_id.map(|i| i.as_uuid()))
                .push_bind(f.redirect_id.map(|r| r.as_uuid()))
                .push_bind(&f.item_type)
                .push_bind(&f.locale)
                .push_bind(&f.key)
                .push_bind(&f.context)
                .push_bind(*detail);
        });
        run!(&self.exec, |e| qb.build().execute(e)).map(|_| ())
    }

    async fn delete_findings_except(&self, run_id: HealthRunId) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM health_findings WHERE run_id <> $1"
        )
        .bind(run_id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
    }

    async fn findings(&self, q: &FindingQuery) -> Result<Page<StoredFinding>, StoreError> {
        let (run_id, after) = check_finding_query(q)?;
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {FINDING_COLUMNS} FROM health_findings WHERE run_id = "
        ));
        qb.push_bind(run_id.as_uuid());
        qb.push(" AND seq > ").push_bind(after);
        for (col, v) in [
            ("category", &q.category),
            ("rule", &q.rule),
            ("severity", &q.severity),
            ("item_type", &q.item_type),
            ("locale", &q.locale),
        ] {
            if let Some(v) = v {
                qb.push(format!(" AND {col} = ")).push_bind(v.clone());
            }
        }
        qb.push(" ORDER BY seq LIMIT ")
            .push_bind(i64::from(q.limit) + 1);
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        let mut items = rows.iter().map(finding).collect::<Result<Vec<_>, _>>()?;
        let limit = usize::try_from(q.limit).unwrap_or(usize::MAX);
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            items.last().map(|f| encode_finding_cursor(run_id, f.seq))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn link_get(&self, urls: &[String]) -> Result<Vec<LinkCheck>, StoreError> {
        check_len("urls", urls.len(), MAX_LINK_BATCH)?;
        if urls.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {LINK_COLUMNS} FROM link_checks WHERE url_hash IN ("
        ));
        let mut list = qb.separated(", ");
        for u in urls {
            list.push_bind(link_url_hash(u));
        }
        list.push_unseparated(")");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        rows.iter().map(link).collect()
    }

    async fn link_put(&self, l: &LinkCheck) -> Result<(), StoreError> {
        check_link(l)?;
        run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO link_checks (url_hash, {LINK_COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (url_hash) DO UPDATE SET \
             url = excluded.url, host = excluded.host, status = excluded.status, \
             http_status = excluded.http_status, detail = excluded.detail, \
             failures = excluded.failures, first_failed_at = excluded.first_failed_at, \
             checked_at = excluded.checked_at, last_seen_at = excluded.last_seen_at"
        )))
        .bind(link_url_hash(&l.url))
        .bind(&l.url)
        .bind(&l.host)
        .bind(l.status.as_str())
        .bind(l.http_status)
        .bind(&l.detail)
        .bind(l.failures)
        .bind(l.first_failed_at.map(pg))
        .bind(pg(l.checked_at))
        .bind(pg(l.last_seen_at))
        .execute(e))
        .map(|_| ())
    }

    async fn link_prune(&self, unseen_before: Timestamp) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM link_checks WHERE last_seen_at < $1"
        )
        .bind(pg(unseen_before))
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
