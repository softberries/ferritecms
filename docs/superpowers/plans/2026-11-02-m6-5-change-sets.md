# M6-5 — AI change sets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every write an AI assistant makes over MCP (OAuth grant or personal token) is recorded with before/after images and grouped into change sets; a person reviews them with diffs in the admin and reverts a whole set or one change, refused where a later edit conflicts unless forced.

**Architecture:** `Actor` gains an `agent` marker that only the MCP guard sets. A `Capture` helper in `ferrite-core::changes` reads canonical snapshots of the touched objects inside each captured core write's transaction and appends rows to `change_sets` / `change_set_changes` (migration `0018_change_sets`). `ChangeSetService` lists sets and reverts them newest first in one transaction through the existing crate-internal `_in` write paths, as the reverting person. REST under `/api/v1/change-sets`, a read-only MCP tool `changes_list`, and the admin's AI activity screens.

**Tech Stack:** Rust 2024, sqlx (SQLite + Postgres), `zstd =0.14.3` (workspace pin, new to `ferrite-core`), `sha2` as `ferrite-core` already uses it, axum 0.8 + utoipa, `rmcp` 3.8, React/TanStack admin with `diff` 9.0.0, Vitest, Playwright.

**Spec:** `docs/design/17-change-sets.md` (read it first; it wins over this plan on behaviour). Also 12 (MCP), 02 §7–8 (migrations), 11 (admin UI).

## Global Constraints

- Wave B: M6-1 (locales: `locale`, `translation_group` on content; `locale` on terms and menus; `site.locales`), M6-2 and M6-6 are merged before this starts. Read their exits first and use their real field names in the snapshots (design 17 §4); where M6-1 keys menus per locale, `ObjectRef::Menu` carries the locale. M6-3 and M6-4 run in parallel: touch the core write paths with the two-line `Capture` calls only, no refactors of their bodies beyond what a task names.
- Migration `0018_change_sets` on **both** backends, `-- ferrite:phase expand`, with `.down.sql`; contract tests on both (`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:5432/ferrite_test`). Use no other migration number.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`; `thiserror` in libraries, `anyhow` only in `crates/ferrite`; no `allow(dead_code)`.
- Capabilities in core, never in handlers or tools. Capture lives in core public service methods, never in `_in` helpers shared by composites (design 17 §3.2).
- `Capture::finish` is the last statement of a captured write's `Tx` (design 17 D9).
- REST changes: `utoipa` DTOs; `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`; regenerate `admin/src/api/schema.d.ts` with the repo's script.
- Constants: idle window 30 min, 500 changes per set, `MAX_IMAGE` 16 MiB, term links kept 1,000, `mcp.change_sets_days` 1..=365 default 90, `changes_list` limit 1..=20 default 5, revert `changes` ≤ 500.
- Admin: `admin/src/i18n/en/ai.json`; 11-admin-ui rules (tokens only, 44 px targets, axe clean).
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A write that fails after `Capture::begin` leaves no set and no change (rollback); a failing `finish` fails the write.
- REST writes with the same personal token record nothing; the same token on `/mcp` does.
- Two concurrent MCP writes of one grant on Postgres land in one set with distinct `seq`s.
- An OAuth token refresh (new `api_tokens` row) keeps writing into the same set (keyed by grant).
- Reverting a set where the assistant updated a page twice needs no force; a person's edit in between is `changed_since`.
- A contributor whose assistant created drafts can revert them; a demoted user sees changes without images and cannot revert writes their role no longer allows.
- Plugin secrets never leave sealed form, in images, DTOs, logs or audit rows.

---

### Task 1: Storage — migration 0018, domain types, `ChangeSetRepo`

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0018_change_sets.{up,down}.sql` (design 17 §5), `crates/ferrite-domain/src/change_set.rs`, `crates/ferrite-store-{sqlite,postgres}/src/repos/change_sets.rs`, `crates/ferrite-store-tests/src/change_sets.rs`
- Modify: `crates/ferrite-domain/src/{ids.rs (ChangeSetId), lib.rs}`, `crates/ferrite-store/src/repos.rs` (trait + `Repos::change_sets`), both stores' `repos/mod.rs`, `crates/ferrite-store/src/catalogue.rs` (two tables, `Export::Rows`), `crates/ferrite-store-tests/src/{lib.rs, migrations.rs, counting.rs, failing_options.rs}`, the other `Repos` impls (`crates/ferrite-core/tests/media.rs`, `crates/ferrite-cluster/tests/jobs.rs`, `crates/ferrite-http/tests/app/reload_race.rs`).

**Interfaces (produces):**
- Domain: `ChangeSetId`, `AgentKind {OAuth, Token}`, `ChangeSet { id, user_id: Option<UserId>, agent_kind, agent_id: Uuid, client, open: bool, started_at, last_at, changes: u32, reverted_changes: u32, reverted_at, reverted_by }`, `NewChangeSet { id, user_id, agent_kind, agent_id, client, open_key, now }`, `Image { Absent, Present { hash: String, bytes: Vec<u8> }, Skipped { hash: String } }`, `NewChange { set_id, seq, at, op, object_kind, object_id, label, before: Image, after: Image }`, `Change` (+ `reverted_at`, `reverted_by`, `revert_forced`), `ChangeSummary` (no bytes; `before_state`/`after_state` as `absent|present|skipped`), `ChangeSetQuery { user: Option<UserId>, all_users: bool, agent_id: Option<Uuid>, open: Option<bool>, limit, cursor }`, `CHANGE_SETS_DAYS_OPTION`, `CHANGE_SET_CURSOR_TAG`.
- `ChangeSetRepo`: `extend_open(open_key, now, idle_since, max_changes, n) -> Option<(ChangeSetId, u32 /* first seq */)>`, `open(&NewChangeSet, n) -> (ChangeSetId, u32)`, `insert_change(&NewChange)`, `get(id)`, `list(&ChangeSetQuery) -> Page<ChangeSet>`, `summaries(id)`, `change(id, seq)`, `changes(id)`, `mark_reverted(id, seqs, by, at, forced)`, `close(id)`, `prune(before, limit) -> u64`.

- [ ] Write `migration_0018_roundtrip` (up, down, up) and contract tests: `extend_open_returns_consecutive_seqs`, `idle_set_is_not_extended`, `full_set_is_not_extended`, `open_closes_the_stale_set_and_starts_a_new_one`, `concurrent_open_meets_in_one_set` (two `Tx`s, Postgres; SQLite sequential), `images_round_trip_bytes_and_hashes` (absent/present/skipped), `list_is_newest_first_with_cursor_and_filters`, `reverted_counts_in_list`, `deleting_user_nulls_user_and_reverted_by`, `prune_deletes_old_sets_and_their_changes`, `catalogue_matches_live_schema` still green.
- [ ] Run on both backends, see them fail; implement; `cargo test -p ferrite-store-sqlite -p ferrite-store-postgres` with `FERRITE_TEST_PG_URL` → pass.
- [ ] Commit `feat(store): change sets and changes (0018_change_sets)`.

### Task 2: The agent marker, snapshots and `Capture`

**Files:**
- Create: `crates/ferrite-core/src/changes/{mod.rs, object.rs, snapshot.rs, capture.rs}`
- Modify: `crates/ferrite-core/src/actor.rs` (`Agent`, `Actor.agent`, `with_agent`), `crates/ferrite-core/src/lib.rs`, `crates/ferrite-core/Cargo.toml` (`zstd.workspace = true`), `crates/ferrite-http/src/mcp/auth.rs` (`guard` sets the agent), `crates/ferrite-http/src/mcp/server.rs` (doc of `Caller`).

**Interfaces:**
- `pub enum Agent { OAuth(OAuthGrantId), Token(ApiTokenId) }` (Copy); `Actor::with_agent`; `Actor::new` → `agent: None`; `with_scopes` keeps it.
- `pub(crate) enum ObjectRef { Content(ContentId), Media(ContentId), Term(TermId), Menu(MenuKey), Type(String), Redirect(RedirectId), RedirectSource(String), Comment(CommentId), Option(String), ActiveTheme, Plugin(String) }` with `kind() -> &'static str`, `id() -> String`, `area() -> Area`.
- `pub(crate) async fn snapshot(repos, &ObjectRef) -> Result<Option<Value>, CoreError>` (design 17 §4 members); `pub fn canonical(&Value) -> Vec<u8>`; `pub(crate) fn image(Option<&Value>) -> Image` (hash, zstd level 3, skipped above `MAX_IMAGE`); `pub(crate) fn open_image(&Image) -> Result<Option<Value>, CoreError>` (decompress with `MAX_IMAGE` capacity).
- `pub(crate) enum Op` with `as_str()` for every op of design 17 §3.3.
- `Capture::{begin, created, finish}` (design 17 §3.2); label from the after (else before) image: title, name, source or key, cut to 200 characters.

- [ ] Unit tests: `actor_new_has_no_agent_and_with_scopes_keeps_it`, `canonical_sorts_keys_recursively` (a `jsonb`-reordered object hashes the same), `snapshot_leaves_out_volatile_columns` (content `version`/`updated_at`/`body_html`, redirect `hits`), `oversize_image_is_skipped_with_hash`, `decompress_refuses_beyond_max_image` (a hostile zstd bomb), `inert_capture_reads_nothing` (a `CountingStore`: no reads for an actor without agent), `capture_opens_a_set_labelled_with_the_client` (grant → client name; token → `API token <name>`; missing row → `AI assistant`), `thirty_minute_gap_opens_a_new_set`, `finish_failure_fails_the_write` (`FailingOptionsStore`-style wrapper failing `insert_change`).
- [ ] HTTP test in `crates/ferrite-http/tests/api/mcp.rs`: `guard_marks_oauth_and_token_agents` (through a write tool once Task 3 lands; here a unit test of the guard's actor).
- [ ] Implement; `cargo test -p ferrite-core changes` passes; clippy clean.
- [ ] Commit `feat(core): agent-marked actors, canonical snapshots and the change capture`.

### Task 3: Capture content, media and comment writes

**Files:**
- Modify: `crates/ferrite-core/src/content.rs` (`create`, `update`), `crates/ferrite-core/src/content/lifecycle.rs` (`transition`), `crates/ferrite-core/src/content/admin.rs` (`restore_revision`), `crates/ferrite-core/src/media.rs` (`record_upload`, `update`, `trash`, `restore`; factor the trash/restore bodies into `pub(crate) async fn trash_in` / `restore_in` for Task 5), `crates/ferrite-core/src/comments.rs` (`moderate`, `submit` for `Commenter::User`)
- Test: `crates/ferrite-core/tests/change_capture.rs` (new)

**Interfaces:** consumes Task 2; produces the ops `content.*`, `media.*`, `comment.*` of design 17 §3.3.

- [ ] Tests (SQLite and Postgres through the core test harness): `agent_create_update_publish_records_three_changes`, `plain_actor_records_nothing`, `stale_version_conflict_records_nothing`, `media_upload_records_one_media_change_not_content_plus_media`, `media_update_captures_alt`, `trash_records_trashed_from`, `comment_reply_and_moderate_are_captured`, `before_and_after_hashes_chain_across_two_updates`.
- [ ] Implement the two-line captures; pass; clippy clean.
- [ ] Commit `feat(core): record assistant writes to content, media and comments`.

### Task 4: Capture structure, settings and extension writes

**Files:**
- Modify: `crates/ferrite-core/src/taxonomy.rs` (`create_term`, `update_term`, `delete_term`), `crates/ferrite-core/src/menu.rs` (`put`), `crates/ferrite-core/src/types.rs` (`create`, `update`), `crates/ferrite-core/src/redirects.rs` (`upsert`, `update`, `delete`), `crates/ferrite-core/src/options.rs` (`OptionService::write`: one `ObjectRef::Option` per key, only when `settings_write`), `crates/ferrite-core/src/extensions/admin.rs` (`activate_theme`, `install_theme` when it activates, `set_plugin_enabled`, `set_theme_settings`, `set_plugin_settings`: pass the `Capture` into the locked `write_*` calls)
- Test: extend `crates/ferrite-core/tests/change_capture.rs`

- [ ] Tests: `term_delete_image_keeps_links`, `menu_set_new_menu_is_created`, `type_update_under_types_lock_is_captured`, `redirect_create_over_existing_source_has_a_before_image`, `settings_update_records_one_change_per_key`, `permalink_change_captured_under_lock`, `plugin_secret_image_stays_sealed` (the image holds `{"sealed": …}`, never the plaintext), `theme_activate_records_previous_theme`.
- [ ] Implement; pass; clippy clean.
- [ ] Commit `feat(core): record assistant writes to terms, menus, types, redirects, settings and extensions`.

### Task 5: `ChangeSetService` — reads and revert of content, media and comments

**Files:**
- Create: `crates/ferrite-core/src/changes/{service.rs, revert.rs, restore_content.rs}`
- Modify: `crates/ferrite-core/src/{lib.rs, events.rs (CHANGE_SET_REVERTED, change_set_reverted(id))}`, `crates/ferrite-core/src/error.rs` only if a shared variant is needed (prefer `RevertError` in `changes`)
- Test: `crates/ferrite-core/tests/change_revert.rs` (new)

**Interfaces:**
- `ChangeSetService::new(store, permalinks: Arc<dyn Permalinks>, lock_wait)`; `list(actor, ChangeSetQuery)`, `get(actor, id) -> ChangeSetDetail`, `change(actor, id, seq) -> ChangeDetail { change, before: Images, after: Images }` (`Images::Hidden` without area read rights), `revert(actor, id, RevertRequest { changes: Option<Vec<u32>>, force, dry_run }) -> Result<RevertReport, RevertError>`.
- `RevertError { Conflicts(Vec<RevertConflict { seq, reason: ConflictReason { ChangedSince { by: Option<UserId>, at: Option<Timestamp> }, LaterChange(u32), Gone, TooLarge }, object }>), Failed { seq, error: CoreError }, Core(CoreError) }`.
- `RevertReport { reverted, skipped, forced, notes: Vec<(u32, String)> }`.
- `pub(crate)` restore functions for kinds `content`, `media`, `comment` (design 17 §7.3).

- [ ] Tests: `revert_create_trashes_the_item`, `revert_publish_unpublishes_and_keeps_published_at`, `revert_two_updates_newest_first_without_force`, `person_edit_between_is_changed_since_naming_the_revision_author`, `force_overwrites_and_marks_forced`, `later_change_in_same_set_refuses_single_revert`, `any_failure_rolls_back_every_change` (a second change whose reverse write is forbidden), `dry_run_writes_nothing`, `revert_closes_the_set_and_audits_and_emits_event`, `media_upload_revert_trashes_media`, `comment_reply_revert_trashes_comment`, `only_owner_or_admin_may_revert` (another editor: `NotFound`/`Forbidden` like grants), `images_hidden_without_area_read`, `reverting_person_role_is_checked` (a demoted user's revert of a publish fails naming `publish_own`), `revert_writes_no_change_set`, `scheduled_time_passed_leaves_draft_with_note`.
- [ ] Implement; pass on both backends; clippy clean.
- [ ] Commit `feat(core): review and revert change sets of content, media and comments`.

### Task 6: Revert of structure, settings and extensions; retention

**Files:**
- Create: `crates/ferrite-core/src/changes/restore_structure.rs`, `crates/ferrite-core/src/changes/restore_admin.rs`
- Modify: `crates/ferrite-core/src/changes/revert.rs` (lock set: `LOCK_TYPES` then `LOCK_EXTENSIONS` before `begin`), `crates/ferrite-core/src/taxonomy.rs` (`pub(crate)` term restore helper re-inserting with id and links), `crates/ferrite-core/src/redirects.rs` (`pub(crate) restore_in(repos, before: Option<&Redirect>, current)`), `crates/ferrite-core/src/extensions/mod.rs` (make `write_activate_theme`, `write_plugin_enabled` `pub(crate)` if they are not), `crates/ferrite-core/src/settings.rs` (`mcp.change_sets_days` 1..=365 default 90), `crates/ferrite-cluster/src/builtin.rs` (`change_sets.prune` handler + periodic), `crates/ferrite-http/src/mcp/tools/settings.rs` (refuse every `mcp.` key)
- Test: extend `crates/ferrite-core/tests/change_revert.rs`; `crates/ferrite-cluster/src/builtin.rs` tests; `crates/ferrite-http/tests/api/mcp_admin.rs` (`mcp_settings_not_writable_via_mcp`)

- [ ] Tests: `revert_term_delete_reinserts_with_links`, `revert_term_create_refused_with_children`, `revert_menu_put_restores_tree_and_new_menu_is_deleted`, `revert_type_create_refused_while_it_has_items`, `revert_redirect_update_restores_source_and_origin`, `revert_settings_in_one_set_many_call`, `revert_permalink_change_takes_types_lock_and_enqueues_redirects`, `revert_plugin_settings_restores_sealed_secret` (the plugin's `settings.get` opens it after the revert), `revert_theme_activation_requires_same_artifact`, `revert_plugin_disable_enables`, `prune_respects_change_sets_days_and_default`, `periodics_list_has_change_sets_prune`.
- [ ] Implement; pass on both backends; clippy clean.
- [ ] Commit `feat(core): revert structure, settings and extension changes; change-set retention`.

### Task 7: REST API `/api/v1/change-sets`

**Files:**
- Create: `crates/ferrite-http/src/api/change_sets.rs`, `crates/ferrite-http/src/api/dto/change_sets.rs`, `crates/ferrite-http/tests/api/change_sets.rs` (+ mod registration)
- Modify: `crates/ferrite-http/src/api/{mod.rs (routes), dto.rs, openapi.rs, state.rs (ApiParts/ApiState.change_sets)}`, `crates/ferrite-http/tests/api/harness.rs`, `crates/ferrite/src/serve.rs` (wiring), `crates/ferrite-http/openapi.json`

**Interfaces:** design 17 §9 routes and DTOs; revert session-only (as `api::oauth::session_only`), CSRF, `Idempotency-Key`; `409 change-conflict` with `report.conflicts`; reverse-write errors carry `report.change`; plugin secrets redacted to `{"secret": "set"|"unset"}`.

- [ ] Tests: `own_sets_listed_after_mcp_writes`, `others_need_manage_users_and_all_users`, `detail_lists_changes_newest_first`, `change_detail_shows_images_or_hidden`, `secret_is_redacted_in_change_detail`, `revert_requires_session_not_token`, `revert_conflict_is_409_with_list`, `revert_force_and_dry_run`, `revert_bad_seq_list_is_422` (unknown, > 500, duplicates folded), `openapi_snapshot_is_current`.
- [ ] Implement; regenerate the spec; pass.
- [ ] Commit `feat(api): change sets — list, detail, diff images and revert`.

### Task 8: MCP — `changes_list`, instructions, the capture guard test

**Files:**
- Create: `crates/ferrite-http/src/mcp/tools/changes.rs`, `crates/ferrite-http/tests/api/mcp_changes.rs`
- Modify: `crates/ferrite-http/src/mcp/tools/mod.rs`, `crates/ferrite-http/src/mcp/server.rs` (`routers()` adds `change_tools`; `instructions()` sentence), `site/content/05-reference/02-mcp-tools.md` (row; count 57), `crates/ferrite-http/tests/api/mcp_reference.rs` if it counts tools

- [ ] Tests: `changes_list_shows_only_own_agent` (a second grant of the same user sees nothing of the first), `changes_list_has_review_url_and_no_images`, `oauth_refresh_keeps_one_set`, `rest_write_with_same_token_records_nothing`, `every_write_tool_records_a_change` (iterate `tools/list`; for each tool with `readOnlyHint` false call it with a fixture input from a table in the test and assert one more change; the table must cover every such tool, so a new write tool fails the test until it captures), `instructions_mention_change_sets`, `reference_page_lists_every_tool`.
- [ ] Implement; pass.
- [ ] Commit `feat(mcp): changes_list and change-set notes for assistants`.

### Task 9: Admin — AI activity, change set diffs, revert dialog, links

**Files:**
- Create: `admin/src/api/change-sets.ts`, `admin/src/ai-activity/{activity-list.tsx, change-set-screen.tsx, change-row.tsx, json-diff.tsx, content-change-diff.tsx, revert-dialog.tsx, model.ts}` (+ `*.test.tsx`), `admin/src/routes/_app/ai-activity.index.tsx`, `admin/src/routes/_app/ai-activity.$id.tsx`, `admin/e2e/ai-activity.spec.ts`
- Modify: `admin/src/api/keys.ts`, `admin/src/api/schema.d.ts` (regenerated), `admin/src/shell/nav.ts` (screen `activity`), `admin/src/palette/palette.tsx`, `admin/src/account/connected-apps-card.tsx`, `admin/src/account/tokens-card.tsx`, `admin/src/users/user-apps.tsx`, `admin/src/users/user-tokens.tsx`, `admin/src/dashboard/ai-card.tsx`, `admin/src/settings/ai-assistants.tsx` (retention field), `admin/src/i18n/en/ai.json`, `admin/src/routeTree.gen.ts` (generated)

- [ ] Vitest: list renders sets with status badges and filters; detail renders content diffs through `diffRevision`/`RevisionDiff` and other kinds through `JsonDiff`; hidden images message; revert dialog runs `dry_run`, lists conflicts, keeps confirm disabled until "Overwrite later edits"; 409 race reopens the dialog; links from Connected apps and tokens.
- [ ] E2E `ai-activity.spec.ts` (Chromium, both backends): create a personal token, make MCP writes through the API helper (create + update a post, rename a term), open AI activity, see the set, expand the post diff, edit the post in the editor, try "Revert all" → conflict listed, tick overwrite → reverted, the post is trashed and the term renamed back; axe clean on both screens.
- [ ] `pnpm -C admin typecheck && pnpm -C admin lint && pnpm -C admin test` and the new spec pass.
- [ ] Commit `feat(admin): AI activity — change sets, diffs and revert`.

### Task 10: Docs

**Files:** `site/content/02-guides/08-ai-assistants.md` (section "Review and undo what an assistant did"; tool count), `site/content/05-reference/02-mcp-tools.md` (prose for `changes_list`), `docs/design/07-admin-api.md` (§1 endpoint rows, §3 screen rows), `docs/design/12-mcp.md` (§5.1 audit bullet points to 17; §10), `docs/design/00-overview.md` (row 17), `README.md` (tool count, one line on undo), `site/VERIFIED.md` (rows), a screenshot of the change set screen (WebP 2×, from the demo site).

- [ ] Every command and claim is run or checked and recorded in `VERIFIED.md`; `cargo test -p ferrite --test docs_site` passes.
- [ ] Commit `docs: AI change sets — review and undo`.

### Task 11: Exit

- [ ] Real client check: Claude Code connected to a local release build (`claude mcp add --transport http ferrite http://localhost:8080/mcp` with a personal token): ask it to draft and publish a post and add it to a menu; ask "what did you change?" (it calls `changes_list`); revert the set in the admin; record outputs.
- [ ] Full gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` with `FERRITE_TEST_PG_URL`, admin typecheck/lint/test, E2E (Chromium both backends; the new spec in Firefox and WebKit), docs export link check.
- [ ] Exit doc `docs/superpowers/m6-5-exit.md` (what was built, what was verified, what was not: e.g. hosted clients, a cluster of two nodes reverting concurrently), rulings in `docs/superpowers/rulings/m6.md` (M6-5 section).
- [ ] Tick the two M6-5 items in `docs/ROADMAP.md`; update `CLAUDE.md` (status line, tool count 57, migration `0018_change_sets`, `ferrite-core::changes` in the crate map).
- [ ] Commit `docs: M6-5 exit`.
