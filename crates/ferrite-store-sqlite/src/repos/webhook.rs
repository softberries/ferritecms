//! Webhook endpoints, events, deliveries and attempts (`0016_webhooks`) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{
    Attempt, Delivery, DeliveryCounts, DeliveryId, DeliveryListed, DeliveryQuery, DeliveryStatus,
    DeliveryUpdate, DisabledReason, Page, Timestamp, UserId, WebhookEndpoint, WebhookEventId,
    WebhookEventRow, WebhookId,
};
use ferrite_store::{
    MAX_PRUNE_BATCH, MAX_WEBHOOK_ENDPOINTS_READ, SCHEMA_META_INSTALL_ID, StoreError, WebhookRepo,
    check_limit, check_prune_limit, check_webhook_attempt, check_webhook_endpoint,
    encode_webhook_delivery_cursor, validate_text, webhook_delivery_cursor,
};
use sqlx::{AssertSqlSafe, Row, sqlite::SqliteRow};
use uuid::Uuid;

use super::SqliteRepos;
use crate::{
    codec::{json_from_db, json_to_db},
    exec::{map_db_err, run},
};

const ENDPOINT_COLUMNS: &str = "id, name, url, events, content_types, enabled, disabled_reason, \
     disabled_at, install_id, secret_sealed, previous_secret_sealed, previous_secret_expires_at, \
     payload_version, failing_since, consecutive_failures, last_success_at, last_failure_at, \
     created_by, created_at, updated_at";

const DELIVERY_COLUMNS: &str = "d.id, d.endpoint_id, d.event_id, d.status, d.attempts, \
     d.next_attempt_at, d.last_status_code, d.last_error, d.redelivery_of, d.created_at, \
     d.finished_at";

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

fn strings(text: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_value(json_from_db(text)?)
        .map_err(|e| StoreError::Database(format!("corrupt webhook list: {e}")))
}

fn endpoint(row: &SqliteRow) -> Result<WebhookEndpoint, StoreError> {
    let id: Uuid = row.try_get("id").map_err(map_db_err)?;
    let events: String = row.try_get("events").map_err(map_db_err)?;
    let types: Option<String> = row.try_get("content_types").map_err(map_db_err)?;
    let reason: Option<String> = row.try_get("disabled_reason").map_err(map_db_err)?;
    let created_by: Option<Uuid> = row.try_get("created_by").map_err(map_db_err)?;
    Ok(WebhookEndpoint {
        id: WebhookId::from_uuid(id),
        name: row.try_get("name").map_err(map_db_err)?,
        url: row.try_get("url").map_err(map_db_err)?,
        events: strings(&events)?,
        content_types: types.as_deref().map(strings).transpose()?,
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
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

fn delivery(row: &SqliteRow) -> Result<Delivery, StoreError> {
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
        created_at: ts_col(row, "created_at")?,
        finished_at: ts_opt(row, "finished_at")?,
    })
}

fn attempt(row: &SqliteRow) -> Result<Attempt, StoreError> {
    let id: Uuid = row.try_get("delivery_id").map_err(map_db_err)?;
    Ok(Attempt {
        delivery_id: DeliveryId::from_uuid(id),
        attempt: row.try_get("attempt").map_err(map_db_err)?,
        started_at: ts_col(row, "started_at")?,
        duration_ms: row.try_get("duration_ms").map_err(map_db_err)?,
        status_code: row.try_get("status_code").map_err(map_db_err)?,
        error: row.try_get("error").map_err(map_db_err)?,
        response_excerpt: row.try_get("response_excerpt").map_err(map_db_err)?,
    })
}

fn lists(e: &WebhookEndpoint) -> Result<(String, Option<String>), StoreError> {
    check_webhook_endpoint(e)?;
    let events = json_to_db(&serde_json::json!(e.events))?;
    let types = e
        .content_types
        .as_ref()
        .map(|t| json_to_db(&serde_json::json!(t)))
        .transpose()?;
    Ok((events, types))
}

fn micros(t: Option<Timestamp>) -> Option<i64> {
    t.map(|t| t.as_micros())
}

#[async_trait]
impl WebhookRepo for SqliteRepos {
    async fn insert_endpoint(&self, e: &WebhookEndpoint) -> Result<(), StoreError> {
        let (events, types) = lists(e)?;
        run!(&self.writer, |x| sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO webhook_endpoints ({ENDPOINT_COLUMNS}) VALUES \
             (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )))
        .bind(e.id.as_uuid())
        .bind(&e.name)
        .bind(&e.url)
        .bind(&events)
        .bind(types.as_deref())
        .bind(e.enabled)
        .bind(e.disabled_reason.map(|r| r.as_str()))
        .bind(micros(e.disabled_at))
        .bind(&e.install_id)
        .bind(&e.secret_sealed)
        .bind(e.previous_secret_sealed.as_deref())
        .bind(micros(e.previous_secret_expires_at))
        .bind(e.payload_version)
        .bind(micros(e.failing_since))
        .bind(e.consecutive_failures)
        .bind(micros(e.last_success_at))
        .bind(micros(e.last_failure_at))
        .bind(e.created_by.map(|u| u.as_uuid()))
        .bind(e.created_at.as_micros())
        .bind(e.updated_at.as_micros())
        .execute(x))?;
        Ok(())
    }

    async fn get_endpoint(&self, id: WebhookId) -> Result<Option<WebhookEndpoint>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(endpoint).transpose()
    }

    async fn get_endpoint_for_update(
        &self,
        id: WebhookId,
    ) -> Result<Option<WebhookEndpoint>, StoreError> {
        // Inside a Tx the reader is the transaction, which holds the single writer.
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.as_ref().map(endpoint).transpose()
    }

    async fn list_endpoints(&self) -> Result<Vec<WebhookEndpoint>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints \
             ORDER BY CAST(name AS BLOB), id LIMIT ?"
        )))
        .bind(i64::from(MAX_WEBHOOK_ENDPOINTS_READ))
        .fetch_all(x))?;
        rows.iter().map(endpoint).collect()
    }

    async fn count_endpoints(&self) -> Result<u64, StoreError> {
        let n: i64 = run!(&self.reader, |x| sqlx::query_scalar(
            "SELECT COUNT(*) FROM webhook_endpoints"
        )
        .fetch_one(x))?;
        Ok(n.max(0) as u64)
    }

    async fn update_endpoint(&self, e: &WebhookEndpoint) -> Result<(), StoreError> {
        let (events, types) = lists(e)?;
        let done = run!(&self.writer, |x| sqlx::query(
            "UPDATE webhook_endpoints SET name = ?, url = ?, events = ?, content_types = ?, \
             enabled = ?, disabled_reason = ?, disabled_at = ?, install_id = ?, \
             secret_sealed = ?, previous_secret_sealed = ?, previous_secret_expires_at = ?, \
             payload_version = ?, failing_since = ?, consecutive_failures = ?, \
             last_success_at = ?, last_failure_at = ?, created_by = ?, updated_at = ? \
             WHERE id = ?"
        )
        .bind(&e.name)
        .bind(&e.url)
        .bind(&events)
        .bind(types.as_deref())
        .bind(e.enabled)
        .bind(e.disabled_reason.map(|r| r.as_str()))
        .bind(micros(e.disabled_at))
        .bind(&e.install_id)
        .bind(&e.secret_sealed)
        .bind(e.previous_secret_sealed.as_deref())
        .bind(micros(e.previous_secret_expires_at))
        .bind(e.payload_version)
        .bind(micros(e.failing_since))
        .bind(e.consecutive_failures)
        .bind(micros(e.last_success_at))
        .bind(micros(e.last_failure_at))
        .bind(e.created_by.map(|u| u.as_uuid()))
        .bind(e.updated_at.as_micros())
        .bind(e.id.as_uuid())
        .execute(x))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete_endpoint(&self, id: WebhookId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM webhook_endpoints WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(x))?;
        Ok(done.rows_affected() > 0)
    }

    async fn active_for_emit(&self) -> Result<Vec<WebhookEndpoint>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {ENDPOINT_COLUMNS} FROM webhook_endpoints WHERE enabled = 1 \
             AND install_id = (SELECT value FROM schema_meta WHERE key = ?) \
             ORDER BY CAST(name AS BLOB), id LIMIT ?"
        )))
        .bind(SCHEMA_META_INSTALL_ID)
        .bind(i64::from(MAX_WEBHOOK_ENDPOINTS_READ))
        .fetch_all(x))?;
        rows.iter().map(endpoint).collect()
    }

    async fn insert_event(&self, event: &WebhookEventRow) -> Result<(), StoreError> {
        validate_text("webhook event type", &event.kind)?;
        let data = json_to_db(&event.data)?;
        ferrite_store::validate_json(&event.data)?;
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO webhook_events (id, type, data, created_at) VALUES (?, ?, ?, ?)"
        )
        .bind(event.id.as_uuid())
        .bind(&event.kind)
        .bind(&data)
        .bind(event.created_at.as_micros())
        .execute(x))?;
        Ok(())
    }

    async fn get_event(&self, id: WebhookEventId) -> Result<Option<WebhookEventRow>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(
            "SELECT id, type, data, created_at FROM webhook_events WHERE id = ?"
        )
        .bind(id.as_uuid())
        .fetch_optional(x))?;
        row.map(|r| {
            let id: Uuid = r.try_get("id").map_err(map_db_err)?;
            let data: String = r.try_get("data").map_err(map_db_err)?;
            Ok(WebhookEventRow {
                id: WebhookEventId::from_uuid(id),
                kind: r.try_get("type").map_err(map_db_err)?,
                data: json_from_db(&data)?,
                created_at: ts_col(&r, "created_at")?,
            })
        })
        .transpose()
    }

    async fn insert_delivery(&self, d: &Delivery) -> Result<(), StoreError> {
        if let Some(e) = &d.last_error {
            validate_text("webhook delivery error", e)?;
        }
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO webhook_deliveries (id, endpoint_id, event_id, status, attempts, \
             next_attempt_at, last_status_code, last_error, redelivery_of, created_at, \
             finished_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(d.id.as_uuid())
        .bind(d.endpoint_id.as_uuid())
        .bind(d.event_id.as_uuid())
        .bind(d.status.as_str())
        .bind(d.attempts)
        .bind(micros(d.next_attempt_at))
        .bind(d.last_status_code)
        .bind(d.last_error.as_deref())
        .bind(d.redelivery_of.map(|r| r.as_uuid()))
        .bind(d.created_at.as_micros())
        .bind(micros(d.finished_at))
        .execute(x))?;
        Ok(())
    }

    async fn get_delivery(&self, id: DeliveryId) -> Result<Option<Delivery>, StoreError> {
        let row = run!(&self.reader, |x| sqlx::query(AssertSqlSafe(format!(
            "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries d WHERE d.id = ?"
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
            .transpose()?;
        let mut sql = format!(
            "SELECT {DELIVERY_COLUMNS}, e.type AS event_type FROM webhook_deliveries d \
             JOIN webhook_events e ON e.id = d.event_id WHERE d.endpoint_id = ?"
        );
        if query.status.is_some() {
            sql.push_str(" AND d.status = ?");
        }
        if after.is_some() {
            sql.push_str(" AND (d.created_at, d.id) < (?, ?)");
        }
        sql.push_str(" ORDER BY d.created_at DESC, d.id DESC LIMIT ?");
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(endpoint.as_uuid());
        if let Some(s) = query.status {
            q = q.bind(s.as_str());
        }
        if let Some((at, id)) = after {
            q = q.bind(at).bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |x| q.fetch_all(x))?;
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
        let done = run!(&self.writer, |x| sqlx::query(
            "UPDATE webhook_deliveries SET status = ?, attempts = ?, next_attempt_at = ?, \
             last_status_code = ?, last_error = ?, finished_at = ? \
             WHERE id = ? AND status = 'pending' AND attempts = ?"
        )
        .bind(u.status.as_str())
        .bind(u.attempts)
        .bind(micros(u.next_attempt_at))
        .bind(u.last_status_code)
        .bind(u.last_error.as_deref())
        .bind(micros(u.finished_at))
        .bind(a.delivery_id.as_uuid())
        .bind(a.attempt - 1)
        .execute(x))?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        run!(&self.writer, |x| sqlx::query(
            "INSERT INTO webhook_attempts (delivery_id, attempt, started_at, duration_ms, \
             status_code, error, response_excerpt) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(a.delivery_id.as_uuid())
        .bind(a.attempt)
        .bind(a.started_at.as_micros())
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
        let done = run!(&self.writer, |x| sqlx::query(
            "UPDATE webhook_deliveries SET status = ?, last_error = ?, finished_at = ?, \
             next_attempt_at = NULL WHERE id = ? AND status = 'pending'"
        )
        .bind(status.as_str())
        .bind(error)
        .bind(finished_at.as_micros())
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
        let marks = vec!["?"; statuses.len()].join(", ");
        let sql = format!(
            "SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries d WHERE d.endpoint_id = ? \
             AND d.created_at >= ? AND d.status IN ({marks}) \
             ORDER BY d.created_at, d.id LIMIT ?"
        );
        let mut q = sqlx::query(AssertSqlSafe(sql))
            .bind(endpoint.as_uuid())
            .bind(since.as_micros());
        for s in statuses {
            q = q.bind(s.as_str());
        }
        let q = q.bind(limit);
        let rows = run!(&self.reader, |x| q.fetch_all(x))?;
        rows.iter().map(delivery).collect()
    }

    async fn delivery_counts(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
    ) -> Result<DeliveryCounts, StoreError> {
        let rows: Vec<(String, i64)> = run!(&self.reader, |x| sqlx::query_as(
            "SELECT status, COUNT(*) FROM webhook_deliveries \
             WHERE endpoint_id = ? AND created_at >= ? GROUP BY status"
        )
        .bind(endpoint.as_uuid())
        .bind(since.as_micros())
        .fetch_all(x))?;
        let mut counts = DeliveryCounts::default();
        for (status, n) in rows {
            counts.add(status.parse()?, n.max(0) as u64);
        }
        Ok(counts)
    }

    async fn attempts(&self, delivery_id: DeliveryId) -> Result<Vec<Attempt>, StoreError> {
        let rows = run!(&self.reader, |x| sqlx::query(
            "SELECT delivery_id, attempt, started_at, duration_ms, status_code, error, \
             response_excerpt FROM webhook_attempts WHERE delivery_id = ? ORDER BY attempt"
        )
        .bind(delivery_id.as_uuid())
        .fetch_all(x))?;
        rows.iter().map(attempt).collect()
    }

    async fn fail_stale_pending(
        &self,
        before: Timestamp,
        error: &str,
        now: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        validate_text("webhook delivery error", error)?;
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |x| sqlx::query(
            "UPDATE webhook_deliveries SET status = 'failed', last_error = ?, finished_at = ?, \
             next_attempt_at = NULL WHERE id IN (SELECT id FROM webhook_deliveries \
             WHERE status = 'pending' AND next_attempt_at < ? LIMIT ?)"
        )
        .bind(error)
        .bind(now.as_micros())
        .bind(before.as_micros())
        .bind(limit)
        .execute(x))?;
        Ok(done.rows_affected())
    }

    async fn prune_events(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        let limit = check_prune_limit(limit)?;
        let done = run!(&self.writer, |x| sqlx::query(
            "DELETE FROM webhook_events WHERE id IN (SELECT e.id FROM webhook_events e \
             WHERE e.created_at < ? AND NOT EXISTS (SELECT 1 FROM webhook_deliveries d \
             WHERE d.event_id = e.id AND d.status = 'pending') ORDER BY e.created_at LIMIT ?)"
        )
        .bind(before.as_micros())
        .bind(limit)
        .execute(x))?;
        Ok(done.rows_affected())
    }
}
