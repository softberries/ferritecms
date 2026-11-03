//! [`ApplyTx`]: the transaction wrapper a release is applied through (design 15 §4.4,
//! §5.2).
//!
//! - [`Mode::Collect`] (apply): every repository is the real transaction's, except that
//!   `events().append` of an `invalidate` event adds its keys to a shared collector
//!   instead of writing a row; [`ApplyTx::flush_invalidation`] then appends **one**
//!   event with all of them (D7).
//! - [`Mode::Sandbox`] (check, whole-site preview): the side channels are no-ops (D13):
//!   no outbox rows (so no Postgres sequence gaps), no jobs, no audit rows, no
//!   idempotency rows, no revisions, no search writes and no webhook rows (the emitter
//!   sees no endpoint). Reads go to the real transaction, which always rolls back.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use ferrite_domain::{
    Attempt, AuditEntry, AuditId, AuditQuery, ContentId, Delivery, DeliveryCounts, DeliveryId,
    DeliveryListed, DeliveryQuery, DeliveryStatus, DeliveryUpdate, Event, IdemBegin,
    IdempotencyKey, Job, JobId, JobLease, JobQuery, JobStatus, NewAudit, NewEvent, NewJob, NodeId,
    Page, Revision, RevisionId, SearchQuery, Timestamp, UserId, WebhookEndpoint, WebhookEventId,
    WebhookEventRow, WebhookId,
};
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, AutosaveRepo, CommentRepo, ContentRepo, ContentTypeRepo,
    EventRepo, IdempotencyRepo, ImportRepo, JobRepo, LocaleRepo, MediaRepo, MenuRepo, MetaRepo,
    NodeMetricsRepo, NodeRepo, OAuthRepo, OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo,
    PluginTrustRepo, PreviewLinkRepo, RateLimitRepo, RedirectRepo, ReleaseRepo, Repos, ReviewRepo,
    RevisionRepo, SchemaMetaRepo, SearchRepo, SessionRepo, StoreError, TaxonomyRepo, TermRepo,
    ThemeRepo, Tx, UserRepo, WebhookRepo, WorkflowRepo,
};

use crate::events::{self, ALL_KEY, INVALIDATE};

/// More merged keys than this become [`ALL_KEY`] (D7).
pub const MAX_RELEASE_KEYS: usize = 2000;

/// What [`ApplyTx`] does with the side channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Apply: collect `invalidate` keys into one event.
    Collect,
    /// Check and preview: side channels are no-ops; the transaction is rolled back.
    Sandbox,
}

/// A [`Tx`] wrapper; see the module docs.
pub struct ApplyTx {
    inner: Box<dyn Tx>,
    mode: Mode,
    keys: Arc<Mutex<BTreeSet<String>>>,
}

const READ_ONLY: StoreError = StoreError::Unsupported("preview is read-only");

impl ApplyTx {
    /// Wraps `inner` in `mode`.
    pub fn new(inner: Box<dyn Tx>, mode: Mode) -> Self {
        Self {
            inner,
            mode,
            keys: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn sandbox(&self) -> bool {
        self.mode == Mode::Sandbox
    }

    /// The keys collected so far, sorted, and clears them.
    pub fn take_keys(&self) -> Vec<String> {
        match self.keys.lock() {
            Ok(mut k) => std::mem::take(&mut *k).into_iter().collect(),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner())
                .into_iter()
                .collect(),
        }
    }

    /// Appends one `invalidate` event with every collected key (or [`ALL_KEY`] above
    /// [`MAX_RELEASE_KEYS`]) to the real outbox; nothing when no key was collected or in
    /// the sandbox.
    pub async fn flush_invalidation(&self) -> Result<(), StoreError> {
        let keys = self.take_keys();
        if keys.is_empty() || self.sandbox() {
            return Ok(());
        }
        let event = if keys.len() > MAX_RELEASE_KEYS {
            events::invalidate([ALL_KEY.to_owned()])
        } else {
            events::invalidate(keys)
        };
        self.inner.events().append(&event).await?;
        Ok(())
    }

    fn collect(&self, keys: &[String]) {
        match self.keys.lock() {
            Ok(mut k) => k.extend(keys.iter().cloned()),
            Err(poisoned) => poisoned.into_inner().extend(keys.iter().cloned()),
        }
    }
}

#[async_trait]
impl Tx for ApplyTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        if self.mode == Mode::Sandbox {
            // A sandbox never commits.
            return self.inner.rollback().await;
        }
        self.inner.commit().await
    }

    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.inner.rollback().await
    }

    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError> {
        Ok(Box::new(ApplyTx {
            inner: self.inner.begin_nested().await?,
            mode: self.mode,
            keys: self.keys.clone(),
        }))
    }
}

macro_rules! delegate {
    ($($method:ident: $repo:ident),* $(,)?) => {
        $(fn $method(&self) -> &dyn $repo {
            self.inner.$method()
        })*
    };
}

impl Repos for ApplyTx {
    delegate!(
        content: ContentRepo,
        meta: MetaRepo,
        types: ContentTypeRepo,
        taxonomies: TaxonomyRepo,
        terms: TermRepo,
        menus: MenuRepo,
        options: OptionRepo,
        redirects: RedirectRepo,
        users: UserRepo,
        artifacts: ArtifactRepo,
        media: MediaRepo,
        plugins: PluginRepo,
        plugin_failures: PluginFailureRepo,
        plugin_kv: PluginKvRepo,
        themes: ThemeRepo,
        nodes: NodeRepo,
        schema_meta: SchemaMetaRepo,
        sessions: SessionRepo,
        api_tokens: ApiTokenRepo,
        rate_limits: RateLimitRepo,
        comments: CommentRepo,
        autosaves: AutosaveRepo,
        imports: ImportRepo,
        node_metrics: NodeMetricsRepo,
        oauth: OAuthRepo,
        locales: LocaleRepo,
        plugin_trust: PluginTrustRepo,
        releases: ReleaseRepo,
        workflows: WorkflowRepo,
        reviews: ReviewRepo,
        preview_links: PreviewLinkRepo,
    );

    fn revisions(&self) -> &dyn RevisionRepo {
        self
    }
    fn events(&self) -> &dyn EventRepo {
        self
    }
    fn jobs(&self) -> &dyn JobRepo {
        self
    }
    fn search(&self) -> &dyn SearchRepo {
        self
    }
    fn audit(&self) -> &dyn AuditRepo {
        self
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self
    }
    fn webhooks(&self) -> &dyn WebhookRepo {
        self
    }
}

#[async_trait]
impl EventRepo for ApplyTx {
    async fn append(&self, event: &NewEvent) -> Result<i64, StoreError> {
        if event.kind == INVALIDATE {
            self.collect(&event.keys);
            return Ok(0);
        }
        if self.sandbox() {
            return Ok(0);
        }
        self.inner.events().append(event).await
    }
    async fn since(&self, after_seq: i64, limit: u32) -> Result<Vec<Event>, StoreError> {
        self.inner.events().since(after_seq, limit).await
    }
    async fn max_seq(&self) -> Result<i64, StoreError> {
        self.inner.events().max_seq().await
    }
    async fn min_seq(&self) -> Result<i64, StoreError> {
        self.inner.events().min_seq().await
    }
    async fn prune(&self, older_than: Timestamp, limit: u32) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Ok(0);
        }
        self.inner.events().prune(older_than, limit).await
    }
}

#[async_trait]
impl JobRepo for ApplyTx {
    async fn enqueue(&self, job: &NewJob) -> Result<Option<JobId>, StoreError> {
        if self.sandbox() {
            return Ok(None);
        }
        self.inner.jobs().enqueue(job).await
    }
    async fn claim(
        &self,
        node: NodeId,
        kinds: &[String],
        limit: u32,
        lease: Duration,
    ) -> Result<Vec<Job>, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.jobs().claim(node, kinds, limit, lease).await
    }
    async fn extend(&self, lease: &JobLease, duration: Duration) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.jobs().extend(lease, duration).await
    }
    async fn complete(&self, lease: &JobLease) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.jobs().complete(lease).await
    }
    async fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        permanent: bool,
    ) -> Result<Option<JobStatus>, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.jobs().fail(lease, error, permanent).await
    }
    async fn release(
        &self,
        lease: &JobLease,
        delay: Duration,
        count_attempt: bool,
    ) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.jobs().release(lease, delay, count_attempt).await
    }
    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.inner.jobs().get(id).await
    }
    async fn list(&self, query: &JobQuery) -> Result<Page<Job>, StoreError> {
        self.inner.jobs().list(query).await
    }
    async fn retry_dead(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.jobs().retry_dead(id).await
    }
    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError> {
        self.inner.jobs().counts().await
    }
    async fn prune(
        &self,
        done_before: Timestamp,
        dead_before: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Ok(0);
        }
        self.inner
            .jobs()
            .prune(done_before, dead_before, limit)
            .await
    }
}

#[async_trait]
impl AuditRepo for ApplyTx {
    async fn append(&self, entry: &NewAudit) -> Result<AuditId, StoreError> {
        if self.sandbox() {
            return Ok(AuditId::new());
        }
        self.inner.audit().append(entry).await
    }
    async fn list(&self, query: &AuditQuery) -> Result<Page<AuditEntry>, StoreError> {
        self.inner.audit().list(query).await
    }
}

#[async_trait]
impl IdempotencyRepo for ApplyTx {
    async fn begin(
        &self,
        key: &IdempotencyKey,
        stale_after: Duration,
    ) -> Result<IdemBegin, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.idempotency().begin(key, stale_after).await
    }
    async fn finish(
        &self,
        user: UserId,
        key_hash: &str,
        token: Timestamp,
        status: u16,
        response: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Ok(false);
        }
        self.inner
            .idempotency()
            .finish(user, key_hash, token, status, response)
            .await
    }
    async fn release(
        &self,
        user: UserId,
        key_hash: &str,
        token: Timestamp,
    ) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Ok(false);
        }
        self.inner
            .idempotency()
            .release(user, key_hash, token)
            .await
    }
    async fn prune(&self, created_before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Ok(0);
        }
        self.inner.idempotency().prune(created_before, limit).await
    }
}

#[async_trait]
impl RevisionRepo for ApplyTx {
    async fn insert(&self, revision: &Revision) -> Result<(), StoreError> {
        if self.sandbox() {
            return Ok(());
        }
        self.inner.revisions().insert(revision).await
    }
    async fn list(&self, id: ContentId, limit: u32) -> Result<Vec<Revision>, StoreError> {
        self.inner.revisions().list(id, limit).await
    }
    async fn get(&self, id: RevisionId) -> Result<Option<Revision>, StoreError> {
        self.inner.revisions().get(id).await
    }
    async fn prune(&self, id: ContentId, keep: u32) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Ok(0);
        }
        self.inner.revisions().prune(id, keep).await
    }
    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.revisions().reassign_author(from, to).await
    }
}

#[async_trait]
impl SearchRepo for ApplyTx {
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError> {
        if self.sandbox() {
            return Ok(());
        }
        self.inner.search().index(id, title, body_text).await
    }
    async fn remove(&self, id: ContentId) -> Result<(), StoreError> {
        if self.sandbox() {
            return Ok(());
        }
        self.inner.search().remove(id).await
    }
    async fn search(&self, query: &SearchQuery) -> Result<Page<ContentId>, StoreError> {
        self.inner.search().search(query).await
    }
}

#[async_trait]
impl WebhookRepo for ApplyTx {
    async fn insert_endpoint(&self, endpoint: &WebhookEndpoint) -> Result<(), StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.webhooks().insert_endpoint(endpoint).await
    }
    async fn get_endpoint(&self, id: WebhookId) -> Result<Option<WebhookEndpoint>, StoreError> {
        self.inner.webhooks().get_endpoint(id).await
    }
    async fn get_endpoint_for_update(
        &self,
        id: WebhookId,
    ) -> Result<Option<WebhookEndpoint>, StoreError> {
        self.inner.webhooks().get_endpoint_for_update(id).await
    }
    async fn list_endpoints(&self) -> Result<Vec<WebhookEndpoint>, StoreError> {
        self.inner.webhooks().list_endpoints().await
    }
    async fn count_endpoints(&self) -> Result<u64, StoreError> {
        self.inner.webhooks().count_endpoints().await
    }
    async fn update_endpoint(&self, endpoint: &WebhookEndpoint) -> Result<(), StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.webhooks().update_endpoint(endpoint).await
    }
    async fn delete_endpoint(&self, id: WebhookId) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.webhooks().delete_endpoint(id).await
    }
    async fn active_for_emit(&self) -> Result<Vec<WebhookEndpoint>, StoreError> {
        if self.sandbox() {
            // No endpoint: the emitter writes nothing.
            return Ok(Vec::new());
        }
        self.inner.webhooks().active_for_emit().await
    }
    async fn insert_event(&self, event: &WebhookEventRow) -> Result<(), StoreError> {
        if self.sandbox() {
            return Ok(());
        }
        self.inner.webhooks().insert_event(event).await
    }
    async fn get_event(&self, id: WebhookEventId) -> Result<Option<WebhookEventRow>, StoreError> {
        self.inner.webhooks().get_event(id).await
    }
    async fn insert_delivery(&self, delivery: &Delivery) -> Result<(), StoreError> {
        if self.sandbox() {
            return Ok(());
        }
        self.inner.webhooks().insert_delivery(delivery).await
    }
    async fn get_delivery(&self, id: DeliveryId) -> Result<Option<Delivery>, StoreError> {
        self.inner.webhooks().get_delivery(id).await
    }
    async fn list_deliveries(
        &self,
        endpoint: WebhookId,
        query: &DeliveryQuery,
    ) -> Result<Page<DeliveryListed>, StoreError> {
        self.inner.webhooks().list_deliveries(endpoint, query).await
    }
    async fn record_attempt(
        &self,
        attempt: &Attempt,
        update: &DeliveryUpdate,
    ) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner.webhooks().record_attempt(attempt, update).await
    }
    async fn set_delivery_status(
        &self,
        id: DeliveryId,
        status: DeliveryStatus,
        error: Option<&str>,
        finished_at: Timestamp,
    ) -> Result<bool, StoreError> {
        if self.sandbox() {
            return Err(READ_ONLY);
        }
        self.inner
            .webhooks()
            .set_delivery_status(id, status, error, finished_at)
            .await
    }
    async fn deliveries_since(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
        statuses: &[DeliveryStatus],
        limit: u32,
    ) -> Result<Vec<Delivery>, StoreError> {
        self.inner
            .webhooks()
            .deliveries_since(endpoint, since, statuses, limit)
            .await
    }
    async fn delivery_counts(
        &self,
        endpoint: WebhookId,
        since: Timestamp,
    ) -> Result<DeliveryCounts, StoreError> {
        self.inner.webhooks().delivery_counts(endpoint, since).await
    }
    async fn attempts(&self, delivery: DeliveryId) -> Result<Vec<Attempt>, StoreError> {
        self.inner.webhooks().attempts(delivery).await
    }
    async fn fail_stale_pending(
        &self,
        before: Timestamp,
        error: &str,
        now: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Ok(0);
        }
        self.inner
            .webhooks()
            .fail_stale_pending(before, error, now, limit)
            .await
    }
    async fn prune_events(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError> {
        if self.sandbox() {
            return Ok(0);
        }
        self.inner.webhooks().prune_events(before, limit).await
    }
}
