# M3 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M3; plan
[`2026-10-29-m3-2-import-job-api-admin.md`](plans/2026-10-29-m3-2-import-job-api-admin.md),
Task 10, ruling P34): *a WordPress site imports through the admin and the API as an
`import.wordpress` job on either profile, with step progress, a stored report (a partial
one for a failed run) and uploads read from a zip; permalink-structure changes get
redirects; the Postgres job claim reads a bounded number of rows; the docs, rulings and
roadmap describe what was built. The exit records the admin import E2E on `chromium-sqlite`
and `chromium-pg`, the full E2E matrix, the Docker cluster example importing export A
through the API with a zipped uploads folder, every workspace gate on both backends and the
P31 EXPLAIN; anything that cannot run is listed as not verified.* M3-1's exit is
[`m3-1-exit.md`](m3-1-exit.md).

**Result (2026-10-29, branch `claude/wizardly-jepsen-4d64dc`: the gates ran on `fd96665` =
`m3-2-import-ui` at `ee4fb73` (the final-review fixes) merged into this branch, plus
`aa32aff`, an E2E harness fix found by the matrix):** every exit check passed. The
workspace tests passed on SQLite and on PostgreSQL 17 (3 003 passed, 0 failed, 1 ignored,
each run; the first Postgres run had 3 load flakes, §3). `e2e:all` passed 772 of 772 in
all four projects after one harness fix: Firefox fetches `/favicon.ico` for the sandboxed
report page, which its CSP refuses (no product change). The Docker cluster example imported
export A with its uploads zipped through the API on node1; both nodes served the post at its
old URL with its image read from MinIO (§2). The example's MinIO images can no longer be
pulled, so the run used Bitnami's legacy builds of the MinIO server and client
through an override file (§2, §5).

| Exit check | Verified by | Result |
|---|---|---|
| 1. Admin import of export A (`import.spec`) on `chromium-sqlite` and `chromium-pg` | §3 | yes: in the `e2e:all` run (both, and Firefox and WebKit); the report opens and the post answers at `/2024/03/every-core-block/` |
| 2. Full E2E matrix (`e2e:all`, four projects) | §3 | yes after `aa32aff`: 772 passed (193 per project); the first run 771 passed, 1 failed (Firefox only, the favicon CSP report) |
| 3. Docker cluster example: import through the API on node1, both nodes serve the post, media from MinIO | §2 | yes, with the MinIO images swapped (§2); which node ran the job was not recorded |
| 4. Workspace gates on both backends (`fmt`, both `clippy` variants, rustdoc, `cargo test --workspace` twice, `corpus_real`, the admin gates) | §3 | yes: 3 003 passed, 0 failed, 1 ignored, each backend; corpus counts identical to M3-1's; 724 Vitest tests in 122 files |
| 5. P31 EXPLAIN (`FERRITE_PG_EXPLAIN=1`) | §1 | yes: `scan rows read: 12 (bound 85)`, 0.656 ms |

What was built is listed in [ROADMAP](../ROADMAP.md) (M3-2, the ticked items) and
described as built in design [08](../design/08-wp-import.md) §1, §2 and §6,
[07](../design/07-admin-api.md) §1 and §3, [04](../design/04-cluster.md) §6–§7 and
[11](../design/11-admin-ui.md) §6–§7; the rulings are P23–P34 and R52–R83 in
[`rulings/m3.md`](rulings/m3.md). In short:

- Storage: migration `0010_import_jobs` on both backends (`artifacts.kind` takes `import`;
  `import_runs.job_id`, `step`, `report_hash`, `import_runs_job_idx`); Postgres is an expand
  migration (one `ALTER TABLE` swaps the CHECK; the expand lint accepts a strictly widening
  same-name `IN` list, R56), SQLite a contract one (a table rebuild, no min-binary).
- Core: `ImportService::enqueue` (the conflict rule under `LOCK_IMPORT`, the inputs recorded
  as `import` artifacts, the payload's user from the actor), `importer`, `job_may_start`,
  `store_report`/`report` through the `ArtifactBlobs` trait (a manifest artifact over the two
  report files), the token-free run reads, `ImportHandle::set_step`; a job's start takes over
  a stale run without a live job (R69).
- Importer: step progress (`on_step`, the fenced `set_step` between steps), `RunFailed` with
  a partial report, `ZipSource` (the end records read first, NFC names, stored and deflate
  only); the CLI writes a failed run's report and takes `--uploads x.zip`.
- The `import.wordpress` job (`crates/ferrite/src/jobs/import.rs`): 6 h, 3 attempts, inputs
  fetched to `<data>/import/job-<id>/`, `Busy` before any fetch while another run is live,
  inputs deleted once the run is done or the job fails for good.
- API: `POST /import/wordpress` (multipart, streamed into the media store, `Idempotency-Key`,
  `202`/`409`/`413`), `GET /import/jobs/{id}`, `/import/runs`, `/import/runs/{id}`, its
  `report.json` and sandboxed `report.html`; one node `ImportService` shared by the API and
  the job (`serve.rs`, `start`).
- Admin: the Import screen (`/admin/import`, `g i`) and the SEO canonical and robots fields.
- `permalink.redirects` (P30) and the Postgres claim's `UNION ALL` split (P31).

Docs changed: this file; `rulings/m3.md` (as-built notes on P24–P31, execution rulings
R52–R83); ROADMAP (M3-2 and M3 ticked, the M3 follow-ups under M4); CLAUDE.md (status,
crate map, how to import through the admin and the API); design 00 (the SEO key line
verified), 01 §1 and §2 (the crate notes, Workers), 02 §4, §7 and §8 (`0010`, the expand
lint), 04 §6 and §7 (the `import.wordpress` and `permalink.redirects` jobs, the `import` lock), 07 §1 and §3 (the Import
rows, the SEO fields, the status line), 08 §1, §2, §5 and §6 (as built), 09 §8 (the
command, imports through the admin and the API; the planned line removed), 11 §6 and §7
(`g i`, the Import screen).

## 1. P31 EXPLAIN

`FERRITE_PG_EXPLAIN=1 FERRITE_TEST_PG_URL=… cargo test -p ferrite-store-postgres --test
claim_explain -- --nocapture` (50 000 future `content.publish_scheduled` rows, 3 due, 2
expired leases; PostgreSQL 17.11). The plan of the `UNION ALL` claim (Task 7, P31):

```
Limit (actual rows=5 loops=1)  Buffers: shared hit=312
  ->  Sort  Sort Key: due.priority DESC, due.run_at, due.id
        ->  Append (actual rows=5 loops=1)
              ->  Subquery Scan on due (actual rows=3)  Buffers: shared hit=308
                    ->  Limit -> LockRows
                          ->  Index Scan using jobs_claim_idx on jobs (actual rows=3 loops=1)
                                Index Cond: (run_at <= now())
                                Filter: ((kind = ANY ('{content.publish_scheduled}'::text[])) AND (status = 'queued'::text))
                                Rows Removed by Filter: 2
                                Buffers: shared hit=305
              ->  Subquery Scan on expired (actual rows=2)  Buffers: shared hit=4
                    ->  Limit -> LockRows -> Sort
                          ->  Index Scan using jobs_lease_idx on jobs jobs_1 (actual rows=2 loops=1)
                                Index Cond: (locked_until < now())
                                Filter: (… AND (attempts < max_attempts) AND (status = 'running'::text))
Planning Time: 0.121 ms
Execution Time: 0.656 ms
scan rows read: 12 (bound 85)
test claim_reads_a_bounded_number_of_rows ... ok
```

"Rows read" is `Actual Rows` plus `Rows Removed by Filter`/`Index Recheck`, times loops (R53).
Before Task 7's split the same test read 50 005 rows (the controller's ledger, Task 7). The
due branch's 305 buffer hits are the index tuples of the future jobs it walks (R54).

## 2. Docker cluster example

`examples/cluster/` (2 nodes, Postgres 18, MinIO, Caddy), macOS 26.6 arm64, Docker 29.8.1,
Compose 5.5.1. `minio/minio` and `minio/mc` are refused by Docker Hub (and quay.io) as of
2026-10-29 ("pull access denied"), so the run added an override file that only swaps the two
images for `bitnamilegacy/minio` and `bitnamilegacy/minio-client` (builds of the MinIO server and `mc`,
"Copyright 2015-2025 MinIO, Inc."), with the same credentials and bucket; the example's
files are unchanged. Requests went straight to each node on the compose network (a
`curlimages/curl` container); `$SP` is the session scratchpad.

```
$ docker compose -f examples/cluster/docker-compose.yml -f $SP/minio-override.yml up --build -d
  node1 healthy, node2 healthy (2 min 36 s, the image build included)
$ printf '<password>\n' | docker compose … exec -T node1 /ferrite user create --data /data \
    --login admin --email admin@example.test --password-stdin
  created user admin (admin)
$ curl … POST http://node1:8080/api/v1/auth/login            # session cookie + csrf_token
$ curl … -H 'Idempotency-Key: m3-exit-1' -F wxr=@site-a.xml -F uploads=@uploads.zip \
    POST http://node1:8080/api/v1/import/wordpress
  HTTP/1.1 202 Accepted
  location: /api/v1/import/jobs/01a187b6-be9e-7673-837a-1f6f78a09bdd
  {"attempts":0,"job_id":"01a187b6-…","last_error":null,"run":null,"status":"queued"}
$ curl … GET http://node1:8080/api/v1/import/jobs/01a187b6-…  (first poll, under 1 s)
  "status":"done","attempts":1,"run":{"state":"done","step":"report","source":"http://localhost:8099",…}
$ curl … GET …/import/runs/<run>/report.json
  created: book 2, comments 4, media 4, menus 1, options 3, page 4, post 8, redirects 7,
  taxonomies 1, terms 9, types 1, users 3; failed: null; missing_media: 0
$ curl http://node1:8080/2024/03/every-core-block/   200 <title>Every core block - Corpus Site</title>
$ curl http://node2:8080/2024/03/every-core-block/   200 <title>Every core block - Corpus Site</title>
  both: src="/_media/d54bb56e…/hero.png", src="/_media/817d4d9a…/square-one.png"
$ curl http://node2:8080/_media/d54bb56e…/hero.png   200 image/png 6422 bytes
$ mc ls --recursive local/ferrite | grep d54bb56e…
  [2026-10-29 12:51:03 UTC] 6.3KiB STANDARD artifacts/d5/d54bb56e…
$ mc ls --recursive local/ferrite | grep <sha256 of site-a.xml>; … | grep <sha256 of uploads.zip>
  (no lines: both input artifacts were deleted once the run was done)
$ curl http://localhost:8080/2024/03/every-core-block/  200 (through Caddy)
$ /ferrite config check --data /data                     ok: mode=cluster database=postgres media=s3 workers=14 mail=off
$ docker compose … down -v                               the example's two volumes removed
```

Users: 3 created and 1 unchanged; the report notes that the WXR's author `admin` was matched
to the account with the same login, not by e-mail (that account was made above with another
address). The four `media.variants` jobs ran on both nodes (one on node1, three on node2, from
the `docker compose logs`). Which node ran the `import.wordpress` job is not recorded: the
job row clears `locked_by` when it is done, and no line of either node's log names that job.

## 3. Gates

Run on `fd96665` (the Rust gates, the admin gates), the E2E matrix again on `aa32aff`.
macOS 26.6 arm64, `rustc` 1.99.1 (`rustup update stable`: unchanged), Node 24.18.0, pnpm
10.28.2, PostgreSQL 17.11 (Homebrew).

```
$ rustup update stable                                                    # stable-aarch64-apple-darwin unchanged - rustc 1.99.1
$ cargo fmt --all --check                                                 # exit 0
$ cargo clippy --workspace --all-targets -- -D warnings                   # exit 0
$ cargo clippy --workspace --all-targets \
    --features ferrite/test-seams,ferrite-http/test-seams,ferrite-import-wp/test-seams \
    -- -D warnings                                                        # exit 0
$ RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps              # exit 0
$ cargo test --workspace --no-fail-fast                                   # 98 targets: 3003 passed; 0 failed; 1 ignored
$ FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test \
    cargo test --workspace --no-fail-fast                                 # first run: 3000 passed; 3 failed; 1 ignored
                                                                          # re-run:    3003 passed; 0 failed; 1 ignored
$ FERRITE_WP_CORPUS=… cargo test -p ferrite-import-wp --test corpus_real  # 1 passed; 79 + 62 items editor-safe
$ FERRITE_PG_EXPLAIN=1 … --test claim_explain                             # 1 passed (§1)
$ pnpm -C admin install --frozen-lockfile                                 # exit 0
$ pnpm -C admin lint                                                      # exit 0
$ pnpm -C admin typecheck                                                 # exit 0
$ pnpm -C admin test                                                      # Test Files 122 passed (122); Tests 724 passed (724)
$ pnpm -C admin api:check                                                 # src/api/schema.d.ts is current
$ pnpm -C admin build                                                     # check-dist: ok
$ pnpm -C admin bundle:check                                              # bundle:check: ok (largest_lazy 28 099 of 31 744)
$ cargo build --release -p ferrite                                        # 2m 34s
$ cargo build --manifest-path plugins/Cargo.toml --target wasm32-wasip2 --release -p admin-e2e
$ FERRITE_TEST_PG_URL=… pnpm -C admin e2e:all                             # at fd96665: 1 failed, 771 passed (10.8m)
$ FERRITE_TEST_PG_URL=… pnpm -C admin e2e:all e2e/import.spec.ts          # at aa32aff: 8 passed, four projects
$ FERRITE_TEST_PG_URL=… pnpm -C admin e2e:all                             # at aa32aff: 772 passed (10.3m)
```

- **The three Postgres-run failures were load flakes.** `job_kinds_are_in_meta::postgres`
  (`crates/ferrite/tests/node.rs`: the node meta was read before it held `job_kinds`) and
  `login_rate_limit_spans_nodes::{sqlite,postgres}` (`crates/ferrite-core/tests/auth.rs`:
  "expected RateLimited, got Ok(Session…)") failed once while the machine was busy; each
  passed 3 of 3 runs alone (load average 9), and the whole
  Postgres workspace run then passed (3 003). Neither test nor the code it checks changed in
  M3-2. Listed as follow-ups.
- **The E2E failure** was `import_export_a_through_the_screen` on `firefox-sqlite` only:
  `securitypolicyviolation: img-src blocked http://127.0.0.1:…/favicon.ico at resource:232`
  plus Firefox's console line for it. Firefox's `FaviconLoader` requests `/favicon.ico` for
  the opened `report.html`, whose `default-src 'none'` refuses it. `aa32aff` lets
  `guard.allow` cover CSP reports and allows exactly that load in the spec, as the spec
  already allowed Chromium's sandboxed-script log line; the report's CSP is unchanged.
- **The early-signal run on `3eb9edc`** (before the final-review fixes) also had 3 Vitest
  failures (5 s timeouts and a shell route test) while a Postgres workspace run went on
  beside it; on `fd96665` on an idle machine Vitest passed 724 of 724.
- The SQLite-only workspace run passes its Postgres cases by skipping them; the second run
  executes them. `CI` was not set. The ignored test is `ferrite-sdk`'s `export_plugin` doc
  example.

## 4. Accepted limits (rulings)

- **The `import` i18n namespace is loaded eagerly** (AD-m5, R83): `import.json` is in the
  entry chunk, like the other namespaces; the bundle budget holds.
- **A redirect can shadow an item published concurrently at an old path** (BE-m1, R76): a
  `permalink.redirects` page transaction can write a redirect from a path an item is
  published at in the same moment; a narrow window, documented in 06.
- **Inputs can outlive a job** (R68, R70): a lease that expires on the last attempt, a runner
  timeout or a panic leaves the inputs; an upload's unrecorded blobs stay when the 5 s lock
  wait fails. The M4 orphan sweep collects them.
- **A job whose lease expired counts as live until it is reclaimed** (R69): a stale run of
  such a job is not taken over meanwhile.
- **A retry on another node parses again** (P27): only the same node resumes the IR.
- **A partial report counts only the last attempt** (P29), and a replaced report's files stay
  until the store is cleaned (M4).
- **`permalink.redirects`** misses an item whose `published_at` moves later during the job
  (R62) and may add redirects from never-live paths of imported posts (R63).
- **The claim's due branch still walks future jobs' index tuples** (R54): about 0.5 ms at
  50 000; a `run_at`-leading index is an M4 follow-up.
- **`zip =9.1.3` was published two days before its use** (R59).
- **A referenced input row's delete is logged at `info`** (final-review re-check minor): the
  row delete of an import input fails when a media item references it (R74), which is
  expected and logged at `info`; any other database error there is logged at `info` too.
- **The run-folder sweep can race a starting run's `prepare_ir`** (final-review re-check
  minor): an attempt's sweep of a run folder whose job is over, and a run of the same file
  that has just found that folder and renames it, can meet in a narrow window; the rename
  then fails and the start errors (a job retries it, a CLI run exits 1).

## 5. Not verified

- **The example's own MinIO images**: `minio/minio` and `minio/mc` could not be pulled
  (2026-10-29); the cluster check ran with Bitnami's legacy builds of the MinIO server
  and client (§2). The example still names the unavailable images (a follow-up).
- **Which node ran the cluster import**: not recorded (§2); the job may run on either
  (P27), and both nodes served the result.
- **A retry of the cluster import on the other node**: the cross-node re-parse (P27) is
  covered by `import_job`'s `job_retry_on_fresh_node_reparses` (two data folders, both
  backends), not by Docker.
- **The import over S3 in the tests**: the automated tests import onto local media; S3 (MinIO)
  was used only by the Docker run.
- **Large imports**: only exports A and B and WordPress's two theme-test files; no
  production export, nothing near the 512 MiB WXR or 4 GiB zip caps, no 500 000-entry zip,
  no site where `permalink.redirects` needs a continuation outside its test (R75).
- **The 6 h timeout and the last-attempt paths in a real runner**: the timeout is a
  constant checked by a unit test; inputs left by a lease expiring on the last attempt
  wait for the M4 sweep (R70).
- **`--fetch-media` against a real remote site** (as in M3-1).
- **A manual Safari pass and real mobile devices**: WebKit ran only as Playwright's build.
- **CI on GitHub**: nothing pushed from this branch.

## 6. Open follow-ups

Listed in [ROADMAP](../ROADMAP.md) under M4, "Deferred from M3".

## 7. Verified claims

Every claim in this file and in the M3-2 doc changes, with how it was checked.

| Claim | Where | Checked by |
|---|---|---|
| Workspace tests: 3 003 passed, 0 failed, 1 ignored on each backend; the first Postgres run 3 failed | §3 | the `cargo test --workspace` logs (summed `test result` lines) |
| The 3 failures pass alone (3 of 3 each) and in the full re-run | §3 | the three re-runs; the second Postgres log |
| The ignored test is `ferrite-sdk`'s `export_plugin` doc example | §3 | `grep ignored` in both logs |
| Corpus counts identical to M3-1's; 79 + 62 items editor-safe | Result, §3 | `corpus_real` output vs `m3-1-exit.md` §1 |
| Admin: 122 files, 724 tests; bundle `largest_lazy` 28 099 of 31 744 | §3 | `pnpm -C admin test`, `bundle:check` output |
| `e2e:all`: 771 + 1 failed, then 772 passed, 193 per project | Result, §3 | the two runs' summary lines; project names counted in the log |
| The Firefox failure was the favicon CSP report | §3 | the Playwright error text |
| `import.spec` asserts the report link and the post at its old URL | check 1 | `admin/e2e/import.spec.ts` (`OLD_URL`, "Open report") |
| EXPLAIN: 12 rows read (bound 85), 0.656 ms; 50 005 before the split | §1 | `claim_explain` output; the ledger's Task 7 line |
| Cluster import: `202`, `Location`, job `done` at step `report`, counts, no missing media | §2 | the `curl` responses |
| Both nodes serve the post; the image is in the MinIO bucket; inputs deleted | §2 | `curl` per node; `mc ls` for the image hash and both input hashes |
| `minio/minio` and `minio/mc` cannot be pulled | §2, §5 | `docker pull` errors (Docker Hub, quay.io) |
| `0010_import_jobs`: columns, `import_runs_job_idx`, Postgres expand, SQLite contract | 02 §4, §7 | both migrations' SQL and headers |
| Expand lint accepts a strictly widening same-name `IN` CHECK | 02 §8, R56 | `ferrite_store::migrate::expand_lint` and `expand_lint_constraint_swaps` |
| Caps 512 MiB / 4 GiB / 4 KiB; report CSP; `private, no-store` | 07 §1, 08 §1 | `IMPORT_MAX_WXR`, `IMPORT_MAX_ARCHIVE`, `MAX_OPTIONS_PART`, `REPORT_CSP`, `REPORT_CACHE` in `crates/ferrite-http/src/api/import.rs` |
| Routes `/import/wordpress`, `/import/jobs/{job_id}`, `/import/runs`, `/import/runs/{id}`, `report.json`, `report.html` | 07 §1, 08 §1, CLAUDE.md | the `utoipa::path` attributes in `api/import.rs` |
| `409` is `CoreError::Changed`; the pre-check before the body | P25, 07 §1 | `api/problem.rs`; `start_wordpress_import` docs |
| Options `dry_run`, `keep_permalinks`, `site_options`, `fetch_media`, `category_base`, `tag_base` | 08 §1, CLAUDE.md | `ImportJobOptions` in `crates/ferrite-core/src/jobs.rs` |
| Job: 6 h, 3 attempts, payload shape, error mapping, `Busy` before fetch, sweeps | 04 §6, 08 §1 | `IMPORT_TIMEOUT`, `IMPORT_WORDPRESS_ATTEMPTS`, `ImportJobPayload`, `jobs/import.rs` (`core_error`, `import_error`, `job_may_start`, `sweep_*`) |
| Takeover of a stale run without a live job, in one statement | R69, 08 §2 | `abandon_stale_for_job` (both backends), `abandon_stale_for_job_spares_live_jobs` |
| Inputs: row first, blob only after; referenced row logged at `info` | R74, 04 §6 | `delete_inputs` in `jobs/import.rs` |
| Enqueue waits 2 s for `LOCK_IMPORT` | R79, 04 §7 | `ENQUEUE_LOCK_WAIT` in `crates/ferrite-core/src/import/jobs.rs` |
| `permalink.redirects`: 500 per transaction, 20 pages per job, continuation key with a 32-hex digest | R75, 04 §6 | `STRUCTURE_PAGE`, `STRUCTURE_JOB_PAGES` in `redirects.rs`; `permalink_redirects_job` |
| Report manifest `{"v":1,"json","html"}`, 16 MiB per file | 08 §6, P29 | `Manifest`, `MAX_REPORT_BYTES` in `import/jobs.rs` |
| Zip limits 500 000 entries, 64 MiB directory, 64 GiB declared; NFC names | P24, R58 | `MAX_ENTRIES`, `MAX_DIRECTORY`, `MAX_DECLARED`, `segments` in `zip_source.rs` |
| `zip =9.1.3` with `deflate-flate2-zlib-rs`; `unicode-normalization =0.1.26` | R59 | workspace `Cargo.toml`; crates.io (9.1.3 published 2026-10-27) |
| Admin: `/admin/import`, `g i`, between Plugins and Settings, polling every 2 s | 07 §3, 11 §6–§7 | `shell/nav.ts`, `IMPORT_POLL_MS` in `api/import.ts` |
| SEO canonical and five robots toggles | 07 §3 | `SeoDraft`, `seo-section.tsx` |
| Yoast, Rank Math and ACF keys verified | 00 | `grep` of the keys in `tests/corpus/site-a.xml` and `site-b.xml` |
| Toolchain and versions | §2, §3 | `rustc --version`, `node --version`, `pnpm --version`, `select version()`, `sw_vers`, `docker --version` |
