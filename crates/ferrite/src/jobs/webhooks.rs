//! `webhook.deliver` (one delivery attempt, design 18 §4.2) and the hourly
//! `webhooks.prune` (§4.6). The schedule is kept by core: each attempt enqueues the next
//! one, so the runner's own attempts only retry infrastructure failures.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic, builtin::PRUNE_BUDGET};
use ferrite_core::{
    CoreError,
    jobs::{WEBHOOK_DELIVER_KIND, WEBHOOK_DELIVER_VERSION, WEBHOOKS_PRUNE_KIND, WebhookDeliver},
    mail::{Mailer, OutgoingMail, check_address, subject_title},
    settings,
    webhooks::{Delivered, RETENTION, WebhookService, WebhookTransport, host_of},
};
use ferrite_domain::{DisabledReason, Job, Timestamp, WebhookEndpoint};
use ferrite_store::{MAX_PRUNE_BATCH, Store};
use serde_json::json;
use tokio::sync::Semaphore;
use url::Url;

/// Longest one attempt may run (the request itself is bounded at 15 s).
pub const WEBHOOK_DELIVER_TIMEOUT: Duration = Duration::from_secs(30);
/// How often `webhooks.prune` runs.
pub const WEBHOOKS_PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Most administrators one auto-disable notice goes to.
const NOTICE_MAX_ADMINS: u32 = 50;

/// Where the auto-disable notice goes (`[mail]` configured).
pub struct WebhookNotice {
    pub mailer: Arc<dyn Mailer>,
    pub store: Arc<dyn Store>,
    /// `site.title` from the config, when the option is unset.
    pub site_title: String,
}

/// `webhook.deliver`: one attempt of one delivery through `transport`. At most `slots`
/// attempts run at once on a node (`max(1, workers - 1)`, so core jobs always find a
/// worker); one more is `Busy` (attempt refunded). A store error is `Retry` (the same
/// attempt again; core keeps it single). An automatic disable mails the active
/// administrators after the attempt's `Tx` committed, best effort.
pub struct WebhookDeliverHandler {
    service: Arc<WebhookService>,
    transport: Arc<dyn WebhookTransport>,
    site_url: Url,
    slots: Semaphore,
    notice: Option<WebhookNotice>,
}

/// Concurrent deliveries on a node with `workers` job workers.
pub fn webhook_slots(workers: usize) -> usize {
    workers.saturating_sub(1).max(1)
}

impl WebhookDeliverHandler {
    pub fn new(
        service: Arc<WebhookService>,
        transport: Arc<dyn WebhookTransport>,
        site_url: Url,
        slots: usize,
        notice: Option<WebhookNotice>,
    ) -> Self {
        Self {
            service,
            transport,
            site_url,
            slots: Semaphore::new(slots.clamp(1, Semaphore::MAX_PERMITS)),
            notice,
        }
    }

    async fn notify(&self, endpoint: &WebhookEndpoint, reason: DisabledReason) {
        let Some(notice) = &self.notice else {
            return;
        };
        let admins = match notice
            .store
            .read()
            .users()
            .list_active_admins(NOTICE_MAX_ADMINS)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(error = %e, "listing admins for the webhook notice failed");
                return;
            }
        };
        let bcc: Vec<String> = admins
            .into_iter()
            .filter(|u| check_address(&u.email).is_ok())
            .map(|u| u.email)
            .collect();
        if bcc.is_empty() {
            tracing::warn!("webhook disabled: no active admin has a usable e-mail address");
            return;
        }
        let title =
            settings::site_title_or_configured(notice.store.as_ref(), &notice.site_title).await;
        let mail = notice_mail(bcc, &title, &self.site_url, endpoint, reason);
        if let Err(e) = notice.mailer.send(mail).await {
            tracing::warn!(error = %e, "the webhook notice could not be queued");
        }
    }
}

/// The auto-disable notice (design 18 §4.4): name, host, reason, failing since and a
/// link to the endpoint in the admin. Never the URL's path or query.
pub fn notice_mail(
    bcc: Vec<String>,
    site_title: &str,
    site_url: &Url,
    e: &WebhookEndpoint,
    reason: DisabledReason,
) -> OutgoingMail {
    let name: String = e
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    let prefix = subject_title(site_title).map_or(String::new(), |t| format!("[{t}] "));
    let why = match reason {
        DisabledReason::Gone => "the receiver answered 410 Gone (asked to stop)",
        DisabledReason::Failing => {
            "every attempt has failed for more than 5 days (at least 10 attempts in a row)"
        }
        DisabledReason::Manual => "it was disabled",
    };
    let since = e.failing_since.map_or_else(
        || "-".to_owned(),
        |t| t.as_datetime().format("%Y-%m-%d %H:%M UTC").to_string(),
    );
    let site = site_url.as_str().trim_end_matches('/');
    let text = format!(
        "The webhook \"{name}\" (host {host}) was disabled: {why}.\n\
         \n\
         Failing since: {since}\n\
         Failed attempts in a row: {failures}\n\
         \n\
         Fix the receiver, then enable the webhook again and redeliver what was missed:\n\
         {site}/admin/webhooks/{id}\n",
        host = host_of(&e.url),
        failures = e.consecutive_failures,
        id = e.id,
    );
    OutgoingMail {
        to: Vec::new(),
        bcc,
        subject: format!("{prefix}Webhook disabled: {name}"),
        text,
    }
}

#[async_trait]
impl JobHandler for WebhookDeliverHandler {
    fn kind(&self) -> &'static str {
        WEBHOOK_DELIVER_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[WEBHOOK_DELIVER_VERSION]
    }

    fn timeout(&self) -> Duration {
        WEBHOOK_DELIVER_TIMEOUT
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: WebhookDeliver = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let Ok(_slot) = self.slots.try_acquire() else {
            return Err(JobError::Busy(
                "busy: every webhook slot of this node is in use".to_owned(),
            ));
        };
        let outcome = self
            .service
            .deliver(
                payload.delivery,
                payload.attempt,
                &self.site_url,
                self.transport.as_ref(),
            )
            .await
            .map_err(|e| match e {
                CoreError::Invalid(_) | CoreError::InvalidField { .. } => {
                    JobError::Permanent(e.to_string())
                }
                other => JobError::Retry(other.to_string()),
            })?;
        if let Delivered::AutoDisabled {
            endpoint, reason, ..
        } = outcome
        {
            tracing::warn!(
                webhook = %endpoint.id,
                host = %host_of(&endpoint.url),
                reason = reason.as_str(),
                "webhook disabled automatically"
            );
            self.notify(&endpoint, reason).await;
        }
        Ok(())
    }
}

/// `webhooks.prune`: deletes delivery-log events older than [`RETENTION`] (with their
/// deliveries and attempts) in batches within [`PRUNE_BUDGET`].
pub struct WebhooksPruneHandler {
    pub service: Arc<WebhookService>,
}

#[async_trait]
impl JobHandler for WebhooksPruneHandler {
    fn kind(&self) -> &'static str {
        WEBHOOKS_PRUNE_KIND
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let now = Timestamp::now();
        let before = Timestamp::from_micros(
            now.as_micros()
                .saturating_sub(i64::try_from(RETENTION.as_micros()).unwrap_or(i64::MAX)),
        )
        .map_err(|e| JobError::Permanent(e.to_string()))?;
        let started = Instant::now();
        let mut total = 0;
        loop {
            let n = self
                .service
                .prune(before, MAX_PRUNE_BATCH)
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?;
            total += n;
            if n < u64::from(MAX_PRUNE_BATCH) || started.elapsed() >= PRUNE_BUDGET {
                break;
            }
        }
        if total > 0 {
            tracing::info!(deleted = total, "pruned webhook deliveries");
        }
        Ok(())
    }
}

/// The [`WEBHOOKS_PRUNE_KIND`] periodic, every [`WEBHOOKS_PRUNE_EVERY`].
pub fn webhooks_prune_periodic() -> Periodic {
    Periodic {
        kind: WEBHOOKS_PRUNE_KIND,
        every: WEBHOOKS_PRUNE_EVERY,
        payload: json!({"v": 1}),
        max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
        offset: Duration::ZERO,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ferrite_domain::WebhookId;

    #[test]
    fn slots_leave_a_worker_free() {
        assert_eq!(webhook_slots(1), 1);
        assert_eq!(webhook_slots(2), 1);
        assert_eq!(webhook_slots(8), 7);
    }

    #[test]
    fn notice_names_host_not_url() {
        let now = Timestamp::now();
        let e = WebhookEndpoint {
            id: WebhookId::new(),
            name: "Deploy\nhook".into(),
            url: "https://hooks.example.com/build?token=s3cr3t".into(),
            events: vec!["*".into()],
            content_types: None,
            enabled: false,
            disabled_reason: Some(DisabledReason::Gone),
            disabled_at: Some(now),
            install_id: "i".into(),
            secret_sealed: "x".into(),
            previous_secret_sealed: None,
            previous_secret_expires_at: None,
            payload_version: 1,
            failing_since: Some(now),
            consecutive_failures: 3,
            last_success_at: None,
            last_failure_at: Some(now),
            created_by: None,
            created_at: now,
            updated_at: now,
        };
        let site = Url::parse("https://example.com/").unwrap();
        let m = notice_mail(
            vec!["a@example.com".into()],
            "Blog",
            &site,
            &e,
            DisabledReason::Gone,
        );
        assert_eq!(m.subject, "[Blog] Webhook disabled: Deployhook");
        assert!(m.text.contains("hooks.example.com"), "{}", m.text);
        assert!(!m.text.contains("s3cr3t") && !m.text.contains("/build"));
        assert!(
            m.text
                .contains(&format!("https://example.com/admin/webhooks/{}", e.id))
        );
        assert!(m.check().is_ok());
    }
}
