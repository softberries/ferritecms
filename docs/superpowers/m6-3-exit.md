# M6-3 exit: editing (live preview, releases, approval workflows)

Branch `m6-3-editing` (from `m6` at 6945d2a, merged with `m6` at 4e5e5ca: M6-4 and M6-5).
Design: [15](../design/15-editing.md) (§19 "As built"). Plan:
[`plans/2026-11-02-m6-3-editing.md`](plans/2026-11-02-m6-3-editing.md). Rulings P4 and P7
([`rulings/m6.md`](rulings/m6.md)) applied.

## 1. What was built

- **Storage** (`0017_releases_workflows`, both backends, expand, lossy down): `releases`,
  `release_items`, `release_locks`, `workflows`, `reviews` (partial unique
  `reviews_open_target_uq`), `review_decisions`, `review_assignees`, `preview_links`;
  `ReleaseRepo`, `WorkflowRepo`, `ReviewRepo`, `PreviewLinkRepo` on both backends and every
  wrapper; seven tables exported, `preview_links` skipped.
- **Core**: `release/` (`ApplyTx` with the collect mode, one merged `invalidate` event per
  apply, and the sandbox mode with no-op side channels; `SandboxStore`; `ReleaseService`:
  create, stage with the staging checks, rebase, unstage, check in a sandbox with one
  savepoint per entry, apply in one transaction, schedule with locks and the
  `release.apply` job, unschedule, scheduled apply as the scheduler re-read, failures with
  a report, `apply_due`, the preview sandbox); item locks on every public write path
  (update, PATCH with relocate, relocate, transitions, delete, revision restore);
  `workflow/` (`WorkflowService` with step validation, the publish gate with admin
  override, P7 live edits by approvers with `review.bypassed`, `ReviewService` with steps,
  assignees, decisions, withdraw, entry reviews, staleness on save and on entry change,
  `editorial.notify` jobs); `preview_link.rs` (`ferp_` tokens hashed, 1–168 h, rate
  limits, re-checked on each use as the creator with `content:read`); `SiteReader::
  with_store`, `preview_working`, `preview_staged`; `allowed_actions` with the workflow
  state (`needs_approval`, `stage`, `override`); webhook events `release.*` and `review.*`;
  P4: `quality::gate::check` per published entry, all or nothing.
- **HTTP**: `/_preview/link/<token>` (item links render, `?working=1` / `?release=`;
  release links set the preview cookie and `303`), `/_preview/exit`, cookie previews of
  the whole site through the sandbox (`AppState::preview_view`: no cache, no counters,
  `no-store`, `noindex`, `no-referrer`, read-only plugins, 1/2 slots, 15 s deadline,
  60 renders per link and minute), the core preview bar; `request.preview_release` in
  themes; REST `/releases` (CRUD, entries, rebase, entry submit, check, schedule,
  unschedule, apply), `/…/preview-links`, `/preview-links/{id}`, `/workflows`, `/reviews`;
  content submit/publish/schedule extras and DTO fields; problem slugs `release-locked`,
  `release-conflict`, `release-not-ready` (with `entries`), `approval-required`,
  `staging-required`.
- **MCP**: `releases_list`, `release_get`, `release_create`, `release_stage`,
  `release_unstage`, `release_check`, `release_schedule`, `release_apply`,
  `release_preview_link`, `reviews_list`; `content_submit_for_review` with reviewers and a
  note; `content_preview_link` returns a signed link; hints for the new refusals. 70 tools
  after the merge.
- **Binary**: `ReleaseApplyHandler`, `ReleaseApplyDueHandler` (30 s), `EditorialNotifyHandler`
  (mail through the node's queue), `PreviewLinksPruneHandler` (daily); wired in `serve`.
- **Admin**: live preview pane (sandboxed frame on a 1 h preview link), Releases (`g r`),
  Reviews (`g v`), the editor's Add to release, Share preview, Save as change, change mode
  and banners, the publish dialog's override, the dashboard card, the content type's
  Workflow section. E2E `live-preview`, `releases`, `workflows` specs.
- **Docs**: guide `02-guides/12-releases-and-reviews.md`, additions to the writing, import
  and AI assistant guides and the MCP tools reference; design 12 §5.2/§5.4, design 15 §19,
  design 00 index, roadmap.

## 2. Verified

- `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (see the lead's report for
  the final run after the merge).
- `FERRITE_TEST_PG_URL=… cargo test --workspace` on both backends: contract tests for the
  0017 tables (26 per backend), `ferrite-core` tests `release_tx` (10), `releases` (26),
  `workflows` (23), `preview_links` (12) per backend, `ferrite` `release_jobs` (3),
  `ferrite-http` app `release_preview` (5), API `releases` (10), `mcp_releases` (5), the
  updated `mcp_content`, `mcp_changes`, `content_extras`, `mcp_reference` tests.
- The OpenAPI snapshot and `admin/src/api/schema.d.ts` regenerated (`api:check` current).
- Admin: `lint`, `typecheck`, `test` (156 files, 954 tests after the merge), `build`,
  `bundle:check` (no budget raised); E2E `live-preview`, `releases`, `workflows`, the editor
  specs, types, dashboard, revisions, palette and shell on `chromium-sqlite` and
  `chromium-pg` before the merge with M6-4/M6-5; after it, `live-preview`, `releases`,
  `workflows`, `shell` and `editor-publish` on both (51 of 52 in the combined run;
  `editor-publish` `phone_publish_bar` timed out once on SQLite under load and passed
  alone).

## 3. Execution rulings

- **R-M6-3-1:** plan tasks 1 and 2 in one commit; one `editorial.rs` per crate instead of
  one file per repository.
- **R-M6-3-2:** the partial unique index is `reviews_open_target_uq` (the SQLite
  constraint map wants `_pkey`/`_uq`).
- **R-M6-3-3:** the 0015/0016 roundtrip tests revert newer migrations first (0017 is
  lossy); the export rich site and the v10 import fixture know the seven new tables.
- **R-M6-3-4 (P4):** `release::quality_gate` calls `quality::gate::check` with the item's
  type for every publishing entry in apply and check; a `QualityBlocked` entry is outcome
  `invalid` and refuses the whole release.
- **R-M6-3-5:** a failing entry of an apply is reported as `ReleaseConflict([entry])` (so
  REST and MCP always get `entries`); the apply claims the release row first (two version
  bumps per apply) so a racing applier fails its version check.
- **R-M6-3-6:** a scheduled apply's permanent failure is handled in core (failed, report,
  notify); the job completes; only store errors retry.
- **R-M6-3-7 (P7):** live edits by approvers in `require_live_edit`; publish/schedule gated
  in `ContentService::transition_with`; new `publish_with_override`,
  `schedule_with_override`, `submit_with` keep the old signatures.
- **R-M6-3-8:** no `ReadView` refactor (plan task 9): `AppState::preview_view` clones the
  state over the sandbox; a new `AppState` field must be copied there.
- **R-M6-3-9:** whole-site preview entries are applied with the link creator's full rights
  (rolled back), the page renders as the `content:read` principal.
- **R-M6-3-10 (P7 D16):** the editor's frame loads through a signed link; scroll restore is
  not possible (opaque origin); links always show the bar.
- **R-M6-3-11:** no `Idempotency-Key` on release apply (version-checked: a retry is `412`);
  optional JSON bodies through `OptionalJson`; the content DTO's `releases` left out when
  empty.
- **R-M6-3-12:** one `ApiState` field `editorial: EditorialApi` for the four services.
- **R-M6-3-13:** `content_preview_link` is a write tool now (`content:write`). After the
  M6-5 merge, `release_apply` captures each entry's item change; the tools that change
  only release or link rows are listed as excluded in `every_write_tool_records_a_change`.
- **R-M6-3-14:** found and fixed during task 8: a savepoint's commit inside the sandbox
  rolled back (checks and previews lost every entry); test
  `sandbox_savepoint_commit_keeps_writes`.

## 4. Not verified

- The full admin E2E suite after the merge (only the specs above); Firefox and WebKit for
  the three new specs.
- Real SMTP for review mail (the handler is tested with `MemoryMailer`); Safari; real
  devices; whether real browsers apply the theme stylesheet inside the sandboxed frame
  (Playwright's trace logged a CORS error).
- Two-node cluster: a scheduled release applied once and seen by both nodes, a link made on
  node A opened on node B (the state is in the database; not run here).
- Releases larger than the 200-entry cap test; preview load under concurrent writers on
  SQLite; the preview slot `503`, the 15 s deadline and the plugin `kv.set` refusal have
  no test of their own; no property test over roles × statuses × review states.
- `import_cli::import_site_a_end_to_end_sqlite` needs 127.0.0.1:8080 free; on this machine
  the user's own `ferrite serve` holds it (an environment issue, not a code bug).

## 5. Follow-ups

Listed in the roadmap under M6-3 "Later": an endpoint for eligible approvers for
non-admins, the 404 of `GET /workflows/{type}`, change mode's local backup and leave guard,
the working preview's SEO in the head, preview scroll restore, `Idempotency-Key`, the
remaining browsers, the missing tests above, plugin hooks for releases and reviews.
