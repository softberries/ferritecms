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

Design written (`docs/design/`). **Start with M0 (plugin host spike)** in `docs/ROADMAP.md` —
it is the riskiest part and must be proven before building CMS features around it.

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
