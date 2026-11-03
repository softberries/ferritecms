# M6-5 exit: AI change sets

Branch `m6-5-change-sets` (from `m6` at 6945d2a4, with M6-1, M6-2 and M6-6 merged).
Design: [17](../design/17-change-sets.md) (its §17 "As built" lists the changes). Plan:
[`plans/2026-11-02-m6-5-change-sets.md`](plans/2026-11-02-m6-5-change-sets.md). Ruling P1
applied as written (30-minute gap / 500-change close, REST never recorded, a failed
capture fails the write, no MCP revert tool, 90 days in `mcp.change_sets_days`, zstd
images); P2 (`change_set.reverted` through `webhooks::emit` in the revert's `Tx`); P6
(MCP counts: 58 tools after `changes_list`). Migration `0018_change_sets` only.

## 1. What was built

- **Storage** (`0018_change_sets`, both backends, expand, not-lossy down): `change_sets`
  (the unique `open_key` where concurrent writers of one agent meet) and
  `change_set_changes` (zstd canonical images, SHA-256 hashes; hash without bytes =
  skipped). `ChangeSetRepo`: `extend_open`/`open` reserving consecutive seqs, keyset
  `list` with user/agent/open filters and reverted counts, `summaries`, `change`,
  `changes`, `mark_reverted`, `close`, `changes_since`, `prune`. Both tables exported
  (appended at the end of the catalogue, like R-M6-6-2).
- **Core** (`ferrite_core::changes`): `Actor.agent` (`Agent::OAuth(grant)` /
  `Agent::Token(id)`, set only by the MCP guard, kept by `with_scopes`); typed snapshots per
  object kind (design 17 §4) in canonical JSON, the 16 MiB cap on both sides; `Capture`
  (`begin`/`created`/`finish`, inert and read-free without an agent, `finish` last in the
  `Tx`) in every public write path the 31 MCP write tools use, M6-1's included;
  `ChangeSetService` (list, detail, one change with images hidden without the viewer's
  read right and sealed secrets shown as set, `agent_sets` for MCP, `changes_since`,
  `revert`). Revert: a person only (an agent actor is `Forbidden`), the set's user or
  `manage_users`; the later-change check, then conflicts by simulating object hashes
  newest first (`changed_since` forceable; `later_change`, `gone`, `too_large` not), locks
  types then extensions, one reverse write per object through the crate's own `_in` paths
  as the person (content/media with the lifecycle and webhook events, terms re-inserted with
  id and links, menus, types, redirects with id and origin, comments, site settings in one
  `set_many_in`, theme/plugin settings written back sealed, theme activation only to the
  same artifact, plugin enable/disable), then `mark_reverted`, `close`, audit
  `change_set.revert` and the `change_set.reverted` webhook event; `dry_run` rolls back.
  Setting `mcp.change_sets_days` (1..=365, default 90); `change_sets.prune` (hourly,
  `ferrite-cluster` builtin).
- **HTTP**: `/api/v1/change-sets` (list, detail with admin paths, one change with images,
  `POST …/revert`: session only, CSRF, `Idempotency-Key`, `409 change-conflict` with
  `report.conflicts`, a reverse write's error with `report.change`); OpenAPI regenerated.
  MCP: the guard marks the actor; `changes_list` (read-only, no scope beyond the token, the
  connection's own sets with `review_url`, never images); the instructions mention change
  sets; `settings_update` refuses every `mcp.*` key.
- **Admin**: AI activity (`/admin/ai-activity`, every role with `read_admin`) and the set
  screen with content diffs (the revisions drawer's) and JSON diffs, the revert dialog
  (dry run, conflicts, Overwrite later edits), Activity links on Connected apps and API
  tokens (account and user sheet), the dashboard's last-24-hours count, the retention field
  in Settings → AI assistants. Vitest; E2E `ai-activity.spec.ts`.
- **Docs**: the AI assistants guide's "Review and undo what an assistant did" with a
  screenshot, the MCP tools reference (58 tools), README, designs 00/07/12/17, VERIFIED rows.

## 2. Verified

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`: pass (see §5 for the runs).
- `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test
  --workspace --no-fail-fast`: see §5 (store contract tests on both backends; core
  `change_capture` 34 and `change_revert` 62 on both; API `change_sets` 20, MCP
  `mcp_changes` 12 incl. `every_write_tool_records_a_change` over all 31 write tools;
  cluster `change_sets_prune`).
- OpenAPI snapshot current; `pnpm -C admin api:check`, `typecheck`, `lint`, `test` (886),
  `bundle:check`: pass.
- E2E on Chromium, SQLite and Postgres: `ai-activity.spec.ts` passes; the touched specs
  (settings, account, users, dashboard, mcp, shell, palette) 88 pass, 2 fail:
  `shell.spec.ts` `shortcut_sheet_lists_bindings` on both backends, which predates this
  branch (M6-6's `g w` "Go to Webhooks" is missing from its expected list; AI activity has
  no shortcut).
- A scripted MCP run against a release build (`seed demo` site, a personal token, four
  writes over `/mcp`), then the set screen in Chromium: the screenshot
  `admin-ai-activity.webp`.
- Fresh-context review of the whole branch (Opus): no Critical; three Important findings
  fixed with tests seen failing first (R-M6-5-12); minors deferred (§4).

## 3. Execution decisions (R-M6-5-n)

- **R-M6-5-1** `ChangeSetQuery` has no `all_users` (`user: None` = every user's; core
  decides) and carries `idle_since`; `NewChangeSet` carries `idle_since`/`max_changes` so
  `open` closes only a stale or full set.
- **R-M6-5-2** The 0016 and 0018 roundtrip tests assert "every later migration goes and
  comes back" instead of fixed lists, so wave B's 0017/0019 merge without edits.
- **R-M6-5-3** M6-1's write paths are captured: `content.translate` (a create),
  `create_localized`/`create_localized_term` (creates), `update_and_relocate` (one
  `content.update`). No MCP write tool is excluded.
- **R-M6-5-4** `ExtensionService` `install_theme_with`/`activate_theme_with`/
  `set_plugin_enabled_with` take `by: Option<&Actor>` so the capture runs in their locked
  `Tx`; an install captures only when it activates.
- **R-M6-5-5** Revert: conflicts by hash simulation, then one reverse write per object to
  its oldest selected before-image (not one per change); settings last in one batch.
- **R-M6-5-6** No pending → draft transition exists: a revert goes trash → restore; a
  `private` target is left with a note; locale/translation-group differences are notes.
- **R-M6-5-7** `change_set.reverted` only through `webhooks::emit` (no events-table row).
- **R-M6-5-8** Images are redacted in core (`{"sealed": …}` → `{"secret": "set"}`).
- **R-M6-5-9** 58 MCP tools (M6-1 made 57); the reference test allows `changes_list` with no
  scope.
- **R-M6-5-10** Admin: AI activity in the sidebar for every `read_admin` role (design §11),
  `nav.test.ts` updated; Activity links on every grant and token; the dashboard count sums
  the first list page's sets touched in 24 h; the E2E edits the post over REST (not the
  editor) and renames a person's term (a whole-set revert of a `term_create` deletes it).
- **R-M6-5-11** README's 0.2.0 release notes keep "56 tools" (history).
- **R-M6-5-12** Review fixes: term–item links stay out of image hashes (kept in images), so
  "create a term, file posts under it" reverts without a false `changed_since`; reverse
  status changes emit the public paths' `content.*`, `media.*` and `comment.moderated`
  webhook events; a reschedule reverts to the original `scheduled_at`.
- **R-M6-5-13** `export_cli`'s schema-10 archive fixture drops the 0018 tables (an older
  archive has none; the loader then loads nothing for them).

## 4. Not verified, and deferred

- A real assistant client (Claude Code connected over OAuth or a token, asked "what did you
  change?") was not run; the MCP calls were scripted (HTTP) and covered by the API tests.
- Hosted clients (Claude.ai, ChatGPT), a two-node cluster reverting concurrently, and the
  new E2E spec in Firefox and WebKit (left for the end-of-M6 cross-browser run).
- Postgres concurrency of two openers is covered by the contract test only, not under load.
- Deferred review minors: a revert loads every image of the set (plan from summaries);
  `zstd::bulk::decompress` reserves 16 MiB per call; the `mcp.*` refusal and "revert needs
  a session" live in handlers (core could refuse agents' `mcp.*` writes); the store's
  `open=true` filter ignores the 500 cap; per-set/per-change lookups in `set_dto` and
  `admin_path`; images are gated per area, not per item (D16 as designed); edits inside an
  item that is trashed in both states are not restored; a set with a type create plus its
  items, or a theme activation from the built-in theme, cannot be reverted whole; the UI
  names only the set's own user (reverted-by and conflict authors show generic text);
  `aria-controls` points at a panel that exists only when expanded; `last_editor` names the
  newest revision's author, which is the same user when another connection of theirs edited.
- Known environment issue: `import_cli::import_site_a_end_to_end_sqlite` fails when the
  user's own `ferrite` holds `127.0.0.1:8080`; not a code problem.

## 5. Final runs

Recorded in the final report (the gate commands and their results).
