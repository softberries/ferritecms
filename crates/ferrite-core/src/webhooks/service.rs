//! [`WebhookService`]: endpoints, test deliveries, rotation, the delivery log and
//! redelivery (design 18 §5–§8). Capabilities are checked here, never in handlers.

use std::sync::Arc;
use std::time::Duration;

use ferrite_domain::{
    Attempt, Delivery, DeliveryCounts, DeliveryId, DeliveryListed, DeliveryQuery, DeliveryStatus,
    DisabledReason, NewAudit, Page, Secret, Timestamp, WebhookEndpoint, WebhookEventId,
    WebhookEventRow, WebhookId,
};
use ferrite_store::{Repos, SCHEMA_META_INSTALL_ID, Store};
use serde_json::{Value, json};

use super::{
    MAX_ENDPOINTS, MAX_NAME, MAX_REDELIVER, ROTATION_WINDOW,
    catalogue::{self, EVENT_TYPES, EventTypeInfo, MAX_PATTERNS, PAYLOAD_VERSION, PING},
    schedule::DELAYS,
    secret,
    url::{UrlPolicy, check_url, host_of},
};
use crate::{
    Actor, Capability, CoreError,
    auth::{SiteKey, limits, plus},
    jobs::{WEBHOOK_DELIVER_PRIORITY, WEBHOOK_TEST_PRIORITY, webhook_deliver_job},
    require, require_reader,
    tx::finish,
};

/// What the list and the detail show about an endpoint's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookState {
    /// Enabled, last attempt fine (or none yet).
    Active,
    /// Enabled, failing since `failing_since`.
    Failing,
    /// Disabled (`disabled_reason` says why).
    Disabled,
    /// Enabled but created under another install id: a copied or restored database.
    Paused,
}

impl WebhookState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Failing => "failing",
            Self::Disabled => "disabled",
            Self::Paused => "paused",
        }
    }
}

/// An endpoint as the API shows it. The sealed secrets in `endpoint` never leave core
/// through a DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookView {
    pub endpoint: WebhookEndpoint,
    pub state: WebhookState,
    /// Until when the previous secret still signs (a rotation in progress).
    pub secret_rotating_until: Option<Timestamp>,
    /// Deliveries created in the last 24 hours, by status.
    pub counts_24h: DeliveryCounts,
}

/// A new endpoint.
#[derive(Debug, Clone, Default)]
pub struct WebhookInput {
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub content_types: Option<Vec<String>>,
    /// Default `true`.
    pub enabled: Option<bool>,
    /// Bring your own secret; generated when `None`.
    pub secret: Option<Secret>,
}

/// A partial update; `content_types: Some(None)` clears the filter.
#[derive(Debug, Clone, Default)]
pub struct WebhookPatch {
    pub name: Option<String>,
    pub url: Option<String>,
    pub events: Option<Vec<String>>,
    pub content_types: Option<Option<Vec<String>>>,
    pub enabled: Option<bool>,
}

/// One delivery with its event and attempts.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryDetail {
    pub delivery: Delivery,
    pub event: WebhookEventRow,
    pub attempts: Vec<Attempt>,
}

/// Endpoints and their deliveries.
pub struct WebhookService {
    pub(super) store: Arc<dyn Store>,
    pub(super) key: SiteKey,
    pub(super) policy: UrlPolicy,
    pub(super) schedule: Vec<Duration>,
}

/// The database's install id (`schema_meta.install_id`); empty when it has none, which
/// pauses every endpoint.
pub(super) async fn current_install(repos: &dyn Repos) -> Result<String, CoreError> {
    Ok(repos
        .schema_meta()
        .get(SCHEMA_META_INSTALL_ID)
        .await?
        .unwrap_or_default())
}

fn state_of(e: &WebhookEndpoint, install: &str) -> WebhookState {
    if !e.enabled {
        WebhookState::Disabled
    } else if e.install_id != install {
        WebhookState::Paused
    } else if e.failing_since.is_some() {
        WebhookState::Failing
    } else {
        WebhookState::Active
    }
}

fn check_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 || n > MAX_NAME {
        return Err(CoreError::invalid_field(
            "name",
            format!("1 to {MAX_NAME} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "name",
            "no control characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

async fn check_content_types(
    repos: &dyn Repos,
    types: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, CoreError> {
    let Some(types) = types else {
        return Ok(None);
    };
    let bad = |m: String| CoreError::invalid_field("content_types", m);
    if types.is_empty() {
        return Err(bad(
            "choose at least one content type, or none for all".into()
        ));
    }
    if types.len() > MAX_PATTERNS {
        return Err(bad(format!("at most {MAX_PATTERNS} content types")));
    }
    let mut out: Vec<String> = Vec::with_capacity(types.len());
    for t in types {
        let known = t.len() <= 64
            && !t.contains('\0')
            && repos
                .types()
                .get(&t)
                .await?
                .is_some_and(|ty| ty.name != "media");
        if !known {
            let shown: String = t.chars().take(64).collect();
            return Err(bad(format!("unknown content type {shown:?}")));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(Some(out))
}

fn audit(actor: Option<&Actor>, action: &str, id: WebhookId, details: Value) -> NewAudit {
    NewAudit {
        actor_id: actor.map(|a| a.user),
        action: action.to_owned(),
        target_kind: "webhook".to_owned(),
        target_id: Some(id.to_string()),
        details,
    }
}

/// Enables `e` (design 18 §4.4): clears the reason and the health, stamps the install id.
fn enable(e: &mut WebhookEndpoint, install: &str) {
    e.enabled = true;
    e.disabled_reason = None;
    e.disabled_at = None;
    e.failing_since = None;
    e.consecutive_failures = 0;
    install.clone_into(&mut e.install_id);
}

impl WebhookService {
    /// The service over `store`, sealing secrets under `key`, checking URLs by `policy`.
    pub fn new(store: Arc<dyn Store>, key: SiteKey, policy: UrlPolicy) -> Self {
        Self {
            store,
            key,
            policy,
            schedule: DELAYS.to_vec(),
        }
    }

    /// The same service with another retry schedule (a test seam).
    #[cfg(feature = "test-seams")]
    pub fn with_schedule(mut self, schedule: Vec<Duration>) -> Self {
        self.schedule = schedule;
        self
    }

    /// The URL policy (`local_targets` and the address rule).
    pub fn policy(&self) -> UrlPolicy {
        self.policy
    }

    async fn view(&self, repos: &dyn Repos, e: WebhookEndpoint) -> Result<WebhookView, CoreError> {
        let install = current_install(repos).await?;
        self.view_with(repos, e, &install).await
    }

    async fn view_with(
        &self,
        repos: &dyn Repos,
        e: WebhookEndpoint,
        install: &str,
    ) -> Result<WebhookView, CoreError> {
        let now = Timestamp::now();
        let since = Timestamp::from_micros(now.as_micros() - 24 * 3600 * 1_000_000)
            .map_err(|_| CoreError::Internal("time out of range"))?;
        let counts_24h = repos.webhooks().delivery_counts(e.id, since).await?;
        let secret_rotating_until = e
            .previous_secret_sealed
            .as_ref()
            .and(e.previous_secret_expires_at)
            .filter(|t| *t > now);
        Ok(WebhookView {
            state: state_of(&e, install),
            secret_rotating_until,
            counts_24h,
            endpoint: e,
        })
    }

    /// The event catalogue (`manage_webhooks` + `webhooks:read`).
    pub fn event_types(&self, actor: &Actor) -> Result<&'static [EventTypeInfo], CoreError> {
        require_reader(actor, Capability::ManageWebhooks)?;
        Ok(EVENT_TYPES)
    }

    /// Every endpoint by name (at most [`MAX_ENDPOINTS`]).
    pub async fn list(&self, actor: &Actor) -> Result<Vec<WebhookView>, CoreError> {
        require_reader(actor, Capability::ManageWebhooks)?;
        let repos = self.store.read();
        let install = current_install(repos).await?;
        let mut out = Vec::new();
        for e in repos.webhooks().list_endpoints().await? {
            out.push(self.view_with(repos, e, &install).await?);
        }
        Ok(out)
    }

    /// One endpoint.
    pub async fn get(&self, actor: &Actor, id: WebhookId) -> Result<WebhookView, CoreError> {
        require_reader(actor, Capability::ManageWebhooks)?;
        let repos = self.store.read();
        let e = repos
            .webhooks()
            .get_endpoint(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        self.view(repos, e).await
    }

    /// Creates an endpoint and returns it with its secret, shown this once.
    pub async fn create(
        &self,
        actor: &Actor,
        input: WebhookInput,
    ) -> Result<(WebhookView, Secret), CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        let name = check_name(&input.name)?;
        check_url(&input.url, &self.policy)?;
        let events = catalogue::check_patterns(&input.events)?;
        let secret = match input.secret {
            Some(s) => {
                secret::parse(s.expose())?;
                s
            }
            None => secret::generate()?,
        };
        let tx = self.store.begin().await?;
        let done = async {
            let content_types = check_content_types(tx.as_ref(), input.content_types).await?;
            if tx.webhooks().count_endpoints().await? >= MAX_ENDPOINTS {
                return Err(CoreError::Invalid(format!(
                    "at most {MAX_ENDPOINTS} webhooks"
                )));
            }
            let install = current_install(tx.as_ref()).await?;
            let id = WebhookId::new();
            let now = Timestamp::now();
            let enabled = input.enabled.unwrap_or(true);
            let e = WebhookEndpoint {
                id,
                name,
                url: input.url.clone(),
                events,
                content_types,
                enabled,
                disabled_reason: (!enabled).then_some(DisabledReason::Manual),
                disabled_at: (!enabled).then_some(now),
                install_id: install.clone(),
                secret_sealed: secret::seal_for(&self.key, id, &secret)?,
                previous_secret_sealed: None,
                previous_secret_expires_at: None,
                payload_version: PAYLOAD_VERSION,
                failing_since: None,
                consecutive_failures: 0,
                last_success_at: None,
                last_failure_at: None,
                created_by: Some(actor.user),
                created_at: now,
                updated_at: now,
            };
            tx.webhooks().insert_endpoint(&e).await?;
            tx.audit()
                .append(&audit(
                    Some(actor),
                    "webhook.create",
                    id,
                    json!({"name": e.name, "host": host_of(&e.url), "events": e.events,
                           "enabled": e.enabled}),
                ))
                .await?;
            self.view_with(tx.as_ref(), e, &install).await
        }
        .await;
        let view = finish(tx, done).await?;
        Ok((view, secret))
    }

    /// Applies `patch` (last write wins). `enabled: true` re-enables (and un-pauses) the
    /// endpoint; `enabled: false` disables it (`manual`).
    pub async fn update(
        &self,
        actor: &Actor,
        id: WebhookId,
        patch: WebhookPatch,
    ) -> Result<WebhookView, CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        let name = patch.name.as_deref().map(check_name).transpose()?;
        if let Some(url) = &patch.url {
            check_url(url, &self.policy)?;
        }
        let events = patch
            .events
            .as_deref()
            .map(catalogue::check_patterns)
            .transpose()?;
        let tx = self.store.begin().await?;
        let done = async {
            let repos = tx.as_ref();
            let mut e = repos
                .webhooks()
                .get_endpoint_for_update(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            let install = current_install(repos).await?;
            let before = e.clone();
            let mut changed: Vec<&'static str> = Vec::new();
            if let Some(name) = name
                && name != e.name
            {
                e.name = name;
                changed.push("name");
            }
            if let Some(url) = patch.url
                && url != e.url
            {
                e.url = url;
                changed.push("url");
            }
            if let Some(events) = events
                && events != e.events
            {
                e.events = events;
                changed.push("events");
            }
            if let Some(types) = patch.content_types {
                let types = check_content_types(repos, types).await?;
                if types != e.content_types {
                    e.content_types = types;
                    changed.push("content_types");
                }
            }
            let now = Timestamp::now();
            let mut toggled = None;
            match patch.enabled {
                Some(true) if !e.enabled || e.install_id != install => {
                    enable(&mut e, &install);
                    toggled = Some("webhook.enable");
                }
                Some(false) if e.enabled => {
                    e.enabled = false;
                    e.disabled_reason = Some(DisabledReason::Manual);
                    e.disabled_at = Some(now);
                    toggled = Some("webhook.disable");
                }
                _ => {}
            }
            if e == before {
                return self.view_with(repos, e, &install).await;
            }
            e.updated_at = now;
            repos.webhooks().update_endpoint(&e).await?;
            if !changed.is_empty() {
                repos
                    .audit()
                    .append(&audit(
                        Some(actor),
                        "webhook.update",
                        id,
                        json!({"name": e.name, "host": host_of(&e.url), "fields": changed}),
                    ))
                    .await?;
            }
            if let Some(action) = toggled {
                repos
                    .audit()
                    .append(&audit(
                        Some(actor),
                        action,
                        id,
                        json!({"name": e.name, "host": host_of(&e.url)}),
                    ))
                    .await?;
            }
            self.view_with(repos, e, &install).await
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes an endpoint with its deliveries and attempts; their pending jobs find
    /// nothing.
    pub async fn delete(&self, actor: &Actor, id: WebhookId) -> Result<(), CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        let tx = self.store.begin().await?;
        let done = async {
            let e = tx
                .webhooks()
                .get_endpoint_for_update(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            tx.webhooks().delete_endpoint(id).await?;
            tx.audit()
                .append(&audit(
                    Some(actor),
                    "webhook.delete",
                    id,
                    json!({"name": e.name, "host": host_of(&e.url)}),
                ))
                .await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    /// Makes a new secret (returned this once); the old one keeps signing for
    /// [`ROTATION_WINDOW`], whose end is returned. A second rotation within the window
    /// replaces the previous secret.
    pub async fn rotate_secret(
        &self,
        actor: &Actor,
        id: WebhookId,
    ) -> Result<(Secret, Timestamp), CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        let new = secret::generate()?;
        let tx = self.store.begin().await?;
        let done = async {
            let mut e = tx
                .webhooks()
                .get_endpoint_for_update(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            let now = Timestamp::now();
            let until = plus(now, ROTATION_WINDOW)?;
            e.previous_secret_sealed = Some(std::mem::replace(
                &mut e.secret_sealed,
                secret::seal_for(&self.key, id, &new)?,
            ));
            e.previous_secret_expires_at = Some(until);
            e.updated_at = now;
            tx.webhooks().update_endpoint(&e).await?;
            tx.audit()
                .append(&audit(
                    Some(actor),
                    "webhook.rotate_secret",
                    id,
                    json!({"name": e.name, "host": host_of(&e.url)}),
                ))
                .await?;
            Ok(until)
        }
        .await;
        let until = finish(tx, done).await?;
        Ok((new, until))
    }

    /// Queues a `webhook.ping` delivery (one attempt, ahead of the queue). Works on a
    /// disabled endpoint, so it can be checked before enabling; a paused copy is
    /// `Invalid`. 10 per user and minute.
    pub async fn test(&self, actor: &Actor, id: WebhookId) -> Result<DeliveryId, CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        limits::hit(
            self.store.as_ref(),
            &limits::webhook_test_user(actor.user),
            limits::WEBHOOK_TEST_PER_USER,
            limits::WINDOW,
        )
        .await?;
        let tx = self.store.begin().await?;
        let done = async {
            let repos = tx.as_ref();
            let e = repos
                .webhooks()
                .get_endpoint(id)
                .await?
                .ok_or(CoreError::NotFound)?;
            if e.install_id != current_install(repos).await? {
                return Err(CoreError::Invalid(PAUSED.into()));
            }
            let now = Timestamp::now();
            let event = WebhookEventRow {
                id: WebhookEventId::new(),
                kind: PING.to_owned(),
                data: json!({"endpoint": id, "message": "Test delivery from FerriteCMS"}),
                created_at: now,
            };
            repos.webhooks().insert_event(&event).await?;
            let d = new_delivery(&e, event.id, None, now);
            repos.webhooks().insert_delivery(&d).await?;
            repos
                .jobs()
                .enqueue(&webhook_deliver_job(d.id, 1, None, WEBHOOK_TEST_PRIORITY))
                .await?;
            repos
                .audit()
                .append(&audit(
                    Some(actor),
                    "webhook.test",
                    id,
                    json!({"name": e.name, "host": host_of(&e.url)}),
                ))
                .await?;
            Ok(d.id)
        }
        .await;
        finish(tx, done).await
    }

    /// A page of the endpoint's deliveries, newest first.
    pub async fn deliveries(
        &self,
        actor: &Actor,
        id: WebhookId,
        query: &DeliveryQuery,
    ) -> Result<Page<DeliveryListed>, CoreError> {
        require_reader(actor, Capability::ManageWebhooks)?;
        let repos = self.store.read();
        repos
            .webhooks()
            .get_endpoint(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        if query.limit == 0 || query.limit > ferrite_domain::MAX_PAGE {
            return Err(CoreError::invalid_field(
                "limit",
                format!("1 to {}", ferrite_domain::MAX_PAGE),
            ));
        }
        repos
            .webhooks()
            .list_deliveries(id, query)
            .await
            .map_err(|e| match e {
                ferrite_store::StoreError::Invalid(_) => {
                    CoreError::invalid_field("cursor", "invalid cursor".to_owned())
                }
                other => other.into(),
            })
    }

    /// One delivery of the endpoint with its event and attempts.
    pub async fn delivery(
        &self,
        actor: &Actor,
        id: WebhookId,
        delivery: DeliveryId,
    ) -> Result<DeliveryDetail, CoreError> {
        require_reader(actor, Capability::ManageWebhooks)?;
        let w = self.store.read().webhooks();
        let d = w
            .get_delivery(delivery)
            .await?
            .filter(|d| d.endpoint_id == id)
            .ok_or(CoreError::NotFound)?;
        let event = w.get_event(d.event_id).await?.ok_or(CoreError::NotFound)?;
        let attempts = w.attempts(d.id).await?;
        Ok(DeliveryDetail {
            delivery: d,
            event,
            attempts,
        })
    }

    /// A new delivery of the same event (`redelivery_of` names the original). The
    /// endpoint must be enabled and not paused.
    pub async fn redeliver(
        &self,
        actor: &Actor,
        id: WebhookId,
        delivery: DeliveryId,
    ) -> Result<DeliveryId, CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        let tx = self.store.begin().await?;
        let done = async {
            let repos = tx.as_ref();
            let e = sendable(repos, id).await?;
            let original = repos
                .webhooks()
                .get_delivery(delivery)
                .await?
                .filter(|d| d.endpoint_id == id)
                .ok_or(CoreError::NotFound)?;
            let d = queue_redelivery(repos, &e, &original).await?;
            repos
                .audit()
                .append(&audit(
                    Some(actor),
                    "webhook.redeliver",
                    id,
                    json!({"name": e.name, "host": host_of(&e.url), "count": 1}),
                ))
                .await?;
            Ok(d)
        }
        .await;
        finish(tx, done).await
    }

    /// Redelivers the endpoint's deliveries created since `since` whose status is one of
    /// `statuses` (default failed and skipped; `pending` is refused), oldest first, at
    /// most [`MAX_REDELIVER`]; returns how many.
    pub async fn redeliver_since(
        &self,
        actor: &Actor,
        id: WebhookId,
        since: Timestamp,
        statuses: Option<Vec<DeliveryStatus>>,
    ) -> Result<u32, CoreError> {
        require(actor, Capability::ManageWebhooks)?;
        let statuses =
            statuses.unwrap_or_else(|| vec![DeliveryStatus::Failed, DeliveryStatus::Skipped]);
        if statuses.is_empty() || statuses.contains(&DeliveryStatus::Pending) {
            return Err(CoreError::invalid_field(
                "statuses",
                "choose delivered, failed or skipped".to_owned(),
            ));
        }
        let tx = self.store.begin().await?;
        let done = async {
            let repos = tx.as_ref();
            let e = sendable(repos, id).await?;
            let found = repos
                .webhooks()
                .deliveries_since(id, since, &statuses, MAX_REDELIVER)
                .await?;
            for original in &found {
                queue_redelivery(repos, &e, original).await?;
            }
            let count = u32::try_from(found.len()).unwrap_or(MAX_REDELIVER);
            if count > 0 {
                repos
                    .audit()
                    .append(&audit(
                        Some(actor),
                        "webhook.redeliver",
                        id,
                        json!({"name": e.name, "host": host_of(&e.url), "count": count}),
                    ))
                    .await?;
            }
            Ok(count)
        }
        .await;
        finish(tx, done).await
    }

    /// Deletes delivery-log events created before `before` (with their deliveries and
    /// attempts) in batches; returns the count. The prune job calls it.
    pub async fn prune(&self, before: Timestamp, batch: u32) -> Result<u64, CoreError> {
        Ok(self
            .store
            .read()
            .webhooks()
            .prune_events(before, batch)
            .await?)
    }
}

/// The message for a paused endpoint (a copied or restored database).
pub const PAUSED: &str =
    "paused: this database was copied or restored; enable the webhook to send from this site";

async fn sendable(repos: &dyn Repos, id: WebhookId) -> Result<WebhookEndpoint, CoreError> {
    let e = repos
        .webhooks()
        .get_endpoint(id)
        .await?
        .ok_or(CoreError::NotFound)?;
    if !e.enabled {
        return Err(CoreError::Invalid(
            "the webhook is disabled; enable it first".into(),
        ));
    }
    if e.install_id != current_install(repos).await? {
        return Err(CoreError::Invalid(PAUSED.into()));
    }
    Ok(e)
}

pub(super) fn new_delivery(
    e: &WebhookEndpoint,
    event: WebhookEventId,
    redelivery_of: Option<DeliveryId>,
    now: Timestamp,
) -> Delivery {
    Delivery {
        id: DeliveryId::new(),
        endpoint_id: e.id,
        event_id: event,
        status: DeliveryStatus::Pending,
        attempts: 0,
        next_attempt_at: Some(now),
        last_status_code: None,
        last_error: None,
        redelivery_of,
        created_at: now,
        finished_at: None,
    }
}

async fn queue_redelivery(
    repos: &dyn Repos,
    e: &WebhookEndpoint,
    original: &Delivery,
) -> Result<DeliveryId, CoreError> {
    let d = new_delivery(e, original.event_id, Some(original.id), Timestamp::now());
    repos.webhooks().insert_delivery(&d).await?;
    repos
        .jobs()
        .enqueue(&webhook_deliver_job(
            d.id,
            1,
            None,
            WEBHOOK_DELIVER_PRIORITY,
        ))
        .await?;
    Ok(d.id)
}
