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
core services) and M1c-2 (router, themes, page cache, plugin wiring, `serve`, `seed demo`) are
done, and the M1 exit criterion is verified (`docs/superpowers/m1-exit.md`). Open M1
follow-ups (deferred minors) stay listed under M1 in the roadmap. Next is M1.5 (cluster mode). Work top-down in
`docs/ROADMAP.md`; per-milestone rulings are in `docs/superpowers/rulings/`.

Crate map (`crates/`): `ferrite` (bin: CLI) -> `ferrite-http` (public site, router, head,
plugin wiring) -> `ferrite-cache` (page cache + outbox invalidation loop), `ferrite-theme`
(themes, hierarchy, MiniJinja renderer), `ferrite-core` (services, permalinks, `SiteReader`),
`ferrite-media`, `ferrite-plugin-host`; `ferrite-core` -> `ferrite-store` (traits) <-
`ferrite-store-sqlite` / `ferrite-store-postgres` (+ `ferrite-store-tests` contract suite);
`ferrite-domain` (types), `ferrite-config`, `ferrite-sdk` (guest SDK). Starter theme:
`themes/blog` (embedded). Not built yet: cluster, import-wp, REST API/admin
(`docs/design/01-architecture.md` §1).

Run a site locally:
`cargo run -p ferrite -- init --data /tmp/site --url http://localhost:8080`, then
`cargo run -p ferrite -- seed demo --data /tmp/site` (optional sample content), then
`cargo run -p ferrite -- serve --data /tmp/site` (prints `Listening on http://…`; stop with
Ctrl-C/SIGTERM). `cargo run -p ferrite -- migrate status --data /tmp/site` exits 0 ok,
2 pending, 3 incompatible, 1 error; with SQLite it (and `seed`) fails while `serve` holds the
data-folder lock. `FERRITE_LOG=debug` sets log verbosity; `FERRITE_<SECTION>__<KEY>` overrides
config (e.g. `FERRITE_SERVER__LISTEN=127.0.0.1:0` for a free port).

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
- Prefer small, reviewable commits per checklist item.

## Naming

- Product: FerriteCMS. Binary/CLI: `ferrite`. Crates: `ferrite-*`.
- Plugin manifest: `ferrite-plugin.toml`. Site config: `ferrite.toml`. DB: `ferrite.db`.
- Name is provisional — keep it easy to rename (no name baked into data formats beyond
  the file names above).
