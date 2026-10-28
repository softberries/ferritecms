# CLAUDE.md — FerriteCMS

## What this project is

FerriteCMS: a WordPress-style CMS as a single Rust binary, with every plugin running in a
capability-scoped WebAssembly sandbox (wasmtime + Component Model). It imports WordPress
content (WXR) but deliberately does **not** run WordPress PHP plugins or themes.

Read before doing anything substantial:

- `docs/BRIEF.md` — product + technical design (source of truth for intent)
- `docs/design/` — detailed technical design (00-overview first; wins over BRIEF on the *how*)
- `docs/ROADMAP.md` — milestones and checklists; work top-down

## Current status

M0 (plugin host spike), M1a (storage), M1b (media), M1c-1 (config, CLI `init`/`migrate`,
core services), M1c-2 (router, themes, page cache, plugin wiring, `serve`, `seed demo`),
M1.5 (cluster mode: node registry, LISTEN/NOTIFY event bus, job queue, locks, themes as
artifacts, hot swap, shared plugin failure counters, compose example) and M2a-1 (REST API
core under `/api/v1` with OpenAPI, auth: argon2id, DB sessions + CSRF, scoped API tokens,
DB rate limits, TOTP, site key; users + `ferrite user`; content/types/taxonomies/menus/
redirects/settings endpoints; previews; logged-in requests; `sessions.prune`) and M2a-2a
(extensions and the plugin platform: themes/plugins API + `ferrite theme|plugin` with an
approval fingerprint per install, delete with 30-day data retention, plugin routes under
`/_p/`, theme and plugin settings with sealed secrets, `[mail]` + `notify-admins` +
password reset by e-mail, `plugin.action` jobs, batched `request.served`, the per-node call
cap and per-render budget) and M2a-2b (media uploads typed from their content, documents
served download-only, `Idempotency-Key`, the `media.variants` job, SVG sanitising; comments
with moderation and the theme form; the system API: status, nodes, audit, jobs with retry,
metrics; the real plugin `http` import with resolved-address SSRF checks; the URL
allowlist, the `__Host-` session cookie, zeroized keys; term archives for custom types; the
`test-seams` feature) are done, so M2a (the M2 backend) is complete; the exits are written
up in `docs/superpowers/m1-exit.md`, `m15-exit.md`, `m2a1-exit.md`, `m2a2a-exit.md` and
`m2a2b-exit.md` (each lists what was not verified). M2b-1 (the admin SPA's foundation
and writing: embedded `/admin/` with its CSP, design system (11-admin-ui), sign-in/TOTP/
reset screens, shell, `⌘K` palette, dashboard, content list, TipTap editor with publishing,
autosave slot, 412 conflict UI and revisions; API additions `allowed`, `author`, counts,
`0007_autosaves`) is done too (`m2b1-exit.md`), and so is M2b-2 (media and content
modelling: media library with drag-and-drop uploads, media picker, featured and inline
images, custom fields from the type schema, SEO fields, content types with the field
builder, taxonomy term trees, menus with `@dnd-kit`, the comment moderation queue,
redirects with CSV; public comment pages (`?cpage=N`); API additions `seo`, media
`q`/`kind`/`trash`/restore, comment `item` and counts, menu `locations`,
`GET /content/refs`, `0008_autosave_seo`; `m2b2-exit.md`), and so is M2b-3
(administration and extensions: account with TOTP enrolment, tokens and sessions; users;
settings; themes with in-browser bundle packing; plugins with the permission report,
write-only secrets and metrics; system with status, nodes, jobs and the audit log; every E2E
spec in Chromium on both backends, Firefox and WebKit; the bundle budget; API additions
user filters, sessions, admin TOTP reset, retained plugin data, audit filters, the test mail,
relation `types`, inline image `srcset`; `ferrite theme pack`; no migration;
`m2b3-exit.md`, verified in two parts, the browser and Docker checks in part B), so M2 is
done (`m2-exit.md`). Open follow-ups stay listed in the roadmap. Next is M3 (WordPress
import). Work top-down in `docs/ROADMAP.md`; per-milestone rulings are in
`docs/superpowers/rulings/` (M2a-2a: `m2a2.md`, M2a-2b: `m2a2b.md`, M2b: `m2b.md`).

Crate map (`crates/`): `ferrite` (bin: CLI, node wiring, hot-swap reloader, job handlers)
-> `ferrite-http` (public site, router, head, previews and logged-in requests, plugin
wiring, plugin reloader, shared failure counters, plugin routes, `request.served` buffer,
SMTP mailer, the extension install pipeline `extensions::Installer`; the REST API in
`src/api/` with its OpenAPI snapshot `openapi.json`) -> `ferrite-cache` (page cache + outbox invalidation
loop with LISTEN wake-ups), `ferrite-theme` (themes, bundles, hierarchy, MiniJinja
renderer), `ferrite-core` (services, permalinks, `SiteReader`, `ExtensionService` +
`ExtensionAdmin`, `ext_settings`, `mail` (`Mailer`, `MailQueue`), action jobs;
`AuthService` (incl. password reset), `UserService`, content types, settings allowlist,
roles + token scopes), `ferrite-media`, `ferrite-plugin-host` (grants, permission report;
-> `ferrite-domain`);
`ferrite` also -> `ferrite-cluster` (node registry, job runner, periodic scheduler, built-in
prune jobs) -> `ferrite-store` (traits); `ferrite-core` -> `ferrite-store` (traits) <-
`ferrite-store-sqlite` / `ferrite-store-postgres` (+ `ferrite-store-tests` contract suite);
`ferrite-domain` (types), `ferrite-config`, `ferrite-sdk` (guest SDK). Starter theme:
`themes/blog` (embedded). Test plugins: `plugins/examples/` (own workspace, built for
`wasm32-wasip2`). Admin SPA: `admin/` (pnpm, Vite + React + TS; `pnpm build` writes
`admin/dist/`, which `ferrite-http`'s `admin` module embeds; without it the binary serves a
placeholder page; Vitest + Playwright E2E, see `admin/README.md`). Not built yet: the import
screen and import-wp (M3), backups (M4) (`docs/design/01-architecture.md` §1; API status per
endpoint in 07 §1, admin status per screen in 07 §3).

Run a site locally: build the admin first (`pnpm -C admin install && pnpm -C admin build`;
needs Node >= 24.18 and pnpm 10.28.2; skip it for the placeholder page), then
`cargo run -p ferrite -- init --data /tmp/site --url http://localhost:8080` (for a solo site
it also creates the site key `<data>/secret.key`), then optionally
`cargo run -p ferrite -- seed demo --data /tmp/site` (sample content), then an account:
`cargo run -p ferrite -- user create --data /tmp/site --login me --email me@example.com`
(prints a one-time password that must be changed at the first login; `--password-stdin`
reads one from stdin instead; seed's own `admin` has no password until
`user reset-password --data /tmp/site --login admin`), then
`cargo run -p ferrite -- serve --data /tmp/site` (prints `Listening on http://…`; stop with
Ctrl-C/SIGTERM). The admin is at `http://localhost:8080/admin/`; for admin work run
`pnpm -C admin dev` (Vite on `:5173`, `http://localhost:5173/admin/`, proxying `/api` and
`/_preview` to `FERRITE_DEV_URL`, default `http://localhost:8080`). The API is at `http://localhost:8080/api/v1` (spec at
`/api/v1/openapi.json`): log in with `POST /api/v1/auth/login` and an `Origin:
http://localhost:8080` header, then send the cookie plus `X-CSRF-Token` on writes, or use a
bearer token from `POST /api/v1/users/{id}/tokens`. `cargo run -p ferrite -- migrate status
--data /tmp/site` exits 0 ok, 2 pending, 3 incompatible, 1 error; with SQLite it (and `seed`
and `user`) fails while `serve` holds the data-folder lock. `FERRITE_LOG=debug` sets log
verbosity; `FERRITE_<SECTION>__<KEY>` overrides config (e.g.
`FERRITE_SERVER__LISTEN=127.0.0.1:0` for a free port).
`cargo run -p ferrite -- config check --data /tmp/site` validates config + env without
touching the database (exit 0 prints `ok: mode=… database=… media=… workers=… mail=…`, exit 1 lists
problems).
Themes and plugins: `ferrite theme install|activate|list` and `ferrite plugin
check|install|list|enable|disable|remove` (`--dir` holds `ferrite-plugin.toml` and one
`*.wasm`; with SQLite `serve` must be stopped), or the API (`/api/v1/themes` takes the bundle
`ferrite theme pack --dir DIR --out FILE` writes, without a data folder;
`/api/v1/plugins`: inspect, then install with the report's `fingerprint` as `approve`), or
the admin's Appearance and Plugins screens. Mail (password reset, plugin `notify-admins`, the
admin's test mail) needs `[mail] smtp_url` + `from` (01 §4).

Run two cluster nodes locally: two data folders whose `ferrite.toml` share `site.url`,
`database.url` (Postgres) + `database.schema`, and `[media] root = "<shared dir>"` with
`local_shared = true`, `cluster.mode = "cluster"`, different `server.listen` ports and
`cluster.node_name`s, and the same site key in `FERRITE_AUTH__SECRET_KEY` (64 hex
characters, e.g. `openssl rand -hex 32`; cluster mode refuses to start without one); `init`
the first folder with `--db postgres://… --schema …`, then
`serve` both. Docker example (not run in CI, only validated): `examples/cluster/`
(`docker compose -f examples/cluster/docker-compose.yml up --build`; design 09 §7).
Cluster integration tests: `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test cluster`
(serialised in-process; about 30 s).

Two deployment profiles must both work: **solo** (SQLite + local media, one node) and
**cluster** (Postgres + S3-compatible media, N nodes). See `docs/design/00-overview.md`.

## Principles (don't violate without discussing)

1. **Single binary, single data folder.** No external services required to run a site on
   the default profile. Postgres/S3 are opt-in; the database is the only coordination service.
2. **Plugins are untrusted.** They get only the WIT imports their manifest declares.
   No raw SQL, no filesystem, no shared memory, no ambient network. Every hook call is
   bounded by memory, fuel and an epoch deadline. A failing plugin must never break a page.
3. **Themes don't execute code.** Templates get data from core plus a fixed set of safe
   helpers. Logic belongs in plugins.
4. **WIT files in `wit/` are the source of truth** for the plugin API. Version them;
   breaking changes need a new world version.
5. **WordPress familiarity, not compatibility.** Mirror WP concepts (hooks with
   priorities, template hierarchy, roles, post types) so WP devs feel at home.

## Conventions

- Rust stable, edition 2024, Cargo workspace under `crates/` (layout in `docs/design/01-architecture.md` §1).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` must pass.
- Errors: `thiserror` in libraries, `anyhow` only in the binary crate.
- Async: `tokio`. Logging: `tracing`.
- SQL migrations live in `crates/ferrite-store-sqlite/migrations/` and
  `crates/ferrite-store-postgres/migrations/` — same versions/names on both, every migration
  has a `.down.sql`, expand-only unless marked `-- ferrite:phase contract` (design 02 §7–8).
- Storage changes need contract tests on **both** backends.
- Tests: unit tests next to code; integration tests in `tests/`. Plugin-host tests must
  include hostile plugins (infinite loop, OOM, panic, undeclared import).
- Postgres-backed store tests: set `FERRITE_TEST_PG_URL` to any Postgres >= 15, e.g.
  `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:5432/ferrite_test cargo test --workspace`.
  Each test creates and drops its own schema, so a shared database is fine. Without the
  variable the Postgres cases print `SKIP` and pass; CI (which sets `CI=true`) panics if
  it is missing and runs a `postgres:18` service.
- API changes: DTOs in `crates/ferrite-http/src/api/` with `utoipa` annotations; regenerate the
  checked-in spec with `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`
  (`openapi_snapshot_is_current` fails on drift). Capabilities are checked in core, never in
  handlers.
- Prefer small, reviewable commits per checklist item.

## Naming

- Product: FerriteCMS. Binary/CLI: `ferrite`. Crates: `ferrite-*`.
- Plugin manifest: `ferrite-plugin.toml`. Site config: `ferrite.toml`. DB: `ferrite.db`.
- Name is provisional — keep it easy to rename (no name baked into data formats beyond
  the file names above).
