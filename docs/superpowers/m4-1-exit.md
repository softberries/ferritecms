# M4-1 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M4; plan
[`2026-10-29-m4-1-data-safety.md`](plans/2026-10-29-m4-1-data-safety.md), Task 14, ruling P24):
*an operator can move a site between backends and machines (`export` / `import-site` / `db copy`
/ `media sync`), back up and restore a solo site by hand or on a schedule, and stored blobs that
nothing references are collected; the M4-1 leftovers (S3 options, location stripping, variant
regeneration, the Postgres lock wait, atomic seeding, the cluster example's images) are closed.
The exit records each command run end to end, a SQLite → Postgres move, a backup taken while
`serve` runs and restored into a new folder, the cluster example brought up, and a "Not verified"
table.*

**Result (2026-10-29, branch `m4-1-exit` from `m4` at `1aa92d4`):** every exit check passed. The
E2E checks (a)–(g) below ran for real on macOS against local PostgreSQL 17 (port 54329) and, for
(g), Docker 29.8.1. One small bug was found and fixed on the way (§3, `c6274fd`); one stale-job
observation is recorded as a follow-up (§3). Gates: §2.

| Exit check | Verified by | Result |
|---|---|---|
| (a) Solo: `init`, `seed demo`, `export --with-media`, a fresh folder, `import-site --with-media`, `serve`, a post page and a media URL | §1 (a) | yes: post `200`, `/_media/…/sample-gradient.png` `200 image/png` 138 510 bytes |
| (b) SQLite → Postgres move: Postgres-backed folder, `import-site` of the SQLite export, `media sync`, `serve`, page renders | §1 (b) | yes: 89 rows, `copied 5 (971995 bytes)`, a second sync `skipped 5`, post and media `200` |
| (c) `db copy` SQLite → Postgres into a fresh schema (and back to a new SQLite file) | §1 (c) | yes: 89 rows both ways; a second copy into the now non-empty schema refused (exit 1) |
| (d) `backup` to a repository folder (twice, `--keep 1`) and to a `.tar.zst`, `restore` both into new folders, `serve`, `media gc --dry-run` | §1 (d) | yes: incremental second run (`0 copied, 5 already in the repository`), restored site serves, new install id, the snapshot carries none |
| (e) Live backup: `serve` with `[backup] schedule = "daily"` runs it at once; three posts written through the API meanwhile; snapshot restored and served | §1 (e) | yes: `scheduled backup done`, the restored site serves and holds the three posts |
| (f) `media regenerate` on the seeded site | §1 (f) | yes: `regenerated 0, skipped 1` by default, `regenerated 1` with `--all`, unknown `--item` exit 1 |
| (g) Cluster example: `up --build -d`, upload via node1, read via node2, `down -v` | §1 (g) | yes: upload `201`, node2 `200 image/png` byte-identical; the GC wrote `ferrite-owner.json` to RustFS |

## What shipped

Per task (merge commits on `m4`; the full text of each ruling is in
[`rulings/m4.md`](rulings/m4.md)):

- **Task 1, storage GC queries** (`5f8fa4e`): migration `0011_artifact_gc` on both backends
  (`artifacts.orphaned_at`, partial index `artifacts_orphaned_at_idx`; expand, not lossy, R3);
  `ArtifactRepo` paging, `existing`, `mark_orphans`, `orphaned_before`, `delete_if_orphaned`;
  `insert_if_absent` clears a mark.
- **Task 2, catalogue and snapshot reader** (`7d14edf`): `ferrite_store::catalogue::TABLES`
  (neutral column types, FK order, `Skip` tables, `schema_meta` only `site_id`), checked against
  the live schema on both backends; `Store::export_reader()` (one read snapshot, keyset pages).
- **Task 3, loader** (`3006383`): `Store::import_writer()`, one write transaction; self-references
  held until the parent is loaded (R6), any failed call poisons it (R7), a missing parent refused
  (R8).
- **Task 4, Postgres lock wait** (`2008d65`): `Store::lock_wait` with one connection,
  `lock_timeout` and a blocking `pg_advisory_lock` (P19).
- **Task 5, archives** (`95f6c3b`): `archive` (`tar.zst`, manifest first, every sha256 checked),
  `ferrite export`, `ferrite import-site`, `ferrite db copy` (`site_copy`, `export_cmd`); missing
  blobs exit 2 and are listed (R10); `db copy` URLs only (R9).
- **Task 6, artifacts GC** (`35bf086`): `gc::run` (mark, delete marked rows then blobs, row-less
  blobs, `tmp/`), the daily `artifacts.gc` job (`jobs/gc.rs`), `ferrite media gc`;
  `Installer::discard_staged` removed (P12).
- **Task 7, S3 options and sync** (`46de23d`): `[media.s3] copy_if_not_exists` and
  `conditional_put` on both builder paths, `MediaStore::list_artifacts` and `copy_to` (R2, R5),
  `ferrite media sync` (`media_url`: `local:` / `s3://`).
- **Task 8, backup and restore** (`4d62ebb`): `ferrite backup` (SQLite only; `VACUUM INTO`,
  `integrity_check`, archive or repository with `ferrite-backup-repo.json`, `artifacts.txt`,
  `--keep`; R11, R12) and `ferrite restore` (staging, `--force` moving the old folder aside,
  mount points, R13).
- **Task 9, scheduled backups** (`f905526`): `[backup]` (`schedule`, `to`, `keep`, `with_media`),
  the `site.backup` job (SQLite repository snapshot, Postgres export archive under `exports/`),
  the location check in `config check` and at `serve` start (R14).
- **Task 10, metadata stripping** (`64ea307`): `[media] strip_metadata`, `ferrite_media::strip`
  (JPEG, PNG, WebP; ICC kept, orientation re-written), uploads and the importer; an image that
  cannot be stripped is refused (R1).
- **Task 11, regenerate** (`dffbbe5`): `ferrite media regenerate [--all] [--item]`, sharing the
  `media.variants` job's `VariantMaker`.
- **Task 12, atomic seed** (`f721bc3`): `ferrite_core::seed::DemoBatch`, every row of
  `seed demo` in one transaction after the blobs.
- **Task 13, cluster example** (`e93f4c9`): RustFS 1.0.4 and `amazon/aws-cli` 2.34.0 instead of
  MinIO, every image pinned by digest; 09 §4a "S3 media buckets" with its claims table.
- **Final-review fixes** (`1aa92d4`): media-store ownership (`schema_meta.install_id`,
  `ferrite-owner.json`, `media gc --claim`; R16, R19), the backup holding `artifacts.gc` (R17), the
  `copy_if_not_exists` placement read-back (R18), JSON `null` in NOT NULL JSON columns, absolute
  SQLite URLs refused for backups.
- **Task 14** (this branch): the docs sweep and this exit, plus `c6274fd` (§3).

Commands added (flags and exit codes in [09 §8](../design/09-operations.md#8-cli-summary)):
`export`, `import-site`, `db copy`, `backup`, `restore`, `media sync`, `media gc`,
`media regenerate`. Config added ([01 §4](../design/01-architecture.md#4-configuration)):
`[backup]`, `[media] strip_metadata` and `gc_grace_days`, `[media.s3] copy_if_not_exists` and
`conditional_put`. Jobs added ([04 §6](../design/04-cluster.md#6-job-queue)): `artifacts.gc`,
`site.backup`; locks `artifacts.gc` and `site.backup` (04 §7). Migration: `0011_artifact_gc`. No
API or OpenAPI change (P23).

Docs changed in Task 14: this file; `rulings/m4.md` (the execution rulings R-pf1–R-pf3 and
R1–R19, as-built notes on P4 and P17); ROADMAP (M4-1 items ticked, stale text fixed, the
tz/seed item split, the M4-1 follow-ups); CLAUDE.md (status, crate map, the new commands); design
01 §1 (crate notes, the config → media dependency), 02 §4 (`orphaned_at`, the migration lists),
03 §8 and 09 §8 (one `AWS_*` list, also in the `media sync` help), 04 §6 and §7 (the GC and
backup rows, the two locks), 06 §1 (theme artifacts are kept), 08 §6 (replaced report files go
to the GC), 09 §4 and §8 (long lines rewrapped), and the `ferrite-core` seed docs.

## 1. End-to-end checks

Binary: `cargo build -p ferrite` (debug). `$E` is the scratch folder
`/private/tmp/claude-501/…/scratchpad/e2e`; `$PG` is
`postgres://ferrite@localhost:54329/ferrite_test`. Log lines and `init`'s "Next steps" are
trimmed. Checks (a)–(e) ran the binary built from `1aa92d4` (plus doc-comment edits); (f) and (g) included the `c6274fd`
fix (it changes one log line of a dry run, §3).

### (a) Solo export and import-site

```
$ ferrite init --data $E/a2/src --url http://localhost:18081            [exit 0]
$ ferrite seed demo --data $E/a2/src
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
$ ferrite export --data $E/a2/src --out $E/a2/site.tar.zst --with-media
Exported 89 rows from 26 tables and 5 artifacts (971995 bytes) at schema 11 to $E/a2/site.tar.zst.
The archive holds password hashes and sealed secrets but not secret.key; keep it private (pipe `--out -` to age or gpg to encrypt it).
[exit 0]
-rw-------@ 1 krzysztofgrajek  wheel  60284 Oct 29 23:29 $E/a2/site.tar.zst
$ ferrite init --data $E/a2/dst --url http://localhost:18081            [exit 0]
$ ferrite import-site $E/a2/site.tar.zst --data $E/a2/dst --with-media
Imported 89 rows (schema 11, exported from sqlite by ferrite 0.1.0 at 2026-10-29T22:29:21.786938Z).
Stored 5 artifacts in the media store (0 were already there).
[exit 0]
$ ferrite migrate status --data $E/a2/dst
database version: 11 / binary version: 11 (ferrite 0.1.0) / pending migrations: none / verdict: up to date
$ FERRITE_SERVER__LISTEN=127.0.0.1:18081 ferrite serve --data $E/a2/dst &
$ curl http://127.0.0.1:18081/ -> 200 10489 bytes
$ curl http://127.0.0.1:18081/welcome-to-ferritecms/ -> 200 8019 bytes
  <title>Welcome to FerriteCMS – FerriteCMS Demo</title>
$ curl http://127.0.0.1:18081/_media/2f6f043d…f60cd684/sample-gradient.png -> 200 image/png 138510 bytes
[serve exit 0 on SIGTERM]
```

### (b) SQLite → Postgres move

```
$ ferrite init --data $E/b/pg --url http://localhost:18082 --db $PG --schema e2e_m41_b
INFO ferrite_store_postgres::migrate: applied migrations [1, …, 11] from=0 to=11        [exit 0]
$ ferrite import-site $E/a2/site.tar.zst --data $E/b/pg
Imported 89 rows (schema 11, exported from sqlite by ferrite 0.1.0 at 2026-10-29T22:29:21.786938Z).
The archive holds 5 artifacts that were not stored; pass --with-media to store them, or copy the media with `ferrite media sync`.
[exit 0]
$ ferrite media sync --data $E/a2/src --to local:$E/b/pg/media
copied 5 (971995 bytes), skipped 0, failed 0                           [exit 0]
$ ferrite media sync --data $E/a2/src --to local:$E/b/pg/media
copied 0 (0 bytes), skipped 5, failed 0                                [exit 0]
$ ferrite migrate status --data $E/b/pg  -> verdict: up to date
$ curl http://127.0.0.1:18082/welcome-to-ferritecms/ -> 200 8019 bytes
$ curl http://127.0.0.1:18082/_media/2f6f043d…/sample-gradient.png -> 200 image/png 138510 bytes
$ psql $PG -Atc "select count(*) from e2e_m41_b.content_items"  -> 16
```

### (c) db copy

```
$ ferrite db copy --from sqlite:$E/a2/src/ferrite.db --to $PG --to-schema e2e_m41_c
INFO ferrite_store_postgres::migrate: applied migrations [1, …, 11] from=0 to=11
Copied 89 rows. Use these lines in the target site's ferrite.toml:

[database]
url = "postgres://ferrite@localhost:54329/ferrite_test"
schema = "e2e_m41_c"

The copy has its own install id: once the site runs on it, its media GC deletes nothing from the media store the old database owns until you run `ferrite media gc --data <DIR> --claim` (do that only when the old database no longer serves the store).
[exit 0]
$ psql … content_items|16  artifacts|5  schema_meta keys|created_by_version,install_id,site_id
$ ferrite db copy --from $PG --from-schema e2e_m41_c --to sqlite:$E/c/back.db
Copied 89 rows. … url = "sqlite://$E/c/back.db"
The database is not named ferrite.db, so it cannot be a data folder's own database: `ferrite backup` refuses such a site (restore puts the database at <data>/ferrite.db). …
[exit 0]
$ sqlite3 $E/c/back.db "select count(*) from content_items;"  -> 16
$ ferrite db copy --from sqlite:$E/a2/src/ferrite.db --to $PG --to-schema e2e_m41_c
Error: the target database is not empty: table users has rows (import into a fresh database made by `ferrite init`)
[exit 1]
```

Both schemas (`e2e_m41_b`, `e2e_m41_c`) were dropped afterwards.

### (d) Backup, restore, media gc

```
$ ferrite backup --data $E/a2/src --to $E/d/repo
Backed up ferrite.toml, secret.key, ferrite.db (database at schema 11, 630784 bytes) to $E/d/repo/snapshots/20261029T223016Z.
Artifacts: 5 copied, 0 already in the repository (971995 bytes copied).                 [exit 0]
$ ferrite backup --data $E/a2/src --to $E/d/repo --keep 1
… to $E/d/repo/snapshots/20261029T223017Z.
Artifacts: 0 copied, 5 already in the repository (0 bytes copied).
Pruned 1 snapshots and 0 artifacts.                                                      [exit 0]
$ ls $E/d/repo  -> artifacts  ferrite-backup-repo.json  snapshots (20261029T223017Z)
$ ferrite backup --data $E/a2/src --to $E/d/site-backup.tar.zst
… Artifacts: 5 (971995 bytes).                                                           [exit 0]
$ ferrite restore $E/d/repo --data $E/d/restored
Restored snapshot 20261029T223017Z of $E/d/repo into $E/d/restored (taken 2026-10-29T22:30:17.703745Z by ferrite 0.1.0, schema 11).
Stored 5 artifacts in the media store (0 were already there).
… verdict: up to date                                                                    [exit 0]
$ ferrite restore $E/d/site-backup.tar.zst --data $E/d/restored-file                    [exit 0]
$ ferrite restore $E/d/repo --data $E/d/restored
Error: $E/d/restored is not empty; restore into a new folder, or pass --force to move it aside
[exit 1]
$ curl http://127.0.0.1:18084/welcome-to-ferritecms/ -> 200 8019 bytes
$ curl http://127.0.0.1:18084/_media/2f6f043d…/sample-gradient.png -> 200 image/png 138510 bytes
$ ferrite media gc --data $E/d/restored --dry-run
dry run: marked 0, unmarked 0, deleted rows 0 (0 bytes), deleted unrecorded blobs 0 (0 bytes), deleted tmp 0; deletes skipped: the media store has no owner marker yet; a pass writes ferrite-owner.json for this site, and deletes start with the pass after it
[exit 0]
$ ferrite media gc --data $E/d/restored
INFO ferrite::gc: wrote ferrite-owner.json for this site; the artifacts GC deletes from the next pass that reads it
marked 0, unmarked 0, deleted rows 0 (0 bytes), deleted unrecorded blobs 0 (0 bytes), deleted tmp 0; deletes skipped: the media store had no owner marker; this pass wrote ferrite-owner.json for this site, and deletes start with the next pass that reads it
[exit 0]
$ ferrite media gc --data $E/d/restored --dry-run
dry run: marked 0, unmarked 0, deleted rows 0 (0 bytes), deleted unrecorded blobs 0 (0 bytes), deleted tmp 0
[exit 0]
```

Install ids (`schema_meta`, R16/R19): the source, the two restored folders and the import-site
target of (a) each have their own `install_id`, all with the source's `site_id`; the repository
snapshot's `ferrite.db` has no `install_id` row:

```
a2/src          install_id 01a189c8-2cd9-…  site_id 01a189c8-2cd8-7741-a202-edfc7b025325
d/restored      install_id 01a189c9-1056-…  site_id (same)
d/restored-file install_id 01a189c9-10a2-…  site_id (same)
a2/dst          install_id 01a189c8-354f-…  site_id (same)
d/repo/snapshots/20261029T223017Z/ferrite.db   site_id only
```

### (e) Live scheduled backup

`ferrite.toml` gained `[backup] schedule = "daily"`, `to = "$E/e/repo"` (absolute, outside the
data folder), `keep = 7`; an admin was made with `user create --password-stdin`.

```
$ ferrite config check --data $E/e/site
ok: mode=solo database=sqlite media=local workers=14 mail=off backup=daily              [exit 0]
$ FERRITE_SERVER__LISTEN=127.0.0.1:18085 ferrite serve --data $E/e/site &
POST /api/v1/auth/login -> 200 (csrf token received)
POST /api/v1/content/post #1 -> 201
POST /api/v1/content/post #2 -> 201
POST /api/v1/content/post #3 -> 201
INFO job{kind=artifacts.gc …}: ferrite::gc: wrote ferrite-owner.json for this site; …
INFO job{kind=site.backup … attempt=1}: ferrite::jobs::backup: scheduled backup done location=$E/e/repo/snapshots/20261029T223243Z schema_version=11 db_bytes=630784 files=ferrite.toml,secret.key,ferrite.db media=true blobs_copied=5 blobs_present=0 blob_bytes=971995 pruned_snapshots=0 pruned_artifacts=0
repo/ferrite-backup-repo.json, repo/artifacts/{2f,36,b2,c6,e6}/…,
repo/snapshots/20261029T223243Z/{artifacts.txt,ferrite.db,ferrite.toml,manifest.json,secret.key}
[serve exit 0 on SIGTERM]
$ ferrite restore $E/e/repo --data $E/e/restored
Restored snapshot 20261029T223243Z of $E/e/repo into $E/e/restored (taken 2026-10-29T22:32:43.803165Z by ferrite 0.1.0, schema 11).
Stored 5 artifacts in the media store (0 were already there).
warning: the restored ferrite.toml schedules backups ([backup] schedule = "daily") to $E/e/repo; a `serve` of this folder writes into that repository too. Set FERRITE_BACKUP__SCHEDULE=off (or change [backup]) before serving a copy that must not.
… verdict: up to date                                                                    [exit 0]
$ curl http://127.0.0.1:18086/welcome-to-ferritecms/ -> 200 8019 bytes
$ curl http://127.0.0.1:18086/_media/2f6f043d…/sample-gradient.png -> 200 image/png 138510 bytes
live site:         Written during the live backup 1, 2, 3
restored snapshot: Written during the live backup 1, 2, 3
site.backup jobs on the live site: done|1
```

The three API writes landed before the snapshot was taken (all three are in it), so this run
shows a backup of a site that `serve` holds open with recent writes, not a write racing the
`VACUUM INTO` itself; that case is the test `scheduled_backup_while_serving_restores`.

### (f) media regenerate

```
$ ferrite media regenerate --data $E/a2/src
regenerated 0, skipped 1, failed 0                                     [exit 0]
$ ferrite media regenerate --data $E/a2/src --all
INFO ferrite::jobs::media: media variants recorded item=01a189c8-34c7-… variants=4
regenerated 1, skipped 0, failed 0                                     [exit 0]
$ ferrite media regenerate --data $E/a2/src --item 00000000-0000-0000-0000-000000000000
Error: 00000000-0000-0000-0000-000000000000 is not a media item
[exit 1]
$ ferrite media gc --data $E/a2/src --dry-run
INFO ferrite::gc: artifacts GC pass dry_run=true marked=0 … deletes_skipped="the media store has no owner marker yet; …"
dry run: marked 0, …, deleted tmp 0; deletes skipped: the media store has no owner marker yet; a pass writes ferrite-owner.json for this site, and deletes start with the pass after it
[exit 0]
```

`--all` re-made byte-identical variants (same hashes), so the dry run marks nothing.

### (g) Cluster example (Docker)

```
$ docker compose -f examples/cluster/docker-compose.yml up --build -d                   [exit 0]
$ docker compose … ps
caddy Up 18 seconds / node1 Up (healthy) / node2 Up (healthy) / postgres Up (healthy) / rustfs Up
$ docker compose … exec -T node1 /ferrite user create --data /data --login e2e --email e2e@example.test --role admin --password-stdin
created user e2e (admin)
$ docker run --rm --network ferrite-cluster_ferrite -v …/g-check:/check:ro node:22-bookworm-slim node /check/check.mjs …
POST node1 /api/v1/auth/login -> 200
POST node1 /api/v1/media -> 201 (6422 bytes, sha256 d54bb56e76a0036ecbefa163c4bd310b434dc1c4aad4405e03145f76ee905533)
  url /_media/d54bb56e…ee905533/cluster-check.png
GET node2 /api/v1/media/01a189d3-a900-7742-8688-befe41e1bbb1 -> 200
GET node2 /_media/d54bb56e…/cluster-check.png -> 200 image/png 6422 bytes, identical: true
GET node1 /api/v1/media/01a189d3-a900-7742-8688-befe41e1bbb1 -> 200
GET node1 /_media/d54bb56e…/cluster-check.png -> 200 image/png 6422 bytes, identical: true
$ docker compose … run --rm --no-deps --entrypoint aws bucket-init --endpoint-url http://rustfs:9000 s3 ls s3://ferrite --recursive
2026-10-29 22:41:52       6422 artifacts/d5/d54bb56e…ee905533   (+ 4 variant objects)
2026-10-29 22:40:31        145 ferrite-owner.json
node2-1 | INFO job{kind=artifacts.gc …}: ferrite::gc: wrote ferrite-owner.json for this site; …
$ curl http://localhost:8080/ (Caddy) -> 200
$ docker compose -f examples/cluster/docker-compose.yml down -v
… Volume ferrite-cluster_rustfsdata Removed, Volume ferrite-cluster_pgdata Removed, Network ferrite-cluster_ferrite Removed
```

The nodes have no HTTP client, so the check ran from `node:22-bookworm-slim` (already present
locally) on the compose network, as in Task 13. The first `artifacts.gc` pass (node2) wrote the
owner marker with RustFS's conditional create.

## 2. Gates

Run on `m4-1-exit` with the Task 14 changes (`CARGO_TARGET_DIR` shared with the final-fix
worktree's build cache):

| Gate | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| the same with `--features ferrite/test-seams,ferrite-http/test-seams,ferrite-import-wp/test-seams` | clean |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | clean |
| `FERRITE_TEST_PG_URL=… cargo test --workspace` with the three `test-seams` features | exit 0: 3 244 passed, 0 failed, 1 ignored, 106 test binaries, no `SKIP` line (the example plugins were built into `plugins/target` first: the nested plugin build inherits `CARGO_TARGET_DIR`, so the first run, with a shared target dir, failed one test reading `plugins/target/…/hello.wasm`; an environment artifact, not a product failure) |

## 3. Found during the exit

- **Fixed (`c6274fd`):** `ferrite media gc --dry-run` on a media store without an owner marker
  logged `ERROR … it does not belong to this site installation`, although nothing is wrong (no
  pass has claimed the store yet, and a real pass logs `INFO` there). A dry run now only reports
  the skipped deletes on stdout. Test: `media_gc_cli_exit_codes` gained the dry run before the
  first pass (red before the fix: the stderr held the `ERROR` line).
- **Recorded, not fixed:** a snapshot taken by the `site.backup` job holds that job's own row as
  `running` (the row is in the database the snapshot copies). In (e) the restored database showed
  `site.backup|running|1`. On a restored site whose `[backup]` is on, that job is claimed again
  once its lease expires and writes one more snapshot (into the same repository when `to` is
  unchanged, which `restore` already warns about); with the schedule off nothing runs it and the
  row stays `running`. Clearing the copied `jobs` rows on restore, as export already skips `jobs`
  (P2), is a ROADMAP follow-up.

## 4. Accepted limits

The accepted residuals of R10, R14, R16, R17 and R19 are listed with their rulings in
[`rulings/m4.md`](rulings/m4.md) and as follow-ups in the ROADMAP ("Deferred from M4-1"): one
site per media store (a copy needs its own `[media]` store), a new shared store waits one GC pass
before deleting, the 00:00 UTC GC/backup contention, the CLI `export --with-media` on Postgres
without the GC lock, and clones made outside Ferrite keeping their install id (09 §4 gives the
reset).

## 5. Not verified

| Claim | Why not verified |
|---|---|
| S3 backup repositories, `media sync` and the owner marker on real providers (AWS S3, R2, B2, Hetzner, Scaleway) | Tests use `s3s-fs` (`backup_repo_on_s3`, `media_sync`); the exit used RustFS only. No provider account. |
| The scheduled Postgres export's S3 upload and its background abort | No test uploads to S3 (`scheduled_export_on_postgres_writes_and_prunes` uses a local `to`); not run here. |
| Loading a scheduled Postgres export with `import-site` | The test only unpacks it; the same `export_to` is round-tripped by the CLI tests. Not run here. |
| `site.backup` on two Postgres nodes at once | Not tested; one run per period rests on the periodic's unique key and the `site.backup` lock, both tested alone. |
| A write racing the `VACUUM INTO` of a scheduled backup | (e)'s writes landed before the snapshot; covered by `scheduled_backup_while_serving_restores` only. |
| `copy_if_not_exists` on any provider, and uploads of 8 MiB or more on RustFS | The example leaves it unset (head + copy fallback); (g) uploaded 6 422 bytes. AWS's answer to `x-amz-copy-source-if-*` values (R18) needs a real AWS call. |
| The 09 §4a rows marked "not verified" (lifecycle JSON field names among them) | Listed in 09 §4a's claims table; not re-checked here. |
| An actual GC deletion in the E2E runs | The grace period is at least 1 day and the first pass on a store deletes nothing (R19); deletions are covered by `gc_deletes_orphan_row_then_blob_after_grace`, `gc_unrecorded_blob_young_kept_old_deleted` and the ownership tests. |
| Two pre-M4-1 sites that already share a store, upgraded together | Not run; the outcome (an arbitrary owner, the other skips deletes) follows from R16/R19 and `gc_first_passes_without_a_marker_delete_nothing`. |
| The GC error policy (failed deletes counted, retried next pass) | No test (parked in Task 6). |
| `media regenerate`'s replaced-row race | No test (needs a seam; parked in Task 11). |
| `db copy`'s undo of a schema step after a panic | No drop guard in async code; not tested. |
| `restore` from a repository writes a new install id | Tested on the archive path; the repository path shares the code. Observed here in (d) for both (`d/restored`, `d/restored-file`). |
| `weekly` schedules end to end | Only `daily` ran here; the period arithmetic is tested. |
| `--to -` / `--out -` piped through `age` or `gpg` | Not run in the exit. |
| TOTP sign-in after `import-site` with the copied `secret.key` | Covered by the cross-backend tests (Review Focus 1); not run here (the demo has no TOTP users). |
| Metadata stripping on real camera photos | Fixtures only (Task 10). |
| `media sync`'s memory peak (about 192 MiB at 8 copies) | Arithmetic in 03 §8, not measured. |
| Linux hosts for the CLI checks | (a)–(f) ran on macOS; (g) ran the Linux image in Docker. |
| The cluster example in CI | Not run in CI (only `docker compose config`, `caddy validate`, `docker build`); run by hand here and in Task 13. |

## 6. Open follow-ups

ROADMAP M4, "Deferred from M4-1" (and the unticked M4 items).

## 7. Verified claims

Claims made in this file, CLAUDE.md, the ROADMAP ticks and the Task 14 design-doc edits, and how
each was checked:

| Claim | Checked against | Verified |
|---|---|---|
| Every command's flags (09 §8 block) | `ferrite <cmd> --help` for the eight new commands | yes |
| Exit codes per command (CLAUDE.md) | the `--help` texts (cli.rs, media_cmd.rs) and the runs above (0, 1 seen; 2 and 3 from help and tests) | yes (help); 2/3 not triggered here |
| `artifacts.gc` / `site.backup` locks and the 300 s backup wait | `ferrite-store/src/lock.rs`, `backup/mod.rs` (`BACKUP_GC_WAIT`, `hold_gc`), `gc.rs` (`try_lock`) | yes |
| A backup's `GcBusy` is exit 1 on the CLI and `Busy` for the job | `run_backup_cmd` (only `Newer` maps to 3), jobs/backup.rs | yes |
| GC deletes only on a pass that reads its own marker | `gc.rs` module doc and `ownership`; (d) and (g) outputs | yes |
| `0011_artifact_gc` adds `orphaned_at` and `artifacts_orphaned_at_idx`, down not lossy | both migration files | yes |
| The URL-given S3 store reads `AWS_COPY_IF_NOT_EXISTS` and `AWS_CONDITIONAL_PUT` | `media_url::s3_from_env` leaves both unset; object_store 0.14.2 `builder.rs` maps every `AWS_*` variable (`aws_copy_if_not_exists`, `aws_conditional_put`, `aws_endpoint_url`) | yes |
| `ferrite-config` depends on `ferrite-media` for `ConditionalPut` and the `copy_if_not_exists` check | `ferrite-config/Cargo.toml`, `load.rs:5`, `load.rs:501` | yes |
| Theme rows are never deleted by the GC and keep their artifact | P10 reference set (`themes.artifact_hash`), no theme delete (P30), `gc.rs` deletes only `artifacts` rows | yes |
| Replaced import report files go to the GC | 02 §4 `import` rows, P10 (only the current `report_hash` and its manifest's hashes are referenced) | yes |
| Strip refusal (R1) | `ferrite-media/src/ingest.rs` docs (`MediaError::Metadata`) | yes |
| The PG scheduled export test uses a local `to`; S3 repo test exists | `backup_schedule.rs` (`to = "../repo"`), `backup.rs::backup_repo_on_s3` | yes |
| Review Focus tests exist | `grep` of each name (`cross_backend_round_trip_is_identical`, `export_import_site_sqlite_to_postgres`, `gc_*`, `import_site_*`, `restore_*`, `scheduled_backup_while_serving_restores`, `pg_lock_wait*`) | yes |
| The cluster images and pins | `examples/cluster/docker-compose.yml` | yes |
| Snapshots carry no `install_id`; restores and copies get their own | `schema_meta` of the folders in (c) and (d) | yes |
| The restored database holds the `site.backup` job as `running` | `sqlite3 $E/e/restored/ferrite.db` | yes |
