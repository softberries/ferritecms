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
// ferrite-store/src/lib.rs (shape, not final code)
#[async_trait]
pub trait Store: Send + Sync + 'static {
    fn backend(&self) -> Backend;                       // Sqlite | Postgres
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError>;
    fn read(&self) -> &dyn Repos;                       // autocommit, read pool
    async fn migrations(&self) -> Result<MigrationStatus, StoreError>;
    async fn migrate_up(&self, to: Option<i64>) -> Result<(), StoreError>;
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<(), StoreError>;
    async fn snapshot(&self, dest: &Path) -> Result<(), StoreError>; // SQLite only; Postgres => Unsupported
}

#[async_trait]
pub trait Tx: Repos + Send {
    async fn commit(self: Box<Self>) -> Result<(), StoreError>;
    async fn rollback(self: Box<Self>) -> Result<(), StoreError>;
}

pub trait Repos: Send + Sync {
    fn content(&self) -> &dyn ContentRepo;
    fn revisions(&self) -> &dyn RevisionRepo;
    fn types(&self) -> &dyn ContentTypeRepo;
    fn terms(&self) -> &dyn TermRepo;
    fn users(&self) -> &dyn UserRepo;
    fn sessions(&self) -> &dyn SessionRepo;
    fn tokens(&self) -> &dyn ApiTokenRepo;
    fn options(&self) -> &dyn OptionRepo;
    fn redirects(&self) -> &dyn RedirectRepo;
    fn menus(&self) -> &dyn MenuRepo;
    fn media(&self) -> &dyn MediaRepo;
    fn comments(&self) -> &dyn CommentRepo;
    fn artifacts(&self) -> &dyn ArtifactRepo;
    fn plugins(&self) -> &dyn PluginRepo;
    fn plugin_kv(&self) -> &dyn PluginKvRepo;
    fn themes(&self) -> &dyn ThemeRepo;
    fn jobs(&self) -> &dyn JobRepo;
    fn events(&self) -> &dyn EventRepo;
    fn nodes(&self) -> &dyn NodeRepo;
    fn search(&self) -> &dyn SearchRepo;
    fn audit(&self) -> &dyn AuditRepo;
    fn rate_limits(&self) -> &dyn RateLimitRepo;
}
```

Notes:

- `async_trait` is used because the traits are used as trait objects. The per-call box
  is negligible next to a DB round trip.
- Transactions are a unit of work: a `Tx` exposes the same repos bound to one
  connection. Core services take `&dyn Repos`, so the same service code runs inside or
  outside a transaction.
- Repos return `ferrite-domain` types, never sqlx rows.
- Event emission (cache invalidation, [04](04-cluster.md)) happens **inside the same
  transaction** as the write (transactional outbox), so a committed write always has
  its event.

## 4. Logical schema

Same logical schema on both backends. Physical type mapping is in §5.

### Content

| Table | Columns (key ones) | Notes |
|---|---|---|
| `content_types` | `name` PK, `label`, `hierarchical`, `has_archive`, `fields` (json schema), `built_in`, `created_at` | `post`, `page`, `media` are built in |
| `content_items` | `id` PK, `type` FK, `status`, `slug`, `parent_id`, `author_id`, `title`, `excerpt`, `body` (json blocks), `body_html` (cached render), `fields` (json, custom fields), `menu_order`, `published_at`, `scheduled_at`, `created_at`, `updated_at`, `version` | `UNIQUE(type, parent_id, slug)` with NULL-parent handling; `status ∈ {draft, pending, scheduled, published, private, trash}`; `version` for optimistic locking |
| `content_meta` | `item_id` FK, `key`, `value` (json), PK(`item_id`,`key`) | Namespaced keys (`seo.title`, `plugin.<name>.*`) — plugin write scopes match on these |
| `revisions` | `id` PK, `item_id` FK, `author_id`, `created_at`, `title`, `excerpt`, `body`, `fields`, `meta` (json snapshot), `note` | Written on every save; retention configurable (default: keep 50 per item) |
| `taxonomies` | `name` PK, `label`, `hierarchical`, `content_types` (json list), `built_in` | `category`, `tag` built in |
| `terms` | `id` PK, `taxonomy` FK, `slug`, `name`, `description`, `parent_id`, `UNIQUE(taxonomy, slug)` | |
| `term_links` | `item_id` FK, `term_id` FK, `position`, PK(`item_id`,`term_id`) | |
| `menus` / `menu_items` | menu: `name` PK, `label`; item: `id`, `menu`, `parent_id`, `position`, `kind` (`item`/`term`/`url`), `target_id`, `url`, `label` | |
| `media` | `item_id` PK/FK → `content_items` (type `media`), `artifact_hash` FK, `mime`, `bytes`, `width`, `height`, `alt`, `original_name`, `variants` (json: name → hash, w, h) | |
| `comments` | `id`, `item_id`, `parent_id`, `user_id` null, `author_name`, `author_email`, `author_url`, `body`, `status`, `ip_hash`, `created_at` | |
| `redirects` | `id`, `source_path` UNIQUE, `target`, `code` (301/302/307/308/410), `hits`, `origin` (`manual`/`import`/`slug-change`), `created_at` | Auto-created on slug change of a published item |

### Users & auth

| Table | Columns | Notes |
|---|---|---|
| `users` | `id`, `login` UNIQUE, `email` UNIQUE (case-insensitive), `display_name`, `password_hash` (argon2id, nullable), `role`, `status`, `must_reset`, `totp_secret` (encrypted, nullable), `created_at`, `last_login_at` | |
| `user_meta` | `user_id`, `key`, `value` json | |
| `sessions` | `id_hash` PK (SHA-256 of token), `user_id`, `csrf_secret`, `created_at`, `last_seen_at`, `expires_at`, `ip_hash`, `user_agent` | DB-backed so any node can serve any request |
| `api_tokens` | `id`, `user_id`, `name`, `token_hash`, `scopes` json, `created_at`, `last_used_at`, `expires_at` | |
| `passkeys` | `id`, `user_id`, `credential` json, `created_at`, `last_used_at` | M2 stretch |
| `rate_limits` | `bucket` PK, `window_start`, `count` | Fixed-window counters; works across nodes |
| `audit_log` | `id`, `at`, `actor_id`, `action`, `target_kind`, `target_id`, `details` json, `node` | Admin actions, plugin installs, capability grants |

### Site, plugins, themes, artifacts

| Table | Columns | Notes |
|---|---|---|
| `options` | `key` PK, `value` json, `autoload`, `updated_at` | Site settings; autoloaded set cached per node, invalidated via events |
| `artifacts` | `hash` PK (sha256 hex), `kind` (`media`/`variant`/`plugin`/`theme`/`export`), `bytes`, `mime`, `created_at` | The blob lives in the MediaStore under `artifacts/<hash[0..2]>/<hash>` |
| `plugins` | `name` PK, `version`, `artifact_hash` FK, `manifest` json, `granted` json (capabilities the admin approved), `enabled`, `disabled_reason`, `installed_at`, `updated_at` | |
| `plugin_kv` | `plugin`, `key`, `value` bytes, `updated_at`, PK(`plugin`,`key`) | Quotas enforced in core (default 10 MB, 10k keys per plugin) |
| `plugin_failures` | `plugin`, `hook`, `bucket_start`, `count`, PK(`plugin`,`hook`,`bucket_start`) | Shared failure counters → auto-disable cluster-wide |
| `themes` | `name` PK, `version`, `artifact_hash`, `manifest` json, `active`, `installed_at` | Exactly one active (partial unique index / trigger) |

### Cluster

| Table | Columns | Notes |
|---|---|---|
| `nodes` | `id` PK, `name`, `version`, `schema_version`, `started_at`, `heartbeat_at`, `meta` json | Stale after 3× heartbeat interval |
| `events` | `seq` PK (autoincrement / bigserial), `kind`, `keys` json, `origin_node`, `created_at` | Transactional outbox for invalidation; pruned after 24h |
| `jobs` | `id`, `kind`, `payload` json, `unique_key` UNIQUE null, `run_at`, `priority`, `status` (`queued`/`running`/`done`/`failed`/`dead`), `attempts`, `max_attempts`, `locked_by`, `locked_until`, `last_error`, `created_at`, `finished_at` | See [04](04-cluster.md#job-queue) |
| `schema_meta` | `key` PK, `value` | `min_binary_version`, `created_by_version`, `site_id` |

### Search

Behind `SearchRepo` so the SQL difference is contained:

- **SQLite**: FTS5 virtual table `search_fts(item_id UNINDEXED, title, body_text)`
  maintained by the repo on writes (not triggers, so the text extraction from blocks
  stays in Rust).
- **Postgres**: `search_docs(item_id PK, title, body_text, tsv tsvector GENERATED ALWAYS
  AS (setweight(to_tsvector(cfg, title),'A') || setweight(to_tsvector(cfg, body_text),'B')) STORED)`
  with a GIN index; text search config from `site.locale` (fallback `simple`).

Ranking will differ slightly between backends; contract tests assert result *sets* for
deterministic fixtures, not exact order beyond "title match ranks first".

## 5. Physical type mapping

| Logical | SQLite | Postgres | Rust |
|---|---|---|---|
| id | `BLOB` (16 bytes) | `uuid` | `Uuid` (v7, D15) wrapped in typed ids (`PostId`, …) |
| timestamp | `INTEGER` unix **microseconds** UTC | `timestamptz` | `Timestamp` newtype over `jiff::Timestamp` or `chrono::DateTime<Utc>` (pick in M1 by sqlx support) |
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
- Two pools: **writer** (max 1 connection) and **reader** (N connections). All `Tx` use
  the writer. This makes write contention explicit and avoids busy-retry loops.
- Data folder lock: `ferrite.lock` with an OS advisory lock (`fs4`/`fd-lock`) held for
  the process lifetime — a second `ferrite serve` on the same folder fails fast.

**Postgres** (`ferrite-store-postgres`):

- One pool; `max_connections` from config; `statement_timeout` default 30 s on app
  connections (migrations use their own connection without it).
- Optional `schema` setting (`search_path`) so several sites can share one server.
- TLS via rustls; `sslmode` honoured from the URL.
- PgBouncer in transaction mode: supported for app traffic **only if** the event
  listener ([04](04-cluster.md)) uses a direct (non-pooled) connection — config key
  `database.listen_url` (defaults to `database.url`). Prepared-statement caching is
  disabled when `database.pgbouncer = true`.

## 7. Migrations

### Layout

```
crates/ferrite-store-sqlite/migrations/
    0001_init.up.sql
    0001_init.down.sql
    0002_search.up.sql
    0002_search.down.sql
crates/ferrite-store-postgres/migrations/
    0001_init.up.sql
    0001_init.down.sql
    0002_search.up.sql
    0002_search.down.sql
```

- sqlx reversible migrations, embedded with `sqlx::migrate!()`; sqlx's
  `_sqlx_migrations` table tracks applied versions and checksums.
- **Same version numbers and names on both backends.** A CI check fails if the sets
  differ.
- Sequential 4-digit versions (not timestamps) — ordering conflicts between branches
  are resolved at merge time, which is fine for a single-repo project.
- Every migration has a `.down.sql`. No exceptions; a lossy down is marked (below).

### Header directives (ours, in SQL comments)

The first lines of an `.up.sql` may contain ferrite directives, parsed by our migration
wrapper (sqlx ignores comments):

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
  (tables are small enough); if needed later, verify sqlx's support for
  non-transactional migrations before relying on it.

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

**CI enforcement**

- *Expand check*: a migration without `phase contract` must not contain `DROP`,
  `RENAME`, `ALTER … TYPE`, or `NOT NULL` without `DEFAULT` (lint over SQL text).
- *Backward-compat test*: CI checks out the previous release tag, builds it, and runs
  its integration tests against a database migrated by the current branch.
  (Starts mattering from the first tagged release.)

## 9. Contract tests

`ferrite-store-tests` is a library of `async fn test_xxx(store: &dyn Store)` functions
plus a macro that instantiates each test for each backend:

```rust
backend_tests! {
    content_create_and_get,
    content_slug_unique_per_parent,
    revisions_written_on_update,
    optimistic_lock_conflict,
    terms_hierarchy,
    jobs_claim_is_exclusive,
    events_seq_monotonic,
    search_finds_title_and_body,
    plugin_kv_quota,
    migrate_up_down_up_roundtrip,
    // …
}
```

- SQLite: fresh temp file per test (not `:memory:`, so WAL + two-pool setup is real).
- Postgres: one server per CI job (`docker run postgres:18` or a GitHub service
  container); one **schema per test** (`CREATE SCHEMA t_<uuid>`) for isolation and
  parallelism. Locally: `FERRITE_TEST_PG_URL` env var; Postgres tests are skipped with a
  loud message if unset, and required in CI.
- Migration tests: for every migration *k*: migrate to *k-1*, seed fixture data,
  `up` *k*, assert, `down` *k*, assert schema equals the *k-1* snapshot
  (via `sqlite_schema` / `information_schema` dumps).

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

- Pick `jiff` vs `chrono` for timestamps based on sqlx 0.9 support for both backends.
- Revision retention policy default (50) and whether to diff-compress revisions — v1
  stores full snapshots.
- Whether `body_html` caching on the row is worth it vs. the page cache — measure in M1.
