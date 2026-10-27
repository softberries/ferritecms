# M2b-3 — Administration, Extensions and the M2 Exit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The admin gains the remaining M2 screens:
- own account: profile, password, TOTP enrolment with a QR code, own tokens, sessions;
- users: list, roles, tokens, TOTP reset;
- settings: general, reading, permalinks, comments, media sizes, mail;
- themes: upload, activate, settings form;
- plugins: install flow with the permission report and fingerprint approval, enable/disable, delete and purge, settings with write-only secrets, metrics;
- system: status, schema, backends, nodes, jobs with dead-job retry, audit log.

Every E2E spec runs on both backends and in Chromium, Firefox and WebKit. CI enforces a bundle-size budget. The M2b-1 and M2b-2 follow-ups and the review minors left open are closed. M2 is exited.

**Architecture:** Four Rust tasks close the API gaps first. They were found by mapping each screen to `crates/ferrite-http/openapi.json` (`.superpowers/m2b3-api-map.md`, research of 2026-10-27, git-ignored). The gaps fall into four areas:
- users and account: TOTP reset by an admin, list filters, a password for an e-mail change, sessions;
- extensions: the pack CLI, richer DTOs, late purge of retained plugin data;
- system: audit filters, mail facts and a test mail, typed node meta, the site title read at use time;
- content follow-ups: inline image `srcset`, relation target types, media restore keeping its status.

Then the admin work, in four groups:
- one kit task;
- six screen tasks on the M2b-1 and M2b-2 foundation (API client, kit, shell, `staleGuard`, `snap()`, `expectAccessible`), each with its E2E spec against the release binary, a contact sheet and axe;
- two follow-up tasks: the editor, and everything else;
- the cross-browser and both-backend E2E matrix, then the bundle budget.

A last task writes the docs, the M2b-3 exit and the M2 exit.

**Tech Stack:** as M2b-2 (pins in `admin/package.json`). One new runtime pin, `uqr` (QR code, see Global Constraints). Rust side unchanged (axum 0.8.9, utoipa 6.0.0, sqlx 0.9). Playwright 1.64.0 now also drives Firefox and WebKit.

**Spec:**
- `docs/ROADMAP.md` "M2b-3": every item, the M2b-1 and M2b-2 follow-up lists and "Docs and the M2 exit".
- `docs/design/07-admin-api.md`:
  - §1: Users, Auth, Settings, Themes, Plugins, System resources;
  - §2: auth, TOTP, tokens, sessions;
  - §3: screen rows Users, Themes, Plugins, System, Settings, and the Dashboard row's "plugin hook p95 latency per plugin with the Plugins screen (M2b-3)".
- `docs/design/11-admin-ui.md`: design language, §6 keyboard map (`g s` Settings), §7 kit, §8 review checklist.
- `docs/design/05-plugin-host.md`: permission report, grants, limits, settings and secrets.
- `docs/design/06-themes-rendering.md`: §1 theme package and bundle, settings, `[images]`, menus; §4 the `image()` helper and `srcset`.
- `docs/design/09-operations.md`: CLI, mail.
- `docs/design/10-testing.md` §3: admin tests and CI.
- `docs/superpowers/m2b2-exit.md`: "Open review minors", §4 contact-sheet findings, §2 E2E race.
- Rulings `docs/superpowers/rulings/m2b.md`: P1–P40 and R1–R36 stay binding. The controller appends this plan's rulings P41–P58 under "M2b-3 plan rulings" at the start, and execution rulings as R37 onwards.

## Plan rulings (appended to `m2b.md` at the start)

- **P41:** M2b-3 is 16 tasks.
  - Four Rust API tasks come first, because the screens consume them.
  - Two follow-up tasks, then the E2E matrix, the bundle budget and the exit.
  - Cost if wrong: the screens wait for the API.
- **P42: Theme upload.** The server keeps its canonical bundle format (no wider untrusted parser).
  - The admin packs a picked or dropped folder into the canonical ustar in the browser. The packer is a TS port of `bundle::pack`, pinned byte for byte against a fixture the Rust packer wrote.
  - A `.tar` file can also be sent as it is; the server's refusal is shown.
  - `ferrite theme pack --dir DIR --out FILE` gives API users a valid file.
  - Cost: two packers to keep equal (the fixture test pins them).
- **P43: An admin can switch off another user's TOTP** (`POST /users/{id}/totp/disable`).
  - Not for oneself (`/auth/totp/disable` with a password).
  - The target's sessions are kept; audited.
  - There are still no recovery codes.
  - Cost: an admin account can remove a second factor without the owner (it can already set their password).
- **P44: Changing one's own e-mail needs `current_password`**, because the e-mail is the reset channel.
  - An admin changing someone else's e-mail does not need it.
  - Cost: one more field in the account form.
- **P45: Sessions:** `GET /auth/sessions`, `DELETE /auth/sessions/{id}`, `POST /auth/sessions/revoke-others`.
  - A session is shown by an opaque id (not the cookie value or its stored hash), created and last-seen times, and `current`.
  - No user agent is stored, so none is shown. No migration in M2b-3.
  - Cost: sessions are told apart only by time.
- **P46: User list filters** `q` (login, display name or e-mail substring), `role`, `status`.
  - Delete asks for a reassignment only when the server's `422` says the user owns content (field `reassign`). There is no count endpoint.
  - Cost: one refused request before the reassign picker shows.
- **P47: Plugin data retention becomes visible.**
  - `GET /plugins/retained` lists deleted plugins whose data is still kept, with the purge date.
  - `POST /plugins/{name}/purge` erases it early. A `422` (field `name`) while a plugin of that name is installed.
  - Cost: one store query on both backends.
- **P48: Hook latency is shown as a p95 upper bound** from the existing six buckets ("p95 ≤ 25 ms"), labelled "This node, since <start>".
  - No host change; cluster-wide series stay M4.
  - Cost: a coarse number.
- **P49: Mail stays config-only.**
  - `GET /system/status` gains `mail: { configured, from }` (never the SMTP URL).
  - `POST /system/mail/test` sends one mail to the caller's own address (`manage_system`; 3 per 10 minutes per user; audited `mail.test`; `422` when mail is not configured).
  - Settings shows a read-only Mail panel with that test.
  - Cost: no mail settings in the UI.
- **P50: Media sizes are the active theme's `[images]`, shown read-only** in Settings from `ThemeDto.images`.
  - No size setting; regenerating variants stays M4.
  - Cost: changing sizes needs a theme change.
- **P51: The TOTP issuer and the password-reset mail read the `site.title` option at use time**, falling back to `ferrite.toml`. This closes the split between Settings and the startup value.
  - Cost: one option read per enrolment or mail.
- **P52: Relation fields gain `types: string[]`** in `FieldDef` (empty = any type, the M2b-2 behaviour).
  - Core refuses an item of another type on write (`422` at the field's path).
  - The field builder and the picker use it.
  - Cost: one item read per relation value on write (batched per save).
- **P53: A trashed media item remembers its status** in `content_meta` `media.trashed_from`, written in the trash `Tx`.
  - Restore re-applies it: `published` only if it was published, else `draft`.
  - Items trashed before this change restore as published (R23 behaviour).
  - Cost: one meta row per trashed media item.
- **P54: R34 is closed by ruling.** Moderators keep no comment rate limit: they are trusted, audited accounts, and a compromised one is disabled.
  - **R36 is closed by ruling.** No released site predates M2b-2, so no migration path for a `refs` type is built; the ROADMAP says so.
  - Cost: none for existing sites.
- **P55: E2E matrix.** CI runs every spec in four parallel jobs: `chromium-sqlite`, `chromium-pg`, `firefox-sqlite`, `webkit-sqlite`.
  - Design snapshots (`snap()` and the contact sheet) run only in `chromium-sqlite`.
  - A spec or step that cannot run in an engine is skipped with `test.skip(browserName === …, "<reason>")`, and every skip is listed in 10 §3.
  - Linux WebKit is not Safari: a manual Safari pass stays in the exit.
  - Cost: about four times the E2E CI minutes, in parallel wall-clock.
- **P56: Bundle budget.** `admin/bundle-budget.json` holds gzip -9 limits per group (entry, entry plus modulepreloads, main CSS, editor chunk, chart chunk, largest other lazy chunk).
  - Each limit is the M2b-3 size measured in Task 15, plus 10 %, rounded up to the next KiB.
  - `pnpm -C admin bundle:check` fails CI over a limit. Raising a limit is a reviewed edit of that file.
  - Cost: a deliberate step for growth.
- **P57: The explicit-theme flash is fixed server-side.**
  - The admin writes a cookie `ferrite_admin_theme=light|dark` (path `/admin/`, `SameSite=Lax`, one year; removed for `system`).
  - Serving `index.html` adds `class="dark"` or `class="light"` to `<html>` from it.
  - No inline script is added, so the CSP is unchanged.
  - Cost: `index.html` is rewritten per request (it is `no-cache` anyway).
- **P58: Settings screens are last-write-wins** (`staleGuard`, P31).
  - A permalink change confirms first, saying every post or page URL moves and no redirects are created (M3).
  - Cost: a dialog on a rare change.

## Global Constraints

- **Everything in the M2b-1 and M2b-2 plans' Global Constraints still holds.** In particular:
  - CLAUDE.md is binding.
  - `thiserror` in libraries, `anyhow` only in the binary.
  - Capabilities are checked in core.
  - `ApiPath<T>`, `check_limit`, `CoreError::invalid_field`.
  - The `unsafe_operations_need_auth_and_csrf` sweep, with no `UNAUTHENTICATED` additions.
  - `test-seams`; no new problem slug.
  - Rust gates on both backends, with and without `admin/dist`.
  - Admin gates `lint`, `typecheck`, `test`, `api:check`, `build`, `e2e`.
  - The design gate: `snap()` plus the contact sheet against 11 §8.
  - `expectAccessible` in light and dark.
  - The CSP gate.
  - `t()` for every string.
  - `uid()` data hygiene.
  - Before each task's final E2E: `pnpm -C admin build` and then `cargo build --release -p ferrite`.
- **Untrusted text is text.** Rendered as text nodes, never `dangerouslySetInnerHTML`:
  - plugin manifests and permission report texts;
  - theme descriptions and authors;
  - node errors;
  - job `last_error`;
  - audit `details`;
  - user display names;
  - token names.

  A plugin or theme homepage is a link only for `http:`/`https:`, with `rel="noopener noreferrer nofollow"`.
- **Secrets are shown once and never stored by the SPA.** This covers a new token's value, the TOTP `secret_base32` and `otpauth_url`, and plugin secret settings:
  - not in a URL, `history`, `localStorage`, the query cache or a log;
  - each held only in component state until the dialog closes.

  A plugin secret input is never pre-filled. An untouched secret is absent from the `PATCH`; a typed value sets it; "Remove" sends `null`.
- **No migration in M2b-3.** The schema version stays 8.
- **New frontend pin.** The newest stable release published on or before 2026-10-13, at least two weeks before 2026-10-27 (`npm view <pkg> time`, checked 2026-10-27):

  | Package | Pin | Published | Licence | Deps | Use |
  |---|---|---|---|---|---|
  | uqr | 0.1.3 | 2026-04-03 | MIT | none | `encode(text)` gives the module matrix. The admin draws its own `<svg>` of `<rect>`s in React (no `innerHTML`). The implementer checks the API against Context7 or the package README before use. |

  Not chosen:
  - `qrcode` 1.5.4: depends on `pngjs` and `yargs`.
  - `qrcode-generator` 2.0.4: returns HTML strings.
- **Exact values:**
  - Lists: users, audit and jobs page by 50.
  - User `q`: trimmed, 1..=100 characters, case-insensitive substring; else `422` field `q`.
  - TOTP QR: 4-module quiet zone, drawn at 200 px; the secret shown in groups of 4 characters with Copy.
  - New-token scopes: the 11 areas (`content media terms menus comments users settings redirects extensions import system`), each `read` or `write`. Expiry: Never, 30, 90 or 365 days.
  - Theme upload: folder limits as the server's (2000 files, 20 MiB, depth 16, path ≤ 255 bytes); hidden entries (name starting with `.`) skipped, as `package::is_hidden` does.
  - After activating a theme, `GET /themes` is polled every 2 s for up to 20 s to show node state.
  - Plugin metrics: refetched every 30 s while the tab is visible.
  - Variants: none new.
  - Shortcuts: `g s` Settings, `g u` Users, `g y` System, `g p` Plugins, `g a` Appearance (themes).
  - Audit `action` filter: an exact action, or a prefix ending in `.` (for example `user.`).

## Review Focus

1. **An admin cannot lock themselves out or remove the last admin by accident.**
   - Expected behaviour:
     - one's own row has no role, status or delete controls;
     - the server's last-admin `422` is shown in the dialog;
     - changing one's own password keeps the session (the new CSRF token is stored);
     - disabling one's own TOTP needs the password.
   - *Tests:*
     - Task 7 E2E `own_row_has_no_role_status_or_delete` and unit `last_admin_refusal_is_shown`;
     - Task 6 E2E `password_change_keeps_the_session`, `totp_disable_needs_password`.
2. **Plugin install approves exactly what was inspected.**
   - Expected behaviour:
     - the report highlights high-risk items, `added` grants on an update, and limits above the default;
     - if the files change between inspect and install, the server's new report replaces the old one and must be approved again;
     - a new plugin arrives disabled.
   - *Tests:*
     - Task 10 `install-flow.test.tsx` `fingerprint_mismatch_shows_the_new_report`;
     - Task 10 E2E `install_inspect_approve_enable` (with the `plugins/examples` test plugin built in CI).
3. **Secrets are write-only and shown once.**
   - Expected behaviour:
     - a plugin secret field is never pre-filled, and saving without touching it keeps it;
     - a new token's value is visible only in the creation dialog;
     - the TOTP secret is gone after the dialog closes;
     - none of them appears in `location.href`, `localStorage` or the query cache.
   - *Tests:*
     - Task 5 `secret-field.test.tsx`;
     - Task 10 E2E `untouched_secret_is_kept`;
     - Task 6 E2E `token_value_is_shown_once`, `totp_secret_not_kept`.
4. **Destructive administration says what it does before doing it.**
   - Expected behaviour:
     - a permalink change warns that URLs move without redirects;
     - a theme upload that replaces an installed theme of that name says so (and that it goes live at once if active);
     - plugin delete offers "Delete, keep data 30 days" and "Delete and erase data" as separate choices;
     - user delete asks for a reassignment when the server says the user owns content.
   - *Tests:*
     - Task 8 E2E `permalink_change_warns`;
     - Task 9 E2E `replacing_a_theme_warns`;
     - Task 10 E2E `delete_keep_vs_erase`;
     - Task 7 E2E `delete_with_content_asks_reassign`.
5. **Untrusted extension and system text never becomes markup.**
   - Inputs: a plugin manifest with `description = "<img src=x onerror=alert(1)>"`, a theme description with HTML, a job `last_error` and an audit `details` value with HTML.
   - Expected behaviour: all show as literal text; the CSP guard sees no violation and no dialog opens.
   - *Tests:* Task 10 E2E `hostile_manifest_renders_as_text`; Task 11 E2E `hostile_job_error_renders_as_text`.

---

### Task 1: Users and account API — admin TOTP reset, user filters, e-mail re-authentication, sessions

**Files:**
- Create: `crates/ferrite-http/tests/api/{users_admin.rs,sessions.rs}`, `crates/ferrite-store-tests/src/user_filters.rs`
- Modify:
  - store: `crates/ferrite-store/src/repos.rs` (`UserRepo::list` takes `&UserQuery`; `SessionRepo` read and delete by public id), both backends' user and session repos, `crates/ferrite-store-tests/{src/lib.rs,tests/contract.rs}`;
  - core: `crates/ferrite-core/src/{users.rs,auth/service.rs}`;
  - API: `crates/ferrite-http/src/api/{users.rs,auth.rs,mod.rs,openapi.rs,dto/users.rs,dto/auth.rs}`, `crates/ferrite-http/openapi.json`;
  - docs: `docs/design/07-admin-api.md` §1 (Users), §2 (sessions, TOTP).

**Interfaces — Produces:**
- `UserQuery { q: Option<String>, role: Option<Role>, status: Option<UserStatus>, limit, cursor }`, used as `UserRepo::list(&UserQuery)`. `q` is a case-insensitive substring of login, display name or e-mail, with `%`, `_` and `\` escaped (`LIKE … ESCAPE '\'`; Postgres `ILIKE`). The order (login) and the cursor are unchanged.
- `GET /users?q&role&status&limit&cursor`. A bad `q` → `422` field `q`; an unknown role or status → `400`.
- `UserService::disable_totp_for(actor, id)`:
  - needs `manage_users` + `users:write`;
  - self → `422` field `id` ("use your own account's TOTP settings");
  - not enabled → `422`;
  - audited `user.totp_disable` with the actor; sessions kept (P43).
- `POST /users/{id}/totp/disable` → `204`.
- `UserPatchInput.current_password?: string`, required when the actor changes their own `email` (P44):
  - missing → `422` field `current_password`;
  - wrong → `403`;
  - rate-limited with the same per-login bucket as `/auth/password`.
- Sessions (P45):
  - `SessionDto { id: string /* first 16 hex of SHA-256 over the stored session key */, created_at, last_seen_at, current: boolean }`.
  - `GET /auth/sessions` → `{ items }`, newest first; own sessions only.
  - `DELETE /auth/sessions/{id}`: deleting `current` → `422` (use sign-out); unknown → `404`.
  - `POST /auth/sessions/revoke-others` → `{ revoked: n }`.
  - All three need `users:write` (sessions only; a bearer token has none, so `401`/`403` as other session-only routes) and are audited `session.revoke`.

**Tests:**
- Contract (both backends):
  - `user_list_filters` (q over login, display name and e-mail; `50%_x` is literal; role and status combine with the cursor);
  - `session_public_id_roundtrip`.
- Core:
  - `disable_totp_for_other_user` (and self `422`, an editor `Forbidden`, audit row);
  - `own_email_change_needs_password` (an admin editing another user needs none).
- API:
  - `users_filter_params`;
  - `sessions_list_and_revoke` (two logins → two items, one `current`; revoke-others leaves one; the deleted session's cookie gets `401`);
  - `session_routes_refuse_tokens`.
- The sweep covers the new routes. Snapshot regenerated.

- [ ] **Step 1:** Write the tests and register them.
- [ ] **Step 2:** Run `cargo test -p ferrite-store-tests --test contract user_list_filters` and `cargo test -p ferrite-http --test api sessions` and expect compile failures.
- [ ] **Step 3:** Implement; regenerate the snapshot; update 07.
- [ ] **Step 4:** Run `cargo test -p ferrite-store-tests -p ferrite-core -p ferrite-http` with `FERRITE_TEST_PG_URL` set and expect PASS.
- [ ] **Step 5:** Commit `feat(api): user filters, admin TOTP reset, password for own e-mail change, sessions`.

### Task 2: Extensions API — richer theme and plugin DTOs, retained plugin data, `ferrite theme pack`

**Files:**
- Create: `crates/ferrite-http/tests/api/extensions_m2b3.rs`, `crates/ferrite/tests/theme_pack.rs`, `admin/src/appearance/__fixtures__/blog-min/` (a minimal theme folder) and `admin/src/appearance/__fixtures__/blog-min.tar` (written by the new CLI from that folder, committed)
- Modify:
  - store: `crates/ferrite-store/src/repos.rs` (retained-plugin marker query), both backends' extension repos, `crates/ferrite-store-tests/src/extensions.rs`;
  - core: `crates/ferrite-core/src/extensions/{admin.rs,mod.rs}`;
  - API: `crates/ferrite-http/src/api/{themes.rs,plugins.rs,mod.rs,openapi.rs,dto/extensions.rs}`, `crates/ferrite-http/openapi.json`;
  - CLI: `crates/ferrite/src/{cli.rs,ext_cmd.rs}`;
  - docs: `docs/design/06-themes-rendering.md` §1 (bundle, pack), `docs/design/07-admin-api.md` §1 (Themes, Plugins), `docs/design/09-operations.md` (CLI).

**Interfaces — Produces:**
- `ThemeDto` gains:
  - `author: string | null`, `license: string | null`, `homepage: string | null`;
  - `has_settings: boolean`;
  - `menus: { name, label }[]`;
  - `images: { name, width, height, crop: boolean }[]` (the manifest's `[images]`; the field names follow `manifest.rs`);
  - `hash: string | null`.

  These come from the stored manifest, or from the embedded theme's manifest.
- `PluginDto` and `PermissionReportDto` gain `description`, `publisher`, `license`, `homepage` (`string | null`).
- `GET /plugins/retained` → `{ items: { name, deleted_at, purge_after }[] }` (`manage_plugins` + `extensions:read`).
- `POST /plugins/{name}/purge` → `204` (`extensions:write`):
  - erases what `purge_plugin_data` erases;
  - audited `plugin.purge`;
  - a plugin of that name installed → `422` field `name`;
  - nothing retained → `404`.
- CLI `ferrite theme pack --dir DIR --out FILE`: writes `bundle::pack` of the folder and prints `packed <n> files, <bytes> bytes, sha256 <hex>`. It needs no data folder.
- `blog-min.tar` is that command's output for `blog-min/`. Task 9's TS packer must reproduce it byte for byte.

**Tests:**
- API:
  - `theme_dto_has_manifest_facts` (the embedded `blog` theme → menus `primary`, `footer`; its `[images]`);
  - `plugin_dto_has_manifest_facts`;
  - `retained_list_and_purge` (delete without purge → listed; purge → gone and the KV rows erased; purge while installed → `422`).
- CLI: `theme_pack_output_installs` (pack → `POST /themes` accepts it; packing twice gives identical bytes).
- Contract (both backends): `retained_plugin_markers`.
- Snapshot regenerated.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement; write the fixture with the new command; update 06, 07, 09.
- [ ] **Step 4:** Run the store, core, API and `ferrite` tests on both backends and expect PASS.
- [ ] **Step 5:** Commit `feat(api): theme and plugin manifest facts, retained plugin data and purge, ferrite theme pack`.

### Task 3: System API — audit filters, mail facts and test mail, typed node meta, site title at use time

**Files:**
- Create: `crates/ferrite-http/tests/api/system_m2b3.rs`
- Modify:
  - store: `crates/ferrite-store/src/repos.rs` (`AuditRepo::list(&AuditQuery)`), both backends' audit repos, `crates/ferrite-store-tests/src/audit.rs`;
  - core: `crates/ferrite-core/src/{system.rs,mail.rs,auth/service.rs}`;
  - API: `crates/ferrite-http/src/api/{system.rs,auth.rs,openapi.rs,dto/system.rs,state.rs}`;
  - wiring: `crates/ferrite/src/serve.rs`;
  - spec: `crates/ferrite-http/openapi.json`;
  - docs: `docs/design/07-admin-api.md` §1 (System), `docs/design/09-operations.md` (mail).

**Interfaces — Produces:**
- `AuditQuery { actor: Option<UserId>, action: Option<String>, target_kind: Option<String>, target_id: Option<String>, limit, cursor }`.
  - An `action` ending in `.` is a prefix match; otherwise it is exact.
  - `GET /system/audit?actor&action&target_kind&target_id&limit&cursor`. `target_id` without `target_kind` → `422`.
- `SystemStatusDto.mail: { configured: boolean, from: string | null }` (P49).
- `POST /system/mail/test` → `202 { to }`:
  - sends to the actor's own e-mail through `MailQueue`;
  - `manage_system` + `system:write`;
  - `check_limit` 3 per 600 s per user;
  - audited `mail.test`;
  - not configured → `422` field `mail`.
- `NodeDto.meta: NodeMetaDto` with every member optional, for rolling deploys:
  - `theme { name, hash, origin, error, fallback }`;
  - `plugins: { [name]: { version, hash, status, error } }`;
  - `plugins_error`, `permalinks_error`, `job_kinds[]`, `site_id`, `site_url`, `mode`, `clock_skew_ms`.

  Unknown members are ignored and never fail a read.
- TOTP issuer and reset mail (P51): the `site.title` option at use time, else `ferrite.toml`. `ApiState.site_title` is removed if nothing else uses it.

**Tests:**
- Contract (both backends): `audit_filters` (actor, exact action, the `user.` prefix, target, and each with the cursor).
- API:
  - `status_reports_mail_without_secrets` (`smtp_url` never appears in the body);
  - `mail_test_sends_to_self_and_is_limited` (a test mailer seam; the fourth call → `429`);
  - `nodes_meta_is_typed`;
  - `totp_issuer_follows_site_title_option` (`PATCH /settings site.title` → the `otpauth_url` issuer changes).
- Snapshot regenerated.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement; update 07 and 09.
- [ ] **Step 4:** Run the store and API tests on both backends and expect PASS.
- [ ] **Step 5:** Commit `feat(api): audit filters, mail status and test mail, typed node meta, site title read at use time`.

### Task 4: Content follow-ups — inline image srcset, relation target types, media restore keeps its status, Rust review minors

**Files:**
- Modify:
  - core: `crates/ferrite-core/src/{fields.rs,media.rs,content/*}`, the core renderer for `ferrite:media/<id>` images (`crates/ferrite-core/src/blocks/*` or the theme renderer, wherever `image` nodes are rendered today);
  - API: `crates/ferrite-http/src/api/{media.rs,content.rs,dto…}`, `crates/ferrite-http/openapi.json`;
  - docs: `docs/design/06-themes-rendering.md` §4, `docs/design/03-media.md`, `docs/design/07-admin-api.md` §1 (Types, Media);
  - plus the files the minors below name.

**Interfaces — Produces:**
- **Inline images (R28):** a body `image { src: "ferrite:media/<id>", alt }` renders as the `image()` helper does:
  - `src` is the largest variant no wider than the theme's largest size, or the original;
  - `srcset` lists every ready variant plus the original with `w` descriptors;
  - `sizes="(max-width: <largest>px) 100vw, <largest>px"`;
  - `width` and `height` from the media's dimensions;
  - `loading="lazy"`, `decoding="async"`.

  A missing or non-image media item keeps today's output. The variant reads are recorded under the page's existing dependencies.
- **Relation targets (P52):**
  - `FieldDef.types: Vec<String>` (`#[serde(default)]`, only for `relation`; another kind with `types` → `422`; every name must be a registered type other than `media`).
  - A relation value whose item type is not listed → `422` at the field's path (for example `fields.related`), with one `get_many` per write.
  - `TypeDto` and the field input carry `types`.
- **Media restore (P53):**
  - trash writes `content_meta` `media.trashed_from` (`"published"` or `"draft"`) in the same `Tx`;
  - restore re-applies it (publish only when it was `published`) and deletes the row;
  - with no row, restore publishes (R23).
- **Rust review minors from `m2b2-exit.md` "Open review minors".** Each is fixed or answered by a ledger ruling; the task report lists each line with its outcome:
  - `seo_of` re-reads the item;
  - no app-level head test after an SEO-only `PATCH`;
  - `MediaKindParam` duplicates `MediaKind`;
  - `MEDIA_KIND` is defined twice;
  - moderation reads the item after commit;
  - blank `q` is `422` on `/redirects` but no search on `/media`. Make both mean "no search".
  - `RedirectQuery::default()` has `limit` 0;
  - `CommentItemDto.permalink` docs omit media;
  - `cpage` is in the cache key on every route. Restrict it to single-item routes.
  - prev/next comment URLs lack `#comments` (R4 fixed the theme; check the helper);
  - the "Inserted newest first." comment;
  - `peer()` is `pub(crate)`;
  - comment totals and page are read outside one `Tx`;
  - no SQL test for a short page under a tight row budget.

**Tests:**
- Core/app (both backends):
  - `inline_image_has_srcset` (an image with two ready variants → `srcset` with three entries and `w` descriptors);
  - `inline_svg_has_no_srcset`;
  - `relation_types_refuse_other_type`;
  - `relation_types_must_name_types`;
  - `media_restore_keeps_draft_status`;
  - `media_restore_without_marker_publishes`;
  - `head_after_seo_only_patch`;
  - `short_comment_page_under_tight_budget`.
- Snapshot regenerated.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement; work the minors list; update 03, 06, 07.
- [ ] **Step 4:** Run `cargo test --workspace` on both backends and expect PASS.
- [ ] **Step 5:** Commit in up to three commits:
  - `feat(site): srcset for inline media images`;
  - `feat(api): relation target types; media restore keeps its status`;
  - `fix: M2b-2 Rust review minors`.

### Task 5: Admin kit for M2b-3 — nav section, account link, code input, QR code, settings form, secret field, API modules

**Files:**
- Create:
  - kit: `admin/src/components/kit/{code-input.tsx,qr-code.tsx,secret-field.tsx,settings-form.tsx,copy-once.tsx}` and their tests `{qr-code.test.tsx,secret-field.test.tsx,settings-form.test.tsx}`;
  - API modules: `admin/src/api/{users.ts,tokens.ts,sessions.ts,themes.ts,audit.ts,nodes.ts}`;
  - i18n: `admin/src/i18n/en/admin.json`.
- Modify:
  - `admin/package.json` and lockfile (`uqr` 0.1.3);
  - `admin/src/routes/login_.totp.tsx` (uses the extracted `CodeInput`);
  - `admin/src/shell/{nav.ts,nav.test.ts,user-menu.tsx}`, `admin/src/palette/sources.ts`;
  - `admin/src/api/{keys.ts,settings.ts,plugins.ts,system.ts,auth.ts}`;
  - `admin/src/components/kit/use-stale-ask.tsx`: settle a pending ask on a second ask or on unmount (M2b-2 minor);
  - the styleguide patterns, `admin/e2e/styleguide.spec.ts`;
  - `docs/design/11-admin-ui.md` §6 (`g s`, `g u`, `g y`, `g p`, `g a`) and §7.

**Interfaces — Produces:**
- `nav.ts`: section `"admin"`, titled "Administration", after Structure. Its items, each shown only when the capability is held:
  - Users (`manage_users`);
  - Appearance (`manage_themes`);
  - Plugins (`manage_plugins`);
  - Settings (`manage_settings`);
  - System (`manage_system`).

  Account is reached from the user menu ("Account", `/admin/account`) and the palette, for every role. Each screen task registers its own entry, as in M2b-2.
- `CodeInput({ value, onChange, onComplete, autoFocus, label })`: the six-cell code input, extracted unchanged from `login_.totp.tsx`.
- `QrCode({ text, label, size = 200 })`: `uqr`'s `encode` → a matrix → one `<svg role="img" aria-label={label}>` with a `<rect>` per dark module.
  - Quiet zone 4 modules.
  - The colours are fixed black on white in both themes, so scanners read it.
  - The text is never placed in an attribute other than through the matrix.
- `SecretField({ name, set: boolean, onChange: (v: string | null | undefined) => void })`:
  - shows "Set" or "Not set";
  - Replace opens an empty password input;
  - Remove sends `null`;
  - untouched → `undefined` (omitted).
- `SettingsForm({ schema: SettingDefDto[], values, secrets?, invalid?, onSubmit, staleKey })`: renders the setting kinds `text`, `textarea`, `bool`, `number`, `select`, `color`, `media` (the picker from M2b-2) and `secret` (`SecretField`). It has:
  - per-field defaults and "Reset to default" (sends `null`);
  - `invalid[]` keys flagged "Stored value is not valid; the default is used";
  - submit through `staleGuard`;
  - a `PATCH` with only the changed keys.
- `CopyOnce({ value, label })`: a read-only mono field with Copy and the text "Shown only now. Store it somewhere safe." It is used for tokens.
- API modules wrap the Task 1–3 endpoints with typed functions and query keys. `qk` gains `users(filters)`, `user(id)`, `tokens(userId)`, `sessions`, `themes`, `themeSettings(name)`, `pluginSettings(name)`, `pluginsRetained`, `system.nodes`, `system.audit(filters)`.

**Tests:**
- `qr_code_test`: a known `otpauth://` text → the matrix size for its version; the svg has the label; snapshot of the module count.
- `secret_field_test`: untouched → `undefined`; replace → the string; remove → `null`; never renders a value.
- `settings_form_test`: each kind renders and round-trips; only changed keys are sent; a stale value asks.
- `nav_test`: rows for the admin section per role.
- `use_stale_ask_test`: a second ask settles the first; unmount settles.
- The styleguide E2E covers the new patterns.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run `pnpm -C admin test` and expect FAIL.
- [ ] **Step 3:** Add the pin (Context7 or the README for `uqr`), then implement.
- [ ] **Step 4:** Run the admin gates and the styleguide E2E and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): kit for administration screens (code input, QR, secret field, settings form, nav section)`.

### Task 6: Account screen

**Files:**
- Create: `admin/src/account/{account-screen.tsx,profile-card.tsx,password-card.tsx,totp-card.tsx,tokens-card.tsx,token-dialog.tsx,sessions-card.tsx}` with unit tests; `admin/src/routes/_app/account.tsx`; `admin/e2e/account.spec.ts`
- Modify: `admin/src/i18n/en/admin.json`; `docs/design/07-admin-api.md` §3 (an Account row)

**Behaviour** (`/admin/account`, every role; a `must_reset` session still reaches only the forced change):
- **Profile:**
  - display name and e-mail;
  - an e-mail change asks for the current password (P44);
  - login, role and created/last-login times are read-only;
  - `qk.me` is invalidated after a save.
- **Password:**
  - current, new and confirm (the existing `changeSchema`);
  - a success toast says other sessions were signed out.
- **Two-factor:**
  - Off: "Set up" opens a dialog:
    - step 1: the QR code from `otpauth_url` and the grouped secret with Copy;
    - step 2: `CodeInput` plus the password, then confirm.
    - Wrong code → inline; wrong password → inline.
    - On success: "Other sessions were signed out".
  - On: "Turn off" asks for the password.
  - Closing the dialog drops the secret.
- **Tokens:**
  - a table with name, scopes (chips), created, last used and expires;
  - Create: name; scopes as an area grid with None / Read / Write, offering only scopes the session holds (from `me.scopes`); expiry.
  - The result dialog uses `CopyOnce`.
  - Revoke confirms. At most 50 tokens (the server's `422` is shown).
- **Sessions:**
  - the list (created, last seen, "This session");
  - Sign out per row;
  - "Sign out all other sessions".

**Tests:**
- Unit:
  - `token_scopes_offer_only_held_scopes`;
  - `totp_dialog_drops_secret_on_close`;
  - `email_change_requires_password`.
- E2E:
  - `password_change_keeps_the_session`;
  - `totp_enrol_and_sign_in_with_code` (the code is computed in the test from `secret_base32` with an RFC 6238 helper in `e2e/support/totp.ts`);
  - `totp_disable_needs_password`;
  - `token_value_is_shown_once` (also: no secret in `location.href` or `localStorage`);
  - `totp_secret_not_kept`;
  - `sign_out_other_sessions` (a second browser context loses its session);
  - `account_screens` (snaps, axe light and dark, desktop and mobile).

- [ ] **Step 1:** Write the unit tests and the E2E spec.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement; register the route, the palette entry and the user-menu link.
- [ ] **Step 4:** Run the gates; build the admin, then the release binary; run `pnpm -C admin e2e account` and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): account screen (profile, password, two-factor, tokens, sessions)`.

### Task 7: Users screen

**Files:**
- Create: `admin/src/users/{users-list.tsx,user-sheet.tsx,new-user-dialog.tsx,delete-user-dialog.tsx}` with unit tests; `admin/src/routes/_app/users.index.tsx`; `admin/e2e/users.spec.ts`
- Modify: `nav.ts` (the Users entry, `g u`); the palette ("New user"); `docs/design/07-admin-api.md` §3 (Users row as built)

**Behaviour** (`/admin/users`, `manage_users`):
- **List:**
  - search `?q=` (debounced 250 ms);
  - role and status filter chips in the URL;
  - columns: avatar, display name and login, e-mail, role, status, two-factor badge, last login;
  - 50 a page with Load more.
- **New user** dialog:
  - login, e-mail, display name, role;
  - password: "Set a password" (the user must change it at first login), or "Send a reset e-mail" (only when `status.mail.configured`; it calls `POST /auth/password-reset` after creation).
  - Created with `must_reset`.
- **The user sheet** (`?id=`):
  - profile fields, role and status (through `staleGuard`);
  - "Set password" (`POST /users/{id}/password`, says they must change it and their sessions end);
  - "Turn off two-factor" (P43, a confirm);
  - their tokens (list and revoke; creation is in Account only);
  - a Delete section.
- **The own row** (H-A1): opens the sheet with role, status, set-password, two-factor and delete hidden, and a link to Account.
- The last-admin and self `422`s are shown in place. The role-change and disable confirms say that sessions end and tokens are revoked (H-A4).
- **Delete:** confirm; on the `422` field `reassign`, the dialog shows "This user owns content. Reassign it to:" with a user picker, then retries (P46).

**Tests:**
- Unit:
  - `delete_dialog_switches_to_reassign_on_422`;
  - `own_row_hides_controls`;
  - `last_admin_refusal_is_shown`: a mocked `422` from a role change renders its detail in the dialog. From the UI this is reachable only through a race, because an active admin acting on another admin is never the last one.
- E2E:
  - `filter_by_q_and_role`;
  - `create_user_with_password_must_change` (sign in as them in a new context → forced change);
  - `own_row_has_no_role_status_or_delete`;
  - `delete_with_content_asks_reassign`;
  - `admin_turns_off_users_totp`;
  - `users_screens`.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the gates; rebuild; run the E2E and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): users screen`.

### Task 8: Settings screen

**Files:**
- Create: `admin/src/settings/{settings-screen.tsx,general.tsx,reading.tsx,permalinks.tsx,comments.tsx,media-sizes.tsx,mail.tsx,permalink-preview.ts}` with tests; `admin/src/routes/_app/settings.$group.tsx` (with `settings.index` redirecting to `general`); `admin/e2e/settings.spec.ts`
- Modify: `nav.ts` (`g s`); `docs/design/07-admin-api.md` §3 (Settings row as built)

**Behaviour** (`/admin/settings/<group>`, `manage_settings`): one page with a group rail (tabs on phones).

| Group | Contents |
|---|---|
| **General** | Title, tagline and locale (core limits checked first). |
| **Reading** | Front page: "Latest posts", or a published page picked by search and named through `GET /content/refs`. Posts per page (1..=100). Discourage search engines, with a hint naming `robots.txt`. |
| **Permalinks** | Presets (Plain `/%slug%/`, Day `/%year%/%month%/%slug%/`, Category `/%category%/%slug%/`) and Custom, with the token chips. A live preview of a sample post's URL from `permalink-preview.ts`, which is a pure function of the structure and a sample. The page structure is shown with its required `%parent_path%`. Saving a change confirms first (P58). |
| **Comments** | Enabled; moderation (`all` / `known`, explained); comments per page. |
| **Media sizes** | Read-only: the active theme's `images` (name, width × height, crop), "Set by the theme <name>", and a note that changing sizes means changing the theme (P50). |
| **Mail** | Read-only: configured or not, and the From address. "Send test e-mail" sends to my address (`manage_system` only). Without mail, a note on `[mail]` in `ferrite.toml` (P49). |

Each group:
- saves its own keys through `staleGuard` (P31) with `mod+s`;
- asks before leaving with unsaved changes;
- shows `422` errors at their field.

**Tests:**
- Unit:
  - `permalink_preview` (each token; the page structure must contain `%parent_path%`);
  - `reading_front_page_named_by_refs`.
- E2E:
  - `save_general_and_see_title_on_site`;
  - `front_page_choice_changes_home`;
  - `permalink_change_warns` (the confirm text; after saving, a post's URL follows the new structure);
  - `settings_stale_guard`;
  - `mail_panel_without_mail`;
  - `settings_screens`.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the gates; rebuild; run the E2E and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): settings screen`.

### Task 9: Themes screen and the browser bundle packer

**Files:**
- Create: `admin/src/appearance/{themes-screen.tsx,theme-card.tsx,upload-theme-dialog.tsx,theme-settings-sheet.tsx,ustar.ts,ustar.test.ts,pick-folder.ts}`; `admin/src/routes/_app/appearance.themes.tsx`; `admin/e2e/themes.spec.ts`
- Modify: `nav.ts` (`g a`); `docs/design/07-admin-api.md` §3 (Themes row as built); `docs/design/06-themes-rendering.md` §1 (the admin packer)

**Interfaces — Produces:**
- `packTheme(files: { path: string; bytes: Uint8Array }[]): Uint8Array`, the canonical ustar of `bundle::pack`:
  - regular files only, sorted by path bytes;
  - mode `0000644`, mtime 0, uid and gid 0, empty user and group names, `ustar\0` `00`;
  - long paths split into prefix and name at a `/` (name ≤ 100 bytes, prefix ≤ 155);
  - two zero blocks at the end.

  It throws a `PackError` naming the file for a path over the limits, depth over 16, more than 2000 files or more than 20 MiB. Hidden entries are skipped before packing.
- `pickFolder` reads a `<input type="file" webkitdirectory>` selection, or a dropped folder through `DataTransferItem.webkitGetAsEntry()`. It strips the top folder name, so `blog/theme.toml` becomes `theme.toml`.

**Behaviour** (`/admin/appearance/themes`, `manage_themes`):
- **Cards:** name, version, author, description, license and homepage (rendered as text), plus Active / Built in badges.
  - The active card shows node state (node name, loaded or "fallback" with the scrubbed error).
  - Activate confirms, then polls node state (2 s for 20 s). A `422` lists the template errors as text.
- **Upload:**
  - "Choose folder" or drop a folder (packed in the browser, P42), or "Choose .tar file" (sent as is).
  - An installed theme with the same name gets a replace warning, saying it goes live at once if it is active (H-T2).
  - Then `POST /themes` (XHR with progress, like `upload.ts`). The server's `422` problems are listed as text.
- **Customize:** a sheet with `SettingsForm` over `GET/PATCH /themes/{name}/settings`, when `has_settings`.

**Tests:**
- Unit:
  - `pack_matches_the_rust_fixture` (pack `blog-min/` read with `node:fs` → equal bytes to `blog-min.tar`);
  - `pack_refuses_limits`;
  - `long_path_splits`;
  - `hidden_files_skipped`.
- E2E:
  - `upload_folder_installs_and_activates` (the `blog-min` fixture via `setInputFiles` on the folder input; Chromium and Firefox support directory upload in Playwright; on WebKit the same test uses the `.tar` path, so the step is per engine);
  - `replacing_a_theme_warns`;
  - `upload_error_lists_template_problems`;
  - `theme_settings_save`;
  - `themes_screens`.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the gates; rebuild; run the E2E and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): themes screen with in-browser bundle packing`.

### Task 10: Plugins screen

**Files:**
- Create: `admin/src/plugins/{plugins-list.tsx,plugin-detail.tsx,install-flow.tsx,permission-report.tsx,plugin-settings.tsx,plugin-metrics.tsx,latency.ts,retained-data.tsx}` with tests; `admin/src/routes/_app/plugins.index.tsx`, `admin/src/routes/_app/plugins.$name.tsx`; `admin/e2e/plugins.spec.ts`; an E2E fixture plugin with a settings schema (including a `secret`) from `plugins/examples/` (built for `wasm32-wasip2` in CI before E2E; the CI job gains that build step)
- Modify: `nav.ts` (`g p`); `.github/workflows/ci.yml` (build the example plugin for E2E); `docs/design/07-admin-api.md` §3 (Plugins row as built)

**Interfaces — Produces:**
- `p95Bound(buckets: number[]): { le_ms: number | null }`. Buckets are ≤1, ≤5, ≤25, ≤100, ≤500 ms and longer. The result is the smallest bound whose cumulative count reaches 95 % of calls; `null` means "over 500 ms"; no calls gives no value.

**Behaviour:**
- **List** (`/admin/plugins`, `manage_plugins`):
  - name, version, publisher, description, and Enabled / Disabled (with the `disabled_reason`) / Broken (`report: null`, H-P2);
  - failures in 24 h (from the metrics series), and per-node status dots;
  - row actions Enable, Disable, Settings, Delete;
  - "Retained data" below: name, deleted, erased after, and Erase now (P47).
- **Install** (a dialog with three steps):
  1. pick the `ferrite-plugin.toml` and the `.wasm` (two inputs or one drop);
  2. `POST /plugins/inspect`, which shows the permission report (below);
  3. "Approve and install" sends both files and `approve` = `fingerprint`.
  - A `422 approve` replaces the report with the server's new one and asks again.
  - Success says the plugin is installed disabled, with Enable.
- **The permission report** (`permission-report.tsx`):
  - the items' `text` with `detail` lines;
  - high-risk items first, with a warning style and icon;
  - on an update, `added` items marked "New";
  - limits above the default marked;
  - plugin, version and artifact hash as facts.
- **Detail** (`/admin/plugins/$name`):
  - the report;
  - nodes;
  - Settings (`SettingsForm` with secrets; `invalid[]` flagged);
  - Metrics: a 24 h failure chart per hook (reuse `dashboard/failures.ts`), and a table of hooks with calls, failures, busy, budget cuts, average and p95 bound (`p95Bound`), and max, labelled "This node, since <time>" (P48); refreshed every 30 s while visible;
  - Delete with two buttons: "Delete, keep data for 30 days" and "Delete and erase data" (H-P3).

**Tests:**
- Unit:
  - `p95_bound` (empty, all ≤1 ms, a tail over 500 ms);
  - `fingerprint_mismatch_shows_the_new_report`;
  - `high_risk_first_and_added_marked`.
- E2E:
  - `install_inspect_approve_enable`;
  - `untouched_secret_is_kept` (set a secret, save another field, `GET` still says set);
  - `delete_keep_vs_erase` (keep → in Retained data; Erase now → gone);
  - `hostile_manifest_renders_as_text`;
  - `metrics_table_shows_this_node`;
  - `plugins_screens`.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the gates; build the example plugin, the admin and the release; run the E2E and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): plugins screen with the install flow, settings and metrics`.

### Task 11: System screen

**Files:**
- Create: `admin/src/system/{system-screen.tsx,status-tab.tsx,nodes-tab.tsx,jobs-tab.tsx,audit-tab.tsx,audit-text.ts}` with tests; `admin/src/routes/_app/system.$tab.tsx` (with `system.index` redirecting to `status`); `admin/e2e/system.spec.ts`
- Modify: `nav.ts` (`g y`); the dashboard's Needs-attention links point at the new tabs; `docs/design/07-admin-api.md` §3 (System row as built)

**Behaviour** (`/admin/system/<tab>`, `manage_system`):
- **Status:**
  - version, mode, database, media backend, site URL;
  - schema (db and binary versions, pending, and the verdict with plain wording);
  - mail (configured or not, From);
  - theme (active, fallback nodes);
  - jobs by status (links to Jobs filtered);
  - nodes live and stale.
- **Nodes:** one card per node:
  - name, version, schema, started, heartbeat (relative), live or stale;
  - `meta` theme (name, origin, hash, error, fallback) and plugins (version and status per plugin, errors as text);
  - job kinds; clock skew over 1 s flagged.
- **Jobs:**
  - status and kind filters in the URL;
  - a table with kind, status, attempts / max, run at, finished, locked by, and the last error (text, expandable);
  - Retry on dead jobs. `handled: false` says "No node handles this kind yet" (H-Y1).
- **Audit:**
  - filters for actor (user picker), action (a select of known actions plus "Starts with" for a prefix) and target;
  - a table with time, actor, a readable action (`audit-text.ts` maps the known actions; unknown ones show raw), target, and details as a key-value list (text).

**Tests:**
- Unit: `audit_text_known_and_unknown`; `schema_verdict_wording`.
- E2E:
  - `status_shows_backends_and_schema`;
  - `retry_dead_job` (a dead job made through the `test-seams` route or a failing `plugin.action`, whichever exists; the implementer names the seam in the report);
  - `audit_filter_by_action_prefix`;
  - `hostile_job_error_renders_as_text`;
  - `system_screens`.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the gates; rebuild; run the E2E and expect PASS.
- [ ] **Step 5:** Commit `feat(admin): system screen (status, nodes, jobs, audit)`.

### Task 12: Editor and content-modelling follow-ups

**Files:** `admin/src/editor/**`, `admin/src/structure/types/**`, `admin/src/routes/_app/content.$type.$id.tsx`, their tests and E2E specs.

**Items.** Each one is fixed with a test that fails first, and the report lists each with its test.
- **From the ROADMAP's M2b-1 follow-ups:**
  - the restore banner's Discard Undo toast is dismissed on unmount;
  - `storeKept` does not re-PUT after `save()`'s own re-PUT;
  - "Restoring…" does not linger after the restore (check against e1cd3a5's toast changes);
  - tests for the discard-then-reload-failure fallback and the kept-copy Discard toast dismissal;
  - tests for the focus fallback after opening revisions from the phone sheet (M6) and for the Undo-without-edit-rights message (M7).
- **From the M2b-2 final review (parked):**
  - `insert-image.ts`: the reconcile effect double-inserts under StrictMode;
  - an upload finishing in the same tick as the `/new` → `/$id` hand-over lands at the body end;
  - `EditorScreen`'s unused upload queue (make `uploads` required);
  - the route's `${id}:0` key repeat after a loader refresh (check, and fix if real);
  - the editor top bar's truncated back link gets a `title`;
  - `insert-image.ts`'s header comment (R28 says the original; after Task 4 it says `srcset`);
  - `RichEditor` reads its value only on mount;
  - the `/new` publish gate gets its unit test.
- **From the CI fix e1cd3a5 re-review (parked):**
  - `onUpdate` must `recompute()`, so a body edit undone to the baseline does not stay "Unsaved changes";
  - a `NetworkError` on an in-flight autosave `PUT` must not re-set `pending` after `markClean`;
  - the slot `DELETE` routes a `401` to `onUnauthenticated`;
  - tests for edits during an in-flight `PUT` or `DELETE`.
- **Relation target types (P52, from Task 4):**
  - the field builder edits a relation's types (a multi-select of types, empty = any);
  - the relation picker searches only those types.

**Tests:** as listed above. E2E `relation_limited_to_types`.

- [ ] **Step 1:** Write the failing tests item by item.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Fix each item.
- [ ] **Step 4:** Run the gates; rebuild; run the editor, types and revisions E2E specs and expect PASS.
- [ ] **Step 5:** Commit per group (up to four commits).

### Task 13: Remaining admin follow-ups — theme cookie, palette snippet, lists and structure minors

**Files:**
- `crates/ferrite-http/src/admin.rs` (the index.html class from the cookie, P57) with a Rust test;
- `admin/src/shell/theme*.ts*`, `admin/src/palette/**`, `admin/src/content/**`, `admin/src/structure/**`, `admin/src/media/**`, `admin/src/comments/**`, `admin/e2e/support/fixtures.ts`;
- the tests; `docs/design/11-admin-ui.md` §4 (flash-free now).

**Items.** Each is fixed with a test, or answered by a ruling in the ledger; the report lists every line.
- **P57:**
  - the cookie is written on a theme change and removed for `system`;
  - `GET /admin/…` `index.html` gains `class="dark"` or `class="light"` on `<html>`;
  - Rust test `index_html_gets_theme_class_from_cookie` (and a cookie with any other value is ignored);
  - E2E `explicit_dark_against_light_os_has_no_light_frame`.
- **Palette:** a hit that matches only in the body shows the item's excerpt (first 120 characters, as text) with "Matches in text". There is no API change. Test `body_only_hit_shows_snippet`.
- **Phone status tabs:** a neighbour tab is never reduced to its bare count under the scroll fade (`content-list-trash` mobile). A snap, plus a test that the active tab scrolls into view.
- **Verify `⌘⌥R`** on a macOS browser that reports AltGraph for Option (R18). Add a unit test with a `KeyboardEvent` carrying `getModifierState("AltGraph") = true` and `altKey = true`.
- **The M2b-2 admin minors** from `m2b2-exit.md` "Open review minors" not fixed by its final fix wave (each re-checked first):
  - `LAYOUT_SPRING` duplicated;
  - `canNest` has two signatures;
  - the tree announces a move for a no-op drop;
  - styleguide truncation without `title`;
  - `upload.ts` duplicates the problem check;
  - an unparsable `2xx` upload body becomes `ApiError` 200;
  - the stale dialog's Cancel/Escape tests;
  - `moderateComment`'s status is typed `string`;
  - the library re-registers `onDone` per render;
  - "Leave and cancel" wording;
  - `library.tsx` and `queue.tsx` split where a piece has its own responsibility;
  - Undo forcing `restore: true` gets its comment;
  - the invisible tile check is clickable on fine pointers;
  - the search preview's `break-all`;
  - the schema-change confirm: removed select options, and wording per change kind;
  - the built-in type delete wording;
  - the schema-change dialog's last chip clips;
  - term dialogs morph while closing;
  - the parent combobox filter matches UUIDs;
  - focus after a term delete;
  - menu rows with an empty label;
  - the custom-link hint/error duplicate;
  - the menu Content tab with no nav types;
  - the queue's `j`/`k` in E2E;
  - `restore.undoFailed` reused for a failed backup restore;
  - the contact-sheet findings 1–3 and 6–8 of `m2b2-exit.md` §4.
- **The E2E 404 allowance:**
  - narrow the regex in `fixtures.ts` so `/content/refs` and `/content/media` 404s fail a test again;
  - fix the relation-search race (`m2b2-exit.md` §2) by giving that test its own type.

**Tests:** as listed above.

- [ ] **Step 1:** Write the failing tests item by item.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Fix each item.
- [ ] **Step 4:** Run the Rust tests for `ferrite-http`, the admin gates, then rebuild and run the full `pnpm -C admin e2e`, and expect PASS.
- [ ] **Step 5:** Commit per group.

### Task 14: Every spec on both backends, Firefox and WebKit

**Files:** `admin/playwright.config.ts`, `admin/package.json` (`e2e:all`), `admin/e2e/support/{fixtures.ts,snap.ts}`, any spec that needs a per-engine step, `.github/workflows/ci.yml`, `docs/design/10-testing.md` §3, `admin/README.md`

**Interfaces — Produces:**
- **Projects:**
  - `chromium-sqlite` (every spec);
  - `chromium-pg` (every spec, when `FERRITE_TEST_PG_URL` is set; the `.smoke` filter is removed);
  - `firefox-sqlite` (`devices["Desktop Firefox"]`);
  - `webkit-sqlite` (`devices["Desktop Safari"]`);
  - all at the desktop viewport. `snap()` and the contact sheet are no-ops outside `chromium-sqlite`, with one log line.
- `pnpm -C admin e2e` runs `chromium-sqlite` (and `chromium-pg` with PG). `pnpm -C admin e2e:all` runs all four.
- **CI:**
  - the `admin` job keeps lint, typecheck, test, `api:check`, build and the bundle check (Task 15). It uploads `admin/dist` and the release binary as artifacts.
  - A matrix job `admin-e2e` (`project: [chromium-sqlite, chromium-pg, firefox-sqlite, webkit-sqlite]`, `fail-fast: false`) downloads them, runs `playwright install --with-deps <engine>`, then `pnpm -C admin e2e --project <project>`.
  - Each job uploads its report and traces on failure, and the `admin-screens` artifact only from `chromium-sqlite`.
- Every engine skip is `test.skip(browserName === "<engine>", "<reason>")` and is listed in 10 §3 (P55).

**Tests:** all four projects pass locally (WebKit and Firefox on macOS; the report notes that CI runs them on Linux) and in CI on the branch.

- [ ] **Step 1:** Change the config and add the scripts.
- [ ] **Step 2:** Run `pnpm -C admin e2e:all` and list the failures per engine.
- [ ] **Step 3:** Fix product bugs found. Engine-only test issues get a per-engine step, or a skip with its reason (each in the report and 10 §3).
- [ ] **Step 4:** Run `e2e:all` until it passes; update the CI workflow; push the branch and watch the four jobs pass (the controller pushes).
- [ ] **Step 5:** Commit `test(admin): every E2E spec on both backends and in Firefox and WebKit`.

### Task 15: Bundle-size budget in CI

**Files:** `admin/scripts/check-bundle.mjs`, `admin/scripts/check-bundle.test.mjs` (Vitest), `admin/bundle-budget.json`, `admin/package.json` (`bundle:check`), `.github/workflows/ci.yml`, `docs/design/10-testing.md` §3, `admin/README.md`

**Interfaces — Produces:**
- `bundle-budget.json`: `{ "measured_at": "<commit>", "limits": { "entry": n, "entry_preloads": n, "css": n, "editor": n, "chart": n, "largest_lazy": n } }` in gzip -9 bytes (P56).
- `check-bundle.mjs`:
  - reads `admin/dist/index.html` for the entry and its modulepreloads;
  - finds the editor and chart chunks by name prefix;
  - gzips each with level 9;
  - prints a table (size, limit, headroom);
  - exits 1 over a limit, naming the group.

**Tests:** `check_bundle_flags_over_limit` (a fixture dist over and under the limits).

- [ ] **Step 1:** Write the test.
- [ ] **Step 2:** Run it and expect FAIL.
- [ ] **Step 3:** Implement. Measure after Tasks 1–14 and write the limits (measured + 10 %, rounded up to KiB). Add the CI step after `build`.
- [ ] **Step 4:** Run `pnpm -C admin build && pnpm -C admin bundle:check` and expect PASS.
- [ ] **Step 5:** Commit `ci(admin): bundle-size budget`.

### Task 16: Docs, the M2b-3 exit and the M2 exit

**Files:** `docs/superpowers/m2b3-exit.md`, `docs/superpowers/m2-exit.md`, `docs/ROADMAP.md`, `CLAUDE.md`, `README.md` (status), `docs/design/01-architecture.md` §1, `docs/design/07-admin-api.md` §3, `docs/design/10-testing.md`, `docs/design/11-admin-ui.md`, `docs/superpowers/rulings/m2b.md`

**Exit (M2b-3):** a fresh `init` + `seed demo` + `serve` on a release build, then:
- sign in, enrol TOTP with a real authenticator code (computed), sign out and back in with it;
- create a token and use it with `curl`;
- create an editor and sign in as them;
- change settings and see them on the site (title, front page, permalinks);
- pack and upload a theme, activate it, customize it;
- install the example plugin through the permission report, enable it, set a secret, see its metrics, delete it with and without erasing data;
- retry a dead job;
- filter the audit log;
- send a test mail through a real SMTP catcher.

Also:
- the workspace tests on both backends, with and without `admin/dist`;
- the admin gates and the four E2E projects;
- the contact sheet against 11 §8;
- the bundle budget;
- `docker build` serving `/admin/`, if a local engine runs; otherwise "CI only".

**M2 exit** (`m2-exit.md`): the M2 checklist from ROADMAP §M2 and 07 (every endpoint and screen in its as-built state), the open follow-ups carried to M3, M4 or Later, and what was never verified (Safari unless done by hand, real S3 clusters if not run).

Every claim in both documents is checked against a command run, or marked not verified (CLAUDE.md), with a table of what could be verified.

- [ ] **Step 1:** Run the exit flows; record the outputs.
- [ ] **Step 2:** Write both exit documents and the docs updates; tick the ROADMAP items.
- [ ] **Step 3:** Run every gate once more.
- [ ] **Step 4:** Commit `docs: M2b-3 exit and M2 exit`.
