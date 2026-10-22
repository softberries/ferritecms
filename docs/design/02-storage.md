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
  - Text key and slug orderings are byte-wise: Postgres queries use `ORDER BY ...
    COLLATE "C"` to match SQLite's `BINARY` collation (R8). Admin lists are therefore not
    locale-aware.
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
- **Editable until the first release tag (R10).** Nothing is deployed yet, so
  `0001_init` (and `0002_search`) may be edited in place and dev databases re-created.
  From the first tag on, applied migrations are immutable and changes need a new version.

### Header directives (ours, in SQL comments)

The first lines of an `.up.sql` may contain ferrite directives, parsed by
`ferrite_store::parse_directives` (sqlx ignores comments). The compatibility verdict
(§8), the SQLite pre-migration snapshot and the expand lint exist in M1a; the
`ferrite migrate` CLI is M1c:

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
