//! One delivery attempt (design 18 §4.2–§4.4): the checks, the signed request, and the
//! attempt, the delivery, the endpoint's health and the next job in one `Tx`.

use std::time::Duration;

use chrono::SecondsFormat;
use ferrite_domain::{
    Attempt, DeliveryId, DeliveryStatus, DeliveryUpdate, DisabledReason, NewAudit, Timestamp,
    WebhookEndpoint, WebhookEventRow,
};
use serde_json::{Map, Value, json};
use url::Url;
use zeroize::Zeroizing;

use super::{
    AUTO_DISABLE_AFTER, AUTO_DISABLE_FAILURES, MAX_WEBHOOK_BODY,
    catalogue::PING,
    schedule::{next_delay_in, random_jitter},
    secret,
    service::{WebhookService, current_install},
    sign,
    transport::{OutgoingWebhook, SendOutcome, WebhookTransport},
    url::host_of,
};
use crate::{
    CoreError,
    auth::{older_than, plus},
    jobs::{WEBHOOK_DELIVER_PRIORITY, webhook_deliver_job},
    tx::finish,
};

/// What one attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    /// Nothing to do: the delivery is gone, finished or this attempt already recorded.
    Nothing,
    /// Not sent and marked `skipped` (the reason as stored).
    Skipped(&'static str),
    /// Sent; the receiver's status, `None` when it never answered.
    Sent { status: Option<u16> },
    /// Sent, and the endpoint was disabled automatically (`failing` or `gone`): the
    /// caller notifies the administrators after this returns.
    AutoDisabled {
        endpoint: Box<WebhookEndpoint>,
        reason: DisabledReason,
        status: Option<u16>,
    },
}

const SKIP_PAUSED: &str = "paused: copied database";
const SKIP_DISABLED: &str = "endpoint disabled";
const SKIP_SECRET: &str = "secret cannot be read: rotate it";
const SKIP_TOO_LARGE: &str = "payload too large";
const SKIP_URL: &str = "the URL is not valid";

fn micros_rfc3339(t: Timestamp) -> String {
    t.as_datetime().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// The request body (design 18 §3.2), rendered from the stored event deterministically:
/// `site.url` without a trailing slash; a content event's `url` is `site.url + path`
/// when it was public at emit time, else `null`.
pub fn render_body(event: &WebhookEventRow, site_url: &Url, version: i32) -> Vec<u8> {
    let site = site_url.as_str().trim_end_matches('/');
    let mut data = match &event.data {
        Value::Object(m) => m.clone(),
        other => {
            let mut m = Map::new();
            m.insert("value".into(), other.clone());
            m
        }
    };
    if let Some(public) = data.remove("public") {
        let url = match (public.as_bool(), data.get("path").and_then(Value::as_str)) {
            (Some(true), Some(path)) => Value::String(format!("{site}{path}")),
            _ => Value::Null,
        };
        data.insert("url".into(), url);
    }
    let body = json!({
        "id": event.id,
        "type": event.kind,
        "timestamp": micros_rfc3339(event.created_at),
        "version": version,
        "site": {"url": site},
        "data": Value::Object(data),
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

/// What the attempt row and the delivery record of one outcome.
struct AttemptResult {
    status: Option<u16>,
    error: Option<String>,
    excerpt: Option<String>,
    retry_after: Option<Duration>,
    elapsed: Duration,
}

fn result_of(outcome: SendOutcome) -> AttemptResult {
    match outcome {
        SendOutcome::Answered {
            status,
            retry_after,
            excerpt,
            elapsed,
        } => AttemptResult {
            status: Some(status),
            error: match status {
                200..=299 => None,
                300..=399 => Some(format!("redirect not followed (status {status})")),
                _ => Some(format!("status {status}")),
            },
            excerpt: (!excerpt.is_empty()).then_some(excerpt),
            retry_after: matches!(status, 429 | 503).then_some(retry_after).flatten(),
            elapsed,
        },
        SendOutcome::Failed { class, elapsed, .. } => AttemptResult {
            status: None,
            error: Some(class.to_owned()),
            excerpt: None,
            retry_after: None,
            elapsed,
        },
    }
}

impl WebhookService {
    /// Makes attempt `attempt` (1-based) of `delivery` through `transport` (design 18
    /// §4.2 steps 1, 2, 4–6; the slot, step 3, is the caller's). A store error is
    /// returned as is: the caller retries the same attempt, and the checks of step 1 and
    /// the conditional record keep it single.
    pub async fn deliver(
        &self,
        delivery: DeliveryId,
        attempt: u32,
        site_url: &Url,
        transport: &dyn WebhookTransport,
    ) -> Result<Delivered, CoreError> {
        let read = self.store.read();
        let w = read.webhooks();
        let Some(d) = w.get_delivery(delivery).await? else {
            return Ok(Delivered::Nothing);
        };
        if d.status != DeliveryStatus::Pending || i64::from(d.attempts) + 1 != i64::from(attempt) {
            return Ok(Delivered::Nothing);
        }
        let (Some(endpoint), Some(event)) = (
            w.get_endpoint(d.endpoint_id).await?,
            w.get_event(d.event_id).await?,
        ) else {
            return Ok(Delivered::Nothing);
        };
        let now = Timestamp::now();
        let skip = |reason: &'static str| async move {
            w.set_delivery_status(d.id, DeliveryStatus::Skipped, Some(reason), now)
                .await?;
            Ok::<_, CoreError>(Delivered::Skipped(reason))
        };
        if endpoint.install_id != current_install(read).await? {
            return skip(SKIP_PAUSED).await;
        }
        if !endpoint.enabled && event.kind != PING {
            return skip(SKIP_DISABLED).await;
        }
        let Ok(current) = secret::open_for(&self.key, endpoint.id, &endpoint.secret_sealed) else {
            return skip(SKIP_SECRET).await;
        };
        let mut keys: Vec<Zeroizing<Vec<u8>>> = vec![current];
        if let (Some(sealed), Some(until)) = (
            &endpoint.previous_secret_sealed,
            endpoint.previous_secret_expires_at,
        ) && until > now
            && let Ok(previous) = secret::open_for(&self.key, endpoint.id, sealed)
        {
            keys.push(previous);
        }
        let body = render_body(&event, site_url, endpoint.payload_version);
        if body.len() > MAX_WEBHOOK_BODY {
            return skip(SKIP_TOO_LARGE).await;
        }
        let Ok(url) = Url::parse(&endpoint.url) else {
            return skip(SKIP_URL).await;
        };
        let id = event.id.to_string();
        let ts = now.as_micros().div_euclid(1_000_000);
        let key_refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
        let signature = sign::header(&key_refs, &id, ts, &body);
        drop(keys);
        let request = OutgoingWebhook {
            url,
            headers: vec![
                ("webhook-id", id),
                ("webhook-timestamp", ts.to_string()),
                ("webhook-signature", signature),
                ("content-type", "application/json".to_owned()),
            ],
            body,
        };
        let outcome = transport.send(request).await;
        self.record(&endpoint, &event, d.id, attempt, now, result_of(outcome))
            .await
    }

    /// Step 6: one `Tx` for the attempt row, the delivery, the endpoint's health, an
    /// auto-disable and the next attempt's job.
    async fn record(
        &self,
        endpoint: &WebhookEndpoint,
        event: &WebhookEventRow,
        delivery: DeliveryId,
        attempt: u32,
        started: Timestamp,
        r: AttemptResult,
    ) -> Result<Delivered, CoreError> {
        let jitter = random_jitter()?;
        let tx = self.store.begin().await?;
        let done = async {
            let repos = tx.as_ref();
            let Some(mut e) = repos
                .webhooks()
                .get_endpoint_for_update(endpoint.id)
                .await?
            else {
                return Ok(Delivered::Nothing);
            };
            let finished = Timestamp::now();
            let success = r.status.is_some_and(|s| (200..300).contains(&s));
            let gone = r.status == Some(410);
            // A test delivery gets one attempt.
            let next = if success || gone || event.kind == PING {
                None
            } else {
                next_delay_in(&self.schedule, attempt, r.retry_after, jitter)
            };
            let next_at = next.map(|d| plus(finished, d)).transpose()?;
            let status = if success {
                DeliveryStatus::Delivered
            } else if next_at.is_some() {
                DeliveryStatus::Pending
            } else {
                DeliveryStatus::Failed
            };
            let attempt_no = i32::try_from(attempt).map_err(|_| CoreError::Internal("attempt"))?;
            let row = Attempt {
                delivery_id: delivery,
                attempt: attempt_no,
                started_at: started,
                duration_ms: i32::try_from(r.elapsed.as_millis()).unwrap_or(i32::MAX),
                status_code: r.status.map(i32::from),
                error: r.error.clone(),
                response_excerpt: r.excerpt.clone(),
            };
            let update = DeliveryUpdate {
                status,
                attempts: attempt_no,
                next_attempt_at: next_at,
                last_status_code: r.status.map(i32::from),
                last_error: r.error.clone(),
                finished_at: (status != DeliveryStatus::Pending).then_some(finished),
            };
            if !repos.webhooks().record_attempt(&row, &update).await? {
                return Ok(Delivered::Nothing);
            }
            if success {
                e.consecutive_failures = 0;
                e.failing_since = None;
                e.last_success_at = Some(finished);
            } else {
                e.consecutive_failures = e.consecutive_failures.saturating_add(1);
                e.failing_since.get_or_insert(finished);
                e.last_failure_at = Some(finished);
            }
            let reason = if !e.enabled {
                None
            } else if gone {
                Some(DisabledReason::Gone)
            } else if e.consecutive_failures >= AUTO_DISABLE_FAILURES
                && e.failing_since
                    .is_some_and(|since| older_than(since, finished, AUTO_DISABLE_AFTER))
            {
                Some(DisabledReason::Failing)
            } else {
                None
            };
            if let Some(reason) = reason {
                e.enabled = false;
                e.disabled_reason = Some(reason);
                e.disabled_at = Some(finished);
                repos
                    .audit()
                    .append(&NewAudit {
                        actor_id: None,
                        action: "webhook.auto_disabled".into(),
                        target_kind: "webhook".into(),
                        target_id: Some(e.id.to_string()),
                        details: json!({"name": e.name, "host": host_of(&e.url),
                                        "reason": reason.as_str(),
                                        "consecutive_failures": e.consecutive_failures}),
                    })
                    .await?;
            }
            repos.webhooks().update_endpoint(&e).await?;
            if let Some(at) = next_at {
                repos
                    .jobs()
                    .enqueue(&webhook_deliver_job(
                        delivery,
                        attempt + 1,
                        Some(at),
                        WEBHOOK_DELIVER_PRIORITY,
                    ))
                    .await?;
            }
            Ok(match reason {
                Some(reason) => Delivered::AutoDisabled {
                    endpoint: Box::new(e),
                    reason,
                    status: r.status,
                },
                None => Delivered::Sent { status: r.status },
            })
        }
        .await;
        finish(tx, done).await
    }
}
