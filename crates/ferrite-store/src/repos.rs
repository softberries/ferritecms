use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use async_trait::async_trait;
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, ContentItem, ContentQuery, ContentType, Event,
    InstalledPlugin, InstalledTheme, Job, JobId, JobLease, JobStatus, Media, Menu, NewContent,
    NewEvent, NewJob, NewNode, NewUser, Node, NodeId, OptionEntry, Page, Redirect, RedirectId,
    Revision, RevisionId, SearchQuery, Taxonomy, Term, TermId, Timestamp, User, UserId,
};

use crate::{Channel, LockGuard, StoreError, Subscription, migrate::MigrationStatus};

/// Which database engine backs a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

/// A database connection pool plus migration control.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// The engine behind this store.
    fn backend(&self) -> Backend;
    /// Repositories bound to the read pool (autocommit, no transaction). Writes through
    /// them run on their own (on SQLite: on the single writer connection).
    ///
    /// **Never write through `read()` or call [`Store::begin`] while the same task holds
    /// a `Tx`.** On SQLite both wait for the writer connection the `Tx` holds and fail
    /// with [`StoreError::Busy`] after 5 s. Pass the `Tx` on as `&dyn Repos` instead.
    fn read(&self) -> &dyn Repos;
    /// Starts a write transaction; dropping it without commit rolls back. Fails with
    /// [`StoreError::Busy`] when no connection frees up in time (SQLite: 5 s for the
    /// single writer; see [`Store::read`] for the self-deadlock to avoid).
    ///
    /// Isolation differs by backend (decision pending, M1c): SQLite serializes all
    /// writers; Postgres runs `READ COMMITTED`.
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError>;
    /// Subscribes to change notifications on `channel` (04 §4, P1). Postgres: a
    /// dedicated `LISTEN` connection per subscription, reconnecting by itself. SQLite:
    /// `Ok(None)`; one process owns the database there, so callers poll instead.
    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError>;
    /// Takes the named lock if it is free (04 §7, P1, P13); `Ok(None)` when it is held,
    /// by this store or (Postgres) by any other process on the same database and schema.
    /// Names are 1 to 64 characters from `[a-z0-9._-]` (`Invalid` otherwise; see
    /// [`LOCK_MIGRATE`](crate::LOCK_MIGRATE) and friends). Postgres: a session-level
    /// advisory lock keyed by `ferrite:<schema>:<name>` on a dedicated connection
    /// (`application_name` `<app>:lock:<name>`), released when the guard is released or
    /// dropped, or when the connection dies. SQLite: an in-process mutex per store (one
    /// process owns the data folder). To wait for a lock use [`lock`](crate::lock()).
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError>;
    /// Compares applied migrations with those embedded in this binary.
    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError>;
    /// Applies pending migrations up to `to` (all if `None`); returns applied versions.
    async fn migrate_up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError>;
    /// Reverts migrations down to version `to`; lossy downs need `allow_lossy`.
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError>;
    /// Checks that applied migration checksums match the embedded ones.
    async fn verify_migrations(&self) -> Result<(), StoreError>;
    /// Writes a consistent copy of the database to `dest`; Postgres returns `Unsupported`.
    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError>;
    /// Closes the connection pools: waits until every checked-out connection is back
    /// (so a dropped `Tx` finishes its rollback), then closes them. Every later call on
    /// this store fails. For a clean shutdown (SQLite: the WAL is checkpointed when the
    /// last connection closes). Idempotent.
    async fn close(&self);
}

/// A write transaction exposing all repositories.
///
/// **Errors poison the transaction (R18), on every backend.** If a statement fails in the
/// database (including `UniqueViolation` and `ForeignKey`), every later call through this
/// `Tx` returns [`StoreError::Aborted`], and `commit` rolls back and returns
/// `Err(Aborted)`. Errors raised before any SQL is sent (`Invalid` from input
/// validation) do not poison. To try something and fall back, run it in a nested
/// transaction from [`Tx::begin_nested`]: its failure poisons only the nested level.
#[async_trait]
pub trait Tx: Repos + Send {
    /// Commits the transaction (a nested one: releases its savepoint into the parent).
    /// A poisoned transaction is rolled back and `Err(Aborted)` returned.
    async fn commit(self: Box<Self>) -> Result<(), StoreError>;
    /// Rolls the transaction back (a nested one: back to its savepoint).
    async fn rollback(self: Box<Self>) -> Result<(), StoreError>;
    /// Opens a nested transaction (a savepoint) inside this one. Its writes become part
    /// of this transaction on `commit` and are undone on `rollback` or drop; an error
    /// inside it poisons only the nested transaction. While it is open, calls through
    /// this (outer) `Tx` return [`StoreError::NestedOpen`]. Nesting can go deeper.
    ///
    /// Ending the outer `Tx` (drop, `commit` or `rollback`) while a nested one is still
    /// alive ends the whole transaction: `commit` rolls back and returns `NestedOpen`, and
    /// the nested handle then fails every call and its `commit` with
    /// [`StoreError::Aborted`]. The database connection is released no later than when
    /// the last handle drops.
    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError>;
}

/// Accessors for every repository.
///
/// Missing-row semantics, the same in every repository and on every backend:
/// - lookups (`get`, `get_by_*`) return `Ok(None)`;
/// - `update`, `RedirectRepo::hit`, `PluginRepo::set_enabled` and
///   `ThemeRepo::activate` return [`StoreError::NotFound`] (`ContentRepo::update` returns
///   `Conflict` instead when the row exists with another version);
/// - `delete` is idempotent: deleting a missing row is `Ok(())`. A delete blocked by
///   dependent rows (`RESTRICT`) is [`StoreError::ForeignKey`].
pub trait Repos: Send + Sync {
    fn content(&self) -> &dyn ContentRepo;
    fn meta(&self) -> &dyn MetaRepo;
    fn revisions(&self) -> &dyn RevisionRepo;
    fn types(&self) -> &dyn ContentTypeRepo;
    fn taxonomies(&self) -> &dyn TaxonomyRepo;
    fn terms(&self) -> &dyn TermRepo;
    fn menus(&self) -> &dyn MenuRepo;
    fn options(&self) -> &dyn OptionRepo;
    fn redirects(&self) -> &dyn RedirectRepo;
    fn users(&self) -> &dyn UserRepo;
    fn artifacts(&self) -> &dyn ArtifactRepo;
    fn media(&self) -> &dyn MediaRepo;
    fn plugins(&self) -> &dyn PluginRepo;
    fn plugin_kv(&self) -> &dyn PluginKvRepo;
    fn themes(&self) -> &dyn ThemeRepo;
    fn events(&self) -> &dyn EventRepo;
    fn jobs(&self) -> &dyn JobRepo;
    fn nodes(&self) -> &dyn NodeRepo;
    fn search(&self) -> &dyn SearchRepo;
    fn schema_meta(&self) -> &dyn SchemaMetaRepo;
}

#[async_trait]
pub trait ContentTypeRepo: Send + Sync {
    /// Inserts or replaces a content type by name.
    async fn upsert(&self, ty: &ContentType) -> Result<(), StoreError>;
    /// Fetches a content type by name.
    async fn get(&self, name: &str) -> Result<Option<ContentType>, StoreError>;
    /// Lists all content types.
    async fn list(&self) -> Result<Vec<ContentType>, StoreError>;
    /// Deletes a content type; built-in types yield `Invalid`; a missing one is a no-op.
    async fn delete(&self, name: &str) -> Result<(), StoreError>;
}

#[async_trait]
pub trait ContentRepo: Send + Sync {
    /// Inserts a new item (version 1) and returns the stored row.
    async fn insert(&self, item: &NewContent) -> Result<ContentItem, StoreError>;
    /// Fetches an item by id.
    async fn get(&self, id: ContentId) -> Result<Option<ContentItem>, StoreError>;
    /// Fetches an item by kind, parent and slug.
    async fn get_by_slug(
        &self,
        kind: &str,
        parent: Option<ContentId>,
        slug: &str,
    ) -> Result<Option<ContentItem>, StoreError>;
    /// Updates WHERE id AND version = item.version, bumping version and updated_at; no row gives `Conflict{current_version}` or `NotFound`.
    async fn update(&self, item: &ContentItem) -> Result<ContentItem, StoreError>;
    /// Lists items matching the query with cursor pagination.
    ///
    /// **`query.ids` empty means no filter, not "match nothing"** (likewise `exclude`):
    /// callers that computed an empty id set must short-circuit instead of calling this,
    /// or they get every item.
    async fn list(&self, query: &ContentQuery) -> Result<Page<ContentItem>, StoreError>;
    /// Counts the items matching the query's filters (`limit`, `cursor` and `order` are
    /// ignored). Uses the same WHERE clause as `list`.
    async fn count(&self, query: &ContentQuery) -> Result<u64, StoreError>;
    /// Fetches up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) items by id in one query,
    /// returned in the order of `ids`; missing ids are skipped and a repeated id yields
    /// its item once (at its first position). More ids is `Invalid`.
    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<ContentItem>, StoreError>;
    /// Slugs of `kind` under `parent` that equal `prefix` or start with `prefix-`
    /// (case-sensitive, binary order, at most [`MAX_SLUG_PREFIX_RESULTS`](crate::MAX_SLUG_PREFIX_RESULTS)).
    /// Matches like `hello-world` are included too, so callers that allocate numeric
    /// suffixes (`hello-2`) must filter the result themselves.
    async fn slugs_with_prefix(
        &self,
        kind: &str,
        parent: Option<ContentId>,
        prefix: &str,
    ) -> Result<Vec<String>, StoreError>;
    /// Deletes an item by id (missing: no-op). An item with children is `ForeignKey`.
    async fn delete(&self, id: ContentId) -> Result<(), StoreError>;
    /// Returns up to `limit` scheduled items whose publish time is at or before `now`.
    async fn due_scheduled(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<ContentItem>, StoreError>;
}

#[async_trait]
pub trait MetaRepo: Send + Sync {
    /// Reads one meta value.
    async fn get(&self, id: ContentId, key: &str) -> Result<Option<serde_json::Value>, StoreError>;
    /// Sets (inserts or replaces) one meta value.
    async fn set(
        &self,
        id: ContentId,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), StoreError>;
    /// Deletes one meta key (missing: no-op).
    async fn delete(&self, id: ContentId, key: &str) -> Result<(), StoreError>;
    /// Lists all meta entries of an item.
    async fn list(&self, id: ContentId) -> Result<Vec<(String, serde_json::Value)>, StoreError>;
    /// Reads the given keys of up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) items in one
    /// query (more ids, or more than [`MAX_META_KEYS`](crate::MAX_META_KEYS) keys, is
    /// `Invalid`). Missing entries are skipped; the result is ordered by the position of
    /// the item in `ids` (a repeated id counts at its first position), then by key in
    /// binary order.
    async fn get_many(
        &self,
        ids: &[ContentId],
        keys: &[&str],
    ) -> Result<Vec<(ContentId, String, serde_json::Value)>, StoreError>;
}

#[async_trait]
pub trait RevisionRepo: Send + Sync {
    /// Stores a revision.
    async fn insert(&self, revision: &Revision) -> Result<(), StoreError>;
    /// Lists up to `limit` revisions of an item, newest first.
    async fn list(&self, id: ContentId, limit: u32) -> Result<Vec<Revision>, StoreError>;
    /// Fetches a revision by id.
    async fn get(&self, id: RevisionId) -> Result<Option<Revision>, StoreError>;
    /// Deletes all but the newest `keep` revisions; returns the deleted count.
    async fn prune(&self, id: ContentId, keep: u32) -> Result<u64, StoreError>;
}

#[async_trait]
pub trait TaxonomyRepo: Send + Sync {
    /// Inserts or replaces a taxonomy by name.
    async fn upsert(&self, taxonomy: &Taxonomy) -> Result<(), StoreError>;
    /// Fetches a taxonomy by name.
    async fn get(&self, name: &str) -> Result<Option<Taxonomy>, StoreError>;
    /// Lists all taxonomies.
    async fn list(&self) -> Result<Vec<Taxonomy>, StoreError>;
}

#[async_trait]
pub trait TermRepo: Send + Sync {
    /// Inserts a term.
    async fn insert(&self, term: &Term) -> Result<(), StoreError>;
    /// Updates a term; a missing term is `NotFound`.
    async fn update(&self, term: &Term) -> Result<(), StoreError>;
    /// Fetches a term by id.
    async fn get(&self, id: TermId) -> Result<Option<Term>, StoreError>;
    /// Fetches a term by taxonomy and slug.
    async fn get_by_slug(&self, tax: &str, slug: &str) -> Result<Option<Term>, StoreError>;
    /// Lists all terms of a taxonomy.
    async fn list(&self, tax: &str) -> Result<Vec<Term>, StoreError>;
    /// Deletes a term (missing: no-op). A term with child terms is `ForeignKey`.
    async fn delete(&self, id: TermId) -> Result<(), StoreError>;
    /// Replaces an item's terms, storing the given order as `position`.
    async fn set_for_item(&self, item: ContentId, terms: &[TermId]) -> Result<(), StoreError>;
    /// Lists an item's terms ordered by position.
    async fn for_item(&self, item: ContentId) -> Result<Vec<Term>, StoreError>;
    /// Lists the terms of up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) items in one
    /// query (more is `Invalid`), each item's terms ordered by position. **Items without
    /// terms are absent from the map**, as are unknown items; use
    /// `map.get(&id).map_or(&[][..], Vec::as_slice)` to read them as empty.
    async fn for_items(
        &self,
        items: &[ContentId],
    ) -> Result<HashMap<ContentId, Vec<Term>>, StoreError>;
}

#[async_trait]
pub trait MenuRepo: Send + Sync {
    /// Fetches a menu with its item tree by name.
    async fn get(&self, name: &str) -> Result<Option<Menu>, StoreError>;
    /// Replaces the whole menu tree in one statement batch.
    async fn put(&self, menu: &Menu) -> Result<(), StoreError>;
    /// Lists menus as (name, label) pairs.
    async fn list(&self) -> Result<Vec<(String, String)>, StoreError>;
    /// Deletes a menu and its items (missing: no-op).
    async fn delete(&self, name: &str) -> Result<(), StoreError>;
}

#[async_trait]
pub trait OptionRepo: Send + Sync {
    /// Fetches an option.
    async fn get(&self, name: &str) -> Result<Option<OptionEntry>, StoreError>;
    /// Sets an option and its autoload flag.
    async fn set(
        &self,
        name: &str,
        value: &serde_json::Value,
        autoload: bool,
    ) -> Result<(), StoreError>;
    /// Deletes an option (missing: no-op).
    async fn delete(&self, name: &str) -> Result<(), StoreError>;
    /// Lists all options flagged autoload.
    async fn autoloaded(&self) -> Result<Vec<OptionEntry>, StoreError>;
}

#[async_trait]
pub trait RedirectRepo: Send + Sync {
    /// Fetches a redirect by id.
    async fn get(&self, id: RedirectId) -> Result<Option<Redirect>, StoreError>;
    /// Fetches a redirect by source path.
    async fn get_by_source(&self, source: &str) -> Result<Option<Redirect>, StoreError>;
    /// Inserts a redirect, or updates the one with the same `source_path` (target, code
    /// and origin change; id, hits, created_at and last_hit_at stay). Returns the stored
    /// row.
    async fn upsert(&self, redirect: &Redirect) -> Result<Redirect, StoreError>;
    /// Deletes a redirect (missing: no-op).
    async fn delete(&self, id: RedirectId) -> Result<(), StoreError>;
    /// Lists redirects with cursor pagination.
    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<Redirect>, StoreError>;
    /// Redirects whose `target` equals `target` exactly (case-sensitive), ordered by
    /// `source_path` in binary order, at most
    /// [`MAX_REDIRECTS_BY_TARGET`](crate::MAX_REDIRECTS_BY_TARGET).
    async fn list_by_target(&self, target: &str) -> Result<Vec<Redirect>, StoreError>;
    /// Records a hit (increments the counter, sets `last_hit_at`); missing: `NotFound`.
    async fn hit(&self, id: RedirectId) -> Result<(), StoreError>;
}

#[async_trait]
pub trait UserRepo: Send + Sync {
    /// Inserts a user and returns the stored row.
    async fn insert(&self, user: &NewUser) -> Result<User, StoreError>;
    /// Fetches a user by id.
    async fn get(&self, id: UserId) -> Result<Option<User>, StoreError>;
    /// Fetches a user by login.
    async fn get_by_login(&self, login: &str) -> Result<Option<User>, StoreError>;
    /// Fetches a user by e-mail, case-insensitively.
    async fn get_by_email(&self, email: &str) -> Result<Option<User>, StoreError>;
    /// Updates a user; a missing user is `NotFound`.
    async fn update(&self, user: &User) -> Result<(), StoreError>;
    /// Lists users with cursor pagination.
    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<User>, StoreError>;
    /// Fetches up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) users in one query, returned
    /// in the order of `ids`; missing ids are skipped and a repeated id yields its user
    /// once (at its first position). More ids is `Invalid`.
    async fn get_many(&self, ids: &[UserId]) -> Result<Vec<User>, StoreError>;
}

#[async_trait]
pub trait ArtifactRepo: Send + Sync {
    /// Inserts an artifact unless its hash exists; returns true if inserted.
    async fn insert_if_absent(&self, artifact: &Artifact) -> Result<bool, StoreError>;
    /// Fetches an artifact by hash.
    async fn get(&self, hash: &ArtifactHash) -> Result<Option<Artifact>, StoreError>;
    /// Deletes an artifact by hash (missing: no-op).
    async fn delete(&self, hash: &ArtifactHash) -> Result<(), StoreError>;
}

#[async_trait]
pub trait MediaRepo: Send + Sync {
    /// Inserts media metadata.
    async fn insert(&self, media: &Media) -> Result<(), StoreError>;
    /// Fetches media metadata by content id.
    async fn get(&self, id: ContentId) -> Result<Option<Media>, StoreError>;
    /// Updates media metadata; missing metadata is `NotFound`.
    async fn update(&self, media: &Media) -> Result<(), StoreError>;
    /// Fetches up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) media rows by content id in
    /// one query, returned in the order of `ids`; missing ids are skipped and a repeated
    /// id yields its row once (at its first position). More ids is `Invalid`.
    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<Media>, StoreError>;
    /// Up to `limit` (1..=[`MAX_PAGE`](crate::MAX_PAGE)) media rows whose original
    /// (`artifact_hash`) or any variant (`variants.<name>.hash`, a string) is `hash`,
    /// ordered by item id. Variant values that are not objects with a string `hash`
    /// are ignored. Used to decide whether `/_media/<hash>/…` may be served.
    ///
    /// Indexed throughout (ruling R29): empty without reading `media` unless the
    /// `artifacts` row of `hash` (primary key) exists with kind `media` or `variant`;
    /// then the original by `media.artifact_hash` (indexed) and variants through the
    /// derived `media_variants` table (indexed by hash), which `insert` and `update`
    /// keep in sync with `variants` in the same transaction.
    async fn find_by_artifact(
        &self,
        hash: &ArtifactHash,
        limit: u32,
    ) -> Result<Vec<Media>, StoreError>;
}

#[async_trait]
pub trait PluginRepo: Send + Sync {
    /// Inserts or replaces an installed plugin by id.
    async fn upsert(&self, plugin: &InstalledPlugin) -> Result<(), StoreError>;
    /// Fetches an installed plugin.
    async fn get(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError>;
    /// Lists installed plugins.
    async fn list(&self) -> Result<Vec<InstalledPlugin>, StoreError>;
    /// Enables or disables a plugin, recording an optional reason; missing: `NotFound`.
    async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        reason: Option<&str>,
    ) -> Result<(), StoreError>;
    /// Removes an installed plugin (missing: no-op).
    async fn delete(&self, id: &str) -> Result<(), StoreError>;
}

#[async_trait]
pub trait PluginKvRepo: Send + Sync {
    /// Reads a plugin's value.
    async fn get(&self, plugin: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    /// Writes a plugin's value. **Bypasses quotas**: mixing it with
    /// [`set_with_quota`](Self::set_with_quota) for the same plugin voids the quota
    /// guarantee, since `set` neither takes the lock nor checks the totals.
    async fn set(&self, plugin: &str, key: &str, value: &[u8]) -> Result<(), StoreError>;
    /// Writes a value only if the plugin's totals stay within quota, atomically (P2):
    /// returns `true` iff after the write the plugin's value bytes are at most
    /// `max_bytes` and its key count at most `max_keys`; otherwise nothing is written and
    /// the result is `false`. Overwriting an existing key counts the new size, not old
    /// plus new. Concurrent callers (any number of tasks or transactions) never push a
    /// plugin past its quota.
    ///
    /// Inside a `Tx` on Postgres the per-plugin lock it takes is held until that `Tx`
    /// ends, so keep such transactions short. A `Tx` that calls this for several plugins
    /// in different orders can deadlock against another one on Postgres; the database
    /// detects it, fails one statement and that `Tx` is poisoned (`Aborted`). Touch
    /// plugins in a consistent order (for example sorted by id).
    async fn set_with_quota(
        &self,
        plugin: &str,
        key: &str,
        value: &[u8],
        max_bytes: u64,
        max_keys: u64,
    ) -> Result<bool, StoreError>;
    /// Deletes a plugin's key (missing: no-op).
    async fn delete(&self, plugin: &str, key: &str) -> Result<(), StoreError>;
    /// Lists up to `limit` keys of a plugin starting with `prefix`.
    async fn list_keys(
        &self,
        plugin: &str,
        prefix: &str,
        limit: u32,
    ) -> Result<Vec<String>, StoreError>;
    /// Returns (bytes, keys) stored by a plugin.
    async fn usage(&self, plugin: &str) -> Result<(u64, u64), StoreError>;
}

#[async_trait]
pub trait ThemeRepo: Send + Sync {
    /// Inserts or replaces an installed theme by id.
    async fn upsert(&self, theme: &InstalledTheme) -> Result<(), StoreError>;
    /// Fetches an installed theme.
    async fn get(&self, id: &str) -> Result<Option<InstalledTheme>, StoreError>;
    /// Lists installed themes.
    async fn list(&self) -> Result<Vec<InstalledTheme>, StoreError>;
    /// Returns the active theme, if any.
    async fn active(&self) -> Result<Option<InstalledTheme>, StoreError>;
    /// Makes the given theme the only active one; a missing theme is `NotFound` (and the
    /// active theme stays active).
    async fn activate(&self, id: &str) -> Result<(), StoreError>;
}

#[async_trait]
pub trait EventRepo: Send + Sync {
    /// Appends an event and returns its sequence number.
    async fn append(&self, event: &NewEvent) -> Result<i64, StoreError>;
    /// Lists up to `limit` events with seq greater than `after_seq`, ascending.
    async fn since(&self, after_seq: i64, limit: u32) -> Result<Vec<Event>, StoreError>;
    /// Returns the highest sequence number, or 0 if empty.
    async fn max_seq(&self) -> Result<i64, StoreError>;
    /// Returns the lowest sequence number still stored (older ones were pruned), or 0 if
    /// empty. A reader whose cursor is below `min_seq - 1` has lost history (P3).
    async fn min_seq(&self) -> Result<i64, StoreError>;
    /// Deletes events older than the given time; returns the deleted count.
    async fn prune(&self, older_than: Timestamp) -> Result<u64, StoreError>;
}

/// The job queue (design 04 §6). Every time comparison (`run_at`, `locked_until`) uses
/// the database clock: `now()` on Postgres, the process clock on SQLite (one process is
/// the only client there). No method takes a `now` argument.
///
/// **Clock inside a `Tx` differs by backend.** Postgres `now()` is the transaction's
/// start time, the same for every statement of the `Tx`; SQLite reads the wall clock per
/// call. A `Tx` that lives for seconds therefore claims, leases and backs off from an
/// older "now" on Postgres than on SQLite (shorter effective leases, jobs due a little
/// later). Claim and lease from `Store::read()` (autocommit), or keep such transactions
/// short.
///
/// **Lease token (P6).** A claim returns rows whose [`Job::lease`] is the token
/// `(id, node, attempts)`. `extend`, `complete`, `fail` and `release` act only while
/// the job is still `running` under exactly that token, so a node whose lease lapsed
/// (even one that reclaimed the same job since) cannot overwrite the current attempt;
/// they return `false`/`None` instead.
///
/// Validation errors (`Invalid`) are raised before any SQL and never poison a `Tx`.
#[async_trait]
pub trait JobRepo: Send + Sync {
    /// Inserts a job and returns its id, or `None` when `unique_key` is already taken
    /// by a job in any status (that is not an error and never poisons a `Tx`).
    /// Invalid: kind not `[a-z0-9._-]{1,64}`, `unique_key` over 200 characters or with
    /// NUL, payload not a JSON object (or with NUL), `max_attempts` outside 1..=100.
    async fn enqueue(&self, job: &NewJob) -> Result<Option<JobId>, StoreError>;
    /// Claims up to `limit` (1..=[`MAX_CLAIM`](crate::MAX_CLAIM)) due jobs whose kind is
    /// in `kinds` (1..=[`MAX_CLAIM_KINDS`](crate::MAX_CLAIM_KINDS)) for `node`: queued
    /// jobs with `run_at <= now`, and running jobs whose lease expired with
    /// `attempts < max_attempts`. Claimed rows become `running` with
    /// `locked_until = now + lease` and one more attempt; they are returned ordered by
    /// `(priority DESC, run_at, id)`. First, expired leases of jobs at `max_attempts`
    /// (of these kinds) go `dead` (P7). Exclusive: no two concurrent calls, on any node,
    /// return the same attempt of a job.
    async fn claim(
        &self,
        node: NodeId,
        kinds: &[String],
        limit: u32,
        lease: Duration,
    ) -> Result<Vec<Job>, StoreError>;
    /// Sets `locked_until = now + lease`; `false` if the lease is no longer held.
    async fn extend(&self, lease: &JobLease, duration: Duration) -> Result<bool, StoreError>;
    /// Marks the job `done`; `false` if the lease is no longer held.
    async fn complete(&self, lease: &JobLease) -> Result<bool, StoreError>;
    /// Records a failed attempt: `dead` when `permanent` or the attempts are used up,
    /// otherwise `queued` again with `run_at = now + job_backoff(attempts)`
    /// ([`job_backoff`](crate::job_backoff)). `error` is stored sanitised
    /// ([`sanitize_job_error`](crate::sanitize_job_error)). Returns the new status, or
    /// `None` if the lease is no longer held.
    async fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        permanent: bool,
    ) -> Result<Option<JobStatus>, StoreError>;
    /// Puts the job back to `queued` with `run_at = now + delay`; with
    /// `count_attempt == false` the attempt is refunded. Counting the final attempt
    /// (`attempts >= max_attempts`) ends the job instead: `dead`, `finished_at` set,
    /// `last_error = "released after final attempt"`. `false` if the lease is no longer
    /// held.
    async fn release(
        &self,
        lease: &JobLease,
        delay: Duration,
        count_attempt: bool,
    ) -> Result<bool, StoreError>;
    /// Fetches a job by id.
    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError>;
    /// Number of jobs per status; every status is present (0 when none).
    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError>;
    /// Deletes `done` jobs finished before `done_before` and `dead` jobs finished before
    /// `dead_before`; queued and running jobs are never deleted. Returns the count.
    async fn prune(
        &self,
        done_before: Timestamp,
        dead_before: Timestamp,
    ) -> Result<u64, StoreError>;
}

/// The node registry (04 §3, P14). Times come from the database clock (Postgres
/// `now()`, which inside a `Tx` is the transaction's start; SQLite the process clock).
#[async_trait]
pub trait NodeRepo: Send + Sync {
    /// Inserts the node with `started_at = heartbeat_at = now`. Invalid: NUL in `name` or
    /// `version`, `meta` not a JSON object (or with NUL). An id already registered is
    /// `UniqueViolation { constraint: "nodes_pkey" }`.
    async fn register(&self, node: &NewNode) -> Result<(), StoreError>;
    /// Sets `heartbeat_at = now` and replaces `meta` (a JSON object); `false` when the
    /// row is gone (pruned, or deregistered).
    async fn heartbeat(&self, id: NodeId, meta: &serde_json::Value) -> Result<bool, StoreError>;
    /// Removes the node's row; a missing row is `Ok(())`.
    async fn deregister(&self, id: NodeId) -> Result<(), StoreError>;
    /// Up to [`MAX_NODES`](crate::MAX_NODES) nodes, ordered by `(started_at, id)`, live or
    /// stale (callers compare `heartbeat_at` with [`NodeRepo::db_now`]).
    async fn list(&self) -> Result<Vec<Node>, StoreError>;
    /// The database clock: Postgres `SELECT now()`, SQLite `Timestamp::now()`.
    async fn db_now(&self) -> Result<Timestamp, StoreError>;
    /// Deletes nodes whose `heartbeat_at` is before `heartbeat_before`; returns the count.
    async fn prune(&self, heartbeat_before: Timestamp) -> Result<u64, StoreError>;
}

/// Full-text search over an item's title and body text. Both backends match exactly the
/// same documents; only the ranking may differ (02 §4, R16, R17).
///
/// - Rust normalizes all text, for indexing and querying alike
///   ([`crate::index_terms`], [`crate::search_terms`]). It applies NFKC, lowercases, and
///   splits on anything that is neither alphanumeric nor a combining mark. Terms are cut
///   to 64 characters. The engines only see the resulting tokens, never a parser or
///   locale.
/// - The query has no syntax: `-x`, quotes, `OR`, `*` and parentheses are separators or
///   ordinary words. Its first 32 distinct terms are prefix matches, and all must match.
///   No terms gives an empty page without touching the database.
/// - Each item indexes at most 4096 distinct terms and 16383 token positions; later
///   tokens are not searchable.
/// - Results come best first (title weighted above body), ties broken by item id
///   descending. The cursor is a keyset over `(rank, id)`, with the rank carried as exact
///   float bits. **Pages are stable only while the index is unchanged**: SQLite's BM25
///   depends on corpus statistics, so re-indexing between pages can reorder results and
///   a cursor may then skip or repeat items. Postgres' rank depends only on the item's
///   own document.
/// - `limit` must be 1 through 200. NUL in any text is `Invalid`.
#[async_trait]
pub trait SearchRepo: Send + Sync {
    /// Indexes (or re-indexes) an item's title and body text.
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError>;
    /// Removes an item from the index.
    async fn remove(&self, id: ContentId) -> Result<(), StoreError>;
    /// Returns one page of matching item ids, best first, restricted to the query's
    /// kinds (empty: all) and statuses (empty: all) via `content_items`.
    async fn search(&self, query: &SearchQuery) -> Result<Page<ContentId>, StoreError>;
}

#[async_trait]
pub trait SchemaMetaRepo: Send + Sync {
    /// Reads a schema_meta value.
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    /// Sets a schema_meta value.
    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError>;
}
