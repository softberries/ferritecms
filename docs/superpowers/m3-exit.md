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

**Result:** PENDING (final gate run on the merged final-review fixes).

| Exit check | Verified by | Result |
|---|---|---|
| 1. Admin import of export A (`import.spec`) on `chromium-sqlite` and `chromium-pg` | §3 | PENDING |
| 2. Full E2E matrix (`e2e:all`, four projects) | §3 | PENDING |
| 3. Docker cluster example: import through the API on node1, both nodes serve the post, media from MinIO | §2 | PENDING |
| 4. Workspace gates on both backends (`fmt`, both `clippy` variants, rustdoc, `cargo test --workspace` twice, `corpus_real`, the admin gates) | §3 | PENDING |
| 5. P31 EXPLAIN (`FERRITE_PG_EXPLAIN=1`) | §1 | PENDING |

What was built is listed in [ROADMAP](../ROADMAP.md) (M3-2, the ticked items) and
described as built in design [08](../design/08-wp-import.md) §1, §2 and §6,
[07](../design/07-admin-api.md) §1 and §3, [04](../design/04-cluster.md) §6 and
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
lint), 04 §6 (the `import.wordpress` job and the `import` lock), 07 §1 and §3 (the Import
rows, the SEO fields, the status line), 08 §1, §2, §5 and §6 (as built), 09 §8 (the
command, imports through the admin and the API; the planned line removed), 11 §6 and §7
(`g i`, the Import screen).

## 1. P31 EXPLAIN

PENDING (final run). Early signal on `3eb9edc`: `scan rows read: 12 (bound 85)`.

## 2. Docker cluster example

PENDING.

## 3. Gates

PENDING (final run on the merged code).

Early signal on `3eb9edc` (before the final-review fixes; not the exit's result): `fmt`,
both `clippy` variants and rustdoc exit 0; `cargo test --workspace` 2 995 passed, 0 failed
in 98 targets on SQLite and the same with `FERRITE_TEST_PG_URL`; `corpus_real` 1 passed;
admin `lint`, `typecheck`, `api:check`, `build`, `bundle:check` exit 0; `pnpm -C admin test`
3 failed, 715 passed, the three 5 s timeouts and a shell route test under a load average of
30 while the Postgres workspace run went on beside it.

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
- **`zip =9.1.3` is two days older than its use** (R59).

## 5. Not verified

PENDING (final run).

## 6. Open follow-ups

Listed in [ROADMAP](../ROADMAP.md) under M4, "Deferred from M3".

## 7. Verified claims

PENDING.
