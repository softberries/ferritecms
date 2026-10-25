# M2b-1 — Admin Foundation & Writing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The binary serves a React admin at `/admin/` with a deliberate design system (tokens, Inter + Geist Mono, light/dark, two densities, a motion language, a living styleguide). In it, a user signs in (password, TOTP, forced password change, reset by e-mail), moves around with a command palette and keyboard shortcuts, reads a dashboard with charts, lists content with status tabs and bulk actions, and writes in a TipTap editor that autosaves, resolves `412` conflicts and shows revisions with a diff. Every screen is screenshot in light and dark at desktop and mobile width, checked by axe, and run under a CSP with no inline scripts and no `eval`.

**Architecture:** The SPA lives in `admin/` (pnpm, Vite, React, TypeScript strict, shadcn/ui on Radix with Tailwind v4, TanStack Router/Query/Table, Motion, Recharts through shadcn `chart`, TipTap 3). It builds to `admin/dist/`. `crates/ferrite-http` embeds that folder with rust-embed (`allow_missing`, so `cargo build` without it compiles in a placeholder page) and serves `/admin/*`: hashed assets are immutable, any other path falls back to `index.html`, and the admin CSP and `Referrer-Policy: no-referrer` are set. The SPA talks only to `/api/v1`, through a client generated from `crates/ferrite-http/openapi.json` (openapi-typescript + openapi-fetch; CI checks for drift). Three API gaps found by mapping screens to endpoints are closed in Rust first: author references and allowed actions on content DTOs, per-status counts, and a per-user autosave slot (migration `0007_autosaves`). Playwright runs the real `ferrite serve` (release build, SQLite; a Postgres smoke project when `FERRITE_TEST_PG_URL` is set), started by its global setup together with a small SMTP sink.

**Tech Stack:** see "Frontend pins" under Global Constraints; Rust side axum 0.8.9, utoipa 6.0.0, rust-embed 8.13 (workspace pin, `debug-embed`), sqlx 0.9.

**Spec:** `docs/design/07-admin-api.md` §3 (Admin SPA, D6/D12) and §1–§2 (conventions, auth, CSRF, sessions, endpoint list); `docs/design/00-overview.md` (principles, D6, D12); `docs/design/10-testing.md` §1, §3; ROADMAP "M2b" (the "Design system first" product requirement, the scaffold item, the password-reset page item, the Playwright/Vitest/CI item; the screens this sub-plan covers: dashboard, content list, editor with revisions, autosave and the 412 conflict UI); user requirements relayed with this plan: React for future freedom, shadcn/ui, a very good chart library, transition effects, good fonts, "state of the art admin user interface which will be much better than WordPress". Earlier rulings (`docs/superpowers/rulings/*.md`) stay binding. At the start the controller copies P1–P26 below into a new `docs/superpowers/rulings/m2b.md` and records execution rulings there (as "M2b P<n>/R<n>"; M2b-2 and M2b-3 append to the same file).

**Sub-plan split (P1).** M2b is three plans, as M2a was:

| Sub-plan | Scope | Size |
|---|---|---|
| **M2b-1 Foundation & writing** (this plan) | admin serving and embedding, CSP, placeholder; scaffold and toolchain; generated API client with a drift check; E2E harness and CI job; design language, tokens, fonts, component kit, styleguide; auth screens incl. the password-reset page; app shell, shortcuts, command palette; dashboard; content list; TipTap editor, publishing panel, autosave, conflict UI, revisions; API gaps: author refs, allowed actions, status counts, autosave slot | 18 tasks |
| **M2b-2 Media & content modelling** | media library (grid/list, drag-and-drop upload with progress and `Idempotency-Key`, alt text, variants), media in the editor (featured image, inline `ferrite:media/<id>` images, picker), custom fields generated from the type schema, SEO fields, content types and field builder, taxonomies and term trees, menus (drag-and-drop tree, `@dnd-kit`), comment moderation queue, public comment pagination (ROADMAP item: `comment-page-N` and the real total), redirects (table, CSV import/export, hits) | about 14 tasks |
| **M2b-3 Administration, extensions & M2 exit** | own account (password, TOTP enrolment with QR code, own tokens), users, roles and tokens, settings screens, themes (upload, activate, settings form from the schema), plugins (install flow with the permission report and fingerprint approval, enable/disable, delete/purge, settings with write-only secrets, metrics charts), system (status, schema, backends, nodes with `nodes.meta`, jobs with dead-job retry, audit log), Firefox and WebKit E2E, both backends for every spec, a bundle-size budget, docs and the M2 exit | about 14 tasks |

Import (WXR upload, progress) ships with M3's import API; backups with the backup API (M4) (P25).

## Global Constraints

- **CLAUDE.md is binding**: principle 1 (one binary: the SPA and its fonts are embedded, so nothing loads from a CDN at runtime and no external service is needed), principle 4 (`wit/` untouched), Rust conventions (`thiserror` in libraries, `anyhow` only in `crates/ferrite`, no `unwrap`/`expect` outside tests, no `#[allow(dead_code)]`, `cargo fmt`, `cargo clippy --all-targets -- -D warnings`), storage changes ship contract tests on both backends, API changes regenerate `crates/ferrite-http/openapi.json` (`FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`) in the same commit, and capabilities are checked in core, never in handlers or in the SPA (the SPA hides what the server says is not allowed; it never decides).
- **Conventions carried from M2a** (use them, do not re-invent): `ApiPath<T>`, `check_limit`, `invalid_cursor()`, `UNIQUE_FIELDS`, `CoreError::invalid_field`; every new unsafe route passes `unsafe_operations_need_auth_and_csrf` (`crates/ferrite-http/tests/api/sweep.rs`) with **no** addition to its `UNAUTHENTICATED` list; test seams go behind the `test-seams` feature (10 §2), never `#[doc(hidden)] pub`. No new problem slug.
- **Rust gates:** `CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace` passes **with and without** `admin/dist` present; `cargo build -p ferrite` works on a fresh clone without Node.
- **Admin gates** (each task that touches `admin/` runs them before its commit): `pnpm -C admin lint`, `pnpm -C admin typecheck`, `pnpm -C admin test`, `pnpm -C admin api:check`, `pnpm -C admin build`, and `pnpm -C admin e2e` (Task 6 onwards; needs `cargo build --release -p ferrite` after `pnpm build`).
- **Design review gate (every UI task, Task 8 onwards):** the task's E2E spec calls `snap(page, name)` on each screen and state it adds. That fixture writes four PNGs (light/dark × desktop 1440×900 / mobile 390×844) to `admin/test-results/screens/` and regenerates `admin/test-results/screens/index.html`, a contact sheet. The reviewer opens the contact sheet and checks it against the checklist in `docs/design/11-admin-ui.md` §8 (Task 7). A task whose screens fail that checklist is rejected like a failing test. Screenshots are review artifacts, not pixel-diff baselines (font rasterisation differs between macOS and the Linux CI; P11). CI uploads the folder as the `admin-screens` artifact.
- **Accessibility gate:** every E2E screen visit runs `expectAccessible(page)` (axe, tags `wcag2a`, `wcag2aa`, `wcag21a`, `wcag21aa`, `wcag22aa`; zero violations, no rule disabled without a ruling) in light **and** dark. Token contrast is unit-tested (Task 7). Every interactive element has a visible `:focus-visible` ring (`--ring`, 2 px, 2 px offset). `prefers-reduced-motion: reduce` removes transform and layout motion.
- **CSP gate:** the admin runs under `script-src 'self'` with no `'unsafe-eval'` (exact header in Task 1). The E2E fixture fails any test on a `securitypolicyviolation` event, a page error or a `console.error`. Consequences: Zod runs `z.config({ jitless: true })` before any schema is built; TipTap runs with `injectCSS: false` (its CSS is imported); Vite builds with `build.assetsInlineLimit: 0` and `build.sourcemap: false`; nothing uses `dangerouslySetInnerHTML` except the shadcn `ChartStyle`, which `style-src 'unsafe-inline'` covers (P8).
- **Strings:** every user-visible string goes through `t()` (i18next, namespace per area, `en` only in M2b). A Vitest setup hook fails a test on a missing key.
- **E2E data hygiene:** tests create their own content with unique titles (`uid()` helper) and never depend on another test's writes. Logins go through storage states made once in global setup. Login rate limits are 10/min per IP and 5/min per login (07 §2), so the auth spec uses its own users and at most 6 password attempts per run.
- **Frontend pins** (exact versions in `admin/package.json`; `pnpm-lock.yaml` committed; `"packageManager": "pnpm@10.28.2"`, `"engines": { "node": ">=24.18" }`). Source: the npm registry (`npm view <pkg> time`, 2026-10-25). Each pin is the newest stable release published on or before 2026-10-11 (at least two weeks old). Context7 = APIs checked against that library's current docs on 2026-10-25.

  | Package | Pin | Published | Context7 | Note |
  |---|---|---|---|---|
  | react, react-dom | 19.3.0 | 2026-09-09 | — (npm only) | 19.3.1 is 2026-10-14, too new |
  | @types/react, @types/react-dom | 19.3.0 | 2026-09-09 | — | |
  | typescript | 5.9.3 | 2025-09-30 | — | 6.x/7.x excluded: typescript-eslint 8.71.1 peers `>=4.8.4 <6.1.0`, openapi-typescript 7.13.0 peers `^5.x` (P5) |
  | vite | 8.3.4 | 2026-10-08 | yes (`base`, `server.proxy`, `assetsInlineLimit`, `modulePreload`) | |
  | @vitejs/plugin-react | 6.1.2 | 2026-10-05 | — | peers `vite ^8` |
  | tailwindcss, @tailwindcss/vite | 4.3.3 | 2026-07-16 | yes, through shadcn's Vite guide | |
  | shadcn (CLI + `shadcn/tailwind.css`) | 4.21.4 | 2026-10-07 | yes (docs of 4.21.0: `init -b radix -t vite`, components.json, theming, chart, data-table) | |
  | radix-ui | 1.7.0 | 2026-10-05 | via shadcn | |
  | tw-animate-css | 1.4.0 | 2025-09-24 | via shadcn | |
  | class-variance-authority 0.7.1, clsx 2.1.1, tailwind-merge 3.7.0 | — | 2024-11-26 / 2024-04-23 / 2026-09-12 | via shadcn | |
  | lucide-react | 1.55.0 | 2026-10-10 | — | ISC |
  | sonner 2.0.8, cmdk 1.1.1 | — | 2026-08-09 / 2025-03-14 | via shadcn registry (`sonner`, `command`) | no `next-themes` (P9) |
  | @tanstack/react-router | 1.170.42 | 2026-10-10 | yes (Vite plugin, `autoCodeSplitting`, `basepath`, `beforeLoad`) | |
  | @tanstack/router-plugin | 1.168.43 | 2026-10-10 | yes | peers `@tanstack/react-router ^1.170.42` |
  | @tanstack/react-query | 5.104.2 | 2026-10-11 | yes (v5 mutations, optimistic updates) | |
  | @tanstack/react-table | 9.2.8 | 2026-10-08 | yes, through shadcn 4.21 data-table (v9 `useTable`, `tableFeatures`) | |
  | motion | 14.1.0 | 2026-10-09 | yes (`motion/react`, `MotionConfig reducedMotion`, `LazyMotion`, `nonce`) | settles 00-overview's "not verified" row |
  | recharts | 3.10.1 | 2026-07-25 | yes, through shadcn chart | the shadcn registry's chart lists `recharts@3.8.0` (same major; P7) |
  | @tiptap/core, pm, react, starter-kit, extension-table, extension-image, extensions, suggestion | 3.31.4 | 2026-09-30 | yes (StarterKit v3 contents, `configure`, `immediatelyRender`, `shouldRerenderOnTransaction`) | 3.32.0 is 2026-10-13 |
  | react-hook-form 7.90.0, @hookform/resolvers 5.9.1, zod 4.6.6 | — | 2026-10-11 / 2026-08-17 / 2026-10-11 | zod yes (`z.config({ jitless: true })`) | resolvers 6.0.0 is 2026-10-19 |
  | i18next 26.4.3, react-i18next 17.0.16 | — | 2026-10-10 / 2026-10-06 | — | |
  | diff | 9.0.0 | 2026-04-13 | — | BSD-3-Clause |
  | date-fns 4.4.0, react-day-picker 10.0.2 | — | 2026-05-29 / 2026-09-30 | via shadcn calendar | |
  | @fontsource-variable/inter | 5.3.0 | 2026-07-19 | — (package inspected: `opsz.css`, `opsz-italic.css`, wght 100–900) | OFL-1.1 |
  | @fontsource-variable/geist-mono | 5.3.0 | 2026-07-19 | — (package inspected: `wght.css`) | OFL-1.1 |
  | openapi-typescript 7.13.0, openapi-fetch 0.17.0 | — | 2026-02-11 | yes (CLI, `createClient`, `Middleware`) | no `--check` flag relied on; drift is a regenerate-and-diff (Task 5) |
  | vitest | 5.0.3 | 2026-09-30 | yes (`environment`, `setupFiles`, `projects`) | |
  | jsdom 30.1.2; @testing-library/react 16.3.3, user-event 14.6.7, jest-dom 7.0.1 | — | 2026-10-04 / 2026-08-27 / 2026-09-02 / 2026-08-09 | — | |
  | @playwright/test | 1.64.0 | 2026-10-07 | yes (global setup, fixtures, `emulateMedia`) | matches the local Playwright 1.64 |
  | @axe-core/playwright | 4.13.0 | 2026-08-11 | yes (`AxeBuilder.withTags`) | 4.14.0 is 2026-10-15 |
  | eslint 10.12.0, @eslint/js 10.0.1, typescript-eslint 8.71.1, eslint-plugin-react-hooks 7.1.1, eslint-plugin-react-refresh 0.5.7, globals 17.13.0, prettier 3.9.10, @types/node 24.19.2 | — | 2026-10-02 … 2026-10-10 | — | peers checked: all accept eslint 10 |

  Not verified: the exact rendering of Inter's optical-size axis in Chromium on Linux (checked in Task 7 on the contact sheet); whether `recharts@3.10.1` changes anything the shadcn chart wrapper relies on (Task 12 builds every dashboard chart against it; on a break, pin `3.8.1` and record a ruling).

- **Exact values:** the admin base path is `/admin/`; the dev server is Vite on `:5173`, proxying `/api` and `/_preview` to `http://localhost:8080`; autosave every 10 s while dirty (07 §3), at most one request in flight; page transitions ≤ 200 ms (07 §3); list pages of 50 (`limit=50`); the dashboard's activity chart covers 30 days; screenshot viewports are 1440×900 and 390×844; the command palette's content search is debounced 150 ms, with up to 8 results per type.

## Review Focus

1. **The password-reset token never leaves the fragment.** The page reads `#token=…` once, calls `history.replaceState` before it renders or sends anything, keeps the token only in memory, and never puts it in a URL, a `Referer`, a log line or an error message. The response carries `Referrer-Policy: no-referrer`; a reload shows "link already used or missing", not the token. *Tests: Task 9 `takeResetToken` unit tests and E2E `reset_token_is_stripped_and_never_sent` (records every request URL and `Referer` the page sends and asserts none contains the token); Task 1 `admin_responses_send_no_referrer`.*
2. **The CSP holds on every screen.** No inline script, no `eval`, no blocked font or image, on every route in both themes. *Tests: Task 1 `admin_index_has_the_csp` (exact header); Task 4 `check-dist` (the built `index.html` has no inline `<script>` and no `data:` URL); Task 6 fixture (any `securitypolicyviolation`, `pageerror` or `console.error` fails the test), which every later spec inherits.*
3. **Writing never loses or corrupts content.** A body with a node the editor cannot represent opens read-only instead of being rewritten. Every node and mark the core renderer knows survives load → save unchanged. A `412` keeps the user's text and offers the server's version side by side. A reload or a dropped session restores the autosave. Leaving with unsaved changes asks first. *Tests: Task 14 `roundtrip_every_renderer_node`, `unknown_node_opens_read_only`; Task 16 `conflict_keeps_my_text`, `autosave_restores_after_reload`, `session_expiry_mid_edit_keeps_the_draft`, `leave_guard_blocks_navigation`; Task 3 `patch_clears_own_autosave_only`.*
4. **Roles see exactly what the server allows.** A contributor sees "Submit for review" and no "Publish". An author's counts and lists never include another author's drafts. A `must_reset` session reaches only the change-password screen. Every action button comes from the item's `allowed`, never from a client-side role check. *Tests: Task 2 `allowed_actions_matrix` (four roles × six statuses × own/other) and `counts_respect_can_read`; Task 13 E2E `author_counts_match_list`; Task 15 E2E `contributor_can_submit_not_publish`; Task 9 E2E `must_reset_session_is_confined`.*
5. **Navigation cannot be turned into an open redirect, and an expired session never strands the user.** `?next=` is accepted only for a same-origin path under `/admin/` (not `//x`, `/\x`, `/admin/../api`, `javascript:`, or an encoded variant). A `401` anywhere sends the user to login and back to the same screen. *Tests: Task 9 `safeNext` unit table and E2E `next_is_not_an_open_redirect`; Task 16 `session_expiry_mid_edit_keeps_the_draft`.*

---

### Task 1: Serve the embedded admin at `/admin/`

**Files:**
- Create: `crates/ferrite-http/build.rs`, `crates/ferrite-http/src/admin.rs`, `crates/ferrite-http/src/admin/placeholder.html`, `crates/ferrite-http/tests/app/admin.rs`
- Modify: `crates/ferrite-http/{Cargo.toml,src/lib.rs,src/app.rs,src/app/state.rs}`, `crates/ferrite-http/tests/app/{main.rs,harness.rs}`, `crates/ferrite/src/serve.rs` (pass the media origin), `.gitignore` (`admin/dist/`, `admin/node_modules/`, `admin/test-results/`, `admin/e2e/.auth/`), `docs/design/07-admin-api.md` §3 "Embedding & serving" (as built), `docs/design/10-testing.md` §2 (the new seam)

**Interfaces — Produces:**
- `ferrite_http::admin::{AdminAssets, AdminFile, EmbeddedAdmin, admin_routes, admin_csp, ADMIN_BASE = "/admin/"}`.
  - `pub trait AdminAssets: Send + Sync { fn get(&self, path: &str) -> Option<AdminFile>; }`; `pub struct AdminFile { pub bytes: std::borrow::Cow<'static, [u8]>, pub sha256: [u8; 32] }`.
  - `EmbeddedAdmin`: `#[derive(rust_embed::Embed)] #[folder = "../../admin/dist/"] #[allow_missing = true]` (attribute confirmed in the rust-embed docs); an empty embed (no `index.html`) makes every page path answer the placeholder.
  - `build.rs`: only `println!("cargo:rerun-if-changed=../../admin/dist")`. It never runs pnpm (07 §3).
  - `admin_csp(media_origin: Option<&str>) -> String` = `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:{ media}; font-src 'self'; connect-src 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; worker-src 'self'`. `{ media}` is ` <origin>` of `[media] public_base_url` in Direct S3 mode, else empty.
  - `AppState` gains `admin: Arc<dyn AdminAssets>` (default `EmbeddedAdmin`) and `admin_csp: String`. A test seam `AppState::with_admin_assets(Arc<dyn AdminAssets>)` sits behind `test-seams`.
- Routing (`build_site` and `build_app`, inside the shared layers, outside the page cache and redirects): `GET|HEAD /admin` → `308` to `/admin/`. Under `/admin/{*path}`: a path under `assets/` is that file or a plain `404`. Any other path is the file if it exists with an extension (`favicon.svg`, `robots.txt`), else `index.html` (the SPA fallback; the placeholder when none is embedded). Methods other than GET and HEAD → `405` with `Allow: GET, HEAD`.
- Headers: `assets/*` → `Cache-Control: public, max-age=31536000, immutable`; `index.html`, the placeholder and other root files → `Cache-Control: no-cache`. Each gets a strong `ETag` of the first 16 hex digits of its SHA-256, and `If-None-Match` → `304`. `Content-Type` comes from a fixed extension table (`html`, `js` = `text/javascript; charset=utf-8`, `css`, `woff2` = `font/woff2`, `svg`, `png`, `ico`, `json`, `txt`, `webmanifest`; anything else `application/octet-stream`). Every admin response gets `Content-Security-Policy` (the string above), `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, `Cross-Origin-Opener-Policy: same-origin`, `X-Content-Type-Options: nosniff`, `Permissions-Policy: camera=(), microphone=(), geolocation=()` and `X-Robots-Tag: noindex`. Compression goes through the existing layer.
- The placeholder is static HTML without script: the title "FerriteCMS admin not built" and one paragraph naming `pnpm -C admin install && pnpm -C admin build`, then rebuilding the binary.

**Tests** (`tests/app/admin.rs`, both profiles, through `with_admin_assets` with an in-memory fixture set): `admin_redirects_bare_path`; `spa_fallback_serves_index` (`/admin/`, `/admin/content/post`, `/admin/reset-password` → index bytes, `no-cache`); `hashed_assets_are_immutable` (`/admin/assets/index-abc.js` → the bytes, `immutable`, JS type, ETag, then `304`); `missing_asset_is_404_not_index`; `admin_index_has_the_csp` (Review Focus 2: exact string, with and without a Direct-S3 media origin); `admin_responses_send_no_referrer` (Review Focus 1: index, assets, placeholder); `admin_rejects_unsafe_methods`; `admin_path_traversal_is_404` (`/admin/assets/../../secret.key`, `%2e%2e`, backslash); `placeholder_without_dist` (an empty asset set → 200 placeholder with the CSP, no `<script`); `admin_is_not_cached_or_redirected` (a redirect row for `/admin/x` is refused at write already, `slug::RESERVED`; page-cache stats unchanged after admin hits). Real binary: in `crates/ferrite/tests/serve/site.rs`, `admin_placeholder_on_real_binary` runs when the embedded set is empty, else `admin_index_on_real_binary`.

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-http --test app admin` and expect compile failures.
- [ ] **Step 3:** Implement `build.rs`, `admin.rs`, the routes, the headers and the seam; update 07 §3 and 10 §2.
- [ ] **Step 4:** Run `cargo test -p ferrite-http --test app` and `cargo test -p ferrite --test serve` on both profiles and expect PASS; `cargo build -p ferrite` without `admin/dist` succeeds.
- [ ] **Step 5:** Commit `feat(http): serve the embedded admin under /admin with its CSP and a placeholder`.

### Task 2: API gaps (1) — author references, allowed actions, status counts

**Files:**
- Create: `crates/ferrite-core/src/content/allowed.rs`, `crates/ferrite-core/tests/content_allowed.rs`, `crates/ferrite-http/tests/api/content_extras.rs`
- Modify: `crates/ferrite-core/src/{content.rs,content/admin.rs,content/lifecycle.rs}`, `crates/ferrite-http/src/api/{content.rs,dto.rs or dto/content.rs,openapi.rs}`, `crates/ferrite-http/tests/api/main.rs`, `crates/ferrite-http/openapi.json`, `docs/design/07-admin-api.md` §1 (Content row and "Reading content")

**Interfaces — Produces:**
- `ferrite_core::content::{AllowedActions, StatusCounts, AuthorRef}`:
  - `AllowedActions { edit, publish, unpublish, schedule, submit, trash, restore, delete: bool }`, from `allowed_actions(actor: &Actor, item: &ContentItem) -> AllowedActions`. It is the conjunction of the permission check each lifecycle method makes and the transition being legal from `item.status`. It lives next to the lifecycle and reuses its predicates; the lifecycle methods call the same predicates, so the two cannot drift.
  - `AuthorRef { id: UserId, login: String, display_name: String }`; `ContentService::authors(&self, actor, ids: &[UserId]) -> Result<HashMap<UserId, AuthorRef>, CoreError>` (`require_read(Content)`; one `users().get_many`, in chunks of `MAX_ID_FILTER`). It only covers the authors of items the actor already read; logins and display names are public on author archives.
  - `StatusCounts { draft, pending, scheduled, published, private, trash: u64 }`; `ContentService::counts(&self, actor, kind: &str) -> Result<StatusCounts, CoreError>`: `require_read(Content)`, a known `kind` (else `NotFound`), then one `content().count(&ContentQuery { kind, statuses: [s], readable_by, .. })` per status, with `readable_by` set when the role lacks `edit_others`, exactly as `list` does.
- DTOs: `ContentDto` and `ContentSummaryDto` gain `author: AuthorRefDto { id, login, display_name }` and `allowed: AllowedActionsDto`; `RevisionSummaryDto` and `RevisionDto` gain `author: AuthorRefDto`. All of these are added members; nothing is removed or renamed.
- `GET /content/{type}/counts` → `ContentCountsDto { draft, pending, scheduled, published, private, trash }` (tag `content`). The static `counts` segment is expected to take priority over `{id}` in axum 0.8's router, and the routing test pins it. If the router refuses the overlap at startup, the route becomes `GET /content-counts/{type}` and a ruling records it.

**Tests:** core (both backends) `allowed_actions_matrix` (Review Focus 4: admin, editor, author, contributor × draft, pending, scheduled, published, private, trash × own/other → the table in 07 §1 "Roles & capabilities" and the R8 contributor rule; each `true` cell is then exercised through the real lifecycle method and succeeds, each `false` cell fails with `Forbidden` or `Invalid`); `counts_respect_can_read` (an author with 2 own drafts next to 3 drafts by others → `draft = 2`; an editor → 5; trash counted separately). API (both profiles): `content_dto_has_author_and_allowed`; `list_items_have_author_and_allowed`; `counts_endpoint_shape_and_routing` (`/content/post/counts` → counts, `/content/post/<uuid>` still the item, `/content/nope/counts` → 404); `revisions_carry_author`. The sweep passes; the snapshot is regenerated.

- [ ] **Step 1:** Write the tests above.
- [ ] **Step 2:** Run `cargo test -p ferrite-core --test content_allowed` and `cargo test -p ferrite-http --test api content_extras` and expect FAIL.
- [ ] **Step 3:** Implement; regenerate the snapshot; update 07 §1.
- [ ] **Step 4:** Run `cargo test -p ferrite-core` and `cargo test -p ferrite-http --test api` on both backends and expect PASS.
- [ ] **Step 5:** Commit `feat(api): author references, allowed actions and status counts for content`.

### Task 3: API gaps (2) — the per-user autosave slot

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0007_autosaves.{up,down}.sql`, both backends' `src/repos/autosave.rs`, `crates/ferrite-store-tests/src/autosave.rs`, `crates/ferrite-core/tests/autosave.rs`, `crates/ferrite-http/tests/api/autosave.rs`
- Modify: `crates/ferrite-domain/src/content.rs` (`Autosave`), `crates/ferrite-store/src/{repos.rs,lib.rs}`, both backends' `src/repos/mod.rs`, every `impl Repos` wrapper (test stores included), `crates/ferrite-store-tests/{src/lib.rs,src/migrations.rs,tests/contract.rs}`, `crates/ferrite-core/src/{content.rs,content/admin.rs}`, `crates/ferrite-http/src/api/{content.rs,dto…,openapi.rs}`, `crates/ferrite-http/tests/api/main.rs`, `crates/ferrite-http/openapi.json`, `docs/design/02-storage.md` §4 (table list) and §7, `docs/design/07-admin-api.md` §1 (Content row; "Autosave" paragraph) and §3 (Editor row: autosave goes to the slot, not a revision)

**Interfaces — Produces:**
- Migration `0007_autosaves` (same name on both backends, `-- ferrite:phase expand`, `-- ferrite:lossy-down`: the down drops unsaved autosaves): `autosaves (item_id → content_items ON DELETE CASCADE, user_id → users ON DELETE CASCADE, base_version bigint NOT NULL, title text NOT NULL, excerpt text NULL, body json NOT NULL, fields json NOT NULL, saved_at timestamp NOT NULL; PRIMARY KEY (item_id, user_id) "autosaves_pkey"; index autosaves_user_idx (user_id))`. Postgres uses `jsonb`; SQLite uses `TEXT CHECK (json_valid(…))`, as in `0006`.
- Domain `Autosave { item_id: ContentId, user_id: UserId, base_version: i64, title: String, excerpt: Option<String>, body: Value, fields: Value, saved_at: Timestamp }`.
- `AutosaveRepo` (`Repos::autosaves()`): `put(&Autosave)` (upsert by key), `get(ContentId, UserId) -> Option<Autosave>`, `delete(ContentId, UserId) -> bool`.
- Core: `ContentService::{get_autosave(&self, actor, id) -> Option<Autosave>, put_autosave(&self, actor, id, draft: AutosaveDraft) -> Autosave, delete_autosave(&self, actor, id) -> bool}`, with `AutosaveDraft { base_version: i64, title: String, excerpt: Option<String>, body: Value, fields: Value }`. Each checks `can_edit` on the current item, and `put` refuses a trashed item (`Invalid`, "restore first"). `put` validates text as a content write does (NUL, control characters) but not the field schema: a draft may be incomplete. It writes no revision, event, action job, search index entry or invalidation. `update_in` deletes the **actor's own** autosave for the item in the same `Tx`; other users' autosaves stay.
- Endpoints (tag `content`; `Authed`; no `If-Match`, per user, last write wins): `GET /content/{type}/{id}/autosave` → `AutosaveDto { base_version, title, excerpt, body, fields, saved_at }` or `404`; `PUT …/autosave` (body `AutosaveInput { base_version, title, excerpt?, body, fields }`) → `200 AutosaveDto`; `DELETE …/autosave` → `204` (also when none existed).
- No retention job: one row per (item, user) at most (P15).

**Tests:** contract (both backends) `migration_0007_roundtrip` (lossy listed); `autosave_upsert_get_delete`; `autosave_cascades_with_item_and_user`. Core: `autosave_needs_edit_rights` (an author on another author's draft → `Forbidden`; a contributor on its own published post → `Forbidden`, per R8); `autosave_writes_nothing_else` (revision count, events, jobs and search index unchanged); `patch_clears_own_autosave_only` (Review Focus 3: two editors autosave, one PATCHes, and only that editor's slot is gone); `trashed_item_refuses_autosave`. API: `autosave_flow` (PUT → GET → PATCH item → GET 404); `autosave_requires_csrf` (the sweep covers it; also an explicit cookie-without-token `403 csrf`).

- [ ] **Step 1:** Write the tests above and register them.
- [ ] **Step 2:** Run `cargo test -p ferrite-store-tests --test contract autosave migration_0007` and `cargo test -p ferrite-core --test autosave` and expect compile failures.
- [ ] **Step 3:** Implement the migrations, repos, service methods and endpoints; regenerate the snapshot; update 02 and 07.
- [ ] **Step 4:** Re-run with `FERRITE_TEST_PG_URL` set and expect PASS; `cargo test -p ferrite-http --test api` passes (sweep included).
- [ ] **Step 5:** Commit `feat: per-user autosave slot for content (migration 0007)`.

### Task 4: Admin scaffold and toolchain

**Files:**
- Create: `admin/{package.json,pnpm-lock.yaml,tsconfig.json,tsconfig.node.json,vite.config.ts,vitest.config.ts,eslint.config.js,.prettierrc.json,components.json,index.html,README.md}`, `admin/public/favicon.svg`, `admin/src/{main.tsx,app.tsx,router.tsx,routeTree.gen.ts}`, `admin/src/routes/{__root.tsx,index.tsx}`, `admin/src/lib/{utils.ts,zod.ts}`, `admin/src/i18n/{index.ts,en/common.json}`, `admin/src/test/setup.ts`, `admin/src/app.test.tsx`, `admin/scripts/check-dist.mjs`
- Modify: `docs/design/00-overview.md` ("Verified facts": Motion row → verified 2026-10-25 via npm and Context7; add the frontend toolchain row)

**Interfaces — Produces:**
- Scripts: `dev` (vite), `build` (`tsc -b && vite build && node scripts/check-dist.mjs`), `typecheck` (`tsc -b --noEmit`), `lint` (`eslint . && prettier --check .`), `format`, `test` (`vitest run`), `e2e` (Task 6), `api:gen` and `api:check` (Task 5).
- `shadcn init -b radix -t vite` (P2) with `components.json` style `radix-nova`, base colour `neutral` (replaced in Task 7), aliases `@/components`, `@/components/ui`, `@/lib`, `@/hooks`, icons `lucide`.
- `vite.config.ts`: `base: "/admin/"`; plugins `tanstackRouter({ target: "react", autoCodeSplitting: true })`, `react()`, `tailwindcss()`; alias `@` → `src`; `build: { outDir: "dist", assetsDir: "assets", assetsInlineLimit: 0, sourcemap: false, emptyOutDir: true }`; `server: { port: 5173, strictPort: true, proxy: { "/api": P, "/_preview": P } }`, where `P = { target: process.env.FERRITE_DEV_URL ?? "http://localhost:8080", changeOrigin: true, configure: proxy => proxy.on("proxyReq", r => r.setHeader("origin", target)) }`. Rewriting `Origin` keeps the server's CSRF origin check passing in development (07 §2).
- `src/lib/zod.ts`: `z.config({ jitless: true })`, imported first by `main.tsx`; every schema imports `z` from here (an ESLint `no-restricted-imports` rule forbids `zod` elsewhere).
- `router.tsx`: `createRouter({ routeTree, basepath: "/admin", defaultPreload: "intent", context: { queryClient } })`. `routeTree.gen.ts` is committed and ignored by lint (P10).
- `i18n/index.ts`: `initI18n(): Promise<void>` (i18next with `react-i18next`, `en`, namespaces per area, `returnNull: false`); `src/test/setup.ts` registers jest-dom, initialises i18n and turns `missingKey` into a test failure.
- `check-dist.mjs`: fails when `dist/index.html` has an inline `<script>` (any `<script>` without `src`), when any file in `dist` contains `data:font` or `data:image` outside an SVG, when a `.map` file exists, or when a `dist/assets` file name has no content hash.
- tsconfig: `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `verbatimModuleSyntax`, target `ES2023`, `moduleResolution: "bundler"`, `jsx: "react-jsx"`.

**Tests:** `app.test.tsx` `renders_root_with_translated_title` (the root route renders the `common:app.name` string); `check-dist` runs as part of `build`; a test `missing_key_fails` in `setup.ts`'s own suite (calling `t("nope")` throws).

- [ ] **Step 1:** Create `package.json` with the exact pins, run `pnpm -C admin install`, then `pnpm dlx shadcn@4.21.4 init -b radix -t vite` in `admin/` and keep its output (CSS and components.json) for Task 7 to restyle.
- [ ] **Step 2:** Write the tests; run `pnpm -C admin test` and expect FAIL.
- [ ] **Step 3:** Write the config, root route, i18n, zod config and `check-dist.mjs`.
- [ ] **Step 4:** Run the admin gates (`lint`, `typecheck`, `test`, `build`) and expect PASS; `cargo build -p ferrite` now embeds `dist`, and `curl` on `serve` shows the root route at `/admin/` with the Task 1 headers.
- [ ] **Step 5:** Commit `feat(admin): Vite + React + TS scaffold with shadcn (Radix), TanStack Router, i18n`.

### Task 5: Generated API client, request layer and query keys

**Files:**
- Create: `admin/src/api/{schema.d.ts,client.ts,problem.ts,csrf.ts,keys.ts,content.ts,auth.ts}`, `admin/src/api/client.test.ts`, `admin/scripts/api-check.mjs`
- Modify: `admin/package.json` (scripts), `admin/src/main.tsx` (QueryClient), `docs/design/07-admin-api.md` §1 last bullet (as built)

**Interfaces — Produces:**
- `api:gen` = `openapi-typescript ../crates/ferrite-http/openapi.json -o src/api/schema.d.ts --root-types`; `api:check` = `scripts/api-check.mjs`, which generates to a temp file and fails with a diff when it differs from `src/api/schema.d.ts`.
- `client.ts`: `export const api = createClient<paths>({ baseUrl: "/api/v1", credentials: "same-origin" })` with one middleware. It sets `X-CSRF-Token` from `csrf.ts` on non-GET/HEAD requests and `Accept: application/json`. On a response with `application/problem+json`, the call throws `ApiError`.
- `problem.ts`: `export class ApiError extends Error { status: number; problem: Problem; slug: string /* last segment of type */; fieldErrors(): Record<string, string>; retryAfter: number | null }`; `isProblem(e: unknown, slug: string): e is ApiError`; `Problem` = the generated `ProblemDocument`.
- `csrf.ts`: `setCsrfToken(t: string | null)`, `getCsrfToken()`. The token is held in memory only; it comes from login, `GET /auth/me` and a password change.
- Global handling: a `QueryCache`/`MutationCache` `onError` sends `unauthenticated` to `/admin/login?next=<current path>`, `password-reset-required` to `/admin/change-password`, and `rate-limited`/`busy` to a toast with the `Retry-After` seconds. Default `retry`: never for 4xx, twice for network errors and 503.
- `keys.ts`: `qk = { meta: ["meta"], me: ["me"], types: ["types"], settings: ["settings"], content: { list: (type, filters) => [...], item: (type, id), counts: (type), revisions: (type, id), revision: (type, id, rid), autosave: (type, id) }, taxonomies: ["taxonomies"], terms: (tax) => [...], system: { status, metrics, jobs: (filters) }, plugins: ["plugins"], pluginMetrics: (name) => [...] }`.
- `content.ts`: `ifMatch(version: number) => ({ "If-Match": \`"${version}"\` })` and typed wrappers `listContent`, `getContent`, `createContent`, `patchContent`, `lifecycle(type, id, action, version, body?)`, `getCounts`, `getAutosave`, `putAutosave`, `deleteAutosave`, `listRevisions`, `getRevision`, `restoreRevision`, each returning the generated DTO type. `auth.ts`: `login`, `loginTotp`, `logout`, `me`, `changePassword`, `requestReset`, `confirmReset`.

**Tests** (`client.test.ts`, `fetch` stubbed): `csrf_header_on_unsafe_only`; `problem_becomes_api_error` (slug, `fieldErrors()` from `errors[]`, `retryAfter`); `if_match_quotes_version`; `unauthenticated_redirects_with_next` (the cache handler navigates once even for parallel failures); `no_retry_on_4xx`. `api:check` passes on the committed file and fails after a one-character edit of `schema.d.ts` (checked manually in Step 4 and noted in the commit body).

- [ ] **Step 1:** Write the tests; run `pnpm -C admin test client` and expect FAIL.
- [ ] **Step 2:** Run `pnpm -C admin api:gen` and commit the generated file with the code.
- [ ] **Step 3:** Implement the modules above.
- [ ] **Step 4:** Run the admin gates and expect PASS; run the manual drift check.
- [ ] **Step 5:** Commit `feat(admin): typed API client generated from openapi.json with a drift check`.

### Task 6: E2E harness, CI admin job and the Docker build stage

**Files:**
- Create: `admin/playwright.config.ts`, `admin/e2e/support/{global-setup.ts,server.ts,smtp-sink.ts,fixtures.ts,snap.ts,contact-sheet.ts,users.ts,uid.ts}`, `admin/e2e/shell-smoke.spec.ts`
- Modify: `admin/package.json` (`e2e` = `playwright test`), `.github/workflows/ci.yml` (job `admin`), `Dockerfile` (a `node:24-bookworm-slim` stage), `docs/design/10-testing.md` §1 (Admin row) and §3 (items 5–7 as built), `docs/design/09-operations.md` §7 (the image builds the admin)

**Interfaces — Produces:**
- `server.ts`: `startSite(opts: { name: string; db?: { url: string; schema: string } }): Promise<Site>`, with `Site { url: string; data: string; stop(): Promise<void> }`. It runs `FERRITE_BIN` (default `../target/release/ferrite`; a missing binary fails with the build command to run) as follows. Pick a free port and an SMTP sink port. `init --data <tmp> --url http://127.0.0.1:<port>` (plus `--db/--schema` for Postgres). `seed demo`. `user reset-password --login admin --password-stdin`. `user create --role editor|author|contributor --password-stdin` for `editor`, `author`, `contributor`, and `resetme` (created without `--password-stdin`, so it is `must_reset`; the harness parses the one-time password the command prints). Then `serve` with `FERRITE_SERVER__LISTEN=127.0.0.1:<port>`, `FERRITE_MAIL__SMTP_URL=smtp://127.0.0.1:<smtp>`, `FERRITE_MAIL__FROM=FerriteCMS <admin@example.test>`, waiting for `Listening on`.
- `smtp-sink.ts`: `startSmtpSink(): Promise<{ port: number; messages: () => Mail[]; waitFor(to: string, ms?: number): Promise<Mail>; stop() }>`, a minimal SMTP server over `node:net` (EHLO/HELO, MAIL, RCPT, DATA, RSET, NOOP, QUIT; no AUTH, no TLS), decoding quoted-printable soft breaks. No new dependency.
- `global-setup.ts`: starts the SQLite site (and, with `FERRITE_TEST_PG_URL`, the Postgres site in schema `e2e_<random>`), logs each role in over the API with the right `Origin`, saves `e2e/.auth/<role>.json` storage states, exports URLs through `process.env`, and returns the teardown (stop the servers; drop the Postgres schema with `psql` when it is on `PATH`, else print its name).
- `fixtures.ts`: `test` extended with `as(role: Role)` (a page on that storage state), `snap(page, name)` (four screenshots: `emulateMedia({ colorScheme })` × the two viewports; it waits for fonts, network idle and the end of animations), `expectAccessible(page)` (axe in the current scheme, then the other), and an automatic guard that fails on `securitypolicyviolation`, `pageerror` and `console.error` (Review Focus 2). `contact-sheet.ts` writes `test-results/screens/index.html` in global teardown.
- Projects: `chromium-sqlite` (every spec) and `chromium-pg` (only `*.smoke.spec.ts`; present when `FERRITE_TEST_PG_URL` is set). `workers: 2`, `retries: 1` on CI, `trace: "retain-on-failure"`.
- CI job `admin` (Ubuntu, a `postgres:18` service as in `check`): checkout; `pnpm/action-setup@v4` (version from `packageManager`); `actions/setup-node@v4` with Node 24 and the pnpm cache; `pnpm -C admin install --frozen-lockfile`; `lint`; `typecheck`; `test`; `api:check`; `build`; rust toolchain and `Swatinem/rust-cache@v2`; `cargo build --release -p ferrite`; `pnpm -C admin exec playwright install --with-deps chromium`; `pnpm -C admin e2e` with `FERRITE_TEST_PG_URL`; always upload `admin/test-results/` as `admin-screens`. The `check` job stays without `admin/dist`, so it covers the placeholder path.
- Dockerfile: stage `admin` (`npm install -g pnpm@10.28.2`, `pnpm install --frozen-lockfile`, `pnpm build`); the Rust builder copies `/src/admin/dist` in before `cargo build`.

**Tests:** `shell-smoke.spec.ts` (also `shell.smoke.spec.ts` for the Postgres project): `admin_loads_under_csp` (`/admin/` as admin renders the root route with no guard failures and the CSP header present on the document response); `assets_are_immutable` (the main JS response has `immutable`). The harness's own `smtp-sink` check: `smtp_sink_receives_a_reset_mail` (a `POST /api/v1/auth/password-reset` for `author` delivers a mail containing `/admin/reset-password#token=`).

- [ ] **Step 1:** Write the specs.
- [ ] **Step 2:** Run `pnpm -C admin e2e` and expect FAIL (no harness).
- [ ] **Step 3:** Implement the harness, fixtures, CI job and Dockerfile stage; update 10 and 09.
- [ ] **Step 4:** Run `pnpm -C admin build && cargo build --release -p ferrite && pnpm -C admin e2e` locally and expect PASS on SQLite and, with `FERRITE_TEST_PG_URL`, on Postgres; `docker build -t ferrite:ci .` succeeds.
- [ ] **Step 5:** Commit `test(admin): Playwright harness against a real ferrite serve; CI admin job`.

### Task 7: Design language — study notes, typography, tokens, density, motion

**Files:**
- Create: `docs/design/11-admin-ui.md`, `admin/src/styles/{tokens.css,globals.css,fonts.css}`, `admin/src/design/{tokens.ts,contrast.ts}`, `admin/src/design/contrast.test.ts`, `admin/src/design/{theme.tsx,density.tsx}`
- Modify: `admin/src/main.tsx`, `admin/components.json`, `docs/design/00-overview.md` (documents table: 11), `docs/design/07-admin-api.md` §3 (link to 11 for UX principles)

**Interfaces — Produces:**
- `docs/design/11-admin-ui.md` records the decisions below (the executor copies them, it does not re-decide them). §1 study notes and borrowed patterns; §2 typography; §3 colour; §4 spacing, radius, elevation, density; §5 motion; §6 keyboard map; §7 component inventory; §8 the design review checklist.
  - **§1 Study notes — patterns borrowed** (observed in the public products; no metrics claimed). **Linear:** ⌘K as the primary navigation, `g`-chord shortcuts and a `?` sheet, compact density, borders over shadows, optimistic updates, a property sidebar edited in place. **Vercel dashboard:** a monochrome base with one accent, status dots, toasts with an undo action, skeletons shaped like the final layout, mono for identifiers. **Stripe Dashboard:** dense tables with filter chips and sticky headers, row hover actions, metric tiles with sparklines and deltas, considered empty states. **Ghost Admin:** a distraction-free editor (large title field, settings in a slide-over, `/` insert menu), publishing as a deliberate confirm step with a summary, a word count. **Notion:** the slash menu and the selection bubble. **GitHub Primer:** status colour semantics and the focus-ring standard. **WordPress patterns to avoid:** admin-notice nags, meta-box clutter, full page reloads, inconsistent screen layouts.
  - **§2 Typography** (P3): UI **Inter Variable** (`@fontsource-variable/inter` `opsz.css` + `opsz-italic.css`; axes `wght` 100–900 and `opsz`; `font-optical-sizing: auto`, so headings get the display cut) and mono **Geist Mono Variable** (`@fontsource-variable/geist-mono` `wght.css`), both OFL-1.1, self-hosted as woff2 hashed into `dist/assets` (no CDN), with fontsource's `unicode-range` subsets (Latin, Latin Extended, Cyrillic, Greek, Vietnamese; the browser fetches only what a page uses) and `font-display: swap`. The licence texts go to `admin/public/licenses/` (copied from the packages). Scale (px / line-height): `xs` 12/16, `sm` 13/18, `base` 14/20 (UI default), `md` 16/24 (editor body 17/28 instead), `lg` 18/26, `xl` 22/28, `2xl` 28/34, `3xl` 36/42. Headings use weight 600 and tracking −0.01em (−0.02em from `2xl` up). Numbers in tables and tiles use `font-variant-numeric: tabular-nums`. Ids, slugs, versions and hashes are mono at `sm`.
  - **§3 Colour** (oklch; light / dark), with all text pairs ≥ 4.5:1 and the `input` and `ring` UI boundaries ≥ 3:1. Starting values were computed on 2026-10-25; the Task 7 test is the gate, and a value changed to pass is recorded in 11 §3. Neutrals have a cool graphite cast (hue 260). `background` 0.99 0.002 250 / 0.165 0.008 260; `foreground` 0.21 0.012 260 / 0.965 0.004 260; `card` 1 0 0 / 0.195 0.009 260; `muted` 0.965 0.004 260 / 0.22 0.01 260; `muted-foreground` 0.50 0.015 260 / 0.71 0.015 260; `border` 0.91 0.005 260 / 0.29 0.01 260; `input` 0.64 0.01 260 / 0.52 0.012 260. The accent **"ember"** (oxidised iron, the product's namesake): `primary` 0.55 0.17 40 / 0.73 0.15 52, `primary-foreground` 0.99 0.005 40 / 0.20 0.03 45, `ring` = `primary`. Semantic: `destructive` 0.55 0.21 27 / 0.70 0.18 25, `success` 0.52 0.13 150 / 0.74 0.14 150, `warning` 0.55 0.13 70 / 0.80 0.14 80, `info` 0.52 0.14 255 / 0.72 0.13 250. Content status tokens: `status-draft` = muted-foreground, `status-pending` = warning, `status-scheduled` = info, `status-published` = success, `status-private` = foreground at 70 %, `status-trash` = destructive. Charts `chart-1..5`: ember, info, success, warning, a violet 0.55 0.16 300 / 0.72 0.14 300; series are also told apart by shape or label, never by colour alone. Sidebar tokens derive from `card` and `muted`.
  - **§4 Spacing, radius, elevation, density:** Tailwind's 4 px spacing scale. `--radius` 0.5rem (sm 0.3, md 0.4, lg 0.5, xl 0.7 rem). Elevation: `--shadow-1` `0 1px 2px oklch(0 0 0 / 0.06)`, `--shadow-2` `0 4px 12px oklch(0 0 0 / 0.08), 0 1px 3px oklch(0 0 0 / 0.06)`, `--shadow-3` `0 12px 32px oklch(0 0 0 / 0.14)` (overlays). In dark mode shadows are replaced by a 1 px `border` plus a lighter `card`. Density through `html[data-density]`: `comfortable` (default) has controls 36 px, table rows 44 px, page padding 24 px; `compact` has 30 px, 34 px, 16 px (`--control-h`, `--row-h`, `--page-pad`). The choice is stored per viewer in `localStorage`, read inside try/catch.
  - **§5 Motion:** durations `--dur-instant` 80 ms (hover, press), `--dur-fast` 120 ms (popovers, tooltips), `--dur-base` 180 ms (page transitions, sheets; ≤ 200 ms per 07 §3), `--dur-slow` 240 ms (drawers only). Easings `--ease-out` `cubic-bezier(0.22, 1, 0.36, 1)`, `--ease-in-out` `cubic-bezier(0.65, 0, 0.35, 1)`. Motion springs `{ type: "spring", stiffness: 520, damping: 42, mass: 1 }` for layout (list reorder) and `{ stiffness: 380, damping: 34 }` for panels. Meaning only: enter/exit of overlays, list insert/remove/reorder, the route change (opacity plus 4 px y), the save-state indicator. Nothing loops, and nothing moves more than 8 px except sheets. `prefers-reduced-motion: reduce` sets every `--dur-*` to 0 ms in CSS and `MotionConfig reducedMotion="user"` turns off transforms and layout motion in Motion.
  - **§6 Keyboard map:** `mod+k` palette; `?` shortcut sheet; `g d` dashboard, `g p` posts, `g g` pages, `g m` media (M2b-2), `g s` settings (M2b-3); `c` new post (`c p`) or new page (`c g`); in lists `j`/`k` move, `x` select, `enter` open, `/` focus search, `#` trash the selection (with undo); in the editor `mod+s` save, `mod+shift+p` publish dialog, `mod+.` toggle the side panel, `mod+alt+r` revisions, `esc` close the panel. `mod` is ⌘ on macOS and Ctrl elsewhere. Single-key shortcuts never fire inside text inputs or the editor.
  - **§7 Component inventory** (Task 8 builds it). **§8 Design review checklist:** alignment to the 4 px grid; one primary action per view; consistent page header (title, description, actions, tabs); loading skeletons, empty, error and permission-denied states present; dark mode at parity (no hard-coded colours; `grep -E "#[0-9a-f]{3,6}|rgb\(" src` finds none outside `tokens.css`); mobile width without horizontal scroll and with ≥ 44 px touch targets; text never truncated without a tooltip or title; numbers tabular; focus visible; motion within §5.
- `tokens.css`: every value above as CSS variables on `:root` and `.dark`, plus the density and motion variables. `globals.css`: `@import "tailwindcss"`, `"tw-animate-css"`, `"shadcn/tailwind.css"`, `"./fonts.css"`, `"./tokens.css"`; `@custom-variant dark (&:is(.dark *))`; `@theme inline` mapping every token (`--color-*`, `--radius-*`, `--font-sans: "Inter Variable", ui-sans-serif, system-ui, sans-serif`, `--font-mono: "Geist Mono Variable", ui-monospace, monospace`, `--shadow-*`, `--ease-*`, the type scale as `--text-*` with `--text-*--line-height`); base layer focus ring `outline: 2px solid var(--ring); outline-offset: 2px` on `:focus-visible`.
- `tokens.ts`: `TOKENS` (names), `CONTRAST_PAIRS: { fg: string; bg: string; min: 4.5 | 3; modes: ("light" | "dark")[] }[]` (foreground on background/card/muted; muted-foreground on background/card/muted; primary-foreground on primary; each semantic and status colour on background and card; input and ring on background at 3). `contrast.ts`: `parseTokens(css: string): Record<"light" | "dark", Record<string, Oklch>>`, `oklchToSrgb`, `relativeLuminance`, `contrastRatio(a, b): number` (WCAG 2.x formula).
- `theme.tsx`: `ThemeProvider` (`light | dark | system`; toggles `.dark` on `<html>`; follows `prefers-color-scheme` under `system`; stored per viewer), `useTheme()`. `density.tsx`: `DensityProvider`, `useDensity()`. A synchronous class switch in `main.tsx` before the first render avoids a flash; it is module code, not an inline script.

**Tests** (`contrast.test.ts`): `every_contrast_pair_meets_its_minimum` (parses `tokens.css` and checks every pair in both modes); `contrast_math_matches_known_values` (black/white 21:1, `#777` on white ≈ 4.48:1); `reduced_motion_zeroes_durations` (a `@media (prefers-reduced-motion: reduce)` block sets every `--dur-*` to `0ms`, by parsing); `no_raw_colours_outside_tokens` (scans `src/**/*.{ts,tsx,css}` except `tokens.css` for hex and `rgb(`/`hsl(` literals).

- [ ] **Step 1:** Write the tests; run `pnpm -C admin test design` and expect FAIL.
- [ ] **Step 2:** Write `11-admin-ui.md` and the CSS, providers and helpers; copy the font licences.
- [ ] **Step 3:** Run the admin gates and expect PASS; record the shipped woff2 total in 11 §2.
- [ ] **Step 4:** Commit `feat(admin): design language — Inter + Geist Mono, ember tokens, density, motion (doc 11)`.

### Task 8: Component kit and the living styleguide

**Files:**
- Create: `admin/src/components/ui/*` (shadcn adds: `button badge card checkbox command dialog dropdown-menu input input-group label popover scroll-area select separator sheet sidebar skeleton sonner switch table tabs textarea toggle toggle-group tooltip chart calendar breadcrumb avatar alert-dialog hover-card kbd`), `admin/src/components/kit/{page-header.tsx,status-badge.tsx,empty-state.tsx,metric-tile.tsx,sparkline.tsx,data-table.tsx,data-table-features.ts,kbd-combo.tsx,save-state.tsx,relative-time.tsx,error-state.tsx,forbidden-state.tsx}`, `admin/src/routes/_app/_styleguide.tsx` (lazy route `/admin/_styleguide`), `admin/src/components/kit/kit.test.tsx`, `admin/e2e/styleguide.spec.ts`
- Modify: `admin/src/components/ui/sonner.tsx` (reads `useTheme` from `@/design/theme`, not `next-themes`; P9), `docs/design/11-admin-ui.md` §7

**Interfaces — Produces:**
- shadcn components restyled only through tokens. Edits to generated files stay minimal and are listed in 11 §7.
- Kit: `PageHeader({ title, description?, actions?, tabs? })`; `StatusBadge({ status: ContentStatus })` (dot plus label, status tokens); `EmptyState({ icon, title, body, action? })`; `ErrorState({ error: unknown, retry })` (problem `detail` shown, never stack traces); `ForbiddenState()`; `MetricTile({ label, value: number, delta?: number, series?: number[], href? })`; `Sparkline({ data: number[] })` (a Recharts `AreaChart` without axes, `aria-hidden`, the value given in text); `DataTable<T>({ columns, data, getRowId, selection?, onRowOpen?, loading, empty })` on TanStack Table v9 `useTable` with `rowSelectionFeature`, `j`/`k`/`x`/`enter` keyboard handling, sticky header and row hover actions; `KbdCombo({ keys: string })`; `SaveState({ state: "saved" | "saving" | "dirty" | "error" | "offline", at?: Date })` (Motion crossfade); `RelativeTime({ at: string })` (`date-fns` `formatDistanceToNowStrict`, absolute time in `title`).
- `/admin/_styleguide` is a lazy chunk shipped in the build behind login and linked from nowhere (no nav entry, no palette entry; P12). It shows every token swatch with its contrast ratio, the type scale, both fonts with weights, spacing and radius, the shadows, the motion tokens (a `prefers-reduced-motion` note), every kit and shadcn component in its states (default, hover via a forced class, focus, disabled, loading, error), both densities, and the five-series chart sample.

**Tests:** `kit.test.tsx` `status_badge_labels_every_status`, `data_table_keyboard_selection` (j/k/x/enter), `save_state_announces` (an `aria-live="polite"` region gets the text), `metric_tile_shows_text_not_only_chart`. E2E `styleguide.spec.ts`: `styleguide_screens` (`snap` for each section; `expectAccessible`), `styleguide_compact_density` (row height 34 px measured).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Add the shadcn components with `pnpm dlx shadcn@4.21.4 add …`; build the kit and the styleguide.
- [ ] **Step 3:** Run the admin gates and the E2E; open the contact sheet and go through 11 §8.
- [ ] **Step 4:** Commit `feat(admin): component kit and the living styleguide`.

### Task 9: Auth screens — login, TOTP, forced change, forgot and reset password

**Files:**
- Create: `admin/src/routes/{login.tsx,login.totp.tsx,change-password.tsx,forgot-password.tsx,reset-password.tsx}`, `admin/src/auth/{session.ts,next.ts,reset-token.ts,password-field.tsx,auth-layout.tsx}`, `admin/src/auth/{next.test.ts,reset-token.test.ts,login.test.tsx}`, `admin/src/i18n/en/auth.json`, `admin/e2e/auth.spec.ts`
- Modify: `admin/src/routes/__root.tsx`, `admin/src/router.tsx`, `admin/e2e/support/{users.ts,global-setup.ts}` (user `totp`, enrolled over the API: `POST /auth/totp/setup` then `confirm` with a computed code), new `admin/e2e/support/totp.ts` (RFC 6238 SHA-1, 6 digits, 30 s, about 30 lines over `node:crypto`)

**Interfaces — Consumes:** Task 5 `auth.ts`, `ApiError`, `setCsrfToken`, `qk.me`.

**Interfaces — Produces:**
- `session.ts`: `useMe(): MeDto | undefined`; `ensureSession(ctx): Promise<MeDto>`, used in the `_app` layout's `beforeLoad` (Task 10). It calls `GET /auth/me`; a `401` → `redirect({ to: "/login", search: { next } })`; `must_reset` → `redirect({ to: "/change-password" })`. It stores `csrf_token` through `setCsrfToken`. `logout()` posts, clears the query cache and the token, and navigates to `/login`.
- `next.ts`: `safeNext(raw: string | null | undefined): string`. It returns `raw` only when it is a path starting with `/admin/` after URL parsing against `location.origin`, with the same origin, no backslash, no `//` prefix and no `..` segment after decoding, and is not `/admin/login` itself; otherwise `/admin/`.
- `reset-token.ts`: `takeResetToken(loc: Pick<Location, "hash" | "pathname" | "search">, history: Pick<History, "replaceState">): string | null`. It parses `token` from the hash and calls `history.replaceState(null, "", loc.pathname + loc.search)` whether or not one was found. `main.tsx` calls it **before** `createRoot`, when the path is `/admin/reset-password`, and hands the value to the route through a module-level one-shot holder (`consumeResetToken()` returns it once, then `null`).
- Screens (`auth-layout.tsx`: centred card on the background, the site title from `GET /meta` + `site.title`, ember mark; mobile full-bleed):
  - Login: login-or-e-mail and password (`autocomplete="username"` / `"current-password"`), a show-password toggle, a submit with pending state. `invalid-credentials` → one generic inline error; `rate-limited` → "Too many attempts, try again in N s" with a countdown; `totp_required` → `/login/totp` with the challenge in router state (never in the URL).
  - TOTP: a 6-digit one-time-code input (`inputmode="numeric"`, `autocomplete="one-time-code"`, auto-submit at 6 digits); an expired challenge (`422`) → back to login with a notice.
  - Change password: current and new password (12+ characters, a live length hint, the policy text from 07 §2); on success it stores the new `csrf_token` and goes to `safeNext`.
  - Forgot password: login or e-mail; `202` → the same neutral confirmation whatever the account; `503 mail-unavailable` → "Password reset by e-mail is not set up on this site; ask an administrator" (07 §2).
  - Reset password: the token from `consumeResetToken()`, new password twice. `204` → "Password changed, sign in". `422 field=token` → "This link is invalid or has expired" with a link to forgot-password. No token → the same expired message.

**Tests:** units `safe_next_table` (Review Focus 5: accepts `/admin/`, `/admin/content/post?status=draft`; refuses `//evil.com`, `/\evil`, `https://evil`, `/admin/../api/v1`, `/admin/%2e%2e/x`, `javascript:alert(1)`, `/api/v1/auth/logout`, `/admin/login`, empty); `take_reset_token_strips_hash` (with and without a token; `replaceState` called once with no `#`); `login_maps_problems` (Testing Library with a stubbed API: invalid credentials, rate-limited countdown, TOTP branch). E2E (`auth.spec.ts`, own users from the harness): `login_password_and_logout`; `login_with_totp` (user `totp`; the code comes from `e2e/support/totp.ts`, waiting for the next 30 s step when the setup's step is still current, since a step is accepted once); `must_reset_session_is_confined` (Review Focus 4: `resetme` logs in → change-password; visiting `/admin/` redirects back; after the change → dashboard); `next_is_not_an_open_redirect` (`/admin/login?next=//evil.example` → lands on `/admin/`); `forgot_and_reset_flow` (forgot for `author` → the SMTP sink's link → reset → login with the new password); `reset_token_is_stripped_and_never_sent` (Review Focus 1: `page.on("request")` records every URL and `referer` header; none contains the token; `page.url()` has no `#`; reload → expired message); `screens` (`snap` + `expectAccessible` for login, TOTP, change, forgot, reset, and the error states).

- [ ] **Step 1:** Write the unit tests and E2E; run them and expect FAIL.
- [ ] **Step 2:** Implement the session layer, guards and screens.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet against 11 §8.
- [ ] **Step 4:** Commit `feat(admin): sign-in, TOTP, forced password change and the password-reset page`.

### Task 10: App shell — sidebar, header, theme and density, transitions, shortcuts

**Files:**
- Create: `admin/src/routes/_app.tsx` (authenticated layout), `admin/src/routes/_app/{not-found.tsx}`, `admin/src/shell/{app-sidebar.tsx,nav.ts,top-bar.tsx,user-menu.tsx,route-transition.tsx,shortcut-sheet.tsx,error-boundary.tsx}`, `admin/src/lib/shortcuts.ts`, `admin/src/lib/shortcuts.test.ts`, `admin/src/shell/nav.test.ts`, `admin/src/i18n/en/shell.json`, `admin/e2e/shell.spec.ts`
- Modify: `admin/src/router.tsx` (`defaultNotFoundComponent`, `defaultErrorComponent`), `admin/src/main.tsx` (`MotionConfig reducedMotion="user"`, `LazyMotion features={domAnimation}`, `Toaster`)

**Interfaces — Consumes:** Task 9 `ensureSession`, `useMe`, `logout`; Task 7 providers; Task 8 kit.

**Interfaces — Produces:**
- `nav.ts`: `navFor(me: MeDto, types: ContentTypeDto[]): NavSection[]` (`NavSection { id, label, items: NavItem[] }`, `NavItem { id, label, to, icon, shortcut?, badge? }`). Sections: Overview (Dashboard); Content (Posts, Pages, each custom type except `media`, by label); later sections appear only when their screens exist (Media, Comments, Appearance, Plugins, Users, Settings, System land in M2b-2/3). An item shows only when `me.capabilities` holds the capability the matching API needs (Content: `read_admin`); the server still enforces it.
- Layout: a collapsible sidebar (shadcn `sidebar`; icon rail when collapsed, state per viewer; a `Sheet` under 768 px). The top bar has breadcrumbs from route `staticData.crumb`, a ⌘K trigger showing the shortcut, a "New" split button (post/page, `allowed` by `edit_own`), a theme toggle (light/dark/system) and density in the user menu, and the user menu (display name, role, "Sign out"). A "Search engines discouraged" pill shows when `reading.discourage_indexing` is true.
- `route-transition.tsx`: `<AnimatePresence mode="wait">` keyed by the matched leaf route, `opacity 0→1, y 4→0` over `--dur-base` with `--ease-out`; reduced motion → opacity only.
- `shortcuts.ts`: `registerShortcut(s: { id: string; keys: string /* "mod+k", "g d", "?" */; label: string; group: string; run: () => void; when?: () => boolean; allowInInput?: boolean }): () => void`, `useShortcut(s)` (registers on mount), `listShortcuts(): Shortcut[]`, one `keydown` listener. Chords time out after 1 000 ms. Inside `input`, `textarea`, `[contenteditable]` only `allowInInput` shortcuts fire.
- `shortcut-sheet.tsx`: `?` opens a dialog listing `listShortcuts()` by group, with `KbdCombo`.
- Errors: a route error shows `ErrorState` inside the shell. Not found shows an `EmptyState` with "Go to dashboard". `403 forbidden` shows `ForbiddenState`.

**Tests:** `shortcuts_test` (chords, timeout, ignored in inputs, `mod` maps to Meta on macOS and Control elsewhere, unregister); `nav_test` (an author sees Posts and Pages and the custom types; nothing above its rights). E2E `shell.spec.ts`: `sidebar_collapses_and_persists`; `mobile_nav_sheet` (390 px: the trigger opens the sheet, focus is trapped, `esc` closes); `g_chords_navigate`; `shortcut_sheet_lists_bindings`; `theme_toggle_and_system` (`emulateMedia` dark under `system` → `.dark` set); `reduced_motion_disables_transforms` (`emulateMedia({ reducedMotion: "reduce" })` → the route container has no `transform` mid-transition); `not_found_inside_shell`; `screens` (dashboard placeholder page, not found, forbidden, sheet open, both densities).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement the layout, nav, transitions, shortcuts and error states.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): app shell with sidebar, shortcuts, theme, density and route transitions`.

### Task 11: Command palette

**Files:**
- Create: `admin/src/palette/{palette.tsx,sources.ts,recent.ts}`, `admin/src/palette/sources.test.ts`, `admin/src/i18n/en/palette.json`, `admin/e2e/palette.spec.ts`
- Modify: `admin/src/shell/top-bar.tsx`, `admin/src/routes/_app.tsx`

**Interfaces — Consumes:** Task 10 `registerShortcut`, `navFor`; Task 5 `listContent`.

**Interfaces — Produces:**
- `palette.tsx`: a shadcn `CommandDialog` opened by `mod+k` (`allowInInput: true`) and the top-bar trigger. Groups: **Recent** (the last 8 opened items and screens, per viewer), **Go to** (every `navFor` item and the `?` sheet), **Create** (new post, new page, new `<type>` for each type the nav shows), **Content** (live search), **Preferences** (theme light/dark/system, density). `cmdk`'s built-in filter covers the static groups.
- `sources.ts`: `searchContent(q: string, types: string[], signal: AbortSignal): Promise<PaletteHit[]>`, with `PaletteHit { type, id, title, status, permalink }`. It runs one `GET /content/{type}?q=<q>&limit=8` per type in parallel, debounced 150 ms, aborting the previous round; a query shorter than 2 characters is skipped. A hit opens the editor.
- `recent.ts`: `pushRecent(entry)`, `listRecent()`, stored per viewer, at most 8, in try/catch.

**Tests:** `sources_test` (debounce, abort, under-2-characters skip, merge order by type then relevance); E2E `palette.spec.ts`: `open_with_mod_k_and_navigate` (to Pages by keyboard only); `search_finds_seeded_post` (types a title word from `seed demo` → the hit opens the editor route); `create_new_post_from_palette`; `switch_theme_from_palette`; `screens` (open, searching, results, empty result).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): command palette with navigation, creation and live content search`.

### Task 12: Dashboard

**Files:**
- Create: `admin/src/routes/_app/index.tsx`, `admin/src/dashboard/{tiles.tsx,activity-chart.tsx,needs-attention.tsx,recent-drafts.tsx,site-health.tsx,activity.ts}`, `admin/src/dashboard/activity.test.ts`, `admin/src/i18n/en/dashboard.json`, `admin/e2e/dashboard.spec.ts`
- Modify: `docs/design/07-admin-api.md` §3 (Dashboard row: what M2b-1 shows; page views move to M4 with the metrics time series, P16)

**Interfaces — Consumes:** Task 2 counts, `author`; Task 8 `MetricTile`, `Sparkline`, `chart`; Task 5 keys.

**Interfaces — Produces:**
- `activity.ts`: `bucketByDay(items: { published_at: string | null }[], days: number, now: Date, timeZone: string): { day: string; count: number }[]` (always `days` buckets, oldest first, in the browser's time zone); `fetchPublished(type, since: Date): Promise<ContentSummaryDto[]>` pages `status=published&order=published&limit=200` until an item is older than `since` or `next_cursor` is null, at most 5 pages (1 000 items; a cap note shows when it is hit).
- Layout (a 12-column grid; one column on mobile):
  - **Tiles** (every role): Published, Drafts, Pending review, Scheduled for `post`, from `GET /content/post/counts`. Each links to the filtered list, and Published carries a 30-day sparkline.
  - **Publishing activity** (every role): an area chart of posts and pages published per day over 30 days (two series, `chart-1`/`chart-2`, tooltip, legend).
  - **Recent drafts** (every role): the actor's 5 latest drafts (`author=me`, `status=draft`), with "Continue editing".
  - **Needs attention**: pending-review items for roles with `edit_others` (5, with a link to all); for admins also dead jobs (`GET /system/jobs?status=dead&limit=5`), disabled plugins (`GET /plugins`, `enabled = false` with `disabled_reason`), a schema verdict other than `up_to_date`, and theme fallback nodes (`GET /system/status`). Each entry links to the API-backed detail screens as M2b-3 adds them (until then, to nothing and plain text).
  - **Site health** (admins with `manage_system`): a page-cache hit-ratio radial bar (`hits / (hits + misses)`, with the numbers as text), jobs by status as a horizontal bar, plugin failures in the last 24 h as hourly stacked bars summed over plugins (`GET /plugins/{name}/metrics` for each enabled plugin, at most 10), and the node label "this node only" (metrics are node-local, M2a-2b P20).
- Each card handles loading (skeleton), empty ("No posts published in the last 30 days" + "Write a post") and error (`ErrorState` in the card; the rest of the page still works). A card the actor may not see is not requested at all.

**Tests:** `activity_test` (bucket edges at local midnight, DST change days, zero-filled days, items without `published_at` ignored); component tests (P17) `needs-attention.test.tsx` `lists_dead_jobs_and_disabled_plugins` (stubbed API with one dead job and one disabled plugin) and `activity-chart.test.tsx` `empty_activity_state` (no published items). E2E `dashboard.spec.ts`, asserting only what the seeded site has: `admin_sees_health_and_attention` (the cache tile shows numbers, the jobs bar renders, "No dead jobs" shows); `author_sees_no_admin_cards` (no request to `/system/*` or `/plugins` is made, checked with `page.on("request")`); `tiles_match_counts` (tile values equal `GET /content/post/counts`); `screens` (admin, editor and author dashboards).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement the cards and the activity helpers; update 07 §3.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet (every chart in both themes, legible at mobile width).
- [ ] **Step 4:** Commit `feat(admin): dashboard with counts, publishing activity, attention list and site health`.

### Task 13: Content list

**Files:**
- Create: `admin/src/routes/_app/content.$type.index.tsx`, `admin/src/content/{list-columns.tsx,list-search.ts,bulk.ts,status-tabs.tsx,filters.tsx}`, `admin/src/content/{list-search.test.ts,bulk.test.ts}`, `admin/src/i18n/en/content.json`, `admin/e2e/content-list.spec.ts`

**Interfaces — Consumes:** Task 2 `allowed`, `author`, counts; Task 8 `DataTable`, `StatusBadge`; Task 10 shortcuts.

**Interfaces — Produces:**
- Route `/admin/content/$type` with search params validated by a Zod schema (`list-search.ts`): `ListSearch { status?: ContentStatus | "all" /* default "all" = every status but trash */; q?: string; mine?: boolean; term?: string /* uuid */; order?: "updated" | "published" | "title"; }`. Defaults are omitted from the URL. A `q` together with `mine`/`term` is resolved as the API requires (07 §1: `q` excludes `author`, `term` and `parent`): when `q` is set, the other two filters are shown disabled with a tooltip.
- Header: the type's label, the count, a "New <type>" primary button. Status tabs (All, Draft, Pending, Scheduled, Published, Private, Trash) with counts from Task 2's endpoint. A search field (`/` focuses it, debounced 200 ms). Filter chips: Mine; a term picker per taxonomy of the type (a `Popover` + `Command` over `GET /taxonomies/{t}/terms`, at most 200); order.
- Columns: select; title (with the permalink path below in mono `xs`); parent (hierarchical types only: the parent's title, resolved from the loaded page or one `GET` per missing parent); status (`StatusBadge`, with the scheduled time); author (`display_name`); date (published or updated, `RelativeTime`); a row-hover actions menu (Edit, Preview `/_preview/<id>` in a new tab, Trash or Restore, Delete permanently for trash), each item present only when `allowed` says so.
- Paging: `useInfiniteQuery` over `next_cursor` with "Load more" and auto-load when the sentinel scrolls into view (an `IntersectionObserver`).
- `bulk.ts`: `runBulk(items: ContentSummaryDto[], action: "trash" | "restore" | "publish" | "delete", run: (item) => Promise<unknown>, concurrency = 4): Promise<{ ok: string[]; failed: { id: string; error: ApiError }[] }>`. Only items whose `allowed[action]` is true are submitted. Each uses its own `If-Match`.
- Undo: trashing (row or bulk) shows a toast "N moved to trash" with **Undo**, which restores each item (`POST …/restore` with its new version); delete-permanently is the only confirm dialog (an `AlertDialog`), per 07 §3 "undo rather than confirm".
- Motion: rows enter and leave with `AnimatePresence` + `layout` (spring from 11 §5); reduced motion → none.

**Tests:** `list_search_test` (defaults omitted, invalid params dropped, `q` disables `mine`/`term`); `bulk_test` (concurrency ≤ 4, skips disallowed, collects failures with their problem). E2E `content-list.spec.ts`: `tabs_counts_and_filtering` (counts equal the tab's list length for seeded posts; search narrows; Mine narrows); `author_counts_match_list` (Review Focus 4: as `author`, the Draft count equals the rows shown and no other author's draft appears); `trash_with_undo` (row trash → the toast's Undo → back with status restored); `bulk_publish_partial_failure` (editor selects drafts including one whose version was bumped by the API meanwhile → one failure listed, the rest published); `keyboard_list_navigation` (j/k/x/enter); `infinite_paging` (an editor-created set of 60 posts → second page loads); `screens` (all tabs, empty trash, search with no results, mobile).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): content list with status tabs, filters, bulk actions and undo`.

### Task 14: Editor (1) — the TipTap document model and the writing surface

**Files:**
- Create: `admin/src/editor/{schema.ts,html-node.tsx,image-node.tsx,slash-menu.tsx,bubble-menu.tsx,editor.tsx,word-count.ts,text.ts,diff.ts}`, `admin/src/editor/{schema.test.ts,text.test.ts,diff.test.ts}`, `admin/src/editor/fixtures/{every-node.json,unknown-node.json,seed-sample.json}`, `admin/src/routes/_app/content.$type.$id.tsx`, `admin/src/routes/_app/content.$type.new.tsx`, `admin/src/styles/editor.css`, `admin/src/i18n/en/editor.json`, `admin/e2e/editor.spec.ts`

**Interfaces — Consumes:** Task 5 `getContent`, `createContent`, `patchContent`, `ifMatch`; Task 2 `allowed`.

**Interfaces — Produces:**
- `schema.ts`: `editorExtensions(opts: { placeholder: string; resolveMedia: (id: string) => Promise<string | null> }): Extensions`. The editor knows exactly the nodes and marks the core renderer handles (`crates/ferrite-core/src/blocks/render.rs`): nodes `doc paragraph heading(levels 2–6; level 1 is the title, P18) bulletList orderedList(start) listItem blockquote codeBlock(language) horizontalRule hardBreak image(src, alt, title) table tableRow tableHeader(colspan, rowspan) tableCell(colspan, rowspan) html(html) text`; marks `bold italic strike code link(href, target)`. That is `StarterKit.configure({ underline: false, heading: { levels: [2,3,4,5,6] }, link: { openOnClick: false, autolink: true, protocols: ["http", "https", "mailto"] } })`, plus `TableKit` from `@tiptap/extension-table`, `Image` from `@tiptap/extension-image` (inline false), `Placeholder` from `@tiptap/extensions`, and two custom nodes: `html` (an atom block with attribute `html`, rendered in the editor as a read-only code preview with an "edit HTML" sheet; the site sanitises it with ammonia, 06 §5) and the image `NodeView`, which resolves `ferrite:media/<uuid>` through `GET /media/{id}` to the variant URL and shows a broken-image state when it is missing. Inserting images arrives with the media picker in M2b-2; M2b-1 displays and keeps them. `SUPPORTED_NODES`, `SUPPORTED_MARKS` are exported.
- `validateBody(doc: unknown): { ok: true } | { ok: false; unknown: string[] }` walks the JSON for any type or mark outside the sets, and for a heading level 1. On load: `ok: false` → the editor is created `editable: false` with a banner ("This item contains content this editor cannot edit safely: <types>. It is shown read-only so nothing is lost.") and every save is disabled (Review Focus 3). A level-1 heading counts as unknown, with the same banner (P18). `useEditor({ extensions, content, immediatelyRender: true, shouldRerenderOnTransaction: false, injectCSS: false, enableContentCheck: true, onContentError })`; `onContentError` routes to the same read-only state.
- `text.ts`: `blockText(doc: unknown): string`, the TS counterpart of core `blocks::extract_text`, for word count and the revision diff; `wordCount(text)`.
- `diff.ts` (used by Task 16's conflict dialog and Task 17's drawer): `diffWords(a: string, b: string): DiffPart[]` (`diff` 9.0.0 `diffWordsWithSpace`), `DiffPart { kind: "same" | "added" | "removed"; text: string }`; `diffRevision(older: { title: string; excerpt: string; body: unknown }, newer: { title: string; excerpt: string; body: unknown }): { title: DiffPart[]; excerpt: DiffPart[]; body: DiffPart[] }` (the body through `blockText`).
- Writing surface (Ghost-like): a centred 720 px column, the title as an auto-growing `textarea` (`2xl`, display cut), the body at 17/28, a bubble menu on selection (bold, italic, strike, code, link with URL validation, H2/H3, quote), a `/` slash menu (`@tiptap/suggestion`: headings, lists, quote, code block, table 3×3, divider, HTML block) with keyboard navigation, the word count and reading time in the footer, and a focus mode (`mod+shift+f`) that hides the shell.
- Routes: `/admin/content/$type/new` creates the draft on the first save (`POST`, then `replace` navigation to `/$id`); `/admin/content/$type/$id` loads `GET` (with `ETag`/`version`). Saving (`mod+s` and the panel button of Task 15) is `PATCH` with `If-Match`; `412` is handed to Task 16's handler (until then a toast).

**Tests:** `schema_test`: `roundtrip_every_renderer_node` (Review Focus 3: `every-node.json` contains each supported node and mark with every attribute the renderer reads; `editor.getJSON()` after `setContent` deep-equals the input, with only documented normalisations listed in the test: TipTap adds `attrs` defaults, and the test asserts the list is exactly those); `unknown_node_opens_read_only` (`unknown-node.json` with `{ type: "callout" }` → `validateBody` reports it, the component is not editable, save disabled); `heading_1_is_unknown`; `seed_sample_roundtrips` (a body copied from `seed demo` output). `text_test`: `block_text_matches_core` (the same fixture text as `crates/ferrite-core/src/blocks/text.rs` tests, copied with a comment linking them). `diff_test`: word-level additions and removals, whitespace kept, empty to text, identical inputs give one `same` part. E2E `editor.spec.ts`: `write_and_save_new_post` (type a title, slash-insert H2 and a list, `mod+s` → the URL gets the id, a reload shows the same document); `bubble_menu_link_refuses_javascript` (`javascript:alert(1)` is refused in the link input); `focus_mode`; `screens` (empty new post, a long seeded post, slash menu open, bubble menu, read-only banner).

- [ ] **Step 1:** Write the tests and fixtures; run them and expect FAIL.
- [ ] **Step 2:** Implement the schema, custom nodes, menus and routes.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): TipTap editor bound to the core block model, slash and bubble menus`.

### Task 15: Editor (2) — the side panel and the publishing flow

**Files:**
- Create: `admin/src/editor/panel/{side-panel.tsx,status-section.tsx,publish-dialog.tsx,schedule-picker.tsx,slug-field.tsx,excerpt-field.tsx,terms-section.tsx,parent-field.tsx,danger-zone.tsx}`, `admin/src/editor/panel/{publish.test.tsx,terms.test.tsx}`, `admin/e2e/editor-publish.spec.ts`
- Modify: `admin/src/routes/_app/content.$type.$id.tsx`, `admin/src/i18n/en/editor.json`

**Interfaces — Consumes:** Task 2 `allowed`; Task 5 `lifecycle`, `patchContent`; Task 14 editor state.

**Interfaces — Produces:**
- `side-panel.tsx`: a right panel (a `Sheet` under 1024 px), toggled by `mod+.`, with sections:
  - **Status**: `StatusBadge`, the published or scheduled time, the author, and the action row driven only by `allowed`. Primary: Publish, Schedule, Update (published), Submit for review (contributors), Save draft. Secondary: Unpublish, Preview. Every lifecycle call sends `If-Match` and replaces the cached item with the response.
  - **Publish dialog** (Ghost's deliberate step): a summary (title, permalink, terms, excerpt state), "Publish now" or "Schedule for" (a `Calendar` + time input in the browser's time zone, sent as RFC 3339 UTC; past times refused client-side and by the API), and a confirm button. `mod+shift+p` opens it.
  - **URL**: the slug field (editing shows the resulting permalink, from the response's `permalink` after save) and a copy-link button.
  - **Excerpt**: a textarea; empty means "derive from the body" (sends `excerpt: null`, per F20 in 07 §1).
  - **Terms**: one section per taxonomy whose `content_types` include the type. A hierarchical taxonomy is a checkbox tree; a flat one is a tag combobox. Creating a term inline shows only when `me.capabilities` includes `manage_terms` (the server checks again).
  - **Parent and order** (hierarchical types): a parent combobox searching `GET /content/{type}?q=` (excluding the item and its descendants as far as the loaded list shows; the server refuses cycles) and `menu_order`.
  - **Danger zone**: Move to trash (with undo, as in Task 13), shown when `allowed.trash`.
- All fields of the panel and the title and body form **one** draft. The save button and `mod+s` PATCH the changed members only.

**Tests:** `publish_test` (buttons rendered for each `allowed` combination; schedule in the past refused; the RFC 3339 conversion keeps the chosen wall-clock time); `terms_test` (tree selection, tag creation hidden without `manage_terms`). E2E `editor-publish.spec.ts`: `contributor_can_submit_not_publish` (Review Focus 4: the contributor sees Submit and no Publish/Schedule; after submit the status is pending and the editor shows it read-only for the contributor if `allowed.edit` is false); `author_publishes_and_unpublishes`; `editor_schedules` (the scheduled time is shown in the list and the dashboard tile changes); `slug_change_updates_permalink`; `assign_category_and_new_tag` (editor); `trash_from_editor_with_undo`; `screens` (panel for each role, publish dialog, schedule picker, mobile sheet).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement the panel and the publishing flow.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): editor side panel and the publish/schedule/submit flow`.

### Task 16: Editor (3) — autosave, the 412 conflict UI and the leave guard

**Files:**
- Create: `admin/src/editor/{autosave.ts,conflict.ts,conflict-dialog.tsx,restore-banner.tsx,leave-guard.ts}`, `admin/src/editor/{autosave.test.ts,conflict.test.ts}`, `admin/e2e/editor-safety.spec.ts`
- Modify: `admin/src/routes/_app/content.$type.$id.tsx`, `admin/src/routes/_app/content.$type.new.tsx`, `admin/src/api/client.ts` (the 401 handler keeps editor state, see below)

**Interfaces — Consumes:** Task 3 autosave endpoints; Task 14/15 draft state and Task 14 `diffRevision`; Task 8 `SaveState`.

**Interfaces — Produces:**
- `autosave.ts`: `createAutosaver(opts: { save: (d: Draft) => Promise<void>; intervalMs: 10_000; now?: () => number }): { markDirty(d: Draft): void; flush(): Promise<void>; dispose(): void; state: Observable<SaveState> }`. Every 10 s while dirty it PUTs the latest draft to `/autosave`, never two requests in flight (a newer draft waits for the running one). It flushes on `visibilitychange` to hidden and on route leave. On a network failure the state is `offline`, retried on the next tick and on the `online` event. A successful explicit save (PATCH) marks it clean (the server deleted the slot, Task 3). `Draft = { base_version, title, excerpt, body, fields }`. A new, never-saved item has no slot: its first explicit save creates it, and until then the draft is kept in memory and the leave guard protects it.
- `restore-banner.tsx`: on load, `GET …/autosave`; when it exists and differs from the item, a banner "Unsaved changes from <relative time> were restored" with **Discard** (DELETE the slot, reload the item) is shown and the autosave becomes the working draft. When `base_version` < the item's version, the banner says the item changed since, and saving goes through the conflict flow.
- `conflict.ts`: `onPreconditionFailed(problem: ApiError, mine: Draft): ConflictState`, where `ConflictState = { theirs: ContentDto /* problem.current */; mine: Draft; fields: ("title" | "excerpt" | "body" | "slug" | "terms")[] /* that differ */ }`. `conflict-dialog.tsx` shows the differing fields side by side (title and excerpt as text, the body as Task 14's `diffRevision` body parts) with three choices: **Keep mine** (re-PATCH with `If-Match: theirs.version`), **Take theirs** (replace the working draft; mine stays in the autosave slot until discarded), **Copy mine** (copies my body text to the clipboard, then takes theirs). The dialog never closes on outside click.
- `leave-guard.ts`: `useLeaveGuard(dirty: boolean)` blocks router navigation with a dialog (Save / Discard / Stay; TanStack Router `useBlocker`) and sets `beforeunload` while dirty.
- Session expiry: a `401` during autosave or save leaves the working draft in memory, flushes it to the autosave slot right after re-login (the login route returns through `next` to the same editor, and the restore banner then finds the slot), and never discards it. The client's 401 handler navigates with `next` and does not clear the editor's draft store (a module-level `pendingDrafts` map keyed by item id, consumed by the editor on mount).

**Tests:** `autosave_test` (fake timers: dirty → one PUT at 10 s; edits during a running PUT → exactly one follow-up; offline → `offline` state then retry on `online`; explicit save → clean, no PUT); `conflict_test` (the differing-fields detection; Keep mine sends the new version). E2E `editor-safety.spec.ts`: `autosave_restores_after_reload` (Review Focus 3: type, wait for "Saved", reload without explicit save → banner and text back); `conflict_keeps_my_text` (Review Focus 3: two pages as two editors; A saves; B saves → the dialog shows both; Keep mine → B's text is stored at the next version; the reverse path Take theirs keeps B's draft in the slot); `session_expiry_mid_edit_keeps_the_draft` (Review Focus 3, 5: delete the session through `POST /api/v1/auth/logout` from another context with the same cookie, then type and save → login page with `next`; log in → back in the editor with the banner and the text); `leave_guard_blocks_navigation`; `screens` (saving, saved, offline, conflict dialog, restore banner).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): autosave slot, 412 conflict resolution and the leave guard`.

### Task 17: Revisions drawer with diff and restore

**Files:**
- Create: `admin/src/editor/revisions/{revisions-drawer.tsx,revision-diff.tsx}`, `admin/src/editor/revisions/revision-diff.test.tsx`, `admin/e2e/revisions.spec.ts`
- Modify: `admin/src/editor/panel/side-panel.tsx` (the "Revisions (n)" entry), `admin/src/i18n/en/editor.json`

**Interfaces — Consumes:** Task 2 `author` on revisions; Task 5 `listRevisions`, `getRevision`, `restoreRevision`; Task 14 `diffRevision`, `DiffPart`.

**Interfaces — Produces:**
- `revision-diff.tsx`: `RevisionDiff({ parts: DiffPart[] })` renders the marks described below.
- `revisions-drawer.tsx`: a right drawer (`--dur-slow`), opened by `mod+alt+r`. The list (newest first, at most 50: author display name, relative time, the `restored from …` note) sits on the left; the diff of the selected revision against the current item sits on the right. Added text is underlined in `success`; removed text is struck through in `destructive`. Both marks are carried by more than colour (an underline and a strike-through), and both are reachable for screen readers as "added"/"removed" text in `aria-label`s. A toggle compares with the previous revision instead. **Restore** posts `…/revisions/{rid}/restore` with `If-Match`. A success toast offers **Undo**, which restores the revision that was current before (the newest one after the restore's own snapshot). A `412` goes through Task 16's conflict flow.
- Revisions are read-only for roles without `role_can_edit` (the API returns `404`; the entry is hidden when `allowed.edit` is false).

**Tests:** `revision_diff_marks_are_not_colour_only` (added parts render `<ins>` with an underline and an `aria-label`, removed parts `<del>` with a strike-through). E2E `revisions.spec.ts`: `three_saves_three_revisions_with_diff` (titles in the list, the diff marks the changed words); `restore_and_undo`; `contributor_sees_no_revisions_on_published` (R26); `screens` (drawer open, diff with additions and removals, mobile full-screen drawer).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): revisions drawer with word diff, restore and undo`.

### Task 18: Docs, ROADMAP and the M2b-1 exit check

**Files:**
- Create: `docs/superpowers/m2b1-exit.md`
- Modify: `docs/ROADMAP.md` (M2b: the split into M2b-1/2/3 with this plan's link; tick the design-system, scaffold and password-reset-page items; split the "Screens" item so the dashboard, content list and editor parts are ticked and the rest is named under M2b-2/M2b-3; the Playwright item stays open until M2b-3, both backends for every spec), `CLAUDE.md` (status: M2b-1 done; next M2b-2; the crate map gains `admin/`; the "Run a site locally" paragraph gains `pnpm -C admin install && pnpm -C admin build` before `cargo run`, and `pnpm -C admin dev` with `FERRITE_DEV_URL`), `docs/design/01-architecture.md` §1 (layout: `admin/`), `docs/design/07-admin-api.md` §3 (status per screen), `docs/design/10-testing.md` (as built), `README.md` (admin screenshot paths from the contact sheet are not committed; one sentence on building the admin)

**Interfaces:** none new.

**Exit check** (each line recorded in the exit doc with the command or the manual steps and what was seen; anything not verified listed under "Not verified"):
1. The workspace tests on both backends, with and without `admin/dist`.
2. The admin gates and `pnpm -C admin e2e` on SQLite and Postgres (smoke).
3. Manual: a fresh `init` + `seed demo` + `serve` on a release build. Sign in as each role, write and publish a post, schedule one, resolve a conflict between two browser windows, restore a revision, reset a password through a real SMTP catcher. Use Chrome and Safari (Safari only manually; Playwright WebKit is M2b-3).
4. The contact sheet reviewed against 11 §8 in full, with findings fixed or recorded as rulings.
5. Bundle sizes from `vite build` (the entry chunk, the editor chunk, fonts) recorded as the baseline for M2b-3's budget.
6. `docker build` with the admin stage; the image serves `/admin/`.

- [ ] **Step 1:** Run the exit checks and write `m2b1-exit.md`.
- [ ] **Step 2:** Update ROADMAP, CLAUDE.md and the design docs.
- [ ] **Step 3:** Commit `docs: M2b-1 exit, roadmap and status`.

## Plan rulings

- **P1 — M2b is three sub-plans** (M2b-1 this plan, 18 tasks; M2b-2 media and content modelling; M2b-3 administration, extensions and the M2 exit; scopes in the header). Writing comes first because it exercises the hardest shared parts (the API client, conflict handling, the editor) that later screens reuse. Cost: M2b-1 ships an admin without media, users or settings screens.
- **P2 — shadcn on Radix** (`shadcn init -b radix`), not Base UI, which shadcn made the default for new projects in July 2026. 07 §3 names Radix, shadcn keeps shipping every component for both, and Radix is the mature library. Cost: a later move to Base UI is a component-by-component re-add.
- **P3 — Inter Variable (with optical sizes) for UI, Geist Mono Variable for code and identifiers**, both OFL-1.1, from fontsource 5.3.0, self-hosted as hashed woff2 in the binary with fontsource's unicode-range subsets (Latin Extended and Cyrillic included for EU users). Inter's `opsz` axis gives display-cut headings and its tabular figures suit dense tables; Geist Mono is compact and pairs cleanly. Cost: about 0.8 MB of fonts in the binary (normal + italic Inter, Geist Mono normal), measured in Task 7.
- **P4 — The accent is "ember" (oxidised iron, oklch hue 40–52), on cool graphite neutrals.** A deliberate identity tied to the product name instead of shadcn's neutral default or the usual blue. The contrast test is the gate. Cost: orange accents read as "warning" to some users, so `warning` sits at hue 70–80 with its own icon and label, never colour alone.
- **P5 — TypeScript 5.9.3**, because the pinned typescript-eslint (`<6.1.0`) and openapi-typescript (`^5.x`) do not accept 6.x or 7.x. Cost: TS 7's faster checker waits for a later bump.
- **P6 — openapi-typescript + openapi-fetch** (as 07 §1 already decided), not hey-api. Types only and a ~6 KB fetch wrapper with middleware for CSRF and problems; no generated runtime code to review. Drift is checked by regenerating and diffing, not by a CLI flag. Cost: request bodies are not runtime-validated (the server validates).
- **P7 — Recharts 3 through the shadcn `chart` component** (07 §3, D12) for every chart. It themes from the same CSS variables, has an accessibility layer, renders SVG (crisp, CSP-clean apart from `ChartStyle`), and its composable API covers area, bar, radial and sparklines. Nivo (heavier per chart type), visx (too low-level for the screen count) and ECharts (canvas, a large bundle, its own theming) were considered. Pinned 3.10.1; the registry lists 3.8.0 (same major). Cost: Recharts' bundle weight (measured in Task 18, not assumed), so charts live in lazy route chunks.
- **P8 — The admin CSP allows `style-src 'unsafe-inline'` but no inline script and no eval.** `ChartStyle` and Sonner inject `<style>` elements, Motion may for `popLayout`, and a static embedded SPA has no per-request nonce. Inline *styles* cannot run script. Zod runs jitless and TipTap with `injectCSS: false`. Cost: CSS injection through a compromised dependency is not blocked.
- **P9 — Our own theme provider; no `next-themes`.** The shadcn `sonner` wrapper is edited to read it. Cost: one edited generated file (listed in 11 §7).
- **P10 — `routeTree.gen.ts` is committed** so `typecheck` works on a fresh clone before a build. Cost: generated-file diffs in route-adding commits.
- **P11 — Screenshots are review artifacts, not pixel baselines.** Every UI task's screens in light/dark × desktop/mobile go to a contact sheet that the reviewer checks against 11 §8; CI uploads it. macOS and Linux rasterise fonts differently, so `toHaveScreenshot` baselines would fail or be loosened into noise. Cost: no automatic visual regression; M2b-3 can add Linux-only baselines generated in CI if wanted.
- **P12 — The styleguide ships in the build as a lazy chunk at `/admin/_styleguide`, behind login and unlinked**, so E2E and reviewers see exactly the shipped components. Cost: a small chunk in the binary that any signed-in user can open (it holds no data).
- **P13 — Author references and allowed actions are added members on content and revision DTOs**, computed in core next to the lifecycle checks, so the SPA never re-implements role rules (CLAUDE.md: capabilities in core). Editors cannot list users (`manage_users`), and logins and display names are public on author archives anyway. Cost: one `users().get_many` per list page.
- **P14 — Status counts are a separate `GET /content/{type}/counts`** (six `count` queries with the list's own `readable_by`), not a field on every list response. Cost: one more request per list view, cached by TanStack Query.
- **P15 — Autosave is a per-(item, user) slot (`0007_autosaves`), not a revision.** Under 07 §3's "draft revision every 10 s" with PATCH, every autosave would write a revision (only 50 are kept, so about 8 minutes of history), queue `post.saved` plugin jobs, invalidate the page cache, and push unfinished edits of a published post live. The slot writes nothing else, survives reloads and devices, and is deleted by the owner's next real save. There is no retention job: at most one row per item and user, removed with either. Cost: an abandoned slot lingers until that user saves or discards the item.
- **P16 — The dashboard shows only data that exists.** Page views need the core counter and time series that M2a-2b P20 moved to M4, so they are not shown (07 §3 is updated in Task 12). Publishing activity is bucketed client-side from published items (at most 1 000). Site health is node-local. Cost: no traffic chart until M4.
- **P17 — Dashboard states that a seeded site cannot produce** (dead jobs, no activity) are covered by component tests with a stubbed API, not by E2E fixtures that would need test-only server hooks. Cost: those states are not exercised against the real server in M2b-1.
- **P18 — Headings in the body are levels 2–6; the title is the page's h1** (WordPress and Ghost do the same). A stored body with a level-1 heading opens read-only like any unsupported node, which keeps it intact. Cost: imported content with h1 blocks needs the M3 importer to demote them.
- **P19 — The editor's schema is exactly the core renderer's node and mark set**, and anything else opens read-only instead of being dropped or rewritten. Underline is disabled because the renderer has no `underline` mark. Cost: no underline, highlight, callouts or embeds until the renderer gains them (each is a core change plus an editor extension).
- **P20 — Bulk actions are client-side fan-out** (four requests at a time, each with its own `If-Match`, failures listed), with no bulk endpoint. Cost: bulk trashing 200 items is 200 requests.
- **P21 — Undo over confirm** (07 §3): trash, revision restore and bulk trash show an Undo toast; only permanent delete asks first. Cost: an undo is lost if the toast is missed (trash is still recoverable from the Trash tab).
- **P22 — One authenticated E2E browser (Chromium) in M2b-1, against a release build on SQLite, plus a Postgres smoke project in CI.** Firefox, WebKit and every spec on Postgres move to M2b-3 with the M2 exit. Cost: Safari-only bugs found manually until then (exit check 3).
- **P23 — The SMTP sink is a ~100-line `node:net` server in the test support code**, not a new dependency. `[mail]` accepts plain `smtp://` to a loopback relay (01 §4). Cost: no SMTP extensions are exercised (none are needed).
- **P24 — The admin sends `Referrer-Policy: no-referrer` on every response, not only the reset page.** A SPA keeps the policy of the document it first loaded, and the admin has no reason to leak its paths to other sites. Cost: the public site does not see the admin as a referrer when "View post" is clicked.
- **P25 — Screens with no API yet are not stubbed:** Import ships with M3, backups with M4. Cost: the sidebar grows as sub-plans land.
- **P26 — i18n from the start (`t()` everywhere, `en` only)**, with missing keys failing tests, because retrofitting strings across 30 screens later costs more than the discipline now. Cost: slightly slower UI tasks.

## Self-review notes

- Spec coverage. ROADMAP M2b "Design system first" → Tasks 7–8 (study notes, fonts, tokens, dark mode, motion), 10 (shortcuts), 11 (palette), the design review gate in Global Constraints and every UI task. Scaffold item → Tasks 1, 4–6 (Vite/React/TS, shadcn, TanStack, Motion, the generated client with the drift check, rust-embed with SPA fallback, immutable assets, placeholder, CSP). Screens in this sub-plan: dashboard (Recharts) → 12; content list → 13; TipTap editor, revisions, autosave, 412 UI → 14–17; the rest named in the header for M2b-2/3. The password-reset page (fragment, `replaceState`, `no-referrer`) → Tasks 1, 9. Comment pagination → M2b-2 (header). Playwright, Vitest and the CI admin job → Task 6 (both backends fully in M2b-3, P22). 07 §3 UX principles: undo over confirm (P21; Tasks 13, 15, 17), motion for meaning ≤ 200 ms with reduced motion (Task 7 §5, Task 10), dark mode via tokens (Task 7), ⌘K (Task 11). 07 §3 stack items: forms (`react-hook-form` + `zod`) in Tasks 9 and 15; tables (TanStack Table) in Tasks 8 and 13; editor (TipTap) in 14; icons (lucide) and toasts (sonner) in 8; `@dnd-kit` is not needed until menus (M2b-2); i18next in 4.
- API gaps found by mapping screens to endpoints: author names (editors cannot list users) → Task 2; per-item allowed actions (capabilities stay in core) → Task 2; status counts for tabs and tiles → Task 2; autosave without revision churn → Task 3; page views → P16 (M4). Everything else these screens need exists in `openapi.json` (auth, content and lifecycle, revisions, types, taxonomies and terms, system status, jobs, plugins and metrics, metrics summary, `/meta`, settings).
- Interfaces are defined once: `admin_routes`/`AdminAssets` (T1); `AllowedActions`, `StatusCounts`, `AuthorRef`, `/counts` (T2); `Autosave`, `AutosaveRepo`, the autosave endpoints (T3); `api`, `ApiError`, `qk`, `ifMatch` (T5); `startSite`, `snap`, `expectAccessible` (T6); tokens, `contrastRatio`, providers (T7); kit components (T8); `ensureSession`, `safeNext`, `takeResetToken` (T9); `navFor`, `registerShortcut` (T10); `searchContent` (T11); `bucketByDay` (T12); `ListSearch`, `runBulk` (T13); `editorExtensions`, `validateBody`, `blockText`, `diffWords`, `diffRevision` (T14; the diff helpers sit there because both T16's conflict dialog and T17's drawer use them); `createAutosaver`, `onPreconditionFailed`, `useLeaveGuard` (T16); `RevisionDiff` (T17).
- Proportion: the plan fixes names, values, tests and rulings; no function bodies are given except the exact header strings and token values the spec requires.
