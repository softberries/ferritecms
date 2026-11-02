//! Webhook endpoints, events, deliveries and attempts (`0016_webhooks`) on Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    Attempt, Delivery, DeliveryCounts, DeliveryId, DeliveryListed, DeliveryQuery, DeliveryStatus,
    DeliveryUpdate, DisabledReason, Page, Timestamp, UserId, WebhookEndpoint, WebhookEventId,
    WebhookEventRow, WebhookId,
};
use ferrite_store::{
    MAX_PRUNE_BATCH, MAX_WEBHOOK_ENDPOINTS_READ, SCHEMA_META_INSTALL_ID, StoreError, WebhookRepo,
    check_limit, check_prune_limit, check_webhook_attempt, check_webhook_endpoint,
    encode_webhook_delivery_cursor, validate_json, validate_text, webhook_delivery_cursor,
};
use sqlx::{AssertSqlSafe, Row, postgres::PgRow, types::Json};
use uuid::Uuid;

use super::PgRepos;
use crate::exec::{map_db_err, run};

const ENDPOINT_COLUMNS: &str = "id, name, url, events, content_types, enabled, disabled_reason, \
     disabled_at, install_id, secret_sealed, previous_secret_sealed, previous_secret_expires_at, \
     payload_version, failing_since, consecutive_failures, last_success_at, last_failure_at, \
     created_by, created_at, updated_at";

const DELIVERY_COLUMNS: &str = "d.id, d.endpoint_id, d.event_id, d.status, d.attempts, \
     d.next_attempt_at, d.last_status_code, d.last_error, d.redelivery_of, d.created_at, \
     d.finished_at";

fn pg(t: Timestamp) -> DateTime<Utc> {
    t.into()
}

fn pg_opt(t: Option<Timestamp>) -> Option<DateTime<Utc>> {
    t.map(pg)
}

fn ts(row: &PgRow, col: &str) -> Result<Timestamp, StoreError> {
    let at: DateTime<Utc> = row.try_get(col).map_err(map_db_err)?;
    Ok(at.into())
}

fn ts_opt(row: &PgRow, col: &str) -> Result<Option<Timestamp>, StoreError> {
    let at: Option<DateTime<Utc>> = row.try_get(col).map_err(map_db_err)?;
    Ok(at.map(Into::into))
}

fn endpoint(row: &PgRow) -> Result<WebhookEndpoint, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let events: Json<Vec<String>> = row.try_get("events").map_err(map_db_err)?;
    let types: Option<Json<Vec<String>>> = row.try_get("content_types").map_err(map_db_err)?;
    let reason: Option<String> = row.try_get("disabled_reason").map_err(map_db_err)?;
    let created_by: Option<Uuid> = row.try_get("created_by").map_err(map_db_err)?;
    Ok(WebhookEndpoint {
        id: WebhookId::from_uuid(id),
        name: row.try_get("name").map_err(map_db_err)?,
        url: row.try_get("url").map_err(map_db_err)?,
        events: events.0,
        content_types: types.map(|t| t.0),
        enabled: row.try_get("enabled").map_err(map_db_err)?,
        disabled_reason: reason.map(|r| r.parse::<DisabledReason>()).transpose()?,
        disabled_at: ts_opt(row, "disabled_at")?,
        install_id: row.try_get("install_id").map_err(map_db_err)?,
        secret_sealed: row.try_get("secret_sealed").map_err(map_db_err)?,
        previous_secret_sealed: row.try_get("previous_secret_sealed").map_err(map_db_err)?,
        previous_secret_expires_at: ts_opt(row, "previous_secret_expires_at")?,
        payload_version: row.try_get("payload_version").map_err(map_db_err)?,
        failing_since: ts_opt(row, "failing_since")?,
        consecutive_failures: row.try_get("consecutive_failures").map_err(map_db_err)?,
        last_success_at: ts_opt(row, "last_success_at")?,
        last_failure_at: ts_opt(row, "last_failure_at")?,
        created_by: created_by.map(UserId::from_uuid),
        created_at: ts(row, "created_at")?,
        updated_at: ts(row, "updated_at")?,
    })
}

fn delivery(row: &PgRow) -> Result<Delivery, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let endpoint: Uuid = row.try_get("endpoint_id").map_err(map_db_err)?;
    let event: Uuid = row.try_get("event_id").map_err(map_db_err)?;
    let status: String = row.try_get("status").map_err(map_db_err)?;
    let of: Option<Uuid> = row.try_get("redelivery_of").map_err(map_db_err)?;
    Ok(Delivery {
        id: DeliveryId::from_uuid(id),
        endpoint_id: WebhookId::from_uuid(endpoint),
        event_id: WebhookEventId::from_uuid(event),
        status: status.parse()?,
        attempts: row.try_get("attempts").map_err(map_db_err)?,
        next_attempt_at: ts_opt(row, "next_attempt_at")?,
        last_status_code: row.try_get("last_status_code").map_err(map_db_err)?,
        last_error: row.try_get("last_error").map_err(map_db_err)?,
        redelivery_of: of.map(DeliveryId::from_uuid),
        created_at: ts(row, "created_at")?,
        finished_at: ts_opt(row, "finished_at")?,
    })
}

fn attempt(row: &PgRow) -> Result<Attempt, StoreError> {
    let id: Uuid = row.try_get("delivery_id").map_err(map_db_err)?;
    Ok(Attempt {
        delivery_id: DeliveryId::from_uuid(id),
        attempt: row.try_get("attempt").map_err(map_db_err)?,
        started_at: ts(row, "started_at")?,
        duration_ms: row.try_get("duration_ms").map_err(map_db_err)?,
        status_code: row.try_get("status_code").map_err(map_db_err)?,
        error: row.try_get("error").map_err(map_db_err)?,
        response_excerpt: row.try_get("response_excerpt").map_err(map_db_err)?,
    })
}

#[async_trait]
impl WebhookRepo for PgRepos {
    async fn insert_endpoint(&self, e: &WebhookEndpoint) -> Result<(), StoreError> {
        check_webhook_endpoint(e)?;
        run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO webhook_endpoints ({ENDPOINT_COLUMNS}) VALUES \
             ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
             $19, $20)"
        )))
        .bind(e.id.as_uuid())
        .bind(&e.name)
        .bind(&e.url)
        .bind(Json(&e.events))
        .bind(e.content_types.as_ref().map(Json))
        .bind(e.enabled)
        .bind(e.disabled_reason.map(|r| r.as_str()))
        .bind(pg_opt(e.disabled_at))
        .bind(&e.install_id)
        .bind(&e.secret_sealed)
        .bind(e.previous_secret_sealed.as_deref())
        .bind(pg_opt(e.previous_secret_expires_at))
        .bind(e.payload_version)
        .bind(pg_opt(e.failing_since))
        .bind(e.consecutive_failures)
        .bind(pg_opt(e.last_success_at))
        .bind(pg_opt(e.last_failure_at))
        .bind(e.created_by.map(|u| u.as_uuid()))
        .bind(pg(e.created_at))
        .bind(pg(e.updated_at))
        .execute(x))?;
        Ok(())
    }

    async fn get_endpoint(&self, id: WebhookId) -> Result<Option<WebhookEndpoint>, StoreError> {
        let row = run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(endpoint).transpose()
    }

    async fn get_endpoint_for_update(
        &self,
        id: WebhookId,
    ) -> Result<Option<WebhookEndpoint>, StoreError> {
        let row = run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE id = $1 FOR UPDATE"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(endpoint).transpose()
    }

    async fn list_endpoints(&self) -> Result<Vec<WebhookEndpoint>, StoreError> {
        let rows = run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints \
             ORDER BY name COLLATE \"C\", id LIMIT $1"
        )))
        .bind(i64::from(MAX_WEBHOOK_ENDPOINTS_READ))
        .fetch_all(x))?;
        rows.iter().map(endpoint).collect()
    }

    async fn count_endpoints(&self) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.exec, |x| sqlx::query_scalar(
            "SELECT COUNT(*) FROM webhook_endpoints"
        )
        .fetch_one(x))?;
        Ok(n.max(0) as u64)
    }

    async fn update_endpoint(&self, e: &WebhookEndpoint) -> Result<(), StoreError> {
        check_webhook_endpoint(e)?;
        let done = run!(&self.exec, |x| {
            sqlx::query(
            "UPDATE webhook_endpoints SET name = $1, url = $2, events = $3, content_types = $4, \
             enabled = $5, disabled_reason = $6, disabled_at = $7, install_id = $8, \
             secret_sealed = $9, previous_secret_sealed = $10, \
             previous_secret_expires_at = $11, payload_version = $12, failing_since = $13, \
             consecutive_failures = $14, last_success_at = $15, last_failure_at = $16, \
             created_by = $17, updated_at = $18 WHERE id = $19"
        )
        .bind(&e.name)
        .bind(&e.url)
        .bind(Json(&e.events))
        .bind(e.content_types.as_ref().map(Json))
        .bind(e.enabled)
        .bind(e.disabled_reason.map(|r| r.as_str()))
        .bind(pg_opt(e.disabled_at))
        .bind(&e.install_id)
        .bind(&e.secret_sealed)
        .bind(e.previous_secret_sealed.as_deref())
        .bind(pg_opt(e.previous_secret_expires_at))
        .bind(e.payload_version)
        .bind(pg_opt(e.failing_since))
        .bind(e.consecutive_failures)
        .bind(pg_opt(e.last_success_at))
        .bind(pg_opt(e.last_failure_at))
        .bind(e.created_by.map(|u| u.as_uuid()))
        .bind(pg(e.updated_at))
        .bind(e.id.as_uuid())
        .execute(x)
        })?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete_endpoint(&self, id: WebhookId) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |x| sqlx::query(
            "DELETE FROM webhook_endpoints WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn active_for_emit(&self) -> Result<Vec<WebhookEndpoint>, StoreError> {
        let rows = run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE enabled \
             AND install_id = (SELECT value FROM schema_meta WHERE key = $1) \
             ORDER BY name COLLATE \"C\", id LIMIT $2"
        )))
        .bind(SCHEMA_META_INSTALL_ID)
        .bind(i64::from(MAX_WEBHOOK_ENDPOINTS_READ))
        .fetch_all(x))?;
        rows.iter().map(endpoint).collect()
    }

    async fn insert_event(&self, event: &WebhookEventRow) -> Result<(), StoreError> {
        validate_text("webhook event type", &event.kind)?;
        validate_json(&event.data)?;
        run!(&self.exec, |x| sqlx::query(
            "INSERT INTO webhook_events (id, type, data, created_at) VALUES ($1, $2, $3, $4)"
        )
        .bind(event.id.as_uuid())
        .bind(&event.kind)
        .bind(Json(&event.data))
        .bind(pg(event.created_at))
        .execute(x))?;
        Ok(())
    }

    async fn get_event(&self, id: WebhookEventId) -> Result<Option<WebhookEventRow>, StoreError> {
        let row = run!(&self.exec, |x| sqlx::query(
            "SELECT id, type, data, created_at FROM webhook_events WHERE id = $1"
        )
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.map(|r| {
            let id: Uuid = r.try_get("id").map_err(map_db_err)?;
            let data: Json<serde_json::Value> = r.try_get("data").map_err(map_db_err)?;
            Ok(WebhookEventRow {
                id: WebhookEventId::from_uuid(id),
                kind: r.try_get("type").map_err(map_db_err)?,
                data: data.0,
                created_at: ts(&r, "created_at")?,
            })
        })
        .transpose()
    }

    async fn insert_delivery(&self, d: &Delivery) -> Result<(), StoreError> {
        if let Some(e) = &d.last_error {
            validate_text("webhook delivery error", e)?;
        }
        run!(&self.exec, |x| sqlx::query(
            "INSERT INTO webhook_deliveries (id, endpoint_id, event_id, status, attempts, \
             next_attempt_at, last_status_code, last_error, redelivery_of, created_at, \
             finished_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"
        )
        .bind(d.id.as_uuid())
        .bind(d.endpoint_id.as_uuid())
        .bind(d.event_id.as_uuid())
        .bind(d.status.as_str())
        .bind(d.attempts)
        .bind(pg_opt(d.next_attempt_at))
        .bind(d.last_status_code)
        .bind(d.last_error.as_deref())
        .bind(d.redelivery_of.map(|r| r.as_uuid()))
        .bind(pg(d.created_at))
        .bind(pg_opt(d.finished_at))
        .execute(x))?;
        Ok(())
    }

    async fn get_delivery(&self, id: DeliveryId) -> Result<Option<Delivery>, StoreError> {
        let row = run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries d WHERE d.id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(delivery).transpose()
    }

    async fn list_deliveries(
        &self,
        endpoint: WebhookId,
        query: &DeliveryQuery,
    ) -> Result<Page<DeliveryListed>, StoreError> {
        let limit = check_limit(query.limit)?;
        let after = query
            .cursor
            .as_deref()
            .map(webhook_delivery_cursor)
            .transpose()?
            .map(|(at, id)| Ok::<_, StoreError>((pg(Timestamp::from_micros(at)?), id)))
            .transpose()?;
        let mut n = 1;
        let mut mark = || {
            n += 1;
            format!("${n}")
        };
        let mut sql = format!(
            "SELECT {DELIVERY_COLUMNS}, e.type AS event_type FROM webhook_deliveries d \
             JOIN webhook_events e ON e.id = d.event_id WHERE d.endpoint_id = $1"
        );
        if query.status.is_some() {
            sql.push_str(&format!(" AND d.status = {}", mark()));
        }
        if after.is_some() {
            let (a, b) = (mark(), mark());
            sql.push_str(&format!(" AND (d.created_at, d.id) < ({a}, {b})"));
        }
        sql.push_str(&format!(
            " ORDER BY d.created_at DESC, d.id DESC LIMIT {}",
            mark()
        ));
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(endpoint.as_uuid());
        if let Some(s) = query.status {
            q = q.bind(s.as_str());
        }
        if let Some((at, id)) = after {
            q = q.bind(at).bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.exec, |x| q.fetch_all(x))?;
        let mut items = rows
            .iter()
            .map(|r| {
                Ok(DeliveryListed {
                    delivery: delivery(r)?,
                    event_type: r.try_get("event_type").map_err(map_db_err)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|l| {
                encode_webhook_delivery_cursor(l.delivery.created_at, l.delivery.id.as_uuid())
            })
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn record_attempt(&self, a: &Attempt, u: &DeliveryUpdate) -> Result<bool, StoreError> {
        check_webhook_attempt(a, u)?;
        let done = run!(&self.exec, |x| sqlx::query(
            "UPDATE webhook_deliveries SET status = $1, attempts = $2, next_attempt_at = $3, \
             last_status_code = $4, last_error = $5, finished_at = $6 \
             WHERE id = $7 AND status = 'pending' AND attempts = $8"
        )
        .bind(u.status.as_str())
        .bind(u.attempts)
        .bind(pg_opt(u.next_attempt_at))
        .bind(u.last_status_code)
        .bind(u.last_error.as_deref())
        .bind(pg_opt(u.finished_at))
        .bind(a.delivery_id.as_uuid())
        .bind(a.attempt - 1)
        .execute(x))?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        run!(&self.exec, |x| sqlx::query(
            "INSERT INTO webhook_attempts (delivery_id, attempt, started_at, duration_ms, \
             status_code, error, response_excerpt) VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind(a.delivery_id.as_uuid())
        .bind(a.attempt)
        .bind(pg(a.started_at))
        .bind(a.duration_ms)
        .bind(a.status_code)
        .bind(a.error.as_deref())
        .bind(a.response_excerpt.as_deref())
        .execute(x))?;
        Ok(true)
    }

    async fn set_delivery_status(
        &self,
        id: DeliveryId,
        status: DeliveryStatus,
        error: Option<&str>,
        finished_at: Timestamp,
    ) -> Result<bool, StoreError> {
        if let Some(e) = error {
            validate_text("webhook delivery error", e)?;
        }
        let done = run!(&self.exec, |x| sqlx::query(
            "UPDATE webhook_deliveries SET status = $1, last_error = $2, finished_at = $3, \
             next_attempt_at = NULL WHERE id = $4 AND status = 'pending'"
        )
        .bind(status.as_str())
        .bind(error)
        .bind(pg(finished_at))
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn deliveries_since(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
        statuses: &[DeliveryStatus],
        limit: u32,
    ) -> Result<Vec<Delivery>, StoreError> {
        let limit = check_prune_limit(limit.min(MAX_PRUNE_BATCH))?;
        if statuses.is_empty() {
            return Ok(Vec::new());
        }
        let names: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
        let rows = run!(&self.exec, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries d WHERE d.endpoint_id = $1 \
             AND d.created_at >= $2 AND d.status = ANY($3) \
             ORDER BY d.created_at, d.id LIMIT $4"
        )))
        .bind(endpoint.as_uuid())
        .bind(pg(since))
        .bind(&names)
        .bind(limit)
        .fetch_all(x))?;
        rows.iter().map(delivery).collect()
    }

    async fn delivery_counts(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
    ) -> Result<DeliveryCounts, StoreError> {
        let rows: Vec<(String, i64)> = run!(&self.exec, |x| sqlx::query_as(
            "SELECT status, COUNT(*) FROM webhook_deliveries \
             WHERE endpoint_id = $1 AND created_at >= $2 GROUP BY status"
        )
        .bind(endpoint.as_uuid())
        .bind(pg(since))
        .fetch_all(x))?;
        let mut counts = DeliveryCounts::default();
        for (status, n) in rows {
            counts.add(status.parse()?, n.max(0) as u64);
        }
        Ok(counts)
    }

    async fn attempts(&self, delivery_id: DeliveryId) -> Result<Vec<Attempt>, StoreError> {
        let rows = run!(&self.exec, |x| sqlx::query(
            "SELECT delivery_id, attempt, started_at, duration_ms, status_code, error, \
             response_excerpt FROM webhook_attempts WHERE delivery_id = $1 ORDER BY attempt"
        )
        .bind(delivery_id.as_uuid())
        .fetch_all(x))?;
        rows.iter().map(attempt).collect()
    }

    async fn prune_events(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.exec, |x| sqlx::query(
            "DELETE FROM webhook_events WHERE id IN (SELECT e.id FROM webhook_events e \
             WHERE e.created_at < $1 AND NOT EXISTS (SELECT 1 FROM webhook_deliveries d \
             WHERE d.event_id = e.id AND d.status = 'pending') ORDER BY e.created_at LIMIT $2)"
        )
        .bind(pg(before))
        .bind(limit)
        .execute(x))?;
        Ok(done.rows_affected())
    }
}
