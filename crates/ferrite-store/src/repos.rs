use async_trait::async_trait;
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, ContentItem, ContentQuery, ContentType, Event,
    InstalledPlugin, InstalledTheme, Media, Menu, NewContent, NewEvent, NewUser, OptionEntry, Page,
    Redirect, RedirectId, Revision, RevisionId, Taxonomy, Term, TermId, Timestamp, User, UserId,
};

use crate::{StoreError, migrate::MigrationStatus};

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
    /// Repositories bound to the read pool (autocommit, no transaction).
    fn read(&self) -> &dyn Repos;
    /// Starts a write transaction; dropping it without commit rolls back.
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError>;
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
    /// this (outer) `Tx` return `Invalid`. Nesting can go deeper.
    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError>;
}

/// Accessors for every repository.
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
    /// Deletes a content type; built-in types yield `Invalid`.
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
    async fn list(&self, query: &ContentQuery) -> Result<Page<ContentItem>, StoreError>;
    /// Deletes an item by id.
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
    /// Deletes one meta key.
    async fn delete(&self, id: ContentId, key: &str) -> Result<(), StoreError>;
    /// Lists all meta entries of an item.
    async fn list(&self, id: ContentId) -> Result<Vec<(String, serde_json::Value)>, StoreError>;
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
    /// Updates a term.
    async fn update(&self, term: &Term) -> Result<(), StoreError>;
    /// Fetches a term by id.
    async fn get(&self, id: TermId) -> Result<Option<Term>, StoreError>;
    /// Fetches a term by taxonomy and slug.
    async fn get_by_slug(&self, tax: &str, slug: &str) -> Result<Option<Term>, StoreError>;
    /// Lists all terms of a taxonomy.
    async fn list(&self, tax: &str) -> Result<Vec<Term>, StoreError>;
    /// Deletes a term.
    async fn delete(&self, id: TermId) -> Result<(), StoreError>;
    /// Replaces an item's terms, storing the given order as `position`.
    async fn set_for_item(&self, item: ContentId, terms: &[TermId]) -> Result<(), StoreError>;
    /// Lists an item's terms ordered by position.
    async fn for_item(&self, item: ContentId) -> Result<Vec<Term>, StoreError>;
}

#[async_trait]
pub trait MenuRepo: Send + Sync {
    /// Fetches a menu with its item tree by name.
    async fn get(&self, name: &str) -> Result<Option<Menu>, StoreError>;
    /// Replaces the whole menu tree in one statement batch.
    async fn put(&self, menu: &Menu) -> Result<(), StoreError>;
    /// Lists menus as (name, label) pairs.
    async fn list(&self) -> Result<Vec<(String, String)>, StoreError>;
    /// Deletes a menu and its items.
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
    /// Deletes an option.
    async fn delete(&self, name: &str) -> Result<(), StoreError>;
    /// Lists all options flagged autoload.
    async fn autoloaded(&self) -> Result<Vec<OptionEntry>, StoreError>;
}

#[async_trait]
pub trait RedirectRepo: Send + Sync {
    /// Fetches a redirect by source path.
    async fn get_by_source(&self, source: &str) -> Result<Option<Redirect>, StoreError>;
    /// Inserts or replaces a redirect keyed by `source_path`.
    async fn upsert(&self, redirect: &Redirect) -> Result<(), StoreError>;
    /// Deletes a redirect.
    async fn delete(&self, id: RedirectId) -> Result<(), StoreError>;
    /// Lists redirects with cursor pagination.
    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<Redirect>, StoreError>;
    /// Records a hit (increments the counter, updates last-hit time).
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
    /// Updates a user.
    async fn update(&self, user: &User) -> Result<(), StoreError>;
    /// Lists users with cursor pagination.
    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<User>, StoreError>;
}

#[async_trait]
pub trait ArtifactRepo: Send + Sync {
    /// Inserts an artifact unless its hash exists; returns true if inserted.
    async fn insert_if_absent(&self, artifact: &Artifact) -> Result<bool, StoreError>;
    /// Fetches an artifact by hash.
    async fn get(&self, hash: &ArtifactHash) -> Result<Option<Artifact>, StoreError>;
    /// Deletes an artifact by hash.
    async fn delete(&self, hash: &ArtifactHash) -> Result<(), StoreError>;
}

#[async_trait]
pub trait MediaRepo: Send + Sync {
    /// Inserts media metadata.
    async fn insert(&self, media: &Media) -> Result<(), StoreError>;
    /// Fetches media metadata by content id.
    async fn get(&self, id: ContentId) -> Result<Option<Media>, StoreError>;
    /// Updates media metadata.
    async fn update(&self, media: &Media) -> Result<(), StoreError>;
}

#[async_trait]
pub trait PluginRepo: Send + Sync {
    /// Inserts or replaces an installed plugin by id.
    async fn upsert(&self, plugin: &InstalledPlugin) -> Result<(), StoreError>;
    /// Fetches an installed plugin.
    async fn get(&self, id: &str) -> Result<Option<InstalledPlugin>, StoreError>;
    /// Lists installed plugins.
    async fn list(&self) -> Result<Vec<InstalledPlugin>, StoreError>;
    /// Enables or disables a plugin, recording an optional reason.
    async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        reason: Option<&str>,
    ) -> Result<(), StoreError>;
    /// Removes an installed plugin.
    async fn delete(&self, id: &str) -> Result<(), StoreError>;
}

#[async_trait]
pub trait PluginKvRepo: Send + Sync {
    /// Reads a plugin's value.
    async fn get(&self, plugin: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    /// Writes a plugin's value.
    async fn set(&self, plugin: &str, key: &str, value: &[u8]) -> Result<(), StoreError>;
    /// Deletes a plugin's key.
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
    /// Makes the given theme the only active one.
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
    /// Deletes events older than the given time; returns the deleted count.
    async fn prune(&self, older_than: Timestamp) -> Result<u64, StoreError>;
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
///   No terms gives `Ok(vec![])` without touching the database.
/// - Each item indexes at most 4096 distinct terms and 16383 token positions; later
///   tokens are not searchable.
/// - Results come best first, with the title weighted above the body, and ties break by
///   item id. `limit` must be 1 through 200. NUL in any text is `Invalid`.
#[async_trait]
pub trait SearchRepo: Send + Sync {
    /// Indexes (or re-indexes) an item's title and body text.
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError>;
    /// Removes an item from the index.
    async fn remove(&self, id: ContentId) -> Result<(), StoreError>;
    /// Returns up to `limit` matching item ids, best first.
    async fn query(&self, q: &str, limit: u32) -> Result<Vec<ContentId>, StoreError>;
}

#[async_trait]
pub trait SchemaMetaRepo: Send + Sync {
    /// Reads a schema_meta value.
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    /// Sets a schema_meta value.
    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError>;
}
