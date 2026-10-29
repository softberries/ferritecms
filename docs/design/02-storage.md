# 02 — Storage & Migrations

## 1. Requirements

| # | Requirement | Source |
|---|---|---|
| S1 | SQLite is the default backend, zero configuration, lives in the data folder | Brief, principle 1 |
| S2 | PostgreSQL is a first-class alternative for bigger setups (managed DB backups, PITR, replicas, multiple nodes) | User (D1) |
| S3 | Every feature works identically on both; one test suite proves it | Anti-goal |
| S4 | Migrations are versioned, reversible (`up`/`down`), and runnable from the CLI | User (D2) |
| S5 | An older binary can keep running against a newer schema within a compatibility window (binary rollback, rolling deploys) | User (D3), D5 |
| S6 | Plugins never touch SQL; they reach data only through WIT interfaces | Principle 2 |
| S7 | A site can move SQLite → Postgres (and back) without re-importing from WordPress | Design |

## 2. Library choice

**sqlx 0.9** with the `sqlite` and `postgres` features, `runtime-tokio`, `tls-rustls`.

**Minimum supported Postgres: 15.** `content_items` uniqueness over a nullable
`parent_id` uses `UNIQUE NULLS NOT DISTINCT` (`ferrite-store-postgres/migrations/0001_init.up.sql`),
which needs PG 15. CI runs Postgres 18; local development used 17.

**Timestamps use `chrono`** (`DateTime<Utc>` inside the `Timestamp` newtype in
`ferrite-domain`), not `jiff`: sqlx 0.9 has no `jiff` feature, only `chrono` and `time`.

Rejected alternatives:

- **sqlx `Any` driver** — lowest-common-denominator types, loses `jsonb`, `uuid`,
  `SKIP LOCKED`, FTS differences still leak. Not worth it.
- **Diesel** — has multi-backend support and up/down migrations, but sync-first and a
  heavier DSL; sqlx is already in the stack and the brief.
- **SeaORM** — adds an ORM layer we don't need; our queries are explicit and few.

### D9: runtime-checked queries, one impl per backend

sqlx's compile-time macros (`query!`) check against a single `DATABASE_URL`. Supporting
both backends in one workspace with those macros is possible in principle (offline data
per crate), but unverified for our layout and it makes builds depend on two live DBs.
**Decision:** use runtime `sqlx::query`/`query_as` with `FromRow` structs, and make the
**contract test suite** (§9) the safety net — every repository method is exercised
against both backends in CI. Revisit if the duplication or bug rate hurts.

## 3. Repository layer

`ferrite-store` defines backend-neutral traits. Core services depend only on these.

```rust
// ferrite-store/src/repos.rs (as implemented in M1a)
#[async_trait]
pub trait Store: Send + Sync + 'static {
    fn backend(&self) -> Backend;                       // Sqlite | Postgres
    fn read(&self) -> &dyn Repos;                       // autocommit, read pool
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError>;
    async fn subscribe(&self, channel: Channel) -> Result<Option<Box<dyn Subscription>>, StoreError>; // M1.5, 04 §4; SQLite: None
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError>; // M1.5, 04 §7
    async fn migration_status(&self, binary: &semver::Version) -> Result<MigrationStatus, StoreError>;
    async fn migrate_up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError>;
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError>;
    async fn verify_migrations(&self) -> Result<(), StoreError>;
    async fn snapshot(&self, dest: &Path) -> Result<(), StoreError>; // SQLite only; Postgres => Unsupported
    async fn close(&self); // M1c-2: waits for checked-out connections, closes the pools; later calls fail
}

#[async_trait]
pub trait Tx: Repos + Send {
    async fn commit(self: Box<Self>) -> Result<(), StoreError>;
    async fn rollback(self: Box<Self>) -> Result<(), StoreError>;
    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError>; // savepoint
}

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
    fn comments(&self) -> &dyn CommentRepo;          // M2a-2b
    fn idempotency(&self) -> &dyn IdempotencyRepo;   // M2a-2b
}
```

The auth repositories exist since M2a-1 (migration `0005_auth`; domain types in
`ferrite_domain::auth`, session constants `SESSION_IDLE` 7 d, `SESSION_ABSOLUTE` 30 d,
`SESSION_TOUCH_EVERY` 5 min):

- **`SessionRepo`** (`sessions()`): `insert`, `get(id_hash)`, `touch(id_hash, at) -> bool`
  (moves `last_seen_at` forward only), `delete`, `delete_for_user(user, keep: Option<id_hash>)
  -> u64`, `list_for_user` (at most `MAX_AUTH_LIST` = 100, newest first, `created_at DESC, id_hash DESC`), `prune(now,
  idle_before, limit)` (deletes `expires_at < now OR last_seen_at < idle_before`, R5 batches).
- **`ApiTokenRepo`** (`api_tokens()`): `insert` (duplicate hash: `api_tokens_hash_uq`),
  `get_by_hash`, `list_for_user` (at most 100, by `created_at`), `count_for_user`,
  `touch(id, at) -> bool` (`last_used_at`, forward only), `delete(user, id) -> bool` (only the
  owner's row), `delete_for_user -> u64`, `prune(now, limit)` (deletes tokens with
  `expires_at < now`; tokens without an expiry stay; used by `sessions.prune`).
- **`RateLimitRepo`** (`rate_limits()`, P5): `hit(bucket, window) -> RateHit { count,
  reset_at, now }` is one upsert on both backends (`ON CONFLICT (bucket) DO UPDATE SET count =
  CASE WHEN excluded.window_start > rate_limits.window_start THEN 1 ELSE rate_limits.count + 1
  END, window_start = GREATEST(rate_limits.window_start, excluded.window_start) RETURNING …`;
  SQLite `max(…)`), so concurrent hits on any node never share a count, and a late hit stamped
  in an older window counts in the stored window instead of rolling it back (ruling R7).
  Callers must call it outside a `Tx` (Postgres `now()` is the transaction's start there, and
  the bucket's row lock would be held until commit). `window_start` is the database clock floored to the window since the epoch
  (Postgres `date_bin` over `now()`; SQLite the process clock), `reset_at = window_start +
  window`, `now` is the clock the hit used (F6: `Retry-After = reset_at − now` on any node).
  Buckets are 1 to `MAX_RATE_BUCKET` (200) characters, windows up to `MAX_RATE_WINDOW` (1 day).
  `prune(window_before, limit)`.
- **`AuditRepo`** (`audit()`, P22): `append(&NewAudit) -> AuditId` (UUIDv7 id, `at` = database
  clock, `node` NULL; `details` must be a JSON object, else `Invalid` before any SQL) and
  `list(limit, cursor)`, newest first (`at DESC, id DESC`), cursor tag `a:at`
  (`AUDIT_CURSOR_TAG`).

Other M2a-1 store additions, on both backends and contract-tested:

- **`UserRepo`**: `get_for_update(id)` (Postgres `FOR UPDATE`, SQLite a plain read on the
  writer; every user read-modify-write uses it inside its `Tx`, because `update` is
  unversioned, F5), `set_totp(id, secret: Option<&Secret>)` (sets or clears the sealed
  secret and resets `totp_last_step` in one statement; `update` writes neither TOTP
  column, F4), `advance_totp_step(id, step) -> bool` (one conditional `UPDATE`; `false` for
  a replayed or older step or an unknown user, P9), `delete(id)` (sessions and tokens
  cascade, audit rows keep a NULL actor, authored content or revisions are `ForeignKey`),
  `count_active_admins()`.
- **`ContentRepo::reassign_author(from, to)`** and **`RevisionRepo::reassign_author(from,
  to)`**: one `UPDATE` each, returning the count; content gets a `version` bump but keeps
  `updated_at` (ruling R8). `UserService::delete` runs both in the deleting `Tx`.
- **`ContentTypeRepo::insert`** and **`TaxonomyRepo::insert`** (a taken name is
  `UniqueViolation` on `content_types_pkey` / `taxonomies_pkey`) next to the old `upsert`,
  and **`TaxonomyRepo::delete(name)`** (built-in: `Invalid`; with terms: `ForeignKey`).
  Type and taxonomy writes in core run under the named lock `types` (ruling R19), which
  `permalink.*` option writes take too.
- **`ContentQuery::readable_by` / `SearchQuery::readable_by`** (`Option<ReadableBy { user,
  now }>`, R17/R23): in SQL, `(status = 'published' AND published_at <= now) OR author_id =
  user` on both backends, so admin lists, counts, cursors and search pages hold only items
  the reader may see. A NULL `published_at` never matches the first branch.

M2a-2b added (migration `0006_media_comments`, contract-tested on both backends):

- **`CommentRepo`** (`comments()`): `insert` (NUL refused; `author_email` stored through
  `email_key`, ASCII-lowercased), `get`, `set_status(id, status) -> Option<Comment>` (one
  `UPDATE … RETURNING`), `delete(id) -> bool` (replies cascade), `list(&CommentQuery {
  status, item, limit, cursor })` newest first (`created_at DESC, id DESC`, cursor tag
  `cm:created` = `COMMENT_CURSOR_TAG`; only the given filters are in the SQL, so each
  shape has its index), the public thread pages (M2b-2 P34; one `WITH RECURSIVE`
  query each, on both backends): `approved_page(item, offset, roots, max_rows)` (roots — approved
  comments whose parent is null or not approved — `offset..offset + roots` in
  `(created_at, id)` order, `roots ≤ MAX_PAGE_ROOTS = 500`, each followed by its approved
  descendants depth first; at most `max_rows ≤ MAX_PAGE_ROWS = 2000` rows, where every
  root is kept and only replies past the budget are cut, in thread order),
  `approved_totals(item) -> { comments, roots }` and `root_position(item, comment)`, and the R2 `known` checks (F19): `has_approved_from(email,
  name)` (an approved anonymous row, `user_id IS NULL` and not `by_account`, e-mail by
  `email_key`, name exact)
  and `has_approved_from_user(user)`. No CHECK on `status` (P2); core validates.
- **`IdempotencyRepo`** (`idempotency()`, P10, F5): `begin(&IdempotencyKey { user,
  key_hash, method, path }, stale_after) -> IdemBegin` — one insert-if-absent; on conflict
  another method or path is `Mismatch`, a finished row `Replay { status, response }`, a
  pending row younger than `stale_after` `InProgress`, an older one is taken over by one
  conditional `UPDATE … WHERE status_code IS NULL AND created_at = <read>` (a losing taker
  re-reads, so concurrent takers on any node get `Started` once).
  `Started { token }` carries the row's `created_at` (database clock); `finish(user,
  key_hash, token, status, &response) -> bool` and `release(user, key_hash, token) -> bool`
  act only on the pending row still carrying that token, so a taken-over original can
  neither record a second result nor free the new owner's key. Stored responses are at most
  `MAX_IDEMPOTENCY_RESPONSE` (64 KiB) of JSON (`Invalid` otherwise); `prune(created_before,
  limit)` (R5 batches; `IDEMPOTENCY_RETENTION` 24 h and `IDEMPOTENCY_STALE` 1 h live in
  `ferrite_domain`, F7).
- **`MediaRepo`** reads and writes `media.sanitized` (`insert`, `update`, `get`,
  `get_many`, `find_by_artifact`).
- The SQLite claim statements (`PRECHECK_SQL`, `REAPER_SQL`, the claim) pin their partial
  indexes with `INDEXED BY` (the claim as a `UNION ALL` of its two branches): without
  statistics the planner otherwise prefers the equality on `jobs_status_created_idx` and
  walks every queued or running row.

M2b-1 added (migration `0007_autosaves`, contract-tested on both backends):

- **`AutosaveRepo`** (`autosaves()`, M2b P15, F11): `put(&Autosave)` inserts or replaces
  the row of (`item_id`, `user_id`) (NUL in a text or the JSON is `Invalid`, an unknown
  item or user `ForeignKey`), `get(item, user) -> Option<Autosave>`, `delete(item, user)
  -> bool`. `Autosave { item_id, user_id, base_version, title, excerpt?, slug?,
  parent_id?, menu_order?, terms?, body, fields, saved_at }`; `terms` is stored as a JSON
  array of term ids. Core's `ContentService::update` deletes the saver's own row in its
  transaction. M2b-2 added `seo?` (migration `0008_autosave_seo`): the editor's SEO patch
  as JSON, stored as given (NUL is `Invalid`), `None` = unchanged.

`JobRepo` (`jobs()`) and
`NodeRepo` (`nodes()`) exist since M1.5 (migration `0004_cluster`); see the cluster table
in §4. Named locks are `Store::try_lock` (Postgres advisory locks, SQLite in-process;
[04 §7](04-cluster.md#7-distributed-locks)); `ferrite_store::lock(store, name, wait)`
retries `try_lock` every 250 ms until `wait`, then `Busy`. Notifications are
`Store::subscribe(Channel::{Events, Jobs})` (Postgres `LISTEN` on a dedicated connection;
SQLite `None`, callers poll; [04 §4](04-cluster.md#4-event-bus--cache-invalidation)).
M1.5 also added `EventRepo::min_seq` (lost-history check, P3), batched prunes (`prune(…,
limit)` with at most `MAX_PRUNE_BATCH` = 10 000 rows per call on events, jobs, nodes and
plugin failures, R5), `PluginFailureRepo` (`plugin_failures()`: `add`/`sum_since`/`clear`/
`prune`, P20), `PluginRepo::auto_disable` (conditional on `enabled` and the observed
`updated_at`, R12) and `PluginRepo::get_for_update`. M2a-2a added (no migration, M2a-2
P2; contract-tested on both backends) `OptionRepo::list_prefix(prefix, after, limit)`
(literal prefix, paged by key), `UserRepo::list_active_admins(limit)`,
`PluginRepo::is_enabled(name)` (one column, the per-request plugin-route check),
`PluginRepo::action_subscribers(hook)` (enabled rows whose manifest `hooks` lists the
hook, filtered in SQL) and `PluginKvRepo::delete_all(plugin, limit)` (batched purge).
The `user_meta` table already exists in `0001_init` but has no repository yet.

**Query and batch API (M1c-1)**, on both backends and contract-tested
(`ferrite-store-tests`). Limits are constants exported by `ferrite-store`
(`src/paging.rs`):

- **`ContentQuery` filters** (`ferrite-domain/src/content.rs`): `kind`, `kinds` (`type IN
  (…)`; empty: no filter), `statuses` (empty:
  any), `parent` (`Any` | `Root` | `Of(id)`), `term`, `author`, `ids`, `exclude`,
  `published_after` (`>=`) and `published_before` (`<`), where items without a publish
  time never match either bound, plus `order`, `limit` and `cursor`. `ids` and `exclude` hold
  at most `MAX_ID_FILTER` (200) entries (`Invalid` beyond); `kinds` at most
  `MAX_CONTENT_KINDS` (20), and `kind` together with a non-empty `kinds` is `Invalid` (M2a-2b
  P25; term archives list every type of their taxonomy). Both apply to `list` and `count`.
- **R3: an empty `ids` (or `exclude`) means no filter, not "match nothing".** A caller that
  computed an empty id set must short-circuit instead of calling `list`/`count`, or it lists
  everything. No core service builds an `ids` query yet; the rule is stated in the
  `ContentRepo::list` and `ContentQuery::ids` doc comments. `SiteReader::related` (M1c-2)
  uses `exclude`, which always holds at least the item itself.
- **`ContentRepo::count(&query)`** uses the same WHERE clause as `list` and ignores `limit`,
  `cursor` and `order`.
- **`get_many(ids)`** on content, users and media: at most 200 ids, one query, result in
  the order of `ids`, missing ids skipped, a repeated id returned once at its first
  position, more than 200 is `Invalid`. `MetaRepo::get_many(ids, keys)` takes at most 200
  ids and `MAX_META_KEYS` (50) keys and returns `(id, key, value)` ordered by the id's
  position, then key in binary order. `TermRepo::for_items(items)` returns a map in which
  **items without terms are absent**.
- **`ContentRepo::slugs_with_prefix(kind, parent, prefix)`** returns slugs equal to
  `prefix` or starting with `prefix-`, case-sensitive, in binary order, at most
  `MAX_SLUG_PREFIX_RESULTS` (1000). It uses the binary range `slug >= 'prefix-' AND slug <
  'prefix.'` (`.` is the byte after `-`) rather than `LIKE`, because SQLite's `LIKE` folds
  ASCII case. It also returns slugs such as `hello-world` for `hello`, so numeric-suffix
  allocation (`hello-2`) must filter the result (`ferrite-core/src/slug.rs`,
  `next_free`, which also keeps the result within 200 characters, ruling R7).
- **`SearchRepo::search(&SearchQuery)`** returns a `Page<ContentId>` filtered by `kinds`
  (at most `MAX_SEARCH_KINDS` = 50; empty: all) and `statuses` (empty: all; duplicates
  ignored); `limit` is 1 through 200. Best first, ties by item id descending. The old
  unfiltered `SearchRepo::query` is gone.
- **Tagged keyset cursors.** Every cursor is `ferrite_domain::Cursor::encode(tag, key, id)`
  and decoding checks the tag, so a cursor from another listing or another sort order is
  `Invalid` instead of being misread. Tags: `c:published`, `c:updated`, `c:menu`,
  `c:title` (content, one per `ContentOrder`), `s:rank` (search; the float rank travels
  as its IEEE-754 bits), `r:source` (redirects), `u:login` (users).
- **`PluginKvRepo::set_with_quota(plugin, key, value, max_bytes, max_keys) -> bool`**
  writes only if, after the write, the plugin's value bytes are at most `max_bytes` and
  its key count at most `max_keys` (an overwrite counts the new size only); otherwise
  nothing is written and it returns `false`. It is one statement on SQLite and takes a
  per-plugin advisory lock on Postgres (held until the `Tx` ends, so keep such `Tx`
  short and touch several plugins in a consistent order to avoid a deadlock that would
  poison the `Tx`). Plain `set` bypasses quotas; mixing the two for one plugin voids the
  guarantee.
- **`MediaRepo::find_by_artifact(hash, limit)`** (M1c-2) returns up to `limit` (1 through
  200) media rows whose original (`artifact_hash`) or any variant (`variants.<name>.hash`) is
  `hash`, ordered by item id; the `/_media` handler uses it to decide whether a file may be
  served ([03 §5](03-media.md#5-serving)). It is indexed throughout (ruling R29): nothing is
  read from `media` unless the `artifacts` row of `hash` (primary key) exists with kind
  `media` or `variant`; then the original matches through `media_artifact_idx` and variants
  through `media_variants_hash_idx`. A contract test checks the query plans on both
  backends.
- **`RedirectRepo::get(id)`** (M1c-2) fetches one redirect row; `RedirectService::delete`
  needs its source path for the `redirect:<source>` event.
- **`Store::close()`** (M1c-2) waits until every checked-out connection is back (so a dropped
  `Tx` finishes its rollback), then closes the pools; every later call fails. SQLite closes
  the readers first so the writer's close checkpoints the WAL. Idempotent. `ferrite serve`
  calls it at shutdown with a 2 s bound.
- **`RedirectRepo::list_by_target(target)`**: redirects whose `target` equals `target`
  exactly (case-sensitive), ordered by `source_path` in binary order, at most
  `MAX_REDIRECTS_BY_TARGET` (1000). Core uses it to re-target redirects when a
  permalink changes (ruling R1).
- **`MigrationStatus.pending: Vec<PendingMigration>`** (was a list of versions):
  `PendingMigration { version, description, directives }` with the parsed header
  `Directives { phase, min_binary, lossy_down }`, oldest first. `ferrite migrate status`
  and the startup check (`ferrite/src/open.rs`, `decide`) read the phase from it to refuse
  contract migrations on Postgres unless `database.allow_contract` is set.

**Transaction rules** (both backends, contract-tested):

- **Poisoning (R18).** Any database error inside a `Tx`, including `UniqueViolation`
  and `ForeignKey`, poisons it: every later call returns `StoreError::Aborted` and
  `commit` rolls back and returns `Err(Aborted)`. Postgres aborts the transaction on any
  statement error and SQLite can roll one back on its own (`SQLITE_FULL`, I/O errors),
  so this is the rule both engines can keep. Validation errors (`Invalid`, raised before
  any SQL is sent) do not poison. To try something and fall back, run it in
  `tx.begin_nested()`: a nested transaction is a savepoint, and its failure poisons only
  the nested level.
- **Nested transactions and the outer handle (`NestedOpen`).** While a nested `Tx` is
  open, every call through the outer `Tx` returns `StoreError::NestedOpen` (it does not
  poison either level). A nested `Tx` that is dropped without `commit`/`rollback` is rolled
  back to its savepoint before the next statement. **Ending the outer `Tx` (drop, `commit`
  or `rollback`) while a nested handle is still alive ends the whole transaction**: the
  outer `commit` rolls back and returns `Err(NestedOpen)`, and the surviving nested handle
  then fails every call, and its own `commit`, with `Aborted`. The connection is released
  no later than when the last handle drops (`exec.rs` in each store crate; the unit
  tests `lazy_release_after_outer_drop` (SQLite and Postgres `exec.rs`) cover an outer handle dropped while a nested
  statement is still running).
- **No self-deadlock (I2, ruling P1).** Never write through `Store::read()` or call
  `Store::begin()` while the same task holds a `Tx`; pass the `Tx` on as `&dyn Repos`.
  On SQLite both wait for the single writer connection the `Tx` holds; the writer pool
  gives up after 5 s with `StoreError::Busy` (any pool acquire timeout is `Busy` on both
  backends). **Fail-fast detection is deliberately not implemented**: the 5 s `Busy` plus
  this rule stands. Core services follow it: they never call plugins or other slow I/O
  while holding a `Tx`, and pass `&dyn Repos` down instead of calling `read()`/`begin()`
  inside one.
- **Isolation policy (ruling P2).** Isolation stays at each backend's default: SQLite
  serializes all writers (one writer connection, so a `Tx` never sees another writer's
  concurrent changes); Postgres runs `READ COMMITTED`, so two concurrent `Tx` can
  interleave. Code must therefore not rely on SQLite's stronger behaviour. Correctness
  under concurrency comes from three things only:
  1. optimistic `version` on content (`ContentRepo::update` returns `Conflict`);
  2. unique constraints plus retry (slug allocation retries on `content_items_slug_uq`,
     `ferrite-core/src/content.rs`);
  3. single-statement atomic store operations for read-modify-write, today
     `PluginKvRepo::set_with_quota`.

  Checks that read and then write in separate statements (the parent-cycle walk for
  content and terms, the redirect-loop check) are not serialized on Postgres; the core docs
  say so (`taxonomy.rs`, `redirects.rs`).

Implementation notes (M1a, `ferrite-store-sqlite` and `ferrite-store-postgres`):

- Each backend's `exec.rs` defines `Exec` (`Pool` or `Tx`) and two macros every repo
  method goes through. `run!(&self.exec, |e| query...)` runs one statement on the pool
  or on the open transaction's connection and maps the driver error to `StoreError`
  (unique and foreign-key violations become typed errors). `atomic!(&self.exec, |conn|
  async body)` runs a multi-statement operation atomically: on a pool it opens and commits
  its own transaction; inside a caller's `Tx` it opens a **savepoint**, so a failure rolls
  back only that operation and leaves the outer transaction usable (R11). Replace-style
  operations (term links per item, menu trees) use it.
- SQLite `SqliteRepos` holds a reader and a writer `Exec`: SELECTs on the autocommit path
  go to the **reader** pool, which is opened `read_only(true)`; every mutation goes to
  the single-connection **writer** pool; a `Tx` uses its own writer transaction (R6).
- Callers of `Store::read()` get autocommit repos; mutations through them still work and
  run on the writer.

Notes:

- `async_trait` is used because the traits are used as trait objects. The per-call box
  is negligible next to a DB round trip.
- Transactions are a unit of work: a `Tx` exposes the same repos bound to one
  connection. Core services take `&dyn Repos`, so the same service code runs inside or
  outside a transaction.
- Repos return `ferrite-domain` types, never sqlx rows.
- Parity rules enforced in the store layer so both backends accept and return the same
  things (`ferrite-store/src/{text,json}.rs`):
  - JSON values containing a NUL character and text containing `\0` are rejected with
    `StoreError::Invalid` on both backends (Postgres `jsonb`/`text` cannot store NUL).
    The check runs on every caller-supplied text bind: writes, lookups, deletes and
    decoded cursors (R7, R13, R15).
  - Text key and slug orderings are byte-wise: Postgres declares key, slug, name, login
    and title columns `COLLATE "C"` (M3) to match SQLite's `BINARY` collation, so their
    indexes serve the ORDER BY (some queries also keep an explicit `COLLATE "C"`, R8).
    Admin lists are therefore not locale-aware.
  - Constraint errors are backend-neutral (I1): `UniqueViolation { constraint }` carries
    the same name everywhere. Both `0001_init` files name every primary key
    `<table>_pkey`, every unique index `<table>_<what>_uq` and every foreign key
    `<table>_<col>_fk`; the SQLite backend maps its `UNIQUE constraint failed: t.a, t.b`
    message to that name with a static table (`ferrite-store-sqlite/src/constraint.rs`,
    unit-tested for completeness). `ForeignKey { constraint }` is always `""`, because
    SQLite does not report which foreign key failed.
  - Missing rows: lookups return `None`; `update`, `hit`, `set_enabled` and `activate`
    return `NotFound`; deletes are idempotent (`Ok`).
  - Content listings (I3) use one index per `ContentOrder`, with columns and directions
    exactly as its ORDER BY: `(type, status, published_at DESC [NULLS LAST], id DESC)`,
    `(type, updated_at DESC, id DESC)`, `(type, menu_order, id)`, `(type, title, id)`.
    Keyset predicates are row comparisons (`(published_at, id) < ($k, $id)`), and the
    `published_at IS NULL` group of `PublishedDesc` is a separate query that runs only
    when the non-NULL part does not fill the page (`ferrite_store::content_list_plan`),
    so every page is an index range scan on both engines.
  - E-mail uniqueness uses `users.email_key`, the address ASCII-lowercased in Rust
    (`email_key`) under a plain unique index on both backends (R12). Non-ASCII case
    variants (`É` vs `é`) are distinct accounts.
  - Foreign keys on `content_items.parent_id` and `terms.parent_id` are `RESTRICT`
    (no `ON DELETE` clause): deleting a parent that still has children fails with
    `StoreError::ForeignKey`; reparenting or deleting children is the core's decision (R9).
  - `content_items.author_id` is non-optional, so an importer must supply a fallback
    author (M3).
- Event emission (cache invalidation, [04](04-cluster.md)) happens **inside the same
  transaction** as the write (transactional outbox), so a committed write always has
  its event.

## 4. Logical schema

Same logical schema on both backends. Physical type mapping is in §5.
Tables marked *not yet created* are planned; the milestone in the mark adds them in a
new migration. Everything else exists in `0001_init` / `0002_search` / `0003_media_variants` /
`0004_cluster` / `0005_auth` / `0006_media_comments` / `0007_autosaves` / `0008_autosave_seo`
on both backends.

### Content

| Table | Columns (key ones) | Notes |
|---|---|---|
| `content_types` | `name` PK, `label`, `hierarchical`, `has_archive`, `fields` (json schema), `built_in`, `created_at` | `post`, `page`, `media` are built in |
| `content_items` | `id` PK, `type` FK, `status`, `slug`, `parent_id`, `author_id`, `title`, `excerpt`, `body` (json blocks), `body_html` (cached render), `fields` (json, custom fields), `menu_order`, `published_at`, `scheduled_at`, `created_at`, `updated_at`, `version` | `UNIQUE(type, parent_id, slug)` with NULL-parent handling; `status ∈ {draft, pending, scheduled, published, private, trash}`; `version` for optimistic locking |
| `content_meta` | `item_id` FK, `key`, `value` (json), PK(`item_id`,`key`) | Namespaced keys (`seo.title`, `plugin.<name>.*`) — plugin write scopes match on these. `seo.title` / `seo.description` (JSON strings) are written by content writes in the item's transaction (M2b P28) and read by the head |
| `revisions` | `id` PK, `item_id` FK, `author_id`, `created_at`, `title`, `excerpt`, `body`, `fields`, `meta` (json snapshot), `note` | Written on every save; retention configurable (default: keep 50 per item) |
| `autosaves` *(since `0007_autosaves`)* | `item_id` → `content_items` `ON DELETE CASCADE` (`autosaves_item_id_fk`), `user_id` → `users` `ON DELETE CASCADE` (`autosaves_user_id_fk`), `base_version` bigint, `title`, `excerpt` null (= derive), `slug` null, `parent_id` null (no FK; the working copy's parent, `null` = top level, R4), `menu_order` int null, `terms` json array null (`null` = unchanged for `slug`, `menu_order`, `terms` and `seo`, F11), `body` json, `fields` json, `seo` json null *(since `0008_autosave_seo`: the editor's SEO patch `{title?, description?}` as sent)*, `saved_at`; PK (`item_id`, `user_id`) `autosaves_pkey`; index `autosaves_user_idx (user_id)` (users FK on delete) | The admin's per-user working copy (M2b P15): never a revision; the user's next real save of the item deletes it; at most one row per (item, user), so no retention job; the `0007` down drops it (lossy), the `0008` down drops `seo` (lossy) |
| `taxonomies` | `name` PK, `label`, `hierarchical`, `content_types` (json list), `built_in` | `category`, `tag` built in |
| `terms` | `id` PK, `taxonomy` FK, `slug`, `name`, `description`, `parent_id`, `UNIQUE(taxonomy, slug)` | |
| `term_links` | `item_id` FK, `term_id` FK, `position`, PK(`item_id`,`term_id`) | |
| `menus` / `menu_items` | menu: `name` PK, `label`; item: `id`, `menu`, `parent_id`, `position`, `kind` (`item`/`term`/`url`), `target_id`, `url`, `label` | |
| `media` | `item_id` PK/FK → `content_items` (type `media`), `artifact_hash` FK, `mime`, `bytes`, `width`, `height`, `alt`, `original_name`, `variants` (json: name → hash, w, h, mime), `sanitized` boolean not null default false (since `0006_media_comments`; SQLite `INTEGER CHECK (sanitized IN (0, 1))`) | `sanitized`: the stored bytes are the SVG sanitiser's output, so the file may be served inline (M2a-2b P7); the `0006` down drops it (lossy) |
| `media_variants` | `hash`, `item_id` FK → `media` (`ON DELETE CASCADE`), `name`, PK(`item_id`,`name`), index on `hash` | Added by `0003_media_variants` (ruling R29). Derived from `media.variants` (one row per entry whose value is an object with a string `hash`), backfilled by the migration and kept in sync by `MediaRepo::insert`/`update` in the same transaction; `media.variants` stays the source of truth |
| `comments` | `id`, `item_id` → `content_items` `CASCADE`, `parent_id` → `comments` `CASCADE`, `user_id` null → `users` `SET NULL`, `author_name`, `author_email` (ASCII-lowercased), `author_url` (`""` = none), `body` (plain text), `status` (`pending`/`approved`/`spam`/`trash`, no CHECK, P2), `ip_hash` null (keyed), `created_at`, `by_account` boolean not null default false (since `0006`; SQLite `INTEGER CHECK (by_account IN (0, 1))`; set on insert when `user_id` is given and kept after the user is deleted, so such a comment never counts for a guest in `has_approved_from`; the `0006` down drops it, lossy; `0006` does not backfill it: before `0006` no code path wrote comments (no `INSERT INTO comments` existed), so there are no rows to mark. Every later insert path, e.g. the M3 WXR importer, must set it whenever `user_id` is set, as `CommentRepo::insert` does); indexes `comments_item_created_idx (item_id, created_at)` (`0001`) and since `0006`: `comments_status_created_idx (status, created_at, id)` and `comments_created_idx (created_at, id)` (moderation queue), `comments_item_status_idx (item_id, status, created_at, id)` (an item's approved thread), `comments_author_email_idx (author_email, status)` and `comments_user_idx (user_id, status)` (`known` checks, F19; also the users FK on delete) | `CommentRepo` since M2a-2b |
| `redirects` | `id`, `source_path` UNIQUE, `target`, `code` (301/302/307/308/410), `hits`, `origin` (`manual`/`import`/`slug-change`), `created_at`, `last_hit_at` | Auto-created on slug change of a published item, and of a term with a public archive (06 §3) |

### Users & auth

| Table | Columns | Notes |
|---|---|---|
| `users` | `id`, `login` UNIQUE, `email` UNIQUE (case-insensitive), `display_name`, `password_hash` (argon2id, nullable), `role`, `status`, `must_reset`, `totp_secret` (encrypted, nullable), `created_at`, `last_login_at`, `totp_last_step` bigint null (since `0005_auth`) | `totp_last_step`: last accepted TOTP step, refuses replays (P9) |
| `user_meta` | `user_id`, `key`, `value` json | |
| `sessions` *(since `0005_auth`)* | `id_hash` PK (`sessions_pkey`, SHA-256 hex of the cookie token), `user_id` → `users` `ON DELETE CASCADE` (`sessions_user_id_fk`), `csrf_secret`, `created_at`, `last_seen_at`, `expires_at` (absolute end), `ip_hash` null, `user_agent` null; indexes `sessions_user_idx`, `sessions_expires_idx`, `sessions_last_seen_idx` | DB-backed so any node can serve any request; no per-node cache (P7) |
| `api_tokens` *(since `0005_auth`)* | `id` PK (`api_tokens_pkey`), `user_id` → `users` `ON DELETE CASCADE` (`api_tokens_user_id_fk`), `name`, `token_hash` UNIQUE (`api_tokens_hash_uq`, SHA-256 hex), `scopes` json array, `created_at`, `last_used_at` null, `expires_at` null; index `api_tokens_user_idx` | |
| `passkeys` *(not yet created: deferred past M2, P3)* | `id`, `user_id`, `credential` json, `created_at`, `last_used_at` | webauthn-rs has only a pre-release |
| `rate_limits` *(since `0005_auth`)* | `bucket` PK (`rate_limits_pkey`), `window_start`, `count` int default 0; index `rate_limits_window_idx` | Fixed-window counters on the database clock; works across nodes (P5) |
| `idempotency_keys` *(since `0006_media_comments`)* | `user_id` → `users` `ON DELETE CASCADE` (`idempotency_keys_user_id_fk`), `key_hash` (SHA-256 hex of the key), `method`, `path`, `status_code` int null (NULL = pending), `response` json null (Postgres `jsonb`, SQLite `TEXT CHECK (json_valid)`, F4), `created_at`; PK (`user_id`, `key_hash`) `idempotency_keys_pkey`; CHECK both-or-neither of `status_code` / `response`; index `idempotency_keys_created_idx (created_at)` | `Idempotency-Key` on `POST /media` (M2a-2b P10); pruned after 24 h by `sessions.prune`; the `0006` down drops it (lossy) |
| `audit_log` *(since `0005_auth`)* | `id` PK (`audit_log_pkey`), `at`, `actor_id` null → `users` `ON DELETE SET NULL` (`audit_log_actor_id_fk`), `action`, `target_kind`, `target_id` null, `details` json object default `{}`, `node` null; indexes `audit_log_at_idx (at, id)`, `audit_log_actor_idx (actor_id)` (user delete, R5) | Admin actions, plugin installs, capability grants; no secrets (P22) |

### Site, plugins, themes, artifacts

| Table | Columns | Notes |
|---|---|---|
| `options` | `key` PK, `value` json, `autoload`, `updated_at` | Site settings; `autoload` marks the set every render reads (F23). No per-node cache: it was removed with no reader (M2a-1 R26), and M2a-2a's readers (theme and plugin settings) read the store directly (M2a-2 P14; a cache is M4) |
| `artifacts` | `hash` PK (sha256 hex), `kind` (`media`/`variant`/`plugin`/`theme`/`export`), `bytes`, `mime`, `created_at` | The blob lives in the MediaStore under `artifacts/<hash[0..2]>/<hash>` |
| `plugins` | `name` PK, `version`, `artifact_hash` FK, `manifest` json, `granted` json (the approved grant: since M2a-2a `{"v":1,"capabilities","limits","routes"}`, 05 §2; the M1c-2 bare `[capabilities]` table and `null` are still read), `enabled`, `disabled_reason`, `installed_at`, `updated_at` | |
| `plugin_kv` | `plugin`, `key`, `value` bytes, `updated_at`, PK(`plugin`,`key`) | Quotas enforced in core (default 10 MB, 10k keys per plugin) |
| `plugin_failures` | `plugin`, `hook`, `bucket_start`, `count`, PK(`plugin`,`hook`,`bucket_start`) | Shared failure counters → auto-disable cluster-wide (`PluginFailureRepo` since M1.5: 1-minute buckets from the database clock, pruned by `plugin_failures.prune` after `max(24 h, 2 × plugins.failure_window)`; 05 §8) |
| `themes` | `name` PK, `version`, `artifact_hash`, `manifest` json, `active`, `installed_at` | Exactly one active (partial unique index / trigger) |

### Cluster

| Table | Columns | Notes |
|---|---|---|
| `nodes` *(since `0004_cluster`)* | `id` PK (`nodes_pkey`), `name`, `version`, `schema_version` bigint, `started_at`, `heartbeat_at` (index `nodes_heartbeat_idx`), `meta` json object default `{}` | `NodeRepo`: `register`/`heartbeat`/`deregister`/`list` (≤ 1000, by `started_at`, `id`)/`db_now`/`prune`, times from the database clock. Stale after 3× heartbeat interval |
| `events` | `seq` PK (autoincrement / bigserial), `kind`, `keys` json, `origin_node`, `created_at` | Transactional outbox for invalidation; pruned after 24 h by `events.prune`, except the newest row (R9). On Postgres `append` notifies `ferrite_events[_<schema>]` in the same statement (P2) |
| `jobs` *(since `0004_cluster`)* | `id` PK (`jobs_pkey`), `kind`, `payload` json (an object), `unique_key` null (`jobs_unique_key_uq`, NULLs distinct), `run_at`, `priority` int default 0, `status` (`queued`/`running`/`done`/`dead`, CHECK; no `failed`, P8), `attempts` int default 0, `max_attempts` int default 5 (≥ 1), `locked_by` (node id text), `locked_until`, `last_error` (≤ 1024 bytes), `created_at`, `finished_at`; indexes `jobs_finished_idx (finished_at)`, `jobs_lease_idx (locked_until) WHERE status = 'running'` (expired leases: reaper, SQLite pre-check), and for due jobs Postgres `jobs_claim_idx (priority DESC, run_at, id) WHERE status IN ('queued','running')` (the claim's ORDER BY, scanned up to LIMIT; R1; since M3-2 the Postgres claim is a `UNION ALL` of the due branch on this index and the expired-lease branch on `jobs_lease_idx`, P31) / SQLite `jobs_due_idx (run_at) WHERE status = 'queued'` (no ORDER BY index: SQLite's planner does not use one for the claim after ANALYZE; since `0006` the SQLite claim is a `UNION ALL` of the due branch on `jobs_due_idx` and the expired-lease branch on `jobs_lease_idx`, each named with `INDEXED BY`, sorted by `priority DESC, run_at, id` and limited across the union; the pre-check and the reaper name their index the same way, and the prune names `jobs_finished_idx`), and since `0006_media_comments` `jobs_status_created_idx (status, created_at, id)`, `jobs_created_idx (created_at, id)` and `jobs_kind_created_idx (kind, created_at, id)` (the admin jobs list by status, by kind or all, F26) | `JobRepo`: claim by kind with lease (Postgres `FOR UPDATE SKIP LOCKED` per `UNION ALL` branch in a `MATERIALIZED` CTE; SQLite on the single writer, reader-pool `EXISTS` pre-check), expired lease at `max_attempts` → `dead` (P7), lease-holder writes conditional on `(id, node, attempts)` (P6); a counted `release` of the final attempt goes `dead` ("released after final attempt"). See [04](04-cluster.md#job-queue) |
| `schema_meta` | `key` PK, `value` | `min_binary_version`, `created_by_version`, `site_id` |

### Search

Behind `SearchRepo` so the SQL difference is contained. **Rust owns all text
normalization** (`ferrite-store/src/search.rs`, R16, R17): indexed text and queries both
go through the same tokenizer, and the engines only ever see opaque tokens, so both
backends match exactly the same documents. No database text-search parser, configuration
or locale is involved (the earlier idea of deriving the FTS config from `site.locale` is
dropped).

- **Tokenization**: Unicode NFKC, lowercase, split on everything that is neither
  alphanumeric nor a combining mark (marks stay with their base, so Hindi and similar
  scripts survive); pieces with no letter or digit are dropped. Search is
  accent-sensitive and has no stemming or stopwords ("simple" semantics).
- **Query**: whitespace/punctuation-separated terms, each matched as a **prefix**, all
  required (AND). No query operators; user input is never interpreted as syntax. An empty
  term list matches nothing without touching the database.
- **Caps**: 64 chars per term, 32 distinct query terms, 4096 distinct indexed terms per
  item, 512 KiB of distinct term bytes, 256 positions per term, token positions up to
  16383 (Postgres tsvector limits).
- **SQLite** (`0002_search.up.sql`): `search_docs(id, item_id UNIQUE → content_items ON
  DELETE CASCADE, title, body_text)` holds the space-joined tokens; `search_fts` is an
  **external-content FTS5** table over it with `tokenize='ascii'` (FTS5 only splits on the
  spaces), kept in sync by triggers. Ranking: `bm25(search_fts, 10.0, 1.0)`.
- **Postgres** (`0002_search.up.sql`): `search_docs(item_id PK → content_items ON DELETE
  CASCADE, tsv tsvector)` with a GIN index. The repo writes `tsv` as an explicit
  `::tsvector` literal (title positions weighted `A`, body `B`) and queries with an
  explicit `::tsquery`; ranking is `ts_rank`.
- Known gap (deferred): case folding is Rust's `to_lowercase`, not full Unicode case
  folding, so `İstanbul` indexes as `i̇stanbul` and a search for `istanbul` misses it
  (identical on both backends).

Ranking order differs between engines; contract tests assert result *sets*, not exact
order beyond "title match ranks first".

## 5. Physical type mapping

| Logical | SQLite | Postgres | Rust |
|---|---|---|---|
| id | `BLOB` (16 bytes) | `uuid` | `Uuid` (v7, D15) wrapped in typed ids (`PostId`, …) |
| timestamp | `INTEGER` unix **microseconds** UTC | `timestamptz` | `Timestamp` newtype over `chrono::DateTime<Utc>` |
| json | `TEXT` (validated JSON; `json_*` functions available) | `jsonb` | `serde_json::Value` or typed via `sqlx::types::Json<T>` |
| bool | `INTEGER` 0/1 | `boolean` | `bool` |
| bytes | `BLOB` | `bytea` | `Vec<u8>` |
| text, case-insensitive unique | `TEXT` + `<col>_key` column (ASCII-lowercased in Rust) with a plain unique index | same | `String` |
| autoincrement | `INTEGER PRIMARY KEY AUTOINCREMENT` | `bigint GENERATED ALWAYS AS IDENTITY` | `i64` |

UUIDv7 keeps index locality on both backends and lets ids be created in Rust before
insert (useful for imports and the outbox).

## 6. Connection setup

**SQLite** (`ferrite-store-sqlite`):

- `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`,
  `temp_store=MEMORY`; `mmap_size` tuned in M1 benchmarks.
- Two pools: **writer** (max 1 connection) and **reader** (N connections, opened
  `read_only(true)`, so a write routed there fails instead of silently racing). All `Tx`
  and all mutations use the writer. This makes write contention explicit and avoids
  busy-retry loops.
- Data folder lock: `ferrite.lock` with an OS advisory lock (`fs4`/`fd-lock`) held for
  the process lifetime — a second `ferrite serve` on the same folder fails fast.

**Postgres** (`ferrite-store-postgres`):

- One pool; `max_connections` from config; `statement_timeout` 30 s and
  `idle_in_transaction_session_timeout` 60 s (P4) on app connections (migrations use their
  own connection without them).
- Optional `schema` setting (`search_path`) so several sites can share one server; at most
  48 bytes, so the per-schema notification channel names fit Postgres' 63-byte limit.
- `application_name` per connection role (`PgOptions.application_name`, default
  `ferrite`): the pool and migrations use it as is, each `LISTEN` subscription
  `<name>:listen` (no `search_path`, no timeouts).
- TLS via rustls; `sslmode` honoured from the URL.
- Requires PostgreSQL 15 or newer (`UNIQUE NULLS NOT DISTINCT`); `PgStore::open`
  checks `server_version_num` and returns `Unsupported("postgres >= 15 required")`.
- **PgBouncer in transaction mode: not supported yet.** Each pooled connection runs
  session-level `SET search_path`, `SET statement_timeout` and
  `SET idle_in_transaction_session_timeout` once, when it connects.
  With transaction pooling, consecutive transactions run on whichever server connection
  is free, so those settings leak to other clients and are missing on ours. Supporting it
  needs per-transaction `SET LOCAL` (or settings in the connection string / role), a
  direct connection for the event listener ([04](04-cluster.md), e.g. a
  `database.listen_url`) and prepared-statement caching turned off. Session pooling
  works.

## 7. Migrations

### Layout

```
crates/ferrite-store-sqlite/migrations/
    0001_init.up.sql
    0001_init.down.sql
    0002_search.up.sql
    0002_search.down.sql
    0003_media_variants.up.sql      # M1c-2, phase expand (R29)
    0003_media_variants.down.sql    # drops the derived table; not lossy
    0004_cluster.up.sql             # M1.5, phase expand, lossy-down: jobs + nodes (P27)
    0004_cluster.down.sql
    0005_auth.up.sql                # M2a-1, phase expand, lossy-down: sessions, tokens, audit
    0005_auth.down.sql
    0006_media_comments.up.sql      # M2a-2b, phase expand, lossy-down: idempotency keys,
    0006_media_comments.down.sql    #   media.sanitized, comments.by_account; comment and job indexes (P2)
    0007_autosaves.up.sql           # M2b-1, phase expand, lossy-down: the autosave slot (P15)
    0007_autosaves.down.sql
    0008_autosave_seo.up.sql        # M2b-2, phase expand, lossy-down: autosaves.seo (P28)
    0008_autosave_seo.down.sql
crates/ferrite-store-postgres/migrations/
    0001_init.up.sql
    0001_init.down.sql
    0002_search.up.sql
    0002_search.down.sql
    0003_media_variants.up.sql
    0003_media_variants.down.sql
    0004_cluster.up.sql
    0004_cluster.down.sql
    0005_auth.up.sql
    0005_auth.down.sql
    0006_media_comments.up.sql
    0006_media_comments.down.sql
    0007_autosaves.up.sql
    0007_autosaves.down.sql
    0008_autosave_seo.up.sql
    0008_autosave_seo.down.sql
```

- sqlx reversible migrations, embedded with `sqlx::migrate!()`; sqlx's
  `_sqlx_migrations` table tracks applied versions and checksums.
- **Same version numbers and names on both backends.** A CI check fails if the sets
  differ.
- Sequential 4-digit versions (not timestamps) — ordering conflicts between branches
  are resolved at merge time, which is fine for a single-repo project.
- Every migration has a `.down.sql`. No exceptions; a lossy down is marked (below).
- **Editable until the first release tag (R10).** Nothing is deployed yet, so
  `0001_init` (and `0002_search`) may be edited in place and dev databases re-created.
  From the first tag on, applied migrations are immutable and changes need a new version.

### Header directives (ours, in SQL comments)

The first lines of an `.up.sql` may contain ferrite directives, parsed by
`ferrite_store::parse_directives` (sqlx ignores comments). The compatibility verdict
(§8), the SQLite pre-migration snapshot and the expand lint exist in M1a; the
`ferrite migrate` CLI arrived in M1c-1:

```sql
-- ferrite:phase expand            -- expand | contract (default: expand)
-- ferrite:min-binary 0.4.0        -- only on contract migrations, see §8
-- ferrite:lossy-down              -- down cannot restore data removed by up
```

### Who runs migrations

- `ferrite serve` runs pending `up` migrations at startup when
  `database.auto_migrate = true` (default `true`).
- sqlx locks the database during migration by default (verified: `Migrator::set_locking`
  defaults to `true`; on Postgres this is an advisory lock). In a cluster, the first
  node migrates; others block on the lock, then see nothing pending.
- SQLite: before applying any migration, a **pre-migration snapshot** is written to
  `data/backups/pre-<from>-to-<to>-<timestamp>.db` via `VACUUM INTO` (keeps last 5).
- Postgres: no automatic dump (would require `pg_dump` on the host). Instead the node
  logs the exact schema version transition and refuses to auto-migrate a **contract**
  migration unless `database.allow_contract = true` or it's run explicitly with
  `ferrite migrate up`. Rationale: contract steps are the ones a provider snapshot
  should precede.

### CLI

```
ferrite migrate status            # applied, pending, db vs binary versions, compat verdict
ferrite migrate up [--to N]       # apply pending (snapshot first on SQLite)
ferrite migrate down --to N [--allow-lossy] [--yes]
ferrite migrate verify            # checksums of applied migrations match embedded ones
```

Implemented in M1c-1 (`crates/ferrite/src/migrate.rs`; flags, exit codes and the "never
creates a database" rules are in [09 §8](09-operations.md#8-cli-summary)). The startup
check described above exists as `ferrite::startup_check` / `decide`
(`crates/ferrite/src/open.rs`, unit- and integration-tested in `tests/startup.rs`); since
M1c-2 `ferrite serve` and `ferrite seed demo` call it before doing anything else.

`down` runs inside one transaction per migration (both backends support transactional
DDL for what we use). It refuses lossy steps without `--allow-lossy` and prints which
tables/columns would lose data.

### Backend-specific caveats (verify in M1)

- SQLite `ALTER TABLE` is limited; column type changes/drops use the
  create-new-table → copy → drop → rename pattern. `PRAGMA foreign_keys` cannot be
  toggled inside a transaction — the rebuild pattern must use
  `PRAGMA defer_foreign_keys=ON` or be ordered to avoid violations. To verify against
  sqlx's per-migration transaction behaviour.
- Postgres `CREATE INDEX CONCURRENTLY` cannot run in a transaction. We avoid it in v1
  (tables are small enough). Checked for `0006` (M2a-2b final review M4): sqlx 0.9 runs a
  migration outside a transaction when its file starts with `-- no-transaction`
  (`sqlx-core` `migrate/source.rs`), but only for the whole file, and it sends the file
  as one multi-statement query, which Postgres runs as an implicit transaction block, so
  `CONCURRENTLY` still fails there (verified on Postgres 17). It would need one
  Postgres-only migration per index, which breaks the same-versions-on-both-backends rule.
  So `0006_media_comments` keeps its name and builds its three `jobs` indexes with plain
  `CREATE INDEX`: that takes a `SHARE` lock on `jobs` (reads go on; enqueue, claim, lease
  extensions, finishing and prune wait) until the migration commits, and its
  `ALTER TABLE ... ADD COLUMN` on `media` and `comments` take a short `ACCESS EXCLUSIVE`
  lock, also held to the commit. `jobs` is pruned, so this is usually well under a
  second; old nodes in a rolling upgrade wait, up to the 30 s `statement_timeout` of
  application connections, after which the waiting statement fails ([09 §3](09-operations.md#3-upgrade--rollback-cluster--postgres)). Its `.down.sql`
  runs in one transaction as every down does (`DROP INDEX` without `CONCURRENTLY`).

## 8. Upgrade compatibility (expand/contract) — D8

Goal (S5): binary *N-1* keeps working against schema produced by binary *N*. This is
what makes binary rollback safe and rolling deploys possible.

**Rules**

1. A release may only **expand**: new tables, new nullable columns or columns with
   defaults, new indexes, new rows in lookup tables.
2. Code in release *N* must tolerate (ignore) columns/tables it doesn't know.
3. Removing or renaming happens in two steps across releases:
   - *N*: add new column, write both, read new (expand).
   - *N+1*: stop writing old (no schema change).
   - *N+2*: drop old — a **contract** migration with `-- ferrite:min-binary <N+1>`.
4. Release *N* never relies on a contract migration that release *N-1* can't survive.

**Startup check**

```
db = highest applied migration version
bin = highest migration embedded in this binary
min = schema_meta.min_binary_version (default 0.0.0)

if db == bin                     → OK
if db <  bin                     → apply pending (auto_migrate) or refuse with "run ferrite migrate up"
if db >  bin and version >= min  → OK, log "running against newer schema (db=X, bin=Y)"
if db >  bin and version <  min  → refuse: "schema requires ferrite >= {min}; run
                                    `ferrite migrate down --to {bin}` with the newer binary,
                                    or upgrade this binary"
```

Unknown applied migrations (db > bin) are expected in that case and do not fail
checksum verification.

**`min_binary_version` preservation.** `schema_meta.min_binary_version` is computed by
the pure `ferrite_store::next_min_binary`: it is raised to the highest `min-binary` among
applied embedded contract migrations, and it is never lowered or erased while the
database holds an applied migration this binary does not know (a newer binary may have
set it). It is lowered only when this binary just reverted the migration whose
`min-binary` equals the stored value. So an older binary running `migrate` cannot
silently remove the guard.

**CI enforcement**

- *Expand check*: a migration without `phase contract` must not contain `DROP`,
  `RENAME`, `ALTER … TYPE`, or `NOT NULL` without `DEFAULT` (lint over SQL text).
- *Backward-compat test*: CI checks out the previous release tag, builds it, and runs
  its integration tests against a database migrated by the current branch.
  (Starts mattering from the first tagged release.)

## 9. Contract tests

`ferrite-store-tests` is a library of `async fn test_xxx(db: &TestDb)` functions plus
macros (`backend_tests!`, `backend_tests_spawning!`) that instantiate each test once per
backend. They take a `&TestDb` rather than `&dyn Store` (`db.store()` gives the store)
because migration tests need schema dumps and tamper helpers. Tests are registered in
`crates/ferrite-store-tests/tests/contract.rs`:

```rust
backend_tests!(
    types_seeded_and_upsert,
    slug_unique_per_type_and_parent,
    optimistic_lock_race,
    search_finds_title_and_body,
    up_down_up_roundtrip,
    // …
);
```

- SQLite: fresh temp file per test (not `:memory:`, so WAL + two-pool setup is real).
- Postgres: one server per run (CI uses a `postgres:18` service container); each test
  gets its own schema (`t_<uuid>`, dropped afterwards) for isolation and parallelism, any
  Postgres >= 15 works. Locally set `FERRITE_TEST_PG_URL`; if unset, Postgres cases print
  a `SKIP` line and pass, and when `CI=true` the missing variable is a panic, so CI cannot
  silently skip them.
- Migration tests: migrate up/down/up and compare schema dumps (`sqlite_schema` /
  `information_schema`), check the verdict after a partial down, checksum tampering is
  detected, and `min_binary_version` survives an older binary. Per-migration "seed
  fixture data at k-1, then up k" tests are not written yet (only `0001` and `0002`
  exist).

## 10. Portable export (D14)

```
ferrite export --out site-2026-10-22.ferrite.tar.zst [--with-media]
ferrite import-site site-2026-10-22.ferrite.tar.zst --data ./site   # into an empty DB
```

- Archive: `manifest.json` (format version, ferrite version, schema version, source
  backend, site id, table list with row counts and sha256), one NDJSON file per table
  in FK-dependency order, optional `artifacts/` with blobs.
- Backend-neutral encoding: ids as UUID strings, timestamps RFC 3339 UTC, JSON as JSON,
  bytes base64.
- Import requires the target to be migrated to the **same schema version** as the
  export (the tool runs `migrate up --to <v>` on an empty DB itself).
- Uses: Postgres backup independent of the provider; SQLite → Postgres move (S7);
  staging copies. `ferrite db copy --from <url> --to <url>` is export+import streamed
  without a file.
- Excluded by default: `sessions`, `rate_limits`, `events`, `nodes`, finished `jobs`.

## 11. Open points (for the builder)

- Revision retention policy default (50) and whether to diff-compress revisions — v1
  stores full snapshots.
- Whether `body_html` caching on the row is worth it vs. the page cache — measure in M1.
