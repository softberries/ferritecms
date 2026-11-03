# M6 exit: the editorial platform

**Criterion** ([ROADMAP](../ROADMAP.md), M6): the six parts of the editorial platform
built and merged, the end-of-M6 review's findings fixed, and the full gate set (workspace
tests on both backends, the admin checks, every E2E project) green on the merged branch.

**Result: done** (2026-11-03, branch `m6-final` from `m6` at ae3b3ff5). `v0.3.0` is the
release that carries M6 (the release commit `release: 0.3.0 — editorial platform` bumps the
versions and updates the README and docs; the tag follows when that commit is merged).
Rulings: [`rulings/m6.md`](rulings/m6.md), P1–P8b.

## 1. The six parts

| Part | What | Exit | Migration | Design |
|---|---|---|---|---|
| M6-1 | Multilingual content: site locales, translation groups, `/de/…` permalinks, `hreflang`, per-locale sitemaps and feeds, theme helpers and switchers, API/MCP/admin with locales, WordPress and Markdown imports keeping languages | [`m6-1-exit.md`](m6-1-exit.md) | `0014_locales` | 13 |
| M6-2 | Plugin trust: minisign-signed plugins, data bound to its publisher, the signed static registry, publisher keys and revocation | [`m6-2-exit.md`](m6-2-exit.md) | `0015_plugin_publishers` | 14 |
| M6-3 | Editing: live preview, releases (staged changes applied together, scheduled, whole-site preview), approval workflows and reviews, signed preview links | [`m6-3-exit.md`](m6-3-exit.md) | `0017_releases_workflows` | 15 |
| M6-4 | Accessibility checks in the editor, a per-type publish gate, the content health job and screen | [`m6-4-exit.md`](m6-4-exit.md) | `0019_content_health` | 16 |
| M6-5 | AI change sets: every MCP write recorded with before/after images, reviewed and reverted by a person | [`m6-5-exit.md`](m6-5-exit.md) | `0018_change_sets` | 17 |
| M6-6 | Outgoing webhooks: Standard Webhooks signing, retries through the job queue, the delivery log | [`m6-6-exit.md`](m6-6-exit.md) | `0016_webhooks` | 18 |

After M6 the MCP server lists 70 tools (38 write, 32 read-only), 2 resources, 1 resource
template and 5 prompts (pinned by `mcp_reference::catalogue_counts_are_pinned`); doctor has
27 checks.

## 2. Review findings and their fixes

The review before this branch (commit ae3b3ff5 on `m6`) fixed preview cache isolation,
foreign release locks, token exposure, the preview link modes and the stager's approval.
This branch fixes the rest, each with a failing test first:

1. **Important — a change-set revert bypassed the M6-3 and M6-4 guards.** The reverse
   writes used `transition_in` and `update_in` alone, skipping the release lock, workflow
   approval and staging (P7) and the publish gate. Now status moves go through
   `content::guarded_transition_in` (the same checks as `transition_with`, without an
   override), in-place edits through `live_edit_checks` (an approver's edit is audited as
   `review.bypassed`), and any content restore checks the release lock first. A live item
   whose before status is not live is unpublished before the edit, so a non-approver's
   revert of an assistant's publish in a workflow type is not a live edit. The revert stays
   one transaction; a refused item fails it with `Failed { seq, error }`. Tests in
   `ferrite-core/tests/change_revert.rs` (approval, gate, `Locked`, bypass audit, staging,
   unpublish-then-edit). Commit 73cc4bce.
2. **Minor — an AI-scheduled release left no change set** (ruling P8a). `ReleaseService::
   schedule`/`unschedule` capture a `release` entry; its revert unschedules a release still
   scheduled and refuses one applied since (never forceable). `every_write_tool_records_a_change`
   now covers `release_schedule` (5 exclusions left). Commit c33bb3d0.
3. **Minor — withdraw and request-changes sent no `content.*` webhook.** `back_to_draft`
   now emits `content.updated` (with `locale` and `translation_group`) in the same `Tx`;
   `ReviewService::new` takes the permalink rules. Design 18 §3.1 updated. Commit 783413d7.
4. **Approvers endpoint.** `GET /api/v1/workflows/{type}/approvers?step=N` returns
   `{id, display_name}` of the active users who may approve the step, for anyone who may
   submit (`edit_own` and a content read scope, checked in `WorkflowService::approvers`).
   The admin's submit and assign pickers use it (authors can now pick and assign); Vitest
   `the_submitter_assigns_reviewers_from_the_steps_approvers`; OpenAPI and `schema.d.ts`
   regenerated. Commit ac38daa0.
5. **Docs drift.** CLAUDE.md, the roadmap, designs 12 and 17 carry the counts above (read
   from the code and pinned by a test), M6 is marked done, the crate map names the M6
   modules, the M6 checklist items that are done are ticked. Commit 8bc287a2.
6. **Rulings** P8a and P8b in `rulings/m6.md` ("Final review rulings").

### Execution decisions

- **R-final-1:** `import_cli::import_site_a_end_to_end_sqlite` failed whenever another
  process held 127.0.0.1:8080: `init --url http://127.0.0.1:8080` writes that listen
  address, and the test served the site with it. The test's SQLite site now sets
  `FERRITE_SERVER__LISTEN=127.0.0.1:0`; the site URL (which the imported links resolve
  against) is unchanged, so nothing the test checks is weaker. Verified with a dummy server
  on 8080 for the whole workspace run.
- **R-final-2:** the approvers list uses the workflow's current steps; an open review keeps
  the steps it started with (D23), so after a workflow edit the assign picker may offer
  someone the review's step does not name. The server's assign check refuses them, as before.
- **R-final-3:** a revert never sets a schedule again (a release whose schedule the
  assistant removed is refused with a pointer to the release screen): scheduling runs a
  sandboxed check in its own transaction, which a revert's single transaction cannot hold.

## 3. Gates (run on `m6-final`)

All run in the `m6-final` worktree with
`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test` (rustc 1.99.1 after
`rustup update stable`).

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | pass |
| `cargo test --workspace --no-fail-fast` (twice; the second after the last fix) | pass: 5,417 tests, 0 failed, both runs (the first with another server holding 127.0.0.1:8080) |
| Plugin workspace | not run: `git diff develop...HEAD -- plugins` is empty, M6 did not touch it |
| OpenAPI snapshot (`openapi_snapshot_is_current`) | pass |
| `pnpm -C admin api:check`, `typecheck`, `lint`, `test`, `build`, `bundle:check` | pass (Vitest 156 files, 955 tests; one earlier run under full cargo load failed and passed on rerun, test not identified) |
| `pnpm -C admin e2e:all` (chromium-sqlite, chromium-pg, firefox-sqlite, webkit-sqlite, public-themes-chromium) | first run 870 passed, 8 failed (fixed below); final run 877 passed, 1 failed: a flake |
| `cargo test -p ferrite --test docs_site` | pass |

The first E2E run's failures were all outside Chromium:

- **Real defect:** `live-preview.spec.ts` (3 tests, Firefox and WebKit): the sandboxed
  preview frame (origin `null`) loads the theme stylesheet in CORS mode there, and
  `/_theme/` sent no `Access-Control-Allow-Origin`, so the preview was unstyled. Theme
  assets now send `Access-Control-Allow-Origin: *` (public, immutable, no credentials);
  test in `ferrite-http/tests/app/assets.rs`. This also settles M6-3's "not verified" note
  about the CORS error in the trace. Commit 31a134a8.
- **Test fixes:** `styleguide_touch_targets` (Firefox) matched two "Clear search" buttons
  once the empty result rendered; the probe is now scoped to the field.
  `editor-safety` `screens` (WebKit) went offline while the M6-4 editor check was in
  flight; the offline step now allows that request to fail, as it does the autosave.
- **Flake:** `design.spec.ts` `compact_density_switches_the_variables` (WebKit, final run):
  `page.reload` never reached `load` and the test timed out. It passed in the first full
  run and 3 of 3 with `--repeat-each 3`; cause not found (the machine ran 4 projects in
  parallel).

## 4. Not verified

- Real hosted MCP clients (Claude.ai, ChatGPT, Claude Desktop connectors) and a real
  assistant session reviewing its own change sets; the MCP calls were scripted.
- A two-node cluster for releases (a scheduled apply seen by both nodes), content health
  runs and change-set reverts; the state is in the database, but nothing was run on two
  nodes.
- Real SMTP for review, release and webhook auto-disable mail (recording mailers and the
  E2E sink only).
- External link checks against real hosts (local test servers and fake probes only).
- The WordPress language import on real Polylang or WPML exports (hand-written files of the
  documented shapes only).
- The real M6-2 signing keys: the plugin publisher key and the registry key are the user's
  to create (09 §7 "Plugin keys"); `crates/ferrite/registry.pub` is a placeholder, so no
  signed release or registry publication has run.
- Safari and real mobile devices; screen readers beyond axe.
- A rolling upgrade with mixed v0.2.0 / M6 nodes and a v0.2.0 binary against the M6 schema
  (`compat.sh`).

## 5. Follow-ups

Open items stay in the roadmap under each M6 part (unticked) and "Later": the M6-1 import
and admin follow-ups, the M6-2 keys and registry work, the M6-3/M6-4 "Later" lists
(including P8b's shared preview modes), and design 18 §12 for webhooks.

## 6. Release

`v0.3.0` carries M6. The release commit (branch `release-0.3.0` from `develop` at 229d81e7)
bumps the workspace and the three shipped plugins (`seo`, `analytics`, `contact-form`:
manifests, crates, lock files) to 0.3.0, updates the version examples, the README's
"What's in" list, the landing page's pill and tool counts, and adds the checked claims to
`site/VERIFIED.md` ("Release 0.3.0"). The M6-2 keys do not exist yet, so the release's
plugins asset is unsigned and no registry index is published (§4); the README says so.
Tagging `v0.3.0` is the user's decision (the M4-2 release gate applies: the tag ruleset lets
only admins create `v*` tags).
