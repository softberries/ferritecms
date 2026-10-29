use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use async_trait::async_trait;
use ferrite_domain::{
    ApiToken, ApiTokenId, Artifact, ArtifactHash, AuditEntry, AuditId, AuditQuery, Autosave,
    Comment, CommentId, CommentQuery, CommentStatus, ContentId, ContentItem, ContentQuery,
    ContentType, Event, FailureBucket, IdemBegin, IdempotencyKey, ImportMapKind, ImportMapRow,
    ImportRun, ImportRunId, ImportRunState, InstalledPlugin, InstalledTheme, Job, JobId, JobLease,
    JobQuery, JobStatus, Media, Menu, NewAudit, NewContent, NewEvent, NewJob, NewNode, NewUser,
    Node, NodeId, OptionEntry, Page, RateHit, Redirect, RedirectId, RedirectQuery, Revision,
    RevisionId, SearchQuery, Secret, Session, Taxonomy, Term, TermId, Timestamp, User, UserId,
    UserQuery,
};

use uuid::Uuid;

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
    /// Waits up to `wait` for the named lock (P19); `Ok(None)` when it stayed held. The
    /// default retries [`try_lock`](Self::try_lock) every [`LOCK_POLL`](crate::LOCK_POLL)
    /// (first attempt immediate, last one at `wait`); SQLite uses it. Postgres overrides
    /// it with one dedicated connection that sets `lock_timeout` and blocks in
    /// `pg_advisory_lock`, so a release wakes the waiter at once. Dropping the future
    /// cancels the wait and frees a lock granted late. Callers use
    /// [`lock`](crate::lock()), which turns `None` into `Busy`.
    async fn lock_wait(
        &self,
        name: &str,
        wait: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(guard) = self.try_lock(name).await? {
                return Ok(Some(guard));
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(crate::LOCK_POLL.min(deadline - now)).await;
        }
    }
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
    /// Opens a reader over one read snapshot of the whole database (M4-1 P3), for an
    /// export or a copy; see [`ExportReader`](crate::ExportReader). Release it with
    /// [`ExportReader::finish`](crate::ExportReader::finish).
    async fn export_reader(&self) -> Result<Box<dyn crate::ExportReader>, StoreError>;
    /// Opens the single-transaction row loader (M4-1 P4), for import-site and `db copy`;
    /// see [`ImportWriter`](crate::ImportWriter). Nothing is written until
    /// [`ImportWriter::commit`](crate::ImportWriter::commit). SQLite: holds the writer
    /// connection until then; Postgres: one dedicated connection (`<application_name>:import`).
    async fn import_writer(&self) -> Result<Box<dyn crate::ImportWriter>, StoreError>;
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
/// - `update`, `PluginRepo::set_enabled` and `ThemeRepo::activate` return [`StoreError::NotFound`] (`ContentRepo::update` returns
///   `Conflict` instead when the row exists with another version);
/// - `delete` is idempotent: deleting a missing row is `Ok(())`. A delete blocked by
///   dependent rows (`RESTRICT`) is [`StoreError::ForeignKey`];
/// - `RedirectRepo::add_hits` on a missing row is `Ok(())` (the redirect was deleted
///   since the hits were counted).
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
    fn plugin_failures(&self) -> &dyn PluginFailureRepo;
    fn plugin_kv(&self) -> &dyn PluginKvRepo;
    fn themes(&self) -> &dyn ThemeRepo;
    fn events(&self) -> &dyn EventRepo;
    fn jobs(&self) -> &dyn JobRepo;
    fn nodes(&self) -> &dyn NodeRepo;
    fn search(&self) -> &dyn SearchRepo;
    fn schema_meta(&self) -> &dyn SchemaMetaRepo;
    fn sessions(&self) -> &dyn SessionRepo;
    fn api_tokens(&self) -> &dyn ApiTokenRepo;
    fn rate_limits(&self) -> &dyn RateLimitRepo;
    fn audit(&self) -> &dyn AuditRepo;
    fn comments(&self) -> &dyn CommentRepo;
    fn idempotency(&self) -> &dyn IdempotencyRepo;
    fn autosaves(&self) -> &dyn AutosaveRepo;
    fn imports(&self) -> &dyn ImportRepo;
}

#[async_trait]
pub trait ContentTypeRepo: Send + Sync {
    /// Inserts or replaces a content type by name.
    async fn upsert(&self, ty: &ContentType) -> Result<(), StoreError>;
    /// Inserts a new content type; a taken name is
    /// `UniqueViolation { constraint: "content_types_pkey" }`.
    async fn insert(&self, ty: &ContentType) -> Result<(), StoreError>;
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
    /// Sets `updated_at` of item `id` to `at`, leaving `version` and everything else as
    /// it is (the WordPress import keeps the source's modification time after a write,
    /// M3-1 P7). A missing item is `NotFound`.
    async fn set_updated_at(&self, id: ContentId, at: Timestamp) -> Result<(), StoreError>;
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
    /// Moves every item authored by `from` to `to` in one `UPDATE`, bumping `version`
    /// (`updated_at` stays); returns the count. An unknown `to` is `ForeignKey`. Callers
    /// reassign revisions in the same `Tx` ([`RevisionRepo::reassign_author`]).
    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError>;
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
    /// Moves every revision authored by `from` to `to` in one `UPDATE`; returns the count.
    /// An unknown `to` is `ForeignKey`.
    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError>;
}

#[async_trait]
pub trait TaxonomyRepo: Send + Sync {
    /// Inserts or replaces a taxonomy by name.
    async fn upsert(&self, taxonomy: &Taxonomy) -> Result<(), StoreError>;
    /// Inserts a new taxonomy; a taken name is
    /// `UniqueViolation { constraint: "taxonomies_pkey" }`.
    async fn insert(&self, taxonomy: &Taxonomy) -> Result<(), StoreError>;
    /// Fetches a taxonomy by name.
    async fn get(&self, name: &str) -> Result<Option<Taxonomy>, StoreError>;
    /// Lists all taxonomies.
    async fn list(&self) -> Result<Vec<Taxonomy>, StoreError>;
    /// Deletes a taxonomy in one statement; a built-in one is `Invalid`, one that still
    /// has terms `ForeignKey`, a missing one a no-op.
    async fn delete(&self, name: &str) -> Result<(), StoreError>;
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
    /// Lists up to `limit` (1..=200, else `Invalid`) options whose key starts with
    /// `prefix` (compared literally) and sorts after `after` (`""`: from the first), in
    /// binary key order. Callers page by passing the last key as `after`.
    async fn list_prefix(
        &self,
        prefix: &str,
        after: &str,
        limit: u32,
    ) -> Result<Vec<OptionEntry>, StoreError>;
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
    /// Lists redirects by `source_path` (binary order, then id) with cursor pagination
    /// (tag [`REDIRECT_CURSOR_TAG`](crate::REDIRECT_CURSOR_TAG)); `limit` 1..=200. With
    /// `q`, only rows whose `source_path` or `target` holds `q` as a case-insensitive
    /// substring (`LIKE`/`ILIKE … ESCAPE '\'` over [`like_contains`](crate::like_contains):
    /// `%`, `_` and `\` match only themselves; SQLite folds ASCII case only). NUL in `q`
    /// is `Invalid`; its length is core's to check.
    async fn list(&self, query: &RedirectQuery) -> Result<Page<Redirect>, StoreError>;
    /// Redirects whose `target` equals `target` exactly (case-sensitive), ordered by
    /// `source_path` in binary order, at most
    /// [`MAX_REDIRECTS_BY_TARGET`](crate::MAX_REDIRECTS_BY_TARGET).
    async fn list_by_target(&self, target: &str) -> Result<Vec<Redirect>, StoreError>;
    /// Adds `n` (>= 1) hits seen up to `at` in one statement: `hits = hits + n`,
    /// `last_hit_at = max(last_hit_at, at)`, so a late flush never moves it back. A
    /// missing row is `Ok` (the redirect was deleted since the hits were counted);
    /// `n = 0` is `Invalid`.
    async fn add_hits(&self, id: RedirectId, n: u32, at: Timestamp) -> Result<(), StoreError>;
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
    /// Fetches a user and, inside a `Tx`, locks its row until the `Tx` ends: Postgres
    /// `SELECT … FOR UPDATE` (other writers of the row wait), SQLite a plain read (the
    /// `Tx` already holds the single writer). Outside a `Tx` it is [`get`](Self::get).
    /// Every read-modify-write of a user reads the row with this inside the `Tx` that
    /// writes it, because [`update`](Self::update) is unversioned.
    async fn get_for_update(&self, id: UserId) -> Result<Option<User>, StoreError>;
    /// Updates a user (every column except `id`, `created_at`, `totp_secret` and
    /// `totp_last_step`); a missing user is `NotFound`. TOTP state changes only through
    /// [`set_totp`](Self::set_totp) and [`advance_totp_step`](Self::advance_totp_step).
    async fn update(&self, user: &User) -> Result<(), StoreError>;
    /// Sets (or with `None` clears) the TOTP secret and resets `totp_last_step` to NULL,
    /// in one statement; a missing user is `NotFound`.
    async fn set_totp(&self, id: UserId, secret: Option<&Secret>) -> Result<(), StoreError>;
    /// Records `step` as the last accepted TOTP step if it is newer than the stored one
    /// (one conditional `UPDATE`, P9). `false` means a replayed or older step, or an
    /// unknown user: the code must be refused. Concurrent calls with the same step on any
    /// node: exactly one gets `true`.
    async fn advance_totp_step(&self, id: UserId, step: i64) -> Result<bool, StoreError>;
    /// Deletes a user (missing: no-op). Sessions and API tokens cascade, audit rows keep
    /// a NULL actor; authored content or revisions make it `ForeignKey` (reassign them
    /// first, in the same `Tx`).
    async fn delete(&self, id: UserId) -> Result<(), StoreError>;
    /// Number of users with role `admin` and status `active`.
    async fn count_active_admins(&self) -> Result<u64, StoreError>;
    /// Up to `limit` users with role `admin` and status `active`, ordered by login
    /// (then id); `limit` outside 1..=[`MAX_AUTH_LIST`](crate::MAX_AUTH_LIST) is
    /// `Invalid`.
    async fn list_active_admins(&self, limit: u32) -> Result<Vec<User>, StoreError>;
    /// Lists users by login (binary order, then id) with cursor pagination, filtered by
    /// `query`: `q` is a case-insensitive substring of the login, display name or e-mail
    /// (`LIKE … ESCAPE '\'`, Postgres `ILIKE`; SQLite folds ASCII case only), with `%`,
    /// `_` and `\` matching only themselves; `role` and `status` match exactly. A NUL in
    /// `q`, a bad `limit` or a bad cursor is `Invalid`.
    async fn list(&self, query: &UserQuery) -> Result<Page<User>, StoreError>;
    /// Fetches up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) users in one query, returned
    /// in the order of `ids`; missing ids are skipped and a repeated id yields its user
    /// once (at its first position). More ids is `Invalid`.
    async fn get_many(&self, ids: &[UserId]) -> Result<Vec<User>, StoreError>;
}

#[async_trait]
/// Artifact rows and the GC queries over them.
///
/// `media.artifact_hash`, `plugins.artifact_hash` and `themes.artifact_hash` are foreign
/// keys; `media_variants.hash` and `import_runs.report_hash` are not, so a writer of
/// those must call [`insert_if_absent`](Self::insert_if_absent) for the hash before it
/// writes the reference. [`delete_if_orphaned`](Self::delete_if_orphaned) may return
/// `StoreError::ForeignKey` when an FK-backed reference races in; callers treat that as
/// "kept".
pub trait ArtifactRepo: Send + Sync {
    /// Inserts an artifact unless its hash exists; returns true if inserted.
    async fn insert_if_absent(&self, artifact: &Artifact) -> Result<bool, StoreError>;
    /// Fetches an artifact by hash.
    async fn get(&self, hash: &ArtifactHash) -> Result<Option<Artifact>, StoreError>;
    /// Deletes an artifact by hash (missing: no-op).
    async fn delete(&self, hash: &ArtifactHash) -> Result<(), StoreError>;
    /// Up to `limit` (1 through `MAX_PAGE`) artifacts ordered by hash, after `after`.
    async fn list_page(
        &self,
        after: Option<&ArtifactHash>,
        limit: u32,
    ) -> Result<Vec<Artifact>, StoreError>;
    /// The given hashes that have a row (at most [`MAX_EXISTING_HASHES`](crate::MAX_EXISTING_HASHES)
    /// hashes, else `Invalid`).
    async fn existing(&self, hashes: &[ArtifactHash]) -> Result<Vec<ArtifactHash>, StoreError>;
    /// One GC mark pass. Sets `orphaned_at = now` on unmarked rows that nothing
    /// references (`media.artifact_hash`, `media_variants.hash`, `plugins.artifact_hash`,
    /// `themes.artifact_hash`, `import_runs.report_hash`) and that are not in
    /// `protected`; clears the mark on marked rows that are referenced or protected.
    async fn mark_orphans(
        &self,
        now: Timestamp,
        protected: &[ArtifactHash],
    ) -> Result<MarkCounts, StoreError>;
    /// Up to `limit` artifacts marked orphaned before `before`, ordered by hash.
    async fn orphaned_before(
        &self,
        before: Timestamp,
        limit: u32,
    ) -> Result<Vec<Artifact>, StoreError>;
    /// Deletes the row when it is marked before `before`, referenced by nothing and not
    /// in `protected`, in one statement (the references are re-checked there); returns
    /// whether it was deleted.
    async fn delete_if_orphaned(
        &self,
        hash: &ArtifactHash,
        before: Timestamp,
        protected: &[ArtifactHash],
    ) -> Result<bool, StoreError>;
}

/// What one [`ArtifactRepo::mark_orphans`] pass changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MarkCounts {
    /// Rows newly marked orphaned.
    pub marked: u64,
    /// Marked rows that are referenced (or protected) again and lost their mark.
    pub unmarked: u64,
}

#[async_trait]
pub trait MediaRepo: Send + Sync {
    /// Inserts media metadata (`sanitized` included).
    async fn insert(&self, media: &Media) -> Result<(), StoreError>;
    /// Fetches media metadata by content id.
    async fn get(&self, id: ContentId) -> Result<Option<Media>, StoreError>;
    /// Fetches media metadata and, inside a `Tx`, locks its row until the `Tx` ends:
    /// Postgres `SELECT … FOR UPDATE` (other writers of the row wait), SQLite a plain
    /// read (the `Tx` already holds the single writer). Outside a `Tx` it is
    /// [`get`](Self::get). Every read-modify-write of a media row (alt text, variants)
    /// reads it with this inside the `Tx` that writes it, because
    /// [`update`](Self::update) is unversioned.
    async fn get_for_update(&self, id: ContentId) -> Result<Option<Media>, StoreError>;
    /// Updates media metadata (`mime`, `bytes`, `width`, `height`, `alt`, `variants`,
    /// `sanitized`; the artifact and `original_name` stay); missing metadata is
    /// `NotFound`.
    async fn update(&self, media: &Media) -> Result<(), StoreError>;
    /// Fetches up to [`MAX_ID_FILTER`](crate::MAX_ID_FILTER) media rows by content id in
    /// one query, returned in the order of `ids`; missing ids are skipped and a repeated
    /// id yields its row once (at its first position). More ids is `Invalid`.
    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<Media>, StoreError>;
    /// Up to `limit` (1..=`MAX_PAGE`) media rows whose original
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
    /// Whether an installed plugin is enabled (`enabled` only, no JSON decoded): `None`
    /// when no row exists. The per-request check of plugin routes (05 §4b).
    async fn is_enabled(&self, id: &str) -> Result<Option<bool>, StoreError>;
    /// Lists installed plugins.
    async fn list(&self) -> Result<Vec<InstalledPlugin>, StoreError>;
    /// Names of the enabled plugins whose stored manifest has `hook` as a key of its
    /// `hooks` object, in binary name order: who gets an action hook's job (M2a-2 P20).
    /// Reads only `name`, `enabled` and the manifest's `hooks`; inside a `Tx` it reads
    /// through it. A `hooks` value that is not an object subscribes to nothing.
    async fn action_subscribers(&self, hook: &str) -> Result<Vec<String>, StoreError>;
    /// Fetches an installed plugin and, inside a `Tx`, locks its row until the `Tx`
    /// ends: Postgres `SELECT … FOR UPDATE` (other writers of the row wait), SQLite a
    /// plain read (the `Tx` already holds the single writer). Outside a `Tx` it is
    /// [`get`](Self::get). Callers locking several rows lock them in name order.
    async fn get_for_update(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError>;
    /// Enables or disables a plugin, recording an optional reason; missing: `NotFound`.
    async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        reason: Option<&str>,
    ) -> Result<(), StoreError>;
    /// Removes an installed plugin (missing: no-op).
    async fn delete(&self, id: &str) -> Result<(), StoreError>;
    /// The deleted plugins whose data is still kept (M2b-3 P47): every option
    /// `plugin.<name>.deleted_at` (the deletion marker a delete writes) whose value is
    /// an integer (unix micros) and whose `<name>` has no `.` and no installed row, by
    /// name in binary order. Other values are skipped. Unbounded: one row per deleted
    /// plugin not yet purged.
    async fn retained(&self) -> Result<Vec<RetainedPlugin>, StoreError>;
    /// Disables the plugin with `reason` in one conditional statement (P20, R12), only
    /// while it is enabled **and** its `updated_at` is still `observed_updated_at` (the
    /// value the caller read before summing its failures): `true` only for the
    /// transition; `false` when the plugin is disabled (its reason is kept), missing, or
    /// changed since the observation (say, enabled again, which also cleared its
    /// counters). Concurrent calls on any nodes: at most one returns `true`. The new
    /// `updated_at` is the database clock.
    async fn auto_disable(
        &self,
        id: &str,
        reason: &str,
        observed_updated_at: Timestamp,
    ) -> Result<bool, StoreError>;
}

/// A deleted plugin whose KV and settings are still kept ([`PluginRepo::retained`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedPlugin {
    pub name: String,
    /// When it was deleted (the marker's value).
    pub deleted_at: Timestamp,
}

/// The marker `key`'s plugin name and its value as a [`RetainedPlugin`], if `key` is a
/// marker of a dot-free name and `value` integer micros: what both backends'
/// [`PluginRepo::retained`] keep of the rows their query selects.
pub fn retained_marker(key: &str, value: &serde_json::Value) -> Option<RetainedPlugin> {
    let name = key.strip_prefix("plugin.")?.strip_suffix(".deleted_at")?;
    if name.is_empty() || name.contains('.') {
        return None;
    }
    let deleted_at = Timestamp::from_micros(value.as_i64()?).ok()?;
    Some(RetainedPlugin {
        name: name.to_owned(),
        deleted_at,
    })
}

/// Shared per-minute plugin failure counters (P20): each node adds its buffered counts,
/// then sums a plugin's recent buckets against the auto-disable threshold. Bucket
/// times are chosen by the caller (the database clock floored to a minute).
#[async_trait]
pub trait PluginFailureRepo: Send + Sync {
    /// Adds `count` (at least 1) failures of `plugin`'s `hook` to the bucket starting at
    /// `bucket_start` (created if absent; `count = count + n` otherwise, one statement).
    async fn add(
        &self,
        plugin: &str,
        hook: &str,
        bucket_start: Timestamp,
        count: u32,
    ) -> Result<(), StoreError>;
    /// Total failures of `plugin`, all hooks, in buckets starting at or after `since`.
    async fn sum_since(&self, plugin: &str, since: Timestamp) -> Result<u64, StoreError>;
    /// `plugin`'s failures per hook and hour (hours aligned to the Unix epoch), from
    /// the buckets starting at or after `since`: newest hour first, then by hook, at
    /// most `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) rows. A minute
    /// bucket counts toward the hour it starts in, so the first hour may be partial.
    async fn buckets(
        &self,
        plugin: &str,
        since: Timestamp,
        limit: u32,
    ) -> Result<Vec<FailureBucket>, StoreError>;
    /// Deletes every bucket of `plugin` (when it is enabled again).
    async fn clear(&self, plugin: &str) -> Result<(), StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) buckets
    /// starting before `before`; returns the count. Callers loop until a batch comes
    /// back short (R5).
    async fn prune(&self, before: Timestamp, limit: u32) -> Result<u64, StoreError>;
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
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH), else
    /// `Invalid`) keys of `plugin` and returns the count; other plugins' keys stay.
    /// Callers loop until a batch comes back short (the plugin purge, M2a-2 P9).
    async fn delete_all(&self, plugin: &str, limit: u32) -> Result<u64, StoreError>;
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
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) events
    /// older than the given time, oldest first; returns the deleted count. Callers loop
    /// until a batch comes back short (R5). The newest event is never deleted (R9), so
    /// once any event exists `min_seq` stays above 0 and a reader can always detect
    /// pruned history.
    async fn prune(&self, older_than: Timestamp, limit: u32) -> Result<u64, StoreError>;
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
    /// One page of jobs, newest first (`created_at DESC, id DESC`), filtered by
    /// `status` and `kind` when given (the cursor tag is
    /// [`JOB_CURSOR_TAG`](crate::JOB_CURSOR_TAG)). Invalid: `limit` outside 1..=200, a
    /// kind not `[a-z0-9._-]{1,64}`, a cursor of another listing.
    async fn list(&self, query: &JobQuery) -> Result<Page<Job>, StoreError>;
    /// Puts a `dead` job back in the queue, in one conditional statement: `queued`, due
    /// now, `attempts = 0`, `finished_at`, `locked_by` and `locked_until` cleared;
    /// `last_error` is kept (the reason it died, until the next attempt overwrites it)
    /// and so is `unique_key` (still the same job). Returns the job as it now is, or
    /// `None` when there is no such job or it is not `dead` (a retry never touches a
    /// queued, running or done job).
    async fn retry_dead(&self, id: JobId) -> Result<Option<Job>, StoreError>;
    /// Number of jobs per status; every status is present (0 when none).
    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) of the
    /// `done` jobs finished before `done_before` and `dead` jobs finished before
    /// `dead_before`; queued and running jobs are never deleted. Returns the count;
    /// callers loop until a batch comes back short (R5).
    async fn prune(
        &self,
        done_before: Timestamp,
        dead_before: Timestamp,
        limit: u32,
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
    /// Up to [`MAX_NODES`](crate::MAX_NODES) nodes, live or stale (callers compare
    /// `heartbeat_at` with [`NodeRepo::db_now`]), newest heartbeat first (`heartbeat_at
    /// DESC, id`): when more rows exist (say a crash loop registering a new id per start,
    /// pruned only after 24 h), the live nodes are the ones listed.
    async fn list(&self) -> Result<Vec<Node>, StoreError>;
    /// The database clock: Postgres `SELECT now()`, SQLite `Timestamp::now()`.
    async fn db_now(&self) -> Result<Timestamp, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) nodes whose
    /// `heartbeat_at` is before `heartbeat_before`; returns the count (R5).
    async fn prune(&self, heartbeat_before: Timestamp, limit: u32) -> Result<u64, StoreError>;
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

/// Login sessions (07 §2, P7), keyed by the SHA-256 hex of the cookie token (`id_hash`).
/// Every text bound (hashes, secrets, user agent) is NUL-checked (`Invalid`). No per-node
/// cache: every request reads the row, so a delete takes effect at once on every node.
#[async_trait]
pub trait SessionRepo: Send + Sync {
    /// Inserts a session. An `id_hash` already stored is
    /// `UniqueViolation { constraint: "sessions_pkey" }`; an unknown user is `ForeignKey`.
    async fn insert(&self, session: &Session) -> Result<(), StoreError>;
    /// Fetches a session by `id_hash`, expired or not (the caller checks the times).
    async fn get(&self, id_hash: &str) -> Result<Option<Session>, StoreError>;
    /// Moves `last_seen_at` forward to `at` (an older `at` leaves it unchanged); `false`
    /// when the session is gone.
    async fn touch(&self, id_hash: &str, at: Timestamp) -> Result<bool, StoreError>;
    /// Deletes a session (missing: no-op).
    async fn delete(&self, id_hash: &str) -> Result<(), StoreError>;
    /// Deletes every session of `user` except the one whose `id_hash` is `keep`; returns
    /// the deleted count.
    async fn delete_for_user(&self, user: UserId, keep: Option<&str>) -> Result<u64, StoreError>;
    /// Up to [`MAX_AUTH_LIST`](crate::MAX_AUTH_LIST) sessions of `user`, newest first
    /// (`created_at DESC, id_hash DESC`).
    async fn list_for_user(&self, user: UserId) -> Result<Vec<Session>, StoreError>;
    /// The session of `user` whose [public id](crate::session_public_id) is `public_id`,
    /// among those [`list_for_user`](Self::list_for_user) returns (the newest
    /// [`MAX_AUTH_LIST`](crate::MAX_AUTH_LIST)); expired or not. Another user's session
    /// is `None`. The public id is not stored (no migration, M2b-3 P45), so this hashes
    /// each listed key.
    async fn get_by_public_id(
        &self,
        user: UserId,
        public_id: &str,
    ) -> Result<Option<Session>, StoreError> {
        Ok(self
            .list_for_user(user)
            .await?
            .into_iter()
            .find(|s| crate::session_public_id(&s.id_hash) == public_id))
    }
    /// Deletes the session [`get_by_public_id`](Self::get_by_public_id) finds; `false`
    /// when there is none.
    async fn delete_by_public_id(&self, user: UserId, public_id: &str) -> Result<bool, StoreError> {
        match self.get_by_public_id(user, public_id).await? {
            Some(session) => {
                self.delete(&session.id_hash).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) sessions with
    /// `expires_at < now` or `last_seen_at < idle_before`; returns the count. Callers loop
    /// until a batch comes back short (R5).
    async fn prune(
        &self,
        now: Timestamp,
        idle_before: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError>;
}

/// Personal API tokens (07 §2, P6), looked up by the SHA-256 hex of the bearer value.
/// Text binds (name, hash, scopes) are NUL-checked (`Invalid`).
#[async_trait]
pub trait ApiTokenRepo: Send + Sync {
    /// Inserts a token. A hash already stored is
    /// `UniqueViolation { constraint: "api_tokens_hash_uq" }`, an id already stored
    /// `api_tokens_pkey`; an unknown user is `ForeignKey`.
    async fn insert(&self, token: &ApiToken) -> Result<(), StoreError>;
    /// Fetches a token by its hash, expired or not (the caller checks `expires_at`).
    async fn get_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, StoreError>;
    /// Up to [`MAX_AUTH_LIST`](crate::MAX_AUTH_LIST) tokens of `user`, oldest first
    /// (`created_at, id`).
    async fn list_for_user(&self, user: UserId) -> Result<Vec<ApiToken>, StoreError>;
    /// Number of tokens `user` holds.
    async fn count_for_user(&self, user: UserId) -> Result<u64, StoreError>;
    /// Moves `last_used_at` forward to `at`; `false` when the token is gone.
    async fn touch(&self, id: ApiTokenId, at: Timestamp) -> Result<bool, StoreError>;
    /// Deletes the token only if `user` owns it; `true` if a row was deleted.
    async fn delete(&self, user: UserId, id: ApiTokenId) -> Result<bool, StoreError>;
    /// Deletes every token of `user`; returns the count.
    async fn delete_for_user(&self, user: UserId) -> Result<u64, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) tokens with
    /// `expires_at < now` (tokens without an expiry stay); returns the count. Callers
    /// loop until a batch comes back short (R5).
    async fn prune(&self, now: Timestamp, limit: u32) -> Result<u64, StoreError>;
}

/// Fixed-window rate-limit counters shared by every node (P5).
#[async_trait]
pub trait RateLimitRepo: Send + Sync {
    /// Counts one hit on `bucket` in the current window of length `window` and returns
    /// the count so far, the window's end and the clock used, in **one statement** (an
    /// upsert), so concurrent hits on any node never share a count. The window start is
    /// the database clock floored to a multiple of `window` since the Unix epoch
    /// (Postgres `now()`, which inside a `Tx` is the transaction's start; SQLite the
    /// process clock). A row from an earlier window starts over at 1. The stored window
    /// never moves backwards: a hit stamped in an older window than the stored one (it
    /// took its clock before waiting for the row) counts in the stored window
    /// (`window_start = max(stored, incoming)`; reset only when the incoming one is newer).
    /// Callers must call this outside a `Tx`.
    ///
    /// Invalid: `bucket` empty, over [`MAX_RATE_BUCKET`](crate::MAX_RATE_BUCKET)
    /// characters or with NUL; `window` zero or over
    /// [`MAX_RATE_WINDOW`](crate::MAX_RATE_WINDOW).
    async fn hit(&self, bucket: &str, window: Duration) -> Result<RateHit, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) buckets
    /// whose window started before `window_before`; returns the count (R5).
    async fn prune(&self, window_before: Timestamp, limit: u32) -> Result<u64, StoreError>;
}

/// The audit log (P22): append-only rows written in the same `Tx` as the change they
/// record.
#[async_trait]
pub trait AuditRepo: Send + Sync {
    /// Appends a row with a new UUIDv7 id, `at` = the database clock and `node` = `None`.
    /// Invalid (before any SQL, so a `Tx` is not poisoned): `details` not a JSON object,
    /// NUL in `details` or in any text.
    async fn append(&self, entry: &NewAudit) -> Result<AuditId, StoreError>;
    /// Lists rows newest first (`at DESC, id DESC`) with cursor pagination (tag
    /// [`AUDIT_CURSOR_TAG`](crate::AUDIT_CURSOR_TAG)), filtered by `query`
    /// ([`check_audit_query`](crate::check_audit_query): `limit` 1..=200, `target_id`
    /// only with `target_kind`, no NUL; else `Invalid`). An `action` ending in `.` is a
    /// literal, case-sensitive prefix (no LIKE wildcards); otherwise it is exact.
    async fn list(&self, query: &AuditQuery) -> Result<Page<AuditEntry>, StoreError>;
}

/// An item's approved comments ([`CommentRepo::approved_totals`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApprovedTotals {
    /// Every approved comment.
    pub comments: u64,
    /// Approved comments whose parent is null or not approved (P34).
    pub roots: u64,
}

/// Comments (M2a-2b P11). Core validates lengths, URLs, depth and the parent's status;
/// the store refuses NUL in every text (`Invalid`) and folds `author_email` with
/// [`email_key`](crate::email_key) on insert and lookup. `status` has no CHECK (P2):
/// rows decode only with a known [`CommentStatus`].
#[async_trait]
pub trait CommentRepo: Send + Sync {
    /// Inserts a comment (with a `user_id`: marked as written by an account, a mark it
    /// keeps). An unknown item, parent or user is `ForeignKey`; a taken id
    /// `UniqueViolation { constraint: "comments_pkey" }`.
    async fn insert(&self, comment: &Comment) -> Result<(), StoreError>;
    /// Fetches a comment by id.
    async fn get(&self, id: CommentId) -> Result<Option<Comment>, StoreError>;
    /// Sets the status in one `UPDATE … RETURNING` and returns the new row; `None` when
    /// the comment is missing.
    async fn set_status(
        &self,
        id: CommentId,
        status: CommentStatus,
    ) -> Result<Option<Comment>, StoreError>;
    /// Deletes a comment and (cascade) its replies; `false` when it was missing.
    async fn delete(&self, id: CommentId) -> Result<bool, StoreError>;
    /// Lists comments newest first (`created_at DESC, id DESC`), filtered by status
    /// and/or item, with cursor pagination (tag
    /// [`COMMENT_CURSOR_TAG`](crate::COMMENT_CURSOR_TAG)); `limit` 1..=200.
    async fn list(&self, query: &CommentQuery) -> Result<Page<Comment>, StoreError>;
    /// How many comments have each status, over every comment or `item`'s: every
    /// [`CommentStatus`] is a key (zero when none). One grouped `SELECT`; a row with an
    /// unknown status is a `Database` error, as on [`get`](Self::get).
    async fn counts(
        &self,
        item: Option<ContentId>,
    ) -> Result<BTreeMap<CommentStatus, u64>, StoreError>;
    /// One page of `item`'s public thread (M2b-2 P34, amended). A **root** is an
    /// approved comment whose parent is null, itself, or not approved; roots are ordered
    /// `(created_at, id)`. Returns roots `offset..offset + roots` (`roots`
    /// 1..=[`MAX_PAGE_ROOTS`](crate::MAX_PAGE_ROOTS)), each followed by its approved
    /// descendants depth first, siblings oldest first (`created_at, id`): the thread
    /// order. At most `max_rows` rows (`roots..=`[`MAX_PAGE_ROWS`](crate::MAX_PAGE_ROWS),
    /// else `Invalid`): every root of the page is always returned, and only replies are
    /// cut, keeping those first in the page's thread order, so a shown reply's parent
    /// is shown too. One recursive query.
    async fn approved_page(
        &self,
        item: ContentId,
        offset: u32,
        roots: u32,
        max_rows: u32,
    ) -> Result<Vec<Comment>, StoreError>;
    /// How many approved comments and roots (as [`approved_page`](Self::approved_page)
    /// defines them) `item` has; zero for an unknown item.
    async fn approved_totals(&self, item: ContentId) -> Result<ApprovedTotals, StoreError>;
    /// The 0-based position among `item`'s roots of the root whose thread holds
    /// `comment` (walking its approved ancestors); `None` when `comment` is not an
    /// approved comment of `item` (or its chain reaches no root).
    async fn root_position(
        &self,
        item: ContentId,
        comment: CommentId,
    ) -> Result<Option<u64>, StoreError>;
    /// Whether an anonymous commenter has an approved comment (R2 `known`, F19): an
    /// approved row inserted without `user_id` whose `author_email` equals
    /// `email_key(email)` and whose `author_name` equals `name` exactly (callers trim
    /// it). A row inserted with a `user_id` never matches, also after the user is
    /// deleted and its `user_id` became NULL (the store marks it `by_account`).
    async fn has_approved_from(&self, email: &str, name: &str) -> Result<bool, StoreError>;
    /// Whether `user` has an approved comment (R2 `known` for signed-in commenters, F19).
    async fn has_approved_from_user(&self, user: UserId) -> Result<bool, StoreError>;
}

/// `Idempotency-Key` records (M2a-2b P10, F5, F6), one per (user, key hash). Every time
/// the store picks (`created_at`, the staleness check) is the database clock (Postgres
/// `now()`, which inside a `Tx` is the transaction's start; SQLite the process clock).
/// Call it outside a `Tx`.
#[async_trait]
pub trait IdempotencyRepo: Send + Sync {
    /// Claims `key` with one insert-if-absent. On conflict it reads the row: another
    /// `method` or `path` is [`IdemBegin::Mismatch`]; a finished row
    /// [`IdemBegin::Replay`]; a pending row younger than `stale_after`
    /// [`IdemBegin::InProgress`]; an older pending row is taken over by one conditional
    /// `UPDATE` (new `created_at`, matched on the `created_at` read), so concurrent
    /// takers on any node get `Started` at most once (a loser re-reads). Invalid:
    /// `key_hash` not 64 lowercase hex, NUL in `method` or `path`, `stale_after` zero or
    /// over a day.
    async fn begin(
        &self,
        key: &IdempotencyKey,
        stale_after: Duration,
    ) -> Result<IdemBegin, StoreError>;
    /// Records the outcome on the pending row still carrying `token`; `false` when the
    /// row is gone, finished or taken over. Invalid: `status` outside 100..=599,
    /// `response` with NUL or over
    /// [`MAX_IDEMPOTENCY_RESPONSE`](crate::MAX_IDEMPOTENCY_RESPONSE) bytes as JSON.
    async fn finish(
        &self,
        user: UserId,
        key_hash: &str,
        token: Timestamp,
        status: u16,
        response: &serde_json::Value,
    ) -> Result<bool, StoreError>;
    /// Deletes the pending row still carrying `token` (a failed request frees its key);
    /// a finished or taken-over row stays. `true` when a row was deleted.
    async fn release(
        &self,
        user: UserId,
        key_hash: &str,
        token: Timestamp,
    ) -> Result<bool, StoreError>;
    /// Deletes up to `limit` (1..=[`MAX_PRUNE_BATCH`](crate::MAX_PRUNE_BATCH)) rows
    /// created before `created_before`, pending or finished; returns the count. Callers
    /// loop until a batch comes back short (R5).
    async fn prune(&self, created_before: Timestamp, limit: u32) -> Result<u64, StoreError>;
}

/// The per-user autosave slot of content items (`0007_autosaves`, M2b P15): at most one
/// row per (item, user). Deleting the item or the user deletes their rows.
#[async_trait]
pub trait AutosaveRepo: Send + Sync {
    /// Inserts or replaces the row of (`autosave.item_id`, `autosave.user_id`). A missing
    /// item or user is [`StoreError::ForeignKey`]; NUL in a text or the JSON is `Invalid`.
    async fn put(&self, autosave: &Autosave) -> Result<(), StoreError>;
    /// The row of (`item`, `user`), if any.
    async fn get(&self, item: ContentId, user: UserId) -> Result<Option<Autosave>, StoreError>;
    /// Deletes the row of (`item`, `user`); `true` when there was one.
    async fn delete(&self, item: ContentId, user: UserId) -> Result<bool, StoreError>;
}

/// WordPress import bookkeeping (`0009_import`, M3-1 P3, P5): the run rows and the
/// source-id map. Use it through a `Tx` so a batch's [`fence`](ImportRepo::fence) and
/// its writes commit together.
#[async_trait]
pub trait ImportRepo: Send + Sync {
    /// Inserts a run. A second `running` run is
    /// `UniqueViolation { constraint: "import_runs_running_uq" }`; an unknown
    /// `started_by` is [`StoreError::ForeignKey`].
    async fn create_run(&self, run: &ImportRun) -> Result<(), StoreError>;
    /// The fence (P5): sets `heartbeat_at = now` on the run when `token` matches and it
    /// is still `running`; `true` iff one row changed. On Postgres the row lock this
    /// takes in a `Tx` makes a concurrent [`set_state`](ImportRepo::set_state) wait for
    /// the commit.
    async fn fence(
        &self,
        run: ImportRunId,
        token: &[u8; 16],
        now: Timestamp,
    ) -> Result<bool, StoreError>;
    async fn get_run(&self, id: ImportRunId) -> Result<Option<ImportRun>, StoreError>;
    /// The takeover (P5): sets `state = abandoned` and `finished_at = now` on run `id`
    /// only while it is `running` with `heartbeat_at < stale_before`; `true` iff one row
    /// changed. The check and the write are one statement, so on Postgres a takeover that
    /// waits for a fenced batch's row lock re-checks the heartbeat the batch committed.
    async fn abandon_stale(
        &self,
        id: ImportRunId,
        stale_before: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError>;
    /// [`abandon_stale`](Self::abandon_stale) for a start by job `job` (M3-2 P27 as
    /// built): also only when the run has no job, is `job`'s own, or its job is not
    /// `running` (no such job included). One statement, like `abandon_stale`.
    async fn abandon_stale_for_job(
        &self,
        id: ImportRunId,
        stale_before: Timestamp,
        now: Timestamp,
        job: JobId,
    ) -> Result<bool, StoreError>;
    /// The run in state `running`, if any.
    async fn running(&self) -> Result<Option<ImportRun>, StoreError>;
    /// Runs, newest first; `limit` is 1..=[`MAX_PAGE`](ferrite_domain::MAX_PAGE).
    async fn list_runs(&self, limit: u32) -> Result<Vec<ImportRun>, StoreError>;
    /// Runs, newest first by (`started_at`, `id`), after `cursor` (the last row's pair
    /// of the previous page); `limit` is 1..=[`MAX_PAGE`](ferrite_domain::MAX_PAGE).
    async fn list_runs_page(
        &self,
        limit: u32,
        cursor: Option<(Timestamp, Uuid)>,
    ) -> Result<Vec<ImportRun>, StoreError>;
    /// The newest run of job `job`, if any.
    async fn run_by_job(&self, job: JobId) -> Result<Option<ImportRun>, StoreError>;
    /// Sets the run's `step`, fenced like [`fence`](ImportRepo::fence): `true` iff the
    /// run exists, is `running` and `token` matches.
    async fn set_step(
        &self,
        id: ImportRunId,
        token: &[u8; 16],
        step: &str,
    ) -> Result<bool, StoreError>;
    /// Sets the run's `report_hash`, whatever its state. A missing run is
    /// [`StoreError::NotFound`].
    async fn set_report(&self, id: ImportRunId, hash: &str) -> Result<(), StoreError>;
    /// Sets the state and the stats; `finished_at` is `now` unless the state is
    /// `running`. A missing run is [`StoreError::NotFound`].
    async fn set_state(
        &self,
        id: ImportRunId,
        state: ImportRunState,
        stats: &serde_json::Value,
        now: Timestamp,
    ) -> Result<(), StoreError>;
    async fn map_get(
        &self,
        source: &str,
        kind: ImportMapKind,
        wp_id: i64,
    ) -> Result<Option<ImportMapRow>, StoreError>;
    /// Inserts or replaces the row of (`source`, `kind`, `wp_id`).
    async fn map_put(&self, row: &ImportMapRow) -> Result<(), StoreError>;
}
