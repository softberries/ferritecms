# M4-1 — Data Safety: Export, Backup, Media Sync and GC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An operator can move a site between backends and machines (`export` / `import-site` / `db copy` / `media sync`), back up and restore a solo site (by hand or on a schedule), and stored blobs that nothing references are collected. The M4-1 leftovers are closed as well: S3 options, location stripping, variant regeneration, the Postgres lock wait, atomic seeding and the cluster example's images.

**Architecture:**
- **Portable rows:** `ferrite-store` gets a static catalogue of every table and its columns, with a neutral type per column. Each backend implements a snapshot row reader and a single-transaction row loader over that catalogue. Export, import-site, `db copy` and the scheduled cluster export are all built on these two pieces.
- **Archives:** one `tar.zst` module in the binary crate writes and reads `manifest.json`, the table files and the blob files. Export archives and backup archives share it.
- **Backup (solo):** a `VACUUM INTO` snapshot, checked with `PRAGMA integrity_check`, together with `ferrite.toml`, `secret.key` and the referenced blobs. It is written either to a backup repository (a directory or `s3://`, where blobs are stored once by hash) or to a single `.tar.zst` file. The `site.backup` periodic job does this inside `serve`, which is how a running SQLite site is backed up.
- **GC:** the `artifacts` table gets an `orphaned_at` mark. The daily `artifacts.gc` job, and `ferrite media gc`, run four steps:
  - mark rows that nothing references, and clear the mark on rows that are referenced again;
  - delete rows whose mark is older than the grace period, then their blobs;
  - delete blobs that have no row and are older than the grace period;
  - delete `tmp/` objects older than a day.

**Tech Stack:**
- Rust: tokio, sqlx 0.9, object_store 0.14 and `tar`.
- New dependencies:
  - `zstd`, exact version;
  - `img-parts`, exact version, used only if Task 10 needs it.
- Docker Compose for the cluster example.

**Spec:**
- Design docs:
  - docs/design/09-operations.md §4 (backup, restore) and §8 (CLI);
  - 02-storage.md §10 (export) and §7 (snapshots);
  - 03-media.md §2 (placement), §4 (EXIF), §6 (GC) and §8 (sync);
  - 04-cluster.md §6 (jobs) and §7 (locks);
  - 05-plugin-host.md:60-72;
  - ROADMAP M4 (`RM:436-458`, `RM:476-498`).
- Rulings:
  - m1a R3;
  - m1b R2, R3, R5;
  - m15 P13, R3, P26;
  - m2a P4;
  - m2a2 P30;
  - m2a2b P3, P8, P27, R11;
  - m3 P23, P29, R68, R70, R74.
- Research digest (git-ignored, every fact with file:line): `.superpowers/m4-1-spec-digest.md`. Its §4 lists the 41 points the docs leave open. The rulings below settle each of them.

## Plan rulings (written to docs/superpowers/rulings/m4.md, section "M4-1 plan rulings", at Task 1)

Format in the ruling file: `- Plan ruling Pn: decision — reason — cost if wrong: …`.

**P1: export is a generic table dump, not domain DTOs.**
- `ferrite_store::catalogue::TABLES` is a `&'static [TableSpec]` listing the tables in FK order.
- Each `TableSpec` has:
  - `name`;
  - `columns: &[(&str, ColType)]`;
  - `key: &[&str]`, the primary key, used for keyset paging;
  - `self_ref: Option<&str>`, the parent column of `content_items`, `terms`, `comments` and `menu_items`;
  - `export: Export` (`Rows`, `Skip`).
- `ColType` is `Id`, `Timestamp`, `Json`, `Bool`, `Bytes`, `Text` or `Int`.
- A row is a `serde_json::Map` keyed by column name, encoded neutrally:
  - `Id`: a hyphenated UUID string;
  - `Timestamp`: RFC 3339 UTC with exactly 6 fractional digits and a `Z` suffix;
  - `Json`: a JSON value;
  - `Bool`: a JSON bool;
  - `Bytes`: standard base64 with padding;
  - `Text`: a string;
  - `Int`: an i64;
  - SQL NULL: `null`.
- Secrets travel as stored: argon2 hashes, sealed TOTP and settings ciphertext, token hashes. m1a R3's DTO concern does not arise, because no domain type is serialized.
- Reason: 34 tables, and most have no list API (digest §3.3). A catalogue that a test checks against the live schema cannot drift silently.
- Cost if wrong: a column type the catalogue gets wrong corrupts that column on a cross-backend copy. The coverage test (Task 2) and the cross-backend round trip (Task 3) are the guard.

**P2: what is exported.**
- `Skip`, because the data is transient or derived:
  - `sessions`, `rate_limits`, `events`, `nodes`, `idempotency_keys`, `plugin_failures`, `jobs` (all of them), `search_docs`;
  - SQLite's `search_fts*`;
  - `_sqlx_migrations`.
- `schema_meta`: only its `site_id` row is exported, and the loader writes that value over the target's.
- Everything else is exported, `media_variants` included (it is plain and backend-neutral).
- After loading, `search_docs` is rebuilt by calling, for every item, the same core indexing function the content writer uses. The implementer finds it from `SearchRepo::index`'s callers.
- Dropping queued jobs loses pending variant jobs. `ferrite media regenerate` (Task 9) repairs them, and the export doc says so.
- Reason: `D02:783` lists the transient tables but misses `idempotency_keys` and `plugin_failures`. Unfinished jobs carry node-local state and plugin actions that must not run twice.
- Cost if wrong: a scheduled plugin action queued at export time does not run on the copy.

**P3: consistency of the dump.**
- The reader holds one read snapshot for the whole dump: on Postgres a `REPEATABLE READ READ ONLY` transaction, on SQLite one read transaction on a reader connection.
- `Store::export_reader()` returns `Box<dyn ExportReader>` with `async fn page(&mut self, table: &TableSpec, after: Option<&Row>, limit: u32) -> Result<Vec<Row>, StoreError>` (keyset by `key`) and `async fn finish(self: Box<Self>)`.
- Reason: a live Postgres site can be exported, and the rows are consistent with each other.
- Cost if wrong: one long-lived snapshot, plus some Postgres vacuum lag during a large export.

**P4: the loader.**
- `Store::import_writer()` returns `Box<dyn ImportWriter>` with these methods:
  - `async fn clear(&mut self, tables: &[&TableSpec])`;
  - `async fn rows(&mut self, table: &TableSpec, rows: Vec<Row>)`;
  - `async fn finish_table(&mut self, table: &TableSpec)`, which fills the deferred self-references;
  - `async fn commit(self: Box<Self>)`.
- The whole load runs in **one write transaction**, so it lands completely or not at all.
- Self-referencing rows are inserted with the parent column set to NULL. `finish_table` then sets the parents in batches from the values it kept back.
- Postgres needs no identity override, because the only identity column (`events.seq`) is skipped.
- **Columns follow the archive (pre-flight ruling):** the catalogue describes the binary's latest schema. The reader requires the source at that schema (otherwise exit 1: "run `ferrite migrate up` first"). The loader writes exactly the columns present in each row. Each must be a catalogue column (an unknown column is refused), with its type taken from the catalogue. An archive at an older schema M therefore loads into a database at M, because migrations are expand-only and drop no columns. Tables absent from the archive stay empty.
- Reason: FKs are not deferrable on Postgres, and atomicity makes a failed import retryable.
- Cost if wrong: a very large site holds one long transaction, and SQLite holds the writer for the whole load.

**P5: the import-site target.**
- `ferrite import-site <archive|-> --data <dir>` needs an initialised data folder, made by `ferrite init`, which gives it `ferrite.toml` and its media configuration.
- Its database must be **fresh**: every exported table is empty, except the migration-seeded built-in `content_types` and `taxonomies` rows.
- Let M be the archive's schema version and N the target's:
  - N = 0 (unmigrated, e.g. an empty Postgres schema): migrate up to M.
  - N > M: `migrate_down(M, allow_lossy = true)`, which is safe because the database holds no data.
  - N = M: nothing to do.
- Then clear the exported tables, load, rebuild search, write `site_id`, and migrate up to the binary's latest under the usual `decide` rules (`open.rs`). A pending Postgres contract migration without `allow_contract` is left pending, with a hint to run `ferrite migrate up`.
- Refusals:
  - an archive whose format version or schema version is newer than the binary: exit 3;
  - a database that is not fresh: exit 1, and the message names the first non-empty table.
- Reason: `D02:778-779` requires the same schema version, and a fresh `init` is the normal starting point.
- Cost if wrong: a broken `.down.sql` blocks importing into a freshly initialised newer site. The workaround is an empty Postgres schema or a deleted `ferrite.db`.

**P6: the site key and encryption.**
- Export never includes `secret.key` (m2a P4, `00:127`).
- After import-site the tool prints how many users have TOTP enabled and the fix: copy the source's `secret.key`, or set `auth.secret_key`, or run `ferrite user disable-totp`.
- A solo **backup** includes `secret.key` unless `--no-secret-key` is passed, because a backup is a whole-site restore point.
- Nothing is encrypted by Ferrite. Archives and snapshot files are written with mode `0600`. `--out -` (export) and `--to -` (a single-file backup) write to stdout so the output can be piped to `age` or `gpg`, and import-site, restore and `db copy` sources read `-` as stdin.
- Reason: with no key management in M4, piping is the honest tool.
- Cost if wrong: users who never pipe keep password hashes and sealed secrets in plaintext archives. The ops docs say this.

**P7: the archive format, shared by export and backup.**
- A tar stream compressed with zstd (level 3). The first entry is `manifest.json`.
- Export layout:
  - `tables/<name>.ndjson`, in catalogue order;
  - with `--with-media`, `artifacts/<ab>/<hash>`.
- Backup layout:
  - `ferrite.db`, `ferrite.toml`, optionally `secret.key`;
  - `artifacts/<ab>/<hash>`.
- Manifest fields:
  - `format: 1`;
  - `kind: "export" | "backup"`;
  - `ferrite_version`;
  - `schema_version` (the highest applied migration);
  - `source_backend: "sqlite" | "postgres"`;
  - `site_id`;
  - `created_at`;
  - `tables: [{name, rows, sha256}]` (export only);
  - `files: [{path, bytes, sha256}]` (backup only, for `ferrite.db`, `ferrite.toml` and `secret.key`);
  - `artifacts: {count, bytes}` or `null`.
- Readers verify the sha256 of every table and file, and refuse on a mismatch.
- Table files are spooled to a temp dir before they are appended, because tar needs sizes. Blobs are streamed with their size from `head`, through `SyncIoBridge` in `spawn_blocking`.
- Reason: `D02:773-777` and `D09:111`.
- Cost if wrong: a format change later needs `format: 2` and a reader for both.

**P8: `db copy`.**
- `ferrite db copy --from <url> --to <url> [--from-schema S] [--to-schema S]`. A URL is `postgres://…` or `sqlite:<path to ferrite.db>`; the SQLite folder lock is taken on the file's parent.
- The rows stream reader → loader with no file in between. The target follows P5's rules, except that an absent SQLite file is created.
- Media is not copied (that is `media sync`), and no `ferrite.toml` is written.
- Exit codes: 0 ok; 1 error; 3 when the source schema is newer than the binary.
- Reason: `D02:781-782`.
- Cost if wrong: the user edits `ferrite.toml` by hand afterwards. The command prints the `[database]` lines to use.

**P9: `media sync`.**
- `ferrite media sync --to <dest> [--from <src> | --data <dir>]`, where `<src>` and `<dest>` are `local:<path>` or `s3://bucket[/prefix]`. With `--data`, the source is the site's configured `MediaStore`.
- Credentials:
  - an `s3://` source taken from the site config uses `[media.s3]`;
  - every URL-given S3 store uses `AmazonS3Builder::from_env()` (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ENDPOINT_URL`, `AWS_ALLOW_HTTP`).
- Behaviour:
  - copies every object under the source's `artifacts/`, recorded or not (no database is needed);
  - skips keys already present at the destination with the same size, and checks the size after each copy;
  - never deletes, never copies `tmp/`, runs 8 copies at once;
  - prints `copied N (B bytes), skipped M, failed F`.
- Exit codes: 0, or 2 when some copies failed, or 1 on an error.
- Reason: `D03:486-487`.
- Cost if wrong: two S3 accounts that both need explicit keys in config. The user puts one set in env instead.

**P10: the GC reference set** (corrects `D03:473`: revisions hold content ids, not hashes). A row is referenced when its hash is named by any of:
- `media.artifact_hash`;
- `media_variants.hash`;
- `plugins.artifact_hash`;
- `themes.artifact_hash`;
- `import_runs.report_hash`;
- **protected** hashes computed in Rust and passed to the store:
  - every 64-lowercase-hex string anywhere in the payload of a `queued` or `running` job (generic, so it covers `media.variants`, `import.wordpress` and any future kind);
  - the `json` and `html` hashes inside every report manifest.

If a manifest cannot be read, all `import`-kind rows are protected for that run.

- Reason: the digest's §3.5 is the list of what the code actually references.
- Cost if wrong: a reference outside this list means a deleted blob. Task 6's `gc_keeps_every_reference_kind` covers each kind.

**P11: grace and marking.**
- Migration `0011_artifact_gc` adds `artifacts.orphaned_at` (timestamp, NULL) and an index, on both backends. It is expand-only.
- One GC pass:
  1. **Mark:** set `orphaned_at = now` on unmarked rows that nothing references and that are not protected, and clear it on marked rows that are referenced again.
  2. **Delete rows:** delete rows with `orphaned_at < now − grace` with a per-hash `delete_if_orphaned(hash, before, protected)`. It re-checks the references in its own transaction, so the FKs refuse media, plugin and theme references. Only when that delete succeeds is the blob deleted (R74's order).
  3. **Delete row-less blobs:** list `artifacts/` and delete blobs that have no row, are not protected, and have a `last_modified` older than the grace period.
  4. **Clean `tmp/`:** delete `tmp/` objects (probes included) older than 24 h.
- `ArtifactRepo::insert_if_absent` on an existing row clears `orphaned_at`. Reusing the same bytes counts as a fresh reference.
- The grace period defaults to 7 days (`D03:475`); `[media] gc_grace_days`, 1..=365, configures it.
- Residual race (accepted, documented in 03 §6): a writer whose put of a blob that already existed was a no-op, and that inserts its row in the milliseconds between GC step 2's commit and its blob delete, ends up with a row and no blob. It needs bytes orphaned for more than 7 days to be re-uploaded at that instant. Re-uploading the file fixes it.
- Reason: this is "unreferenced for > 7 days" taken literally, and it protects rollbacks to an old plugin artifact.
- Cost if wrong: one extra column, plus a GC that needs two runs to delete anything.

**P12: GC runs.**
- **Job:** `artifacts.gc` is a daily `Periodic` (`every` 86 400 s), with its handler in `crates/ferrite/src/jobs/gc.rs`, because `ferrite-cluster` has no media dependency. Its timeout is 3 600 s; a pass cut short continues the next day.
- **CLI:** `ferrite media gc --data <dir> [--dry-run] [--grace-days N]` runs the same `gc::run` in process. On SQLite, `serve` must be stopped.
- Both take the store lock `artifacts.gc` with `try_lock`. When it is held, the job returns `Busy` and the CLI exits 3.
- Dry-run marks nothing and deletes nothing. It reports what would be marked and deleted, per class.
- Output: `marked N, unmarked M, deleted rows R (B bytes), deleted unrecorded blobs U (B bytes), deleted tmp T`.
- Exit codes: 0, 3 busy, 1 error.
- `Installer::discard_staged` stops deleting blobs and leaves them to the GC. This closes the RM:498 same-bytes install race; the cost is that refused install bytes stay up to the grace period.
- R68's upload cleanup is kept, since it already holds `LOCK_IMPORT`.

**P13: backup.**
- Command: `ferrite backup --data <dir> --to <target> [--no-media] [--no-secret-key] [--keep N]`. SQLite only; on Postgres it exits 1 with a pointer to provider backups and `ferrite export`.
- Like every SQLite CLI command, it needs `serve` stopped. A running site is backed up by `[backup]` (P15). This corrects BRIEF:217's "safe live".
- Targets:
  - **a path ending in `.tar.zst`, or `-`:** a single archive (P7, `kind: "backup"`);
  - **a directory or `s3://bucket[/prefix]`:** a backup **repository**:
    - `<repo>/artifacts/<ab>/<hash>`, shared and copied only when missing ("incremental by hash");
    - `<repo>/snapshots/<UTC yyyymmddThhmmssZ>/{manifest.json, ferrite.db, ferrite.toml, secret.key}`;
    - the manifest also lists `artifact_hashes`.
- Sequence:
  1. Write the snapshot to `<data>/backups/backup-<ts>.db`. The name must not match `pre-*.db`, because pre-migration pruning deletes those files.
  2. Open it read-only with sqlx, run `PRAGMA integrity_check`, which must return `ok`, and read its `artifacts` hashes.
  3. Copy those blobs from the site's `MediaStore`.
  4. Write the snapshot files.
  5. Verify: compare the sha256 of the uploaded `ferrite.db` with a re-download.
  6. Delete the local snapshot.
- `--keep N` (default: keep all) prunes the repository's oldest snapshots. It then deletes repository blobs that no kept manifest lists.
- Repository S3 credentials: `from_env` (as in P9).
- Exit codes: 0, 1.
- Reason: `D09:109-128`.
- Cost if wrong: there is no ad-hoc live backup of a SQLite site without the schedule. That is a later API or admin item.

**P14: restore.**
- Command: `ferrite restore <source> --data <dir> [--snapshot <ts>] [--force]`.
- `<source>` is a backup `.tar.zst`, `-`, or a repository (directory or `s3://`). Without `--snapshot`, the newest snapshot is used.
- The target must not exist or must be empty. With `--force` an existing folder is renamed to `<dir>.pre-restore-<ts>` (a sibling, never deleted), which is refused while its `ferrite.lock` is held.
- Writes:
  - `ferrite.toml` and `ferrite.db` (mode `0600`);
  - `secret.key` when the backup has one;
  - the blobs, put through a `MediaStore` opened from the **restored** config (local or S3), skipping blobs that already exist.
- It then opens the store with `MustExist` and prints `migrate status`. It does not migrate; `serve` does that under `auto_migrate`.
- A backup whose schema is newer than the binary: exit 3, and nothing is written.
- Exit codes: 0, 1, 3.
- Reason: `D09:114`.
- Cost if wrong: a restore onto S3 media trusts that the blobs already there are correct (they are addressed by content hash).

**P15: scheduled backups.**
- A new config section `[backup]`:
  - `schedule = "off" | "daily" | "weekly"`, default `off`;
  - `to = "<dir>|s3://…"`, required unless `off`; relative paths are relative to the data folder;
  - `keep = 7` (1..=1000);
  - `with_media = true`.
- `config check` prints `backup=<schedule>`.
- When `schedule` is not `off`, `serve` registers the `site.backup` handler and a `Periodic`. Its timeout is 21 600 s, and it holds the store lock `site.backup`.
  - **SQLite:** the P13 repository backup, run in process, where `Store::snapshot` is safe next to writers.
  - **Postgres:** a P7 export archive with `--with-media` = `with_media`, written to `<data>/backups/export-<ts>.tmp`, then uploaded to `<to>/exports/<ts>.ferrite.tar.zst` and deleted locally. It keeps the newest `keep`.
- A failure returns `Retry` (`max_attempts` 3).
- Reason: `D09:113`.
- Cost if wrong: weekly is the coarsest schedule, and cron syntax is a later item.

**P16: S3 options.**
- `[media.s3]` gets two keys:
  - `copy_if_not_exists`: a string in object_store's own encoding, `header:<name>:<value>`, `header-with-status:<name>:<value>:<status>` or `multipart`, parsed with object_store's parser;
  - `conditional_put = "etag" | "disabled"`, default `etag`, which is already the builder's default.
- Both are set on the builder for the explicit-keys path and the `from_env` path alike.
- This corrects `RM:446`: conditional put is already on. `disabled` is for stores that reject `If-None-Match`.
- `multipart` is accepted, with a config-check warning that it does not copy object attributes (`RM:447`).
- Reason: the digest's §3.4 object_store 0.14.2 read.
- Cost if wrong: a wrong `copy_if_not_exists` header for a provider fails placements. `ferrite config check` cannot detect that; the docs say to test with an upload.

**P17: location and metadata stripping.**
- `[media] strip_metadata = false` (default: keep the WordPress behaviour, R11). When it is `true`, `stage_stream` strips EXIF and XMP **losslessly** from JPEG, PNG and WebP originals before hashing and storing them:
  - JPEG: the APP1 Exif and APP1 XMP segments;
  - PNG: the `eXIf` chunk and the `iTXt` chunk with the XMP keyword;
  - WebP: the `EXIF` and `XMP ` chunks, with the VP8X flags fixed.
- ICC data is kept. An EXIF Orientation other than 1 is written back as a minimal EXIF block that holds only that tag.
- It applies to browser uploads and to the WordPress importer, which share `stage_stream`. GIF and SVG are untouched.
- `img-parts` may be used if it covers all three formats (check with Context7). Otherwise write a small segment walker; the tests decide.
- Reason: `RM:450`, applied to every ingest because the leak is the same either way.
- Cost if wrong: deduplication against byte-identical WordPress originals stops working once it is on.

**P18: `media regenerate`.**
- Command: `ferrite media regenerate --data <dir> [--all] [--item <id>]`. It runs **in process**, like `seed`, with no jobs, so there is no unique-key clash (digest §3.8).
- Sizes are the core sizes plus the sizes of the theme active in the store (`load_theme`, as seed does).
- By default it only processes raster media whose `variants` lack a name in the current spec set. `--all` redoes every one.
- For each item: process the original with the existing code, `put_artifact` every variant, then `record_variants`, conditional on the media row still holding that artifact. Items above 64 MiB and non-raster items are skipped.
- The variant blobs that are replaced are left for the GC.
- Progress goes to stderr every 50 items. At the end it prints `regenerated N, skipped S, failed F`.
- Exit codes: 0, 2 when some items failed, 1 on an error.
- Reason: `RM:449` and P8.
- Cost if wrong: a solo site has to stop `serve` to regenerate. A job-based variant is a later item.

**P19: the Postgres lock wait.**
- A new `Store::lock_wait(&self, name, wait: Duration) -> Result<Option<Box<dyn LockGuard>>, StoreError>`. Its default implementation is the current 250 ms poll. `ferrite_store::lock()` calls it.
- **Postgres:** one dedicated connection (the same options and `application_name` as `try_lock`), then `SET lock_timeout = '<wait ms>ms'`, then `SELECT pg_advisory_lock(key)`.
  - SQLSTATE `55P03` (lock_not_available) means `Ok(None)`.
  - On success, run `SET lock_timeout = 0` and keep the connection as the guard's.
  - A cancelled future drops the connection, which releases any lock the server granted late.
- **SQLite:** keeps the default.
- Lock keys are unchanged (`lock_keys_are_golden`).
- Reason: m15 R3.
- Cost if wrong: if `lock_timeout` did not apply to advisory locks, waits would hang. Task 4's tests prove that it does apply.

**P20: seed atomicity.**
- `seed demo` puts all its blobs first, then writes every row through one `ferrite_core::seed::DemoBatch` that wraps a single `Tx`.
- Where a service lacks a `Tx`-taking variant, the existing method is split into `*_in(tx, …)` plus the wrapper, with no change in behaviour.
- A failure leaves no rows; its blobs are left for the GC. `--force` keeps its meaning.
- Reason: `RM:454`.
- Cost if wrong: a pub(crate) seam in core grows. Seed is its only user.

**P21: cluster example images.**
- Replace `minio/minio` and `minio/mc` with a **maintained** S3-compatible server and a client that has a shell, both pinned as `name:tag@sha256:<digest>`.
- The implementer picks the server by these criteria, recorded in the execution ruling:
  - a release within the last 6 months;
  - a single container;
  - support for PutObject `If-None-Match`, CopyObject and ListObjectsV2;
  - a bucket that can be created from a one-shot init container.
- Candidates to check, in order: RustFS, Garage, SeaweedFS, Versity S3 Gateway.
- Verify with `docker compose up` on this machine: an upload on node1 is read back through node2. Pin `postgres:18` and `caddy:2` by digest too.
- Reason: `RM:486`.
- Cost if wrong: the image moves on and the digest goes stale. A digest never breaks a pull until it is deleted.

**P22: the ops docs.**
- A new section, docs/design/09-operations.md §4a "S3 media buckets", with:
  - the lifecycle rules (abort incomplete multipart uploads after 1 day; expire `tmp/` after 1 day), each with an AWS JSON example;
  - the `copy_if_not_exists` values per provider, R2 path-style, B2 region naming and Hetzner and Scaleway endpoints.
- Each provider claim is checked against the provider's current documentation and marked in a "verified / not verified" table (user rule). An unverifiable claim stays in the "not verified" table and is not stated as fact.

**P23: no audit, API or admin surface.** These commands are operator CLI and jobs, so no `audit_log` rows are written (as for `user` and `ext`). A Backups screen and API are later items (m2b P25).

**P24: the milestone exit.** `docs/superpowers/m4-1-exit.md` lists:
- each command run end to end on both profiles, with output;
- a SQLite → Postgres move: export, import-site, `media sync`, `serve`, a page rendered;
- a backup taken while `serve` runs (the scheduled job with a 1-minute test seam period, or the job triggered through `test-seams`), restored into a new folder and served;
- the cluster example brought up;
- a "Not verified" table.

## Global Constraints

- Lint and doc gates:
  - `cargo fmt --check`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, with and without `--features ferrite/test-seams,ferrite-http/test-seams,ferrite-import-wp/test-seams`;
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`.
- `thiserror` in libraries, `anyhow` only in the `ferrite` binary. No `#[allow(dead_code)]`: remove unused code.
- **Migration `0011_artifact_gc`** goes on both backends with a `.down.sql`, and is expand-only (`ALTER TABLE … ADD COLUMN`, `CREATE INDEX`). Same version and name on both.
- Storage changes need contract tests on both backends: `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test`.
- No API or OpenAPI change in this milestone. If one is needed, stop and rule.
- New dependencies take an exact `=x.y.z` version, `default-features = false` where possible, and a Context7 check of the current version and API before use. Only `zstd` and, if Task 10 needs it, `img-parts`. `ferrite` may add `object_store` (the workspace's).
- New config keys go in the parser, the model, the `render_default_toml` template (commented out), docs/design/01-architecture.md §4, and `config check`. An unknown key stays an error.
- CLI exit codes follow P5, P8, P9 and P12–P14. Every new command has `assert_cmd` tests in `crates/ferrite/tests/` on SQLite, plus Postgres where the command supports it (skip without `FERRITE_TEST_PG_URL`; CI sets it).
- Files written that hold secrets (archives, snapshots, `secret.key`, `ferrite.toml`) use mode `0600` (`write_private` in `init.rs`).
- Commits are small. Messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Cross-backend fidelity:**
   - a SQLite site exported and imported into Postgres, then exported again, gives byte-identical table files, except the manifest's `source_backend` and `created_at`;
   - timestamps keep their microseconds, and booleans, bytes and JSON survive;
   - TOTP still opens when the key is copied.

   Tests: Task 3 `cross_backend_round_trip_is_identical`, Task 5 `export_import_site_sqlite_to_postgres`.
2. **Never losing referenced bytes:**
   - each reference kind in P10 keeps its blob through a GC with grace 0;
   - an artifact re-inserted after marking is unmarked;
   - a blob with no row but younger than the grace period survives;
   - a dry run changes nothing.

   Tests: Task 6 `gc_keeps_every_reference_kind`, `gc_reinsert_clears_mark`, `gc_dry_run_changes_nothing`.
3. **Hostile or broken archives:**
   - a tampered table file gives a sha256 refusal before any row is written;
   - a `../` or absolute tar path is refused;
   - an archive from a newer format or schema exits 3;
   - a truncated stream fails cleanly with the target unchanged (one transaction).

   Tests: Task 5 `import_site_refuses_tampered_table`, `import_site_refuses_path_traversal`, `import_site_truncated_leaves_target_fresh`.
4. **Restore safety:**
   - restore never writes into a non-empty folder without `--force`;
   - `--force` refuses while `serve` holds `ferrite.lock`;
   - the old folder is moved aside, never deleted;
   - a backup taken while `serve` writes passes `integrity_check` and serves after a restore.

   Tests: Task 7 `restore_refuses_non_empty`, `restore_force_refuses_locked`, Task 8 `scheduled_backup_while_serving_restores`.
5. **Lock and job behaviour under contention:**
   - the Postgres `lock_wait` returns `None` after the wait, and the next waiter gets the lock as soon as it is released, with no 250 ms quantum;
   - a cancelled wait does not leak a held lock;
   - two GC runs at once leave one `Busy`.

   Tests: Task 4 `pg_lock_wait_*`, Task 6 `gc_concurrent_runs_one_busy`.

---

### Task 1: Storage — `orphaned_at`, artifact listing and GC queries

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0011_artifact_gc.{up,down}.sql`
- Modify: `crates/ferrite-domain/src/plugin.rs` (`Artifact` gets `orphaned_at: Option<Timestamp>`), `crates/ferrite-store/src/repos.rs` (`ArtifactRepo`), both backends' artifact repos, every `Artifact { … }` literal (in seed, extensions, import and media)
- Create: `docs/superpowers/rulings/m4.md` (P1–P24, copied from this plan)
- Test: `crates/ferrite-store-tests/src/artifact_gc.rs`, registered in `tests/contract.rs` for both backends

**Interfaces (produces):**
```rust
// ArtifactRepo, added:
async fn list_page(&self, after: Option<&ArtifactHash>, limit: u32) -> Result<Vec<Artifact>, StoreError>; // by hash
async fn existing(&self, hashes: &[ArtifactHash]) -> Result<Vec<ArtifactHash>, StoreError>; // ≤ 1000, else Invalid
async fn mark_orphans(&self, now: Timestamp, protected: &[ArtifactHash]) -> Result<MarkCounts, StoreError>;
async fn orphaned_before(&self, before: Timestamp, limit: u32) -> Result<Vec<Artifact>, StoreError>;
async fn delete_if_orphaned(&self, hash: &ArtifactHash, before: Timestamp, protected: &[ArtifactHash]) -> Result<bool, StoreError>;
// insert_if_absent: on conflict clears orphaned_at (still returns false).
pub struct MarkCounts { pub marked: u64, pub unmarked: u64 }
```
`protected` goes to SQL as one JSON array parameter (`json_each` / `jsonb_array_elements_text`). "Referenced" means P10's SQL part.

- [ ] **Step 1:** Write the rulings file. Write the failing contract tests:
  - `artifact_list_page_is_ordered_and_paged`;
  - `mark_orphans_marks_unreferenced_and_clears_referenced`: one row each for media, variant (`media_variants`), plugin, theme, `import_runs.report_hash`, protected and unreferenced;
  - `insert_if_absent_clears_orphan_mark`;
  - `delete_if_orphaned_respects_before_refs_and_protected`: false for a referenced row, a young row or a protected row; true otherwise;
  - `existing_returns_present_subset`.
- [ ] **Step 2:** Run `cargo test -p ferrite-store-tests` with `FERRITE_TEST_PG_URL` set. Expect FAIL (unresolved methods).
- [ ] **Step 3:** Implement the migration, the domain field, the trait and both backends.
- [ ] **Step 4:** Run the contract suite on both backends and the migration checks (`cargo test -p ferrite-store-sqlite -p ferrite-store-postgres`). Expect PASS.
- [ ] **Step 5:** Commit `feat(store): artifact orphan marks and GC queries (0011)`.

### Task 2: Storage — the table catalogue and the snapshot reader

**Files:**
- Create: `crates/ferrite-store/src/catalogue.rs` (`TableSpec`, `ColType`, `Export`, `Row = serde_json::Map<String, Value>`, `TABLES`), `crates/ferrite-store/src/export.rs` (the `ExportReader` trait)
- Modify: `crates/ferrite-store/src/repos.rs` (`Store::export_reader`), both backends (`export.rs` each: per-`ColType` decoding into the neutral encoding)
- Test: `crates/ferrite-store-tests/src/export.rs`

**Interfaces (produces):** `catalogue::TABLES`, `catalogue::table(name) -> Option<&'static TableSpec>`, `Store::export_reader(&self) -> Result<Box<dyn ExportReader>, StoreError>`, and `ExportReader::{page, finish}` (P3).

- [ ] **Step 1:** Write the failing tests:
  - `catalogue_matches_live_schema`: per backend, introspect the columns of every table (`pragma_table_info` / `information_schema.columns`) after `migrate_up(None)`. The catalogue must have exactly those tables, minus the SQLite FTS tables and `_sqlx_migrations`, and exactly those columns.
  - `catalogue_is_fk_ordered`: introspect the FKs; each referenced table comes earlier, or is the table itself, in which case the column is `self_ref`.
  - `export_reader_encodes_neutrally`: seed one row per `ColType`, including a timestamp with microseconds `…T01:02:03.000004Z`, bytes `[0, 255]`, nested JSON and both booleans, then assert the exact `Row` JSON.
  - `export_reader_pages_by_key`.
  - `export_reader_sees_one_snapshot`: on Postgres, a row inserted after `export_reader()` is not seen; on SQLite the same assertion through the writer pool.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement the catalogue from the migrations (digest §3.3 lists the 34 tables; P2 decides the `Export` values) and the readers.
- [ ] **Step 4:** Both backends PASS.
- [ ] **Step 5:** Commit `feat(store): table catalogue and snapshot row reader`.

### Task 3: Storage — the single-transaction loader

**Files:**
- Create: `crates/ferrite-store/src/import_writer.rs` (the `ImportWriter` trait), both backends' `import_writer.rs`
- Modify: `crates/ferrite-store/src/repos.rs` (`Store::import_writer`)
- Test: `crates/ferrite-store-tests/src/export.rs`, plus a cross-backend test in `crates/ferrite-store-tests/tests/cross_backend.rs` (Postgres-gated)

**Interfaces:** consumes Task 2; produces `Store::import_writer(&self) -> Result<Box<dyn ImportWriter>, StoreError>` (P4).

- [ ] **Step 1:** Write the failing tests:
  - `import_writer_round_trip`: build a rich site (users with TOTP bytes, a nested page tree, nested terms and comments, menus with children, media with variants, plugin kv bytes, audit rows, an import map), dump it, load it into a fresh store, dump again, and compare equal.
  - `import_writer_rollback_leaves_nothing`: an invalid row in a late table, then commit returns an error and every table is still empty.
  - `import_writer_self_refs_any_order`: a child row given before its parent.
  - `cross_backend_round_trip_is_identical`: SQLite → Postgres → SQLite, compared per table as equal rows.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement. Batched multi-row inserts of at most 500 rows. SQLite runs inside the writer transaction. The `*_key` lowercase columns are copied as they are.
- [ ] **Step 4:** Both backends PASS.
- [ ] **Step 5:** Commit `feat(store): transactional row loader`.

### Task 4: Postgres lock wait with `lock_timeout`

**Files:**
- Modify: `crates/ferrite-store/src/{repos.rs,lock.rs}`, `crates/ferrite-store-postgres/src/lock.rs`, `crates/ferrite-store-postgres/src/lib.rs`
- Test: `crates/ferrite-store-tests/src/lock.rs`

**Interfaces:** produces `Store::lock_wait` (P19). `ferrite_store::lock(store, name, wait)` keeps its signature and calls it.

- [ ] **Step 1:** Write the failing tests:
  - `pg_lock_wait_times_out`: held elsewhere, a wait of 300 ms returns `None` within [300 ms, 1.5 s];
  - `pg_lock_wait_wakes_on_release`: released at 200 ms, acquired before 300 ms;
  - `pg_lock_wait_cancel_does_not_leak`: drop the wait future at 100 ms, release the holder, then `try_lock` succeeds within 2 s;
  - `pg_lock_wait_opens_one_connection`: count backends with `application_name` `<app>:lock:<name>` during a 1 s wait, which must be ≤ 1.

  The existing wait tests keep passing on SQLite.
- [ ] **Step 2:** Run them. Expect FAIL (poll quantum, connection count).
- [ ] **Step 3:** Implement. Update the doc comments in `repos.rs` and 04 §7 ("a held connection with `lock_timeout`").
- [ ] **Step 4:** PASS on both backends. `lock_keys_are_golden` is unchanged.
- [ ] **Step 5:** Commit `feat(store-postgres): wait for locks with lock_timeout on one connection`.

### Task 5: The archive module, `export`, `import-site` and `db copy`

**Files:**
- Create: `crates/ferrite/src/archive.rs` (manifest types, a writer and a reader over `tar` plus `zstd`), `crates/ferrite/src/site_copy.rs` (`dump(reader, sink)`, `load(source, writer)`, the freshness check, the P5 migrate dance, the search rebuild), `crates/ferrite/src/export_cmd.rs`
- Modify: `crates/ferrite/src/{cli.rs,lib.rs,open.rs}` (`open_store_url(url, schema)`), `Cargo.toml` (`zstd`)
- Test: `crates/ferrite/tests/export_cli.rs`

**Interfaces:** consumes Tasks 2–3. Produces:
- `archive::{Manifest, ArchiveWriter::new(impl Write) -> …, append_file(path, &Path), append_stream(path, size, impl Read), finish()}`;
- `archive::ArchiveReader` (an entry iterator with sha256 checks against the manifest);
- `site_copy::{export_to(store, media: Option<&MediaStore>, out: impl Write), import_from(read: impl Read, store, media)}`.

Task 7 reuses `ArchiveWriter` and `ArchiveReader`.

- [ ] **Step 1:** Write the failing CLI tests:
  - `export_import_site_round_trip_sqlite`: seed demo, export `--with-media`, `init` a new folder, import-site, then `serve` it and fetch a post page and a media URL;
  - `export_import_site_sqlite_to_postgres` (PG-gated);
  - `export_to_stdout_and_import_from_stdin`;
  - `import_site_refuses_non_fresh` (exit 1, names the table);
  - `import_site_refuses_newer_schema` (exit 3, by editing a manifest);
  - `import_site_refuses_tampered_table`;
  - `import_site_refuses_path_traversal` (a hand-built tar);
  - `import_site_truncated_leaves_target_fresh`;
  - `import_site_into_newer_fresh_target_migrates_down_then_up` (an archive at 11 hand-edited to schema 10: drop `orphaned_at` from the artifacts rows, set `schema_version` 10, recompute the sha256s; target at 11);
  - `import_site_prints_totp_hint`;
  - `db_copy_sqlite_to_postgres` (PG-gated);
  - `export_file_mode_is_0600`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** PASS (SQLite always, PG with the URL).
- [ ] **Step 5:** Commit `feat(cli): export, import-site and db copy`.

### Task 6: Artifacts GC — the job, `media gc`, the orphan sweep

**Files:**
- Create: `crates/ferrite/src/gc.rs` (`run(store, media, opts) -> GcReport`), `crates/ferrite/src/jobs/gc.rs` (the handler and its `Periodic`), `crates/ferrite/src/media_cmd.rs` (the `media` subcommand group: `gc` here, `sync` in Task 7, `regenerate` in Task 9)
- Modify: `crates/ferrite-media/src/store.rs` (`list_artifacts() -> impl Stream<Item = Result<StoredObject>>` and `list_tmp()`, with `StoredObject { hash: Option<ArtifactHash>, key, size, last_modified }`; `delete_key` for `tmp/`), `crates/ferrite/src/serve.rs` (registration and job kinds), `crates/ferrite-http/src/extensions.rs` (`discard_staged` no longer deletes; P12), `crates/ferrite-config` (`[media] gc_grace_days`), `crates/ferrite-store/src/lock.rs` (`LOCK_GC = "artifacts.gc"`)
- Test: `crates/ferrite/tests/gc.rs`; `crates/ferrite-media/tests/store.rs` (listing on local and s3s)

**Interfaces:** consumes Task 1. Produces `MediaStore::{list_artifacts, list_tmp, delete_key}` (Task 7 uses `list_artifacts`), `gc::{GcOptions { grace: Duration, dry_run: bool, now: Timestamp }, GcReport, run}`.

- [ ] **Step 1:** Write the failing tests:
  - `list_artifacts_reports_size_and_age` (local and s3);
  - `gc_keeps_every_reference_kind` (grace 0, two passes; one fixture per P10 kind, the unfinished-job payload and report manifest contents included);
  - `gc_deletes_orphan_row_then_blob_after_grace`;
  - `gc_reinsert_clears_mark`;
  - `gc_unrecorded_blob_young_kept_old_deleted` (use `now` in the future rather than sleeping);
  - `gc_tmp_older_than_a_day_deleted`;
  - `gc_dry_run_changes_nothing`;
  - `gc_concurrent_runs_one_busy`;
  - `media_gc_cli_exit_codes`;
  - `serve_registers_artifacts_gc` (job kinds list).
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement. Batches of 1 000 hashes per `existing` and `orphaned_before` call. Log the counts at `info`.
- [ ] **Step 4:** PASS on SQLite and Postgres, local and s3.
- [ ] **Step 5:** Commit `feat: artifacts GC job and ferrite media gc`.

### Task 7: S3 options and `media sync`

**Files:**
- Modify: `crates/ferrite-media/src/{config.rs,store.rs}` (`S3Config.copy_if_not_exists: Option<String>`, `conditional_put: ConditionalPut`; `build_s3`), `crates/ferrite-config/src/{model.rs,load.rs}` (`[media.s3]` keys, the template, a warning for `multipart`), `crates/ferrite/src/site.rs`, `crates/ferrite/src/media_cmd.rs` (`sync`), `crates/ferrite/src/media_url.rs` (new: `parse_media_url("local:…" | "s3://…") -> MediaStore` through `from_env`)
- Test: `crates/ferrite-config` unit tests, `crates/ferrite-media/tests/store.rs` (an s3s case with `copy_if_not_exists` set, if s3s honours the header; else a builder-config unit test), `crates/ferrite/tests/media_sync.rs`

- [ ] **Step 1:** Write the failing tests:
  - `parse_s3_copy_if_not_exists_values` (valid strings, invalid → error naming the key);
  - `conditional_put_disabled_parses`;
  - `config_check_warns_multipart`;
  - `media_sync_local_to_s3_copies_and_skips` (run twice: the second run copies 0);
  - `media_sync_s3_to_local_from_data`;
  - `media_sync_partial_failure_exits_2`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement. Docs: 03 §2 and §5 (the options, correcting "not set"), 01 §4.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(media): S3 copy_if_not_exists/conditional_put and ferrite media sync`.

### Task 8: `backup` and `restore`

**Files:**
- Create: `crates/ferrite/src/backup/{mod.rs,target.rs,repo.rs,restore.rs}` (`Target::parse(&str, data_dir)` → `File(PathBuf) | Stdout | Repo { store: Arc<dyn ObjectStore>, prefix }`)
- Modify: `crates/ferrite/src/{cli.rs,lib.rs}`, `crates/ferrite/Cargo.toml` (`object_store`, workspace)
- Test: `crates/ferrite/tests/backup.rs`

**Interfaces:** consumes Task 5's `archive` and Task 6's `list_artifacts`. Produces `backup::{run_backup(config, store, media, BackupOptions) -> BackupReport, restore(…)}` and `backup::repo::{write_snapshot, prune(keep)}` for Task 9.

- [ ] **Step 1:** Write the failing tests:
  - `backup_to_archive_and_restore_serves`;
  - `backup_repo_is_incremental` (the second backup copies 0 blobs and its manifest lists them all);
  - `backup_repo_keep_prunes_snapshots_and_unlisted_blobs`;
  - `backup_repo_on_s3` (s3s via `AWS_ENDPOINT_URL`);
  - `backup_no_secret_key`;
  - `backup_refuses_postgres`;
  - `backup_name_survives_pre_migration_prune` (the name does not match `pre-*.db`);
  - `restore_refuses_non_empty`;
  - `restore_force_moves_aside`;
  - `restore_force_refuses_locked` (serve running);
  - `restore_newer_schema_exits_3`;
  - `restore_snapshot_flag_picks_older`;
  - `backup_corrupt_snapshot_fails_integrity` (a test seam corrupts the copy);
  - `backup_files_mode_0600`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement P13 and P14.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(cli): ferrite backup and restore`.

### Task 9: `[backup]` config and the `site.backup` job

**Files:**
- Modify: `crates/ferrite-config/src/{model.rs,load.rs}` (`BackupConfig`), `crates/ferrite/src/config_cmd.rs`, `crates/ferrite/src/serve.rs`
- Create: `crates/ferrite/src/jobs/backup.rs`
- Test: `crates/ferrite/tests/serve/backup.rs` (or the existing serve test layout), plus config unit tests

**Interfaces:** consumes Tasks 5 and 8.

- [ ] **Step 1:** Write the failing tests:
  - `backup_config_parses_and_validates` (`to` required unless off; keep bounds; an unknown key is an error);
  - `config_check_prints_backup`;
  - `scheduled_backup_while_serving_restores`: SQLite serve with `test-seams` period overrides; write content during the job; restore the snapshot into a new folder; serve it and find the content;
  - `scheduled_export_on_postgres_writes_and_prunes` (PG-gated, keep 2 after 3 runs);
  - `backup_off_registers_nothing`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement P15. Document `[backup]` in 01 §4 and 09 §4.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat: scheduled backups ([backup], site.backup job)`.

### Task 10: Optional metadata stripping on ingest

**Files:**
- Create: `crates/ferrite-media/src/strip.rs` (`strip_metadata(bytes: &[u8], mime: &str) -> Result<Option<Vec<u8>>, MediaError>`, where `None` means unchanged)
- Modify: `crates/ferrite-media/src/ingest.rs` (an option on `stage_stream`'s caller-supplied settings: the implementer threads a `strip: bool` through `stage_stream` and its two callers), `crates/ferrite-config` (`[media] strip_metadata`), `crates/ferrite-http/src/media.rs`, the importer's media path
- Test: unit tests in `strip.rs` with fixture images under `crates/ferrite-media/tests/fixtures/` (a JPEG with GPS and Orientation 6, a PNG with eXIf and XMP, a WebP with EXIF and XMP), plus an ingest test

- [ ] **Step 1:** Write the failing tests:
  - `jpeg_gps_removed_orientation_kept_icc_kept` (decode before and after: the pixels are identical, and the EXIF parsed by a tiny reader in the test has only Orientation);
  - `png_exif_and_xmp_removed`;
  - `webp_exif_xmp_removed_flags_fixed` (it still decodes);
  - `no_metadata_returns_none`;
  - `corrupt_image_is_stored_unchanged` (stripping never fails an upload; it logs and keeps the bytes);
  - `ingest_hashes_stripped_bytes_when_on`;
  - `off_by_default_keeps_bytes`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement P17. Docs: 03 §4.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(media): optional EXIF/XMP stripping on ingest`.

### Task 11: `ferrite media regenerate`

**Files:**
- Modify: `crates/ferrite/src/media_cmd.rs` (`regenerate`), with shared variant code factored out of `crates/ferrite/src/jobs/media.rs` (one function used by the job and the command, no behaviour change)
- Test: `crates/ferrite/tests/media_regenerate.rs`

- [ ] **Step 1:** Write the failing tests:
  - `regenerate_adds_new_theme_size` (install a theme with an extra `[images]` size, run it, and the variant exists in `media.variants`);
  - `regenerate_skips_complete_items`;
  - `regenerate_all_redoes`;
  - `regenerate_item_flag`;
  - `regenerate_skips_svg_and_oversize`;
  - `regenerate_failed_item_exits_2`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement P18. Docs: 03 §5 ("Regenerating variants … M4" becomes the command), 09 §8.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(cli): ferrite media regenerate`.

### Task 12: Atomic `seed demo`

**Files:**
- Create: `crates/ferrite-core/src/seed.rs` (`DemoBatch`)
- Modify: the core services that need `*_in` splits, `crates/ferrite/src/seed.rs`
- Test: `crates/ferrite/tests/cli.rs` (seed cases), plus a core unit test

- [ ] **Step 1:** Write the failing tests:
  - `seed_failure_leaves_no_rows`: a `test-seams` hook fails the batch after the posts; then content, terms, menus and options are all unchanged, and `seed demo` without `--force` succeeds afterwards;
  - `seed_force_still_adds_batch`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3:** Implement P20. Docs: 09 §8 (remove "Not atomic").
- [ ] **Step 4:** PASS. The existing seed tests are unchanged.
- [ ] **Step 5:** Commit `feat(seed): demo content in one transaction`.

### Task 13: Cluster example images and the S3 ops docs

**Files:**
- Modify: `examples/cluster/docker-compose.yml`, `examples/cluster/README*` (if present), `crates/ferrite/tests/examples.rs` (endpoint and service names), `docs/design/09-operations.md` (§7, plus the new §4a), `docs/design/03-media.md` §5 (link)
- Test: `cargo test -p ferrite --test examples`; `docker compose -f examples/cluster/docker-compose.yml config --quiet`; a manual `docker compose up --build` with an upload on node1 read back on node2, recorded in the execution ruling. `docker compose down -v` afterwards, removing only this example's volumes.

- [ ] **Step 1:** Research the candidates (P21) and record the choice and its evidence (release date, conditional write support) in the ledger.
- [ ] **Step 2:** Update the compose file with digests, and the tests. The `examples` test passes.
- [ ] **Step 3:** Run `docker compose up --build` and the cross-node check. Record the output.
- [ ] **Step 4:** Write 09 §4a (P22) with its verified / not-verified table.
- [ ] **Step 5:** Commit `chore(examples): maintained S3 image pinned by digest; S3 bucket ops docs`.

### Task 14: Docs and the M4-1 exit

**Files:** `docs/design/{02-storage.md §10, 03-media.md §6/§8, 04-cluster.md (job table, locks), 09-operations.md §4/§8}`, `docs/BRIEF.md:217` ("safe live" corrected), `docs/ROADMAP.md` (tick the M4-1 items, add follow-ups), `CLAUDE.md` (status, crate map: `archive`, `site_copy`, `backup`, `gc`, `media_cmd`), `docs/superpowers/rulings/m4.md` (execution rulings from the ledger), `docs/superpowers/m4-1-exit.md`.

- [ ] **Step 1:** Update the docs. Every new command goes in 09 §8 with its flags and exit codes.
- [ ] **Step 2:** Run all gates (Global Constraints), plus `cargo test --workspace` with `FERRITE_TEST_PG_URL`.
- [ ] **Step 3:** Run P24's end-to-end checks and paste the output into the exit doc, with the "Not verified" table.
- [ ] **Step 4:** Commit `docs: M4-1 exit`.

## Task dependencies

- **Wave 1 (parallel, no shared files):**
  - A: Tasks 1 → 2 → 3 (store);
  - B: Task 4 (Postgres lock);
  - C: Task 10 (strip) → Task 7 (S3 options + sync; creates `media_cmd.rs` if Task 6 has not, and the controller merges the two);
  - D: Task 12 (seed) → Task 13 (examples).
- **Wave 2:** Task 5 (after A), Task 6 (after Task 1), Task 11 (after Task 6's `media_cmd.rs`).
- **Wave 3:** Task 8 (after 5 and 6), then Task 9, then Task 14.

`cli.rs` and `lib.rs` are touched by Tasks 5, 6, 7, 8 and 11. The conflicts are one-line enum and module additions, which the controller resolves at merge.
