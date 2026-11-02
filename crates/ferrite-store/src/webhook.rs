//! The webhook repository (`0016_webhooks`, design 18 §7) and its delivery-log cursor.

use async_trait::async_trait;
use ferrite_domain::{
    Attempt, Cursor, Delivery, DeliveryCounts, DeliveryId, DeliveryListed, DeliveryQuery,
    DeliveryStatus, DeliveryUpdate, Page, Timestamp, WebhookEndpoint, WebhookEventId,
    WebhookEventRow, WebhookId,
};
use serde_json::json;
use uuid::Uuid;

use crate::{StoreError, validate_text};

/// Cursor tag of an endpoint's delivery log (see [`encode_webhook_delivery_cursor`]).
pub const WEBHOOK_DELIVERY_CURSOR_TAG: &str = "wd:created";

/// At most this many endpoints are read by [`WebhookRepo::list_endpoints`] and
/// [`WebhookRepo::active_for_emit`] (core allows 20 per site).
pub const MAX_WEBHOOK_ENDPOINTS_READ: u32 = 200;

/// Encodes the delivery-log cursor pointing just after the row (`created_at`, `id`).
pub fn encode_webhook_delivery_cursor(created_at: Timestamp, id: Uuid) -> String {
    Cursor::encode(
        WEBHOOK_DELIVERY_CURSOR_TAG,
        &json!(created_at.as_micros()),
        id,
    )
}

/// Decodes a cursor made by [`encode_webhook_delivery_cursor`] into (`created_at` micros,
/// `id`).
pub fn webhook_delivery_cursor(cursor: &str) -> Result<(i64, Uuid), StoreError> {
    let bad = || StoreError::Invalid("cursor does not match the query order".into());
    match Cursor::decode(cursor, WEBHOOK_DELIVERY_CURSOR_TAG)? {
        (serde_json::Value::Number(n), id) => n
            .as_i64()
            .filter(|at| Timestamp::from_micros(*at).is_ok())
            .map(|at| (at, id))
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// NUL-checks every text of an endpoint row.
pub fn check_webhook_endpoint(e: &WebhookEndpoint) -> Result<(), StoreError> {
    validate_text("webhook name", &e.name)?;
    validate_text("webhook url", &e.url)?;
    validate_text("webhook install id", &e.install_id)?;
    validate_text("webhook secret", &e.secret_sealed)?;
    if let Some(p) = &e.previous_secret_sealed {
        validate_text("webhook secret", p)?;
    }
    e.events
        .iter()
        .chain(e.content_types.iter().flatten())
        .try_for_each(|v| validate_text("webhook events", v))
}

/// NUL-checks the texts of an attempt and its delivery update.
pub fn check_webhook_attempt(a: &Attempt, u: &DeliveryUpdate) -> Result<(), StoreError> {
    a.error
        .iter()
        .chain(&a.response_excerpt)
        .chain(&u.last_error)
        .try_for_each(|v| validate_text("webhook attempt", v))
}

/// Endpoints, events, deliveries and attempts. Every text bound is NUL-checked
/// (`Invalid`). No cache: every emit and every attempt reads the rows.
#[async_trait]
pub trait WebhookRepo: Send + Sync {
    /// Inserts an endpoint; a known id is `UniqueViolation`.
    async fn insert_endpoint(&self, endpoint: &WebhookEndpoint) -> Result<(), StoreError>;
    async fn get_endpoint(&self, id: WebhookId) -> Result<Option<WebhookEndpoint>, StoreError>;
    /// Every endpoint by name (binary order), then id, at most
    /// [`MAX_WEBHOOK_ENDPOINTS_READ`].
    async fn list_endpoints(&self) -> Result<Vec<WebhookEndpoint>, StoreError>;
    async fn count_endpoints(&self) -> Result<u64, StoreError>;
    /// Replaces the whole row (`id` and `created_at` stay); `NotFound` when missing.
    async fn update_endpoint(&self, endpoint: &WebhookEndpoint) -> Result<(), StoreError>;
    /// Deletes an endpoint with its deliveries and attempts; `true` if it existed.
    async fn delete_endpoint(&self, id: WebhookId) -> Result<bool, StoreError>;
    /// The enabled endpoints whose `install_id` is the database's
    /// `schema_meta.install_id` (one statement), by name.
    async fn active_for_emit(&self) -> Result<Vec<WebhookEndpoint>, StoreError>;

    async fn insert_event(&self, event: &WebhookEventRow) -> Result<(), StoreError>;
    async fn get_event(&self, id: WebhookEventId) -> Result<Option<WebhookEventRow>, StoreError>;

    /// Inserts a delivery; an unknown endpoint or event is `ForeignKey`.
    async fn insert_delivery(&self, delivery: &Delivery) -> Result<(), StoreError>;
    async fn get_delivery(&self, id: DeliveryId) -> Result<Option<Delivery>, StoreError>;
    /// The deliveries of `endpoint`, newest first (`created_at DESC, id DESC`), with their
    /// event's type; cursor tag [`WEBHOOK_DELIVERY_CURSOR_TAG`]; `limit` 1..=200.
    async fn list_deliveries(
        &self,
        endpoint: WebhookId,
        query: &DeliveryQuery,
    ) -> Result<Page<DeliveryListed>, StoreError>;
    /// Records attempt `attempt.attempt` and applies `update`, only while the delivery is
    /// `pending` with `attempts = attempt - 1` (one conditional update, then the insert);
    /// `false` (nothing written) otherwise.
    async fn record_attempt(
        &self,
        attempt: &Attempt,
        update: &DeliveryUpdate,
    ) -> Result<bool, StoreError>;
    /// Moves a `pending` delivery to `status` with `last_error` and `finished_at`
    /// (`next_attempt_at` cleared); `false` when it is not pending or missing.
    async fn set_delivery_status(
        &self,
        id: DeliveryId,
        status: DeliveryStatus,
        error: Option<&str>,
        finished_at: Timestamp,
    ) -> Result<bool, StoreError>;
    /// Deliveries of `endpoint` created at or after `since` with one of `statuses`,
    /// oldest first, at most `limit` (1..=10 000).
    async fn deliveries_since(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
        statuses: &[DeliveryStatus],
        limit: u32,
    ) -> Result<Vec<Delivery>, StoreError>;
    /// Deliveries of `endpoint` created at or after `since`, by status.
    async fn delivery_counts(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
    ) -> Result<DeliveryCounts, StoreError>;
    /// The attempts of a delivery, oldest first.
    async fn attempts(&self, delivery: DeliveryId) -> Result<Vec<Attempt>, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) events
    /// created before `before` that have no `pending` delivery (their deliveries and
    /// attempts by cascade); returns the count.
    async fn prune_events(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_cursor_roundtrip() {
        let id = Uuid::now_v7();
        let at = Timestamp::now();
        let c = encode_webhook_delivery_cursor(at, id);
        assert_eq!(webhook_delivery_cursor(&c).unwrap(), (at.as_micros(), id));
        let other = Cursor::encode("og:created", &json!(at.as_micros()), id);
        assert!(webhook_delivery_cursor(&other).is_err());
    }
}
