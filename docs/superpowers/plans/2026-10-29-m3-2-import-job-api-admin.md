# M3-2 — WordPress Import: Job, API, Admin Screen and the M3 Exit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Admins can run a WordPress import from the admin (upload, progress, report), the import runs as a cluster job on either profile, changing `permalink.*` redirects the old URLs, and M3 exits.

**Architecture:**
- **Inputs:** the uploaded WXR and an optional uploads zip are stored as artifacts in the `MediaStore`, so any node can run the job.
- **Job:** an `import.wordpress` job fetches the artifacts to a local work folder and calls `ferrite_import_wp::run::run`. It stores the report as an artifact and links it from `import_runs`.
- **API and admin:** the REST API starts jobs and reads runs, jobs and reports. The admin screen polls them.
- **Permalink redirects:** a `permalink.redirects` job, enqueued by every `permalink.*` write, walks published items and redirects their old paths using the existing redirect write path.

**Tech Stack:**
- Rust: tokio, sqlx, axum, utoipa.
- The `zip` crate, new: exact version, `deflate` feature only.
- Admin: React, TanStack Router and Query, openapi-fetch, Vitest, Playwright.

**Spec:**
- Design docs:
  - docs/design/08-wp-import.md §1, §2, §5, §6;
  - 07-admin-api.md §1 (Import row, Idempotency-Key conventions 07:20-46);
  - 04-cluster.md §6–§7 (jobs, locks);
  - 11-admin-ui.md §7–§8.
- Rulings:
  - docs/superpowers/rulings/m3.md P1–P22, R1–R51;
  - m2a2b.md P10 and P26;
  - m15.md P9, P13 and R17;
  - m2b.md P25, P28, P55, P56 and R62.
- Research digest (git-ignored, every fact with file:line): `.superpowers/m3-2-spec-digest.md`.

## Plan rulings (written to docs/superpowers/rulings/m3.md, section "M3-2 plan rulings", at Task 1)

**P23: inputs are artifacts.**
- `POST /import/wordpress` streams the `wxr` part, and the optional `uploads` zip part, into the `MediaStore` (`put_stream`, SHA-256 addressed) as artifacts of a new kind `import`. The migration adds `import` to the `artifacts.kind` CHECK on both backends.
- The job downloads them to `<data>/import/job-<job-id>/` on whichever node claims it.
- Caps:
  - WXR: `IMPORT_MAX_WXR` = 512 MiB.
  - Zip: `IMPORT_MAX_ARCHIVE` = 4 GiB.
  - The route body limit is their sum plus 1 MiB of framing.
- The input artifacts are deleted when the run reaches `done`, or when the job goes `dead`.
- Reason: in cluster mode the node that receives the request is not the one that runs it.
- Cost: the uploaded data briefly exists twice, once in the store and once in the job's local copy.

**P24: an uploads archive is a zip, read in place.**
- `ZipSource: MediaSource` resolves attachment paths against the archive's central directory, without extracting it.
- Entries are matched by their path below the first `uploads/` segment, or else by the full path. An entry name that is absolute or contains `..` is refused.
- Per entry, `max_upload` applies, plus a declared-vs-actual size check. The archive is refused when it has more than 500 000 entries or more than 64 GiB of total declared uncompressed size.
- Only stored and deflate entries are read.
- Reason: WordPress uploads ship as zips, and reading in place needs no extraction disk space.
- Cost: one new dependency.

**P25: the request.** The request is multipart with these parts:
- `wxr` (required);
- `uploads` (optional);
- `options` (optional JSON, at most 4 KiB): `{dry_run, keep_permalinks, site_options, fetch_media, category_base, tag_base}`, with the CLI's defaults.

Requests are idempotent with `Idempotency-Key`, using the media upload's rules (07:20-46).

The response is `202 Accepted` with `ImportJobDto` and `Location: /api/v1/import/jobs/{job_id}`.

Errors:
- `413` when the body is over a cap.
- `400` for a malformed body or a malformed `options` part.
- `409 conflict` while another import job is queued or running. Starting checks the job queue for `import.wordpress` in `queued` or `running` and the `import_runs` running row.

`takeover` is not exposed; the job decides it (P27).

**P26-1: the read API.**
- `GET /import/jobs/{job_id}` returns `{job_id, status, attempts, last_error, run: ImportRunDto?}`. Polling targets this before the run exists.
- `GET /import/runs?limit&cursor` returns runs newest first.
- `GET /import/runs/{id}` returns `ImportRunDto {id, source, state, step, dry_run, started_by, started_at, heartbeat_at, finished_at, counts, error?, job_id?, report: bool}`.
- `GET /import/runs/{id}/report.json` and `GET /import/runs/{id}/report.html` return the report. The HTML is sent with `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox`, `X-Content-Type-Options: nosniff` and `Cache-Control: private, no-store`.
- Every route requires `Capability::Import`: `import:read` to read, `import:write` to start.
- Design 07's `GET /import/{job}` is "as built" as these routes.
- Cost: a wider API than the doc's single row.

**P27: the job.**
- Kind `import.wordpress`, payload `v` 1: `{wxr, uploads?, options, user, source_name}`, where `user` is the POSTing admin's id. There is no `unique_key`; the enqueue checks P25's conflict rule.
- `max_attempts` 3; `timeout()` 6 h.
- At run time the user must still be an active admin. A user who is no longer an active admin is `Permanent`, and so is a missing artifact.
- Error mapping:
  - `CoreError::Unavailable` (busy) → `JobError::Busy`.
  - `CoreError::Changed` (taken over) → `Permanent`.
  - Input or parse errors (`ImportError::Xml`, `Php`, `Invalid`) → `Permanent`.
  - Anything else → `Retry`.
- On retry, the job takes over its own previous run when that run is stale and its `job_id` equals this job's. Otherwise `takeover` is false.
- The job's working folder is node-local. A retry on another node re-parses, and `import_map` keeps that idempotent.
- Reason: at-least-once execution (04:298) with resume on the same node.
- Cost: a retry on a different node redoes the parse.

**P28: run progress.**
- Migration 0010 adds to `import_runs`:
  - `job_id uuid NULL`;
  - `step text NULL`, one of `parse`, `pass1`…`pass4`, `fixup` or `report`;
  - `report_hash text NULL`, the report artifact.
- `ImportHandle::set_step(name)` writes `step` with the run-token fence. The driver calls it as each step begins; this is production code, not the test seam.
- The admin shows "step N of 7" plus `heartbeat_at`.
- Cost: no per-item percentage.

**P29: a failed run still gets a report.**
- `run::run` returns `Result<Report, RunFailed { error: ImportError, report: Option<Report> }>`.
- After the run has started, every failure builds the report from what is done, using the IR, which survives a failed run. The report gets `failed: Some({step, error})` and a leading note.
- The CLI writes the report files and still exits 1. The job stores the report artifact on every attempt.
- Cost: a partial report's counts are those of the last attempt only.

**P30: permalink-change redirects (M2a-2b P26).**
- Every write of `permalink.post` or `permalink.page` enqueues a `permalink.redirects` job, in the same transaction as the write. This covers `OptionService::set_many`/`set`/`delete` and the importer's `ImportBatch::options`.
- The job's payload holds the previous and the new `permalink.*` values.
- The job walks published items in pages of 500 by id. For each one it computes `old = path(prev rules)` and `new = path(current rules)`. Where they differ it writes a redirect `old → new`:
  - Origin: `slug-change`. There is no new origin, so no CHECK rebuild.
  - Chains: `move_redirects`, the same as `record_item_move`.
  - A source that is a live published path is refused.
  - Existing rows with origin `manual` or `import` are kept.
- `unique_key` is `permalink.redirects:<options-version>`. A later change re-runs with its own previous values, and chains collapse through `move_redirects`.
- Term-path changes (`%category%` on a term rename) are out of scope; they are still unredirected and noted in 06.
- Between the settings commit and the job finishing, old URLs 404 for that short window.
- The Settings confirm copy (`admin.json:501`) changes to say that redirects are created.
- Cost: that window; one redirect row per moved item.

**P31: Postgres claim EXPLAIN.**
- An env-gated test (`FERRITE_PG_EXPLAIN=1` plus `FERRITE_TEST_PG_URL`) seeds 50 000 future `content.publish_scheduled` rows, 3 due rows and 2 expired leases. It runs `EXPLAIN (ANALYZE, BUFFERS)` on the claim and prints the plan.
- Pass: rows read by the claim's scan ≤ 10 × `LIMIT` + due rows.
- If the current query fails, split it into `UNION ALL` of the due branch (`jobs_claim_idx`) and the expired-lease branch (`jobs_lease_idx`), as SQLite does, then re-run.
- The plan output goes in the exit doc either way. No migration unless an index is needed; if it is, that becomes a ruling.

**P32: the admin import screen.**
- Route `/admin/import` (`_app/import.tsx`), in the sidebar's admin section between Plugins and Settings, shown when the user has `can("import")`.
- Chord: `g i` if it is free in 11 §6, otherwise none; record which.
- Layout:
  - A page header.
  - A start card with a `DropZone single` for the WXR, an optional zip `DropZone single`, toggles for dry run, keep permalinks, site options and fetch media, an "Advanced" disclosure for the category and tag bases, and one primary "Start import" button.
  - An active-run panel, polling `GET /import/jobs/{id}` every 2 s while the job is `queued`/`running`. It shows the step stepper, heartbeat `RelativeTime`, counts and the error.
  - A runs `DataTable` with started, source, state, step, created/updated/kept/skipped and report links. HTML opens in a new tab; JSON is a download.
- Upload uses XHR with progress and `Idempotency-Key`, as `uploadMedia` does.
- Every 11 §8 checklist item applies, including the axe checks, both colour schemes and mobile width.
- The lazy route chunk must fit `largest_lazy` in `admin/bundle-budget.json`, unchanged.

**P33: SEO canonical and robots in the editor.**
- `SeoSection` gains a canonical URL input (`type="url"`, error from `seo.canonical`) and five robots toggles: `noindex`, `nofollow`, `noarchive`, `nosnippet`, `noimageindex`.
- `SeoDraft`, the autosave slot, the save patch and the 412 conflict diff all carry both. "Reset to defaults" clears all four.

**P34: the M3 exit.**
- `docs/superpowers/m3-exit.md` records these checks:
  1. An admin import of export A through the UI, as an E2E spec on `chromium-sqlite` and `chromium-pg`, asserting the report link and an imported post at its old URL.
  2. The full E2E matrix (`e2e:all`: four projects).
  3. The Docker cluster example, `up --build`, importing export A through the API on node1 with uploads zipped. The job may run on either node. Both nodes then serve the post, and its media comes from MinIO.
  4. Every workspace gate on both backends.
  5. The P31 EXPLAIN output.
- Anything that cannot run is listed as not verified, never claimed.

## Global Constraints

- `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`, both with and without `--features ferrite/test-seams,ferrite-http/test-seams,ferrite-import-wp/test-seams`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`. CI gates rustdoc.
- `thiserror` in libraries, `anyhow` only in the `ferrite` binary. No `#[allow(dead_code)]` (user rule): remove unused code.
- **Migration 0010** (`0010_import_jobs`) goes on both backends with a `.down.sql`, and is expand-only. The SQLite `artifacts.kind` CHECK change needs a table rebuild: mark it `-- ferrite:phase contract` if the rebuild requires it, and justify that in the migration comment.
- Storage changes need contract tests on both backends: `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test`.
- **API changes:** utoipa DTOs in `crates/ferrite-http/src/api/dto/`. Then regenerate:
  - `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`
  - `pnpm -C admin api:gen`
  - `pnpm -C admin api:check`
  Capabilities are checked in core, never in handlers.
- New dependency: only `zip` (exact `=x.y.z`, `default-features = false`, `features = ["deflate"]`). Check its current version and API with Context7 before use.
- The admin follows 11 §8 and the design bar (memory: state-of-the-art, far better than WP). Run `pnpm -C admin lint`, `typecheck`, `test`, `build` and `bundle:check`. Run the E2E specs touched by a task on `chromium-sqlite`.
- Commits are small, one per task step group. Messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Cluster and retry:** a job retried on another node, or after a crash mid-pass, must finish with the same rows as a clean run, and must never take over another job's run (Task 4 `job_retry_takes_over_only_its_own_run`, `job_retry_on_fresh_node_reparses`).
2. **Hostile uploads:**
   - an over-cap body gives `413` before the store is touched (Task 5);
   - a zip with `../` names, a zip bomb (declared 1 KiB, actual 10 MiB), or 600 000 entries is refused or noted, never fatal for the run, and the server keeps running (Task 3 `zip_source_*`);
   - an XML/HTML WXR sent with the wrong content type is still accepted, because the WXR part is not media-sniffed.
3. **Permission and scope:**
   - an editor, or a token without `import:*`, gets `403` on every import route;
   - an admin demoted between POST and run makes the job `dead` with a clear `last_error` (Task 4);
   - the report HTML cannot run script (CSP sandbox; Task 5).
4. **Permalink change redirects:**
   - changing `permalink.post` twice in a row leaves one hop from each old path, with no chains (`move_redirects`);
   - a manual redirect on an old path survives;
   - a page and a custom type follow their own rules (Task 6).
5. **The admin under failure:**
   - an upload interrupted at 60 % retries with the same `Idempotency-Key` and creates one job;
   - a failed run shows its error and its partial report link;
   - polling stops when the job is `done` or `dead` (Task 9 unit and E2E).

---

### Task 1: Storage — migration 0010, artifact kind `import`, run columns

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0010_import_jobs.{up,down}.sql`
- Modify: `crates/ferrite-domain/src/import.rs` (`ImportRun` gains `job_id: Option<JobId>`, `step: Option<String>`, `report_hash: Option<String>`), `crates/ferrite-domain/src/plugin.rs` (`ArtifactKind::Import`), `crates/ferrite-store/src/repos.rs` (`ImportRepo`), both backends' `repos/import.rs`, `crates/ferrite-store-tests/src/import.rs`
- Write `docs/superpowers/rulings/m3.md` "M3-2 plan rulings" P23–P34 (one line each, in the file's format)

**Interfaces (produces):**
- `ImportRepo::create_run(&NewImportRun { …, job_id: Option<JobId> })`
- `ImportRepo::set_step(id, token, step) -> Result<bool, StoreError>`: fenced, true when the run is ours and running.
- `ImportRepo::set_report(id, hash) -> Result<(), StoreError>`
- `ImportRepo::list_runs_page(limit, cursor: Option<(Timestamp, Uuid)>) -> Result<Vec<ImportRun>, StoreError>`: newest first, cursor = last `(started_at, id)`.
- `ImportRepo::run_by_job(job_id) -> Result<Option<ImportRun>, StoreError>`: newest.

- [ ] **Step 1: Write failing contract tests** in `ferrite-store-tests/src/import.rs`, registered for both backends:
  - `import_run_job_link_and_step`: a run created with a `job_id` is found by `run_by_job`. `set_step` with the wrong token returns false and with the right token true. `step` reads back.
  - `import_run_report_hash`
  - `import_runs_page_cursor`: 5 runs at the same `started_at`, pages of 2, no duplicates or gaps.
  - `artifact_kind_import_round_trips`
  - `migration_0010_roundtrip`: up/down/up.
- [ ] **Step 2: Run** `FERRITE_TEST_PG_URL=… cargo test -p ferrite-store-sqlite -p ferrite-store-postgres import_run artifact_kind_import migration_0010`. Expected: FAIL.
- [ ] **Step 3: Implement** the migration (indexes: `import_runs_job_idx (job_id)`) and the repo methods on both backends.
- [ ] **Step 4: Run** the same command. Expected: PASS. Then run the full contract suite on both backends.
- [ ] **Step 5: Commit** `feat(store): import runs link jobs, steps and reports; artifact kind import (0010)`, then `docs: M3-2 plan rulings`.

### Task 2: Core — import jobs, run reads, importer check, report artifacts

**Files:**
- Modify: `crates/ferrite-core/src/import/mod.rs`, `crates/ferrite-core/src/jobs.rs`
- Create: `crates/ferrite-core/src/import/jobs.rs`
- Test: `crates/ferrite-core/tests/import_jobs.rs` (both backends)

**Interfaces (produces):**
- `pub const IMPORT_WORDPRESS_KIND: &str = "import.wordpress"`; `ImportJobPayload { v: u32, wxr: String, uploads: Option<String>, options: ImportJobOptions, user: UserId, source_name: String }`; `ImportJobOptions { dry_run, keep_permalinks, site_options, fetch_media, category_base, tag_base }` with the CLI's defaults (serde, `deny_unknown_fields`).
- `ImportService::enqueue(&self, actor: &Actor, payload: ImportJobPayload) -> Result<JobId, CoreError>`:
  - `require(Capability::Import)`.
  - P25's conflict check is `CoreError::Conflict`.
  - The enqueue is written in one transaction.
- `ImportService::importer(&self, user: UserId) -> Result<Actor, CoreError>`: an active admin, else `CoreError::Forbidden`. The CLI's `importer()` uses it too; remove its copy.
- `ImportService::{list_runs(actor, limit, cursor), get_run(actor, id), job_status(actor, job_id) -> ImportJobView { job: Job, run: Option<ImportRun> }}`: `require_reader(Capability::Import)`.
- `ImportService::start(..)` gains `job_id: Option<JobId>`. `ImportHandle::set_step(&self, step: &str)` is fenced; `Changed` when taken over.
- `ImportService::{store_report(handle_or_run_id, json: Bytes, html: Bytes), report(actor, run_id, ReportFormat) -> Result<Bytes, CoreError>}` writes both bytes as `import` artifacts through the `MediaStore`. `report_hash` points to a small JSON `{json, html}` manifest artifact, or to two columns, whichever is simpler; record which.

- [ ] **Step 1: Failing tests** (both backends):
  - `enqueue_requires_import_capability` (editor → Forbidden)
  - `enqueue_conflicts_while_an_import_job_is_queued`
  - `importer_refuses_a_demoted_or_suspended_admin`
  - `set_step_is_fenced`
  - `job_status_links_the_run`
  - `report_round_trips_and_needs_import_read`
- [ ] **Step 2: Run** `cargo test -p ferrite-core --test import_jobs`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on both backends. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): import jobs, run reads and report artifacts`.

### Task 3: Importer — step reporting, partial report on failure, `ZipSource`

**Files:**
- Modify: `crates/ferrite-import-wp/src/run.rs`, `report.rs`, `Cargo.toml` (`zip`); workspace `Cargo.toml`
- Create: `crates/ferrite-import-wp/src/zip_source.rs`
- Modify: `crates/ferrite/src/import_cmd.rs`: on failure, write the partial report and still exit 1; `--uploads` accepts a `.zip` file as well as a directory.
- Test: unit tests in `zip_source.rs`; `tests/rerun.rs` / `tests/passes.rs`; `crates/ferrite/tests/import_cli.rs`

**Interfaces (produces):**
- `ImportOptions.on_step: Option<Arc<dyn Fn(&str) + Send + Sync>>` (production, not a seam), called as each step begins. Task 4 adds the fenced `handle.set_step` call next to it (Task 2 provides `set_step`).
- `run::run(..) -> Result<Report, RunFailed>`; `pub struct RunFailed { pub error: ImportError, pub report: Option<Report> }` (`Display` = the error); `Report.failed: Option<Failure { step: String, error: String }>`.
- `ZipSource::open(path: &Path) -> Result<ZipSource, ImportError>` implements `MediaSource`.

- [ ] **Step 1: Failing tests:**
  - `failed_run_has_a_partial_report`: `fail_after_batches` in pass 3 gives `Err(RunFailed{report: Some(r)})`, with `r.failed.step == "pass3"`, counts for passes 1–2, and the leading note.
  - `steps_are_reported_in_order`: `on_step` sees `parse, pass1, pass2, pass3, pass4, fixup, report`.
  - `zip_source_resolves_uploads_paths`: site A uploads zipped under `wp-content/uploads/…` gives 4 media.
  - `zip_source_refuses_traversal_names`
  - `zip_source_entry_bomb_is_missing_not_fatal`: declared 1 KiB, actual 10 MiB, so that file is reported missing and the run goes on.
  - `zip_source_refuses_too_many_entries`
  - CLI `failed_import_writes_a_partial_report_and_exits_1`
  - CLI `uploads_zip_is_accepted`
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp -p ferrite --test import_cli`. Expected: FAIL.
- [ ] **Step 3: Implement.** Check the `zip` API in Context7. A zip read happens in `spawn_blocking`, with the per-entry stream bounded by `max_upload`.
- [ ] **Step 4: Run** on both backends, plus `corpus_real`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): step progress, partial report on failure, uploads zip`.

### Task 4: The `import.wordpress` job handler

**Files:**
- Create: `crates/ferrite/src/jobs/import.rs`
- Modify: `crates/ferrite/src/jobs.rs` (`handlers()`), `crates/ferrite/src/serve.rs` (pass `data_dir`, `MediaStore`, `ImportService`), `crates/ferrite-import-wp/src/run.rs` (call the fenced `handle.set_step` as each step begins, never while a batch is open: SQLite has one writer)
- Test: `crates/ferrite/tests/import_job.rs` (both backends; in-process `serve` like `tests/cluster`)

**Interfaces:** consumes Tasks 2 and 3. `ImportWordpressHandler::{kind, timeout = 6 h, run}`:
1. Parse the payload; a bad one is `Permanent`.
2. Resolve `importer(user)`.
3. Fetch the artifacts to `<data>/import/job-<id>/` (missing → `Permanent`).
4. Build `ImportContext`/`ImportOptions` as `import_cmd.rs` does: node permalinks via `SwappablePermalinks`; `ZipSource`, then `HttpSource` when `fetch_media`.
5. Set `takeover` per P27.
6. Run.
7. Store the report artifact on success and on failure.
8. Delete the input artifacts on `done`, or on a `Permanent` error.
9. Remove the job folder.
10. Map errors per P27.

- [ ] **Step 1: Failing tests:**
  - `job_imports_site_a`: enqueue through `ImportService::enqueue` with artifacts put directly; the job runs to `done`; the run has `job_id`, `step = report` and a report; the input artifacts are gone.
  - `job_retry_takes_over_only_its_own_run`
  - `job_busy_when_another_run_is_live` (→ Busy, attempt refunded)
  - `job_dead_when_user_demoted`
  - `job_dead_when_artifact_missing`
  - `job_retry_on_fresh_node_reparses`: a second data dir; the same rows as a clean run.
- [ ] **Step 2: Run** `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test import_job`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on both backends. Expected: PASS.
- [ ] **Step 5: Commit** `feat(jobs): import.wordpress job`.

### Task 5: REST API — start, jobs, runs, reports

**Files:**
- Create: `crates/ferrite-http/src/api/import.rs`, `crates/ferrite-http/src/api/dto/import.rs`
- Modify: `api/mod.rs` (routes, `uploads(..)` limit), `api/openapi.rs` (`import` tag), `api/state.rs` (`ImportService`), `idempotency.rs` doc, `openapi.json`, `admin/src/api/schema.d.ts`
- Test: `crates/ferrite-http/tests/api/import.rs`

**Interfaces:**
- Produces the routes of P25/P26-1.
- DTOs: `ImportStartUpload` (the multipart schema), `ImportOptionsDto`, `ImportJobDto`, `ImportRunDto`, `ImportRunPageDto`. Don't clash with the existing `ImportReportDto` (redirect CSV).
- Streams `wxr`/`uploads` with `MediaStore::put_stream(stream, cap)` and no sniffing. Records the artifacts; on any later failure, deletes what it stored.

- [ ] **Step 1: Failing tests:**
  - `start_import_returns_202_with_location`
  - `start_import_replays_with_idempotency_key`
  - `start_import_conflicts_while_one_is_queued`
  - `start_import_413_over_cap_before_storing`
  - `start_import_400_on_bad_options`
  - `import_routes_need_import_capability` (editor, token without scope)
  - `job_status_then_run_then_report_json_and_html`, with the CSP and `nosniff` headers on the HTML
  - `runs_page_cursor`
  - `openapi_snapshot_is_current`, after regeneration
- [ ] **Step 2: Run** `cargo test -p ferrite-http --test api import`. Expected: FAIL.
- [ ] **Step 3: Implement**, then regenerate the OpenAPI and admin types.
- [ ] **Step 4: Run** on both backends, plus `pnpm -C admin api:check`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(api): import jobs, runs and reports`.

### Task 6: Permalink-change redirects (P30)

**Files:**
- Modify: `crates/ferrite-core/src/options.rs` (enqueue in the `permalink.*` write tx), `crates/ferrite-core/src/import/batch.rs` or `options` (the importer's write enqueues too), `crates/ferrite-core/src/jobs.rs` (kind, payload), `crates/ferrite-core/src/redirects.rs` (a `structure_moves(repos, prev, next, after_id, limit)` helper reusing `move_redirects`), `admin/src/i18n/en/admin.json:501`, `admin/e2e/settings.spec.ts` if it asserts that text, `docs/design/06-themes-rendering.md:253`
- Create: handler `crates/ferrite/src/jobs/permalinks.rs`
- Test: `crates/ferrite-core/tests/permalink_redirects.rs` (both backends)

**Interfaces:** `PERMALINK_REDIRECTS_KIND = "permalink.redirects"`; payload `{v:1, prev: {post, page}, next: {post, page}, version}`.

- [ ] **Step 1: Failing tests:**
  - `post_structure_change_redirects_published_posts`, and drafts get none
  - `two_changes_leave_single_hops`
  - `manual_redirect_on_old_path_survives`
  - `page_structure_follows_page_rule`
  - `source_that_is_a_live_path_is_refused`
  - `import_permalink_write_enqueues_too`
  - `job_pages_through_1200_items`
- [ ] **Step 2: Run** `cargo test -p ferrite-core --test permalink_redirects`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on both backends, plus `pnpm -C admin test` for the copy and the settings E2E on `chromium-sqlite`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): redirects for permalink structure changes (M2a-2b P26)`.

### Task 7: Postgres job-claim EXPLAIN (P31)

**Files:**
- Create: `crates/ferrite-store-postgres/tests/claim_explain.rs`
- Modify, only if the check fails: `crates/ferrite-store-postgres/src/repos/job.rs`, `docs/design/04-cluster.md` §6 ("Claim (Postgres, as built)"), `docs/design/02-storage.md`

- [ ] **Step 1: Write the test.** Gate it on `FERRITE_PG_EXPLAIN`, printing `SKIP` otherwise. Seed per P31, then run `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` on the claim's CTE select with `LIMIT 8`. Assert that the summed `Actual Rows` of the scan nodes ≤ 10 × 8 + 3 + 2, and print the text plan.
- [ ] **Step 2: Run** `FERRITE_PG_EXPLAIN=1 FERRITE_TEST_PG_URL=… cargo test -p ferrite-store-postgres --test claim_explain -- --nocapture`. Record the result.
- [ ] **Step 3: If it fails,** split the claim into `UNION ALL` per P31. Keep `ORDER BY priority DESC, run_at, id LIMIT` over the union, and `FOR UPDATE SKIP LOCKED` in each branch. Re-run, and run the job contract suite on both backends.
- [ ] **Step 4: Commit** `test(store-postgres): claim EXPLAIN with 50k future jobs` (plus `perf(store-postgres): …` if changed). Put the plan output in the commit body.

### Task 8: Admin — SEO canonical and robots (P33)

**Files:** `admin/src/editor/panel/seo-section.tsx`, `editor-state.ts`, `autosave.ts`, `conflict.ts`, `admin/src/i18n/en/editor.json`, their tests; `admin/e2e/editor-fields.spec.ts`

- [ ] **Step 1: Failing Vitest tests:**
  - `seo canonical and robots are part of the draft`: patch, rebase, autosave restore.
  - `conflict lists SEO when only robots differ`
  - `canonical error from seo.canonical shows under the field`
  - `reset clears all four`
- [ ] **Step 2: Run** `pnpm -C admin test editor`. Expected: FAIL.
- [ ] **Step 3: Implement** to 11 §8: toggles as a labelled group, with the canonical field's help text.
- [ ] **Step 4: Run** `pnpm -C admin lint typecheck test build bundle:check`, plus the E2E `editor-fields` on `chromium-sqlite`. The E2E sets canonical and `noindex`, then asserts the stored SEO and the `<head>`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(admin): canonical and robots in the SEO panel`.

### Task 9: Admin — the import screen (P32)

**Files:**
- Create: `admin/src/routes/_app/import.tsx`, `admin/src/import/{import-screen.tsx, start-card.tsx, run-panel.tsx, runs-table.tsx}`, `admin/src/api/import.ts`, `admin/src/i18n/en/import.json`; tests next to them; `admin/e2e/import.spec.ts` (site-wide spec per m2b R62)
- Modify: `admin/src/shell/nav.ts`, `admin/src/api/upload.ts` (`uploadWordpressImport`), `admin/src/api/keys.ts`, `routeTree.gen.ts`

- [ ] **Step 1: Failing Vitest tests:**
  - `start sends wxr, zip and options with an Idempotency-Key and retries with the same key`
  - `polling runs while queued/running and stops at done/dead`
  - `failed run shows error and partial report link`
  - `nav shows Import only with the import capability`
  - `empty, loading, error and forbidden states`
- [ ] **Step 2: Run** `pnpm -C admin test import`. Expected: FAIL.
- [ ] **Step 3: Implement.** Build the contact sheet against 11 §8 in both colour schemes and at mobile width.
- [ ] **Step 4: Run** the admin gates. Then run the E2E `import.spec.ts` on `chromium-sqlite` and `chromium-pg`: upload export A with a zip of its uploads, wait for `done`, open the report, `GET` the post's old URL and get the post. Also run the axe checks. Expected: PASS.
- [ ] **Step 5: Commit** `feat(admin): WordPress import screen`.

### Task 10: Docs and the M3 exit (P34)

**Files:**
- Modify: `docs/design/08-wp-import.md` (§1, §6 as built), `07-admin-api.md` (the Import row, §3 admin row), `04-cluster.md` (jobs table, claim), `09-operations.md` (move the planned line), `11-admin-ui.md` (the Import screen and chord), `00-overview.md:177` (stale), `01-architecture.md:150`, `02-storage.md` (0010), `docs/ROADMAP.md` (tick M3-2 and M3), `CLAUDE.md` (status: M3 done, next M4)
- Write: `docs/superpowers/rulings/m3.md` "M3-2 execution rulings" (R52 onwards, from the ledger)
- Create: `docs/superpowers/m3-exit.md`

- [ ] **Step 1: Write the docs.** Check every claim against the code or a command (the user rule: a verified-claims table in the exit doc).
- [ ] **Step 2: Gates:**
  - `rustup update stable`
  - fmt; both clippy variants; rustdoc
  - `cargo test --workspace`, with and without `FERRITE_TEST_PG_URL`
  - corpus_real
  - admin `lint typecheck test api:check build bundle:check`
  - `pnpm -C admin e2e:all` (four projects)
  - the P31 EXPLAIN
  - Docker: `docker compose -f examples/cluster/docker-compose.yml up --build`, then an import of export A with an uploads zip through `POST /api/v1/import/wordpress` on node1. Check that node1 and node2 both serve the post at its old URL, and that its image comes from MinIO.
  Paste the summary lines into the exit doc. Anything that could not run goes under "Not verified".
- [ ] **Step 3: Commit** `docs: M3 exit`.

## Task dependencies

- **Wave 1 (parallel, no shared files):** Task 1, Task 3 (`on_step` only; Task 4 adds the `handle.set_step` call), Task 6, Task 7, Task 8.
- **Wave 2:** Task 2 (needs 1), then Task 4 (needs 2 and 3) and Task 5 (needs 2) in parallel.
- **Wave 3:** Task 9 (needs 5's types), then Task 10.
