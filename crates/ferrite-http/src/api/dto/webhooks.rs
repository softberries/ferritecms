//! `/webhooks` bodies (M6-6, design 18 §8). No response body holds a secret except the
//! create and rotate answers ([`CreatedWebhookDto`], [`RotatedSecretDto`]), which derive
//! no `Debug`.

use chrono::{DateTime, Utc};
use ferrite_core::webhooks::{DeliveryDetail, EventTypeInfo, WebhookView};
use ferrite_domain::{
    Attempt, DeliveryCounts, DeliveryListed, DeliveryStatus, DisabledReason, WebhookEventRow,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::content::{nullable, present};

/// What state an endpoint is in.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WebhookStateDto {
    Active,
    Failing,
    Disabled,
    /// Created under another install id: a copied or restored database. Enable it to send
    /// from this site.
    Paused,
}

/// Why an endpoint is disabled.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DisabledReasonDto {
    Manual,
    /// Failing for more than 5 days with at least 10 failed attempts in a row.
    Failing,
    /// The receiver answered `410 Gone`.
    Gone,
}

impl From<DisabledReason> for DisabledReasonDto {
    fn from(r: DisabledReason) -> Self {
        match r {
            DisabledReason::Manual => Self::Manual,
            DisabledReason::Failing => Self::Failing,
            DisabledReason::Gone => Self::Gone,
        }
    }
}

/// A delivery's status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatusDto {
    Pending,
    Delivered,
    Failed,
    Skipped,
}

impl From<DeliveryStatus> for DeliveryStatusDto {
    fn from(s: DeliveryStatus) -> Self {
        match s {
            DeliveryStatus::Pending => Self::Pending,
            DeliveryStatus::Delivered => Self::Delivered,
            DeliveryStatus::Failed => Self::Failed,
            DeliveryStatus::Skipped => Self::Skipped,
        }
    }
}

impl From<DeliveryStatusDto> for DeliveryStatus {
    fn from(s: DeliveryStatusDto) -> Self {
        match s {
            DeliveryStatusDto::Pending => Self::Pending,
            DeliveryStatusDto::Delivered => Self::Delivered,
            DeliveryStatusDto::Failed => Self::Failed,
            DeliveryStatusDto::Skipped => Self::Skipped,
        }
    }
}

/// Deliveries by status.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeliveryCountsDto {
    pub delivered: u64,
    pub failed: u64,
    pub pending: u64,
    pub skipped: u64,
}

impl From<DeliveryCounts> for DeliveryCountsDto {
    fn from(c: DeliveryCounts) -> Self {
        Self {
            delivered: c.delivered,
            failed: c.failed,
            pending: c.pending,
            skipped: c.skipped,
        }
    }
}

/// An endpoint. Never its secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookDto {
    pub id: Uuid,
    pub name: String,
    /// The whole URL (a deploy hook may carry a token in it): shown to `webhooks:read`
    /// callers only; the audit log holds its host.
    pub url: String,
    /// Subscription patterns: exact types, `<group>.*` or `*`.
    pub events: Vec<String>,
    /// Content types `content.*` events are narrowed to; `null` for all.
    pub content_types: Option<Vec<String>>,
    pub enabled: bool,
    pub state: WebhookStateDto,
    pub disabled_reason: Option<DisabledReasonDto>,
    pub disabled_at: Option<DateTime<Utc>>,
    /// Since the first failed attempt after a success (cleared by a `2xx`).
    pub failing_since: Option<DateTime<Utc>>,
    pub consecutive_failures: i32,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub payload_version: i32,
    /// Until when the previous secret still signs (a rotation in progress).
    pub secret_rotating_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Deliveries created in the last 24 hours, by status.
    pub counts_24h: DeliveryCountsDto,
}

impl From<WebhookView> for WebhookDto {
    fn from(v: WebhookView) -> Self {
        use ferrite_core::webhooks::WebhookState as S;
        let e = v.endpoint;
        Self {
            id: e.id.as_uuid(),
            name: e.name,
            url: e.url,
            events: e.events,
            content_types: e.content_types,
            enabled: e.enabled,
            state: match v.state {
                S::Active => WebhookStateDto::Active,
                S::Failing => WebhookStateDto::Failing,
                S::Disabled => WebhookStateDto::Disabled,
                S::Paused => WebhookStateDto::Paused,
            },
            disabled_reason: e.disabled_reason.map(Into::into),
            disabled_at: e.disabled_at.map(|t| t.as_datetime()),
            failing_since: e.failing_since.map(|t| t.as_datetime()),
            consecutive_failures: e.consecutive_failures,
            last_success_at: e.last_success_at.map(|t| t.as_datetime()),
            last_failure_at: e.last_failure_at.map(|t| t.as_datetime()),
            payload_version: e.payload_version,
            secret_rotating_until: v.secret_rotating_until.map(|t| t.as_datetime()),
            created_at: e.created_at.as_datetime(),
            updated_at: e.updated_at.as_datetime(),
            counts_24h: v.counts_24h.into(),
        }
    }
}

/// `GET /webhooks`: every endpoint, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookList {
    pub items: Vec<WebhookDto>,
}

/// `POST /webhooks`.
#[derive(Deserialize, ToSchema)]
pub struct WebhookInput {
    /// 1 to 100 characters.
    pub name: String,
    /// Absolute `https` URL (no user information, no fragment, at most 2048 bytes).
    pub url: String,
    /// 1 to 50 patterns: exact types, `<group>.*` or `*`.
    pub events: Vec<String>,
    /// Narrows `content.*` events to these content types (1 to 50); absent or `null`
    /// for all.
    #[serde(default)]
    pub content_types: Option<Vec<String>>,
    /// Default `true`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Bring your own secret: `whsec_` + base64 of 24 to 64 bytes. Generated when
    /// absent.
    #[serde(default)]
    #[schema(format = Password)]
    pub secret: Option<String>,
}

/// `PATCH /webhooks/{id}`: absent members keep the stored value; `content_types: null`
/// clears the filter; `enabled: true` re-enables (and un-pauses) the endpoint.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WebhookPatch {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Vec<String>, required = false)]
    pub events: Option<Vec<String>>,
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Vec<String>>, required = false)]
    pub content_types: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = bool, required = false)]
    pub enabled: Option<bool>,
}

/// `201` of `POST /webhooks`: the endpoint and its secret, shown this once.
#[derive(Serialize, ToSchema)]
pub struct CreatedWebhookDto {
    pub webhook: WebhookDto,
    /// `whsec_…`: store it now; it is never shown again.
    pub secret: String,
}

/// `200` of `POST /webhooks/{id}/secret`.
#[derive(Serialize, ToSchema)]
pub struct RotatedSecretDto {
    /// The new secret, shown this once.
    pub secret: String,
    /// Until then deliveries carry a signature with the previous secret too.
    pub previous_valid_until: DateTime<Utc>,
}

/// `202` of a test delivery or a redelivery.
#[derive(Debug, Serialize, ToSchema)]
pub struct QueuedDeliveryDto {
    pub delivery: Uuid,
}

/// `202` of `POST /webhooks/{id}/redeliver`.
#[derive(Debug, Serialize, ToSchema)]
pub struct RedeliveredDto {
    /// Deliveries queued again (at most 1 000).
    pub count: u32,
}

/// `POST /webhooks/{id}/redeliver`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RedeliverInput {
    /// Deliveries created at or after this time.
    pub since: DateTime<Utc>,
    /// Default `["failed", "skipped"]`; `pending` is refused.
    #[serde(default)]
    pub statuses: Option<Vec<DeliveryStatusDto>>,
}

/// One delivery (one event for this endpoint).
#[derive(Debug, Serialize, ToSchema)]
pub struct DeliveryDto {
    pub id: Uuid,
    /// The event id, also the `webhook-id` header of every attempt.
    pub event_id: Uuid,
    pub event_type: String,
    pub status: DeliveryStatusDto,
    pub attempts: i32,
    pub last_status_code: Option<i32>,
    /// The error class of the last attempt (`status 503`, `timed out`, …).
    pub last_error: Option<String>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// The delivery this one redelivers.
    pub redelivery_of: Option<Uuid>,
}

fn delivery_dto(d: ferrite_domain::Delivery, event_type: String) -> DeliveryDto {
    DeliveryDto {
        id: d.id.as_uuid(),
        event_id: d.event_id.as_uuid(),
        event_type,
        status: d.status.into(),
        attempts: d.attempts,
        last_status_code: d.last_status_code,
        last_error: d.last_error,
        next_attempt_at: d.next_attempt_at.map(|t| t.as_datetime()),
        created_at: d.created_at.as_datetime(),
        finished_at: d.finished_at.map(|t| t.as_datetime()),
        redelivery_of: d.redelivery_of.map(|r| r.as_uuid()),
    }
}

impl From<DeliveryListed> for DeliveryDto {
    fn from(l: DeliveryListed) -> Self {
        delivery_dto(l.delivery, l.event_type)
    }
}

/// `GET /webhooks/{id}/deliveries`: one page, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeliveryList {
    pub items: Vec<DeliveryDto>,
    pub next_cursor: Option<String>,
}

/// One attempt of a delivery.
#[derive(Debug, Serialize, ToSchema)]
pub struct AttemptDto {
    pub attempt: i32,
    pub started_at: DateTime<Utc>,
    pub duration_ms: i32,
    pub status_code: Option<i32>,
    pub error: Option<String>,
    /// At most 1 KiB of the receiver's answer, as plain text (show it escaped).
    pub response_excerpt: Option<String>,
}

impl From<Attempt> for AttemptDto {
    fn from(a: Attempt) -> Self {
        Self {
            attempt: a.attempt,
            started_at: a.started_at.as_datetime(),
            duration_ms: a.duration_ms,
            status_code: a.status_code,
            error: a.error,
            response_excerpt: a.response_excerpt,
        }
    }
}

/// The event a delivery carries.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookEventDto {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub timestamp: DateTime<Utc>,
    /// The event's `data` as stored (`url` is added at delivery): a JSON object.
    pub data: serde_json::Value,
}

impl From<WebhookEventRow> for WebhookEventDto {
    fn from(e: WebhookEventRow) -> Self {
        Self {
            id: e.id.as_uuid(),
            kind: e.kind,
            timestamp: e.created_at.as_datetime(),
            data: e.data,
        }
    }
}

/// `GET /webhooks/{id}/deliveries/{delivery}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeliveryDetailDto {
    pub delivery: DeliveryDto,
    pub event: WebhookEventDto,
    pub attempts: Vec<AttemptDto>,
}

impl From<DeliveryDetail> for DeliveryDetailDto {
    fn from(d: DeliveryDetail) -> Self {
        let kind = d.event.kind.clone();
        Self {
            delivery: delivery_dto(d.delivery, kind),
            event: d.event.into(),
            attempts: d.attempts.into_iter().map(Into::into).collect(),
        }
    }
}

/// One event type of the catalogue.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventTypeDto {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub group: &'static str,
    pub description: &'static str,
}

impl From<&EventTypeInfo> for EventTypeDto {
    fn from(t: &EventTypeInfo) -> Self {
        Self {
            kind: t.kind,
            group: t.group,
            description: t.description,
        }
    }
}

/// `GET /webhooks/events`.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventCatalogueDto {
    /// The payload version.
    pub version: i32,
    pub types: Vec<EventTypeDto>,
}
