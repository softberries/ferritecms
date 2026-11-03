//! Outgoing webhooks (design 18): endpoints, events, deliveries and attempts.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{DeliveryId, Timestamp, UserId, WebhookEventId, WebhookId, string_enum};

string_enum!(
    /// Why an endpoint is disabled (design 18 §4.4).
    DisabledReason {
        Manual => "manual",
        Failing => "failing",
        Gone => "gone",
    }
);

string_enum!(
    /// The state of one delivery: `pending` until it is `delivered`, `failed` (the last
    /// attempt failed, or `410`) or `skipped` (endpoint disabled or paused, secret
    /// unreadable).
    DeliveryStatus {
        Pending => "pending",
        Delivered => "delivered",
        Failed => "failed",
        Skipped => "skipped",
    }
);

/// A registered endpoint, the whole row. `Debug` leaves out the sealed secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct WebhookEndpoint {
    pub id: WebhookId,
    pub name: String,
    pub url: String,
    /// Subscription patterns: exact types, `<group>.*` or `*`.
    pub events: Vec<String>,
    /// Content type names narrowing `content.*` events; `None` for every type.
    pub content_types: Option<Vec<String>>,
    pub enabled: bool,
    pub disabled_reason: Option<DisabledReason>,
    pub disabled_at: Option<Timestamp>,
    /// `schema_meta.install_id` when created or last enabled; another value means a copy.
    pub install_id: String,
    /// The current secret, sealed for this endpoint.
    pub secret_sealed: String,
    /// The secret before the last rotation, sealed, valid until `previous_secret_expires_at`.
    pub previous_secret_sealed: Option<String>,
    pub previous_secret_expires_at: Option<Timestamp>,
    pub payload_version: i32,
    pub failing_since: Option<Timestamp>,
    pub consecutive_failures: i32,
    pub last_success_at: Option<Timestamp>,
    pub last_failure_at: Option<Timestamp>,
    pub created_by: Option<UserId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl fmt::Debug for WebhookEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookEndpoint")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("events", &self.events)
            .field("content_types", &self.content_types)
            .field("enabled", &self.enabled)
            .field("disabled_reason", &self.disabled_reason)
            .field("install_id", &self.install_id)
            .field("secret_sealed", &"[redacted]")
            .field(
                "previous_secret_sealed",
                &self.previous_secret_sealed.as_ref().map(|_| "[redacted]"),
            )
            .field("consecutive_failures", &self.consecutive_failures)
            .finish_non_exhaustive()
    }
}

/// One event, stored once whatever the number of endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookEventRow {
    pub id: WebhookEventId,
    /// The event type, e.g. `content.published`.
    pub kind: String,
    pub data: serde_json::Value,
    pub created_at: Timestamp,
}

/// One event for one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub id: DeliveryId,
    pub endpoint_id: WebhookId,
    pub event_id: WebhookEventId,
    pub status: DeliveryStatus,
    /// Attempts recorded so far.
    pub attempts: i32,
    pub next_attempt_at: Option<Timestamp>,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    pub redelivery_of: Option<DeliveryId>,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// A delivery with its event's type, as the log lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryListed {
    pub delivery: Delivery,
    pub event_type: String,
}

/// One try of a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub delivery_id: DeliveryId,
    /// 1-based.
    pub attempt: i32,
    pub started_at: Timestamp,
    pub duration_ms: i32,
    pub status_code: Option<i32>,
    /// The error class only (design 18 §6.1).
    pub error: Option<String>,
    /// At most 1 KiB of the response body.
    pub response_excerpt: Option<String>,
}

/// The delivery fields an attempt updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryUpdate {
    pub status: DeliveryStatus,
    pub attempts: i32,
    pub next_attempt_at: Option<Timestamp>,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    pub finished_at: Option<Timestamp>,
}

/// A page of an endpoint's deliveries, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryQuery {
    pub status: Option<DeliveryStatus>,
    /// 1..=[`MAX_PAGE`](crate::MAX_PAGE).
    pub limit: u32,
    pub cursor: Option<String>,
}

impl Default for DeliveryQuery {
    fn default() -> Self {
        Self {
            status: None,
            limit: 50,
            cursor: None,
        }
    }
}

/// Deliveries of one endpoint by status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryCounts {
    pub delivered: u64,
    pub failed: u64,
    pub pending: u64,
    pub skipped: u64,
}

impl DeliveryCounts {
    /// Adds `n` to the count of `status`.
    pub fn add(&mut self, status: DeliveryStatus, n: u64) {
        match status {
            DeliveryStatus::Delivered => self.delivered += n,
            DeliveryStatus::Failed => self.failed += n,
            DeliveryStatus::Pending => self.pending += n,
            DeliveryStatus::Skipped => self.skipped += n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_sealed_secrets() {
        let now = Timestamp::now();
        let e = WebhookEndpoint {
            id: WebhookId::new(),
            name: "n".into(),
            url: "https://hooks.example.com/x?token=abc".into(),
            events: vec!["*".into()],
            content_types: None,
            enabled: true,
            disabled_reason: None,
            disabled_at: None,
            install_id: "i".into(),
            secret_sealed: "SEALEDCURRENT".into(),
            previous_secret_sealed: Some("SEALEDPREVIOUS".into()),
            previous_secret_expires_at: None,
            payload_version: 1,
            failing_since: None,
            consecutive_failures: 0,
            last_success_at: None,
            last_failure_at: None,
            created_by: None,
            created_at: now,
            updated_at: now,
        };
        let dbg = format!("{e:?}");
        assert!(
            !dbg.contains("SEALED") && !dbg.contains("token=abc"),
            "{dbg}"
        );
    }

    #[test]
    fn statuses_roundtrip() {
        for s in DeliveryStatus::ALL {
            assert_eq!(s.as_str().parse::<DeliveryStatus>().unwrap(), *s);
        }
        for r in DisabledReason::ALL {
            assert_eq!(r.as_str().parse::<DisabledReason>().unwrap(), *r);
        }
    }
}
