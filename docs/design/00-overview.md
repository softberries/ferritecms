# FerriteCMS — Design Overview

Status: **draft for review** · Date: 2026-10-22

This folder is the detailed technical design. `docs/BRIEF.md` stays the product brief
(the *why*); these documents are the *how*. Where they disagree, these documents win and
the brief gets updated.

## Documents

| # | Document | Covers |
|---|---|---|
| 01 | [Architecture](01-architecture.md) | Crates, runtime components, request lifecycle, config, error model |
| 02 | [Storage & migrations](02-storage.md) | SQLite + Postgres backends, schema, migrations, upgrade compatibility |
| 03 | [Media storage](03-media.md) | `MediaStore`: local folder and S3-compatible, variants, serving |
| 04 | [Multi-instance](04-cluster.md) | Running N nodes: cache invalidation, sessions, jobs, artifact sync |
| 05 | [Plugin host](05-plugin-host.md) | WIT world, capabilities, limits, failure isolation, SDK |
| 06 | [Themes & rendering](06-themes-rendering.md) | Template hierarchy, MiniJinja env, page cache, dependency tracking |
| 07 | [Admin & API](07-admin-api.md) | REST API v1, auth, roles, React admin SPA |
| 08 | [WordPress import](08-wp-import.md) | WXR parsing, mapping, media, redirects, report |
| 09 | [Operations](09-operations.md) | Upgrade, rollback, backup/restore, TLS, metrics, packaging |
| 10 | [Testing strategy](10-testing.md) | Test layers, dual-backend contract tests, hostile plugins, CI |

## Goals

1. One binary. A site on the default setup is one binary and one data folder.
2. Plugins are untrusted. They run in a WASM sandbox with explicit capabilities and a
   budget per call. A failing plugin never breaks a page.
3. WordPress developers feel at home: hooks with priorities, a template hierarchy, roles
   and post types.
4. WordPress content can be imported with SEO intact (301 redirects).
5. **Scale-out is an option, not a rewrite.** The same binary can run with Postgres +
   S3-compatible object storage as N stateless nodes behind a load balancer.
6. **Upgrades are reversible.** A bad upgrade can be undone: binary and schema.

## Non-goals (v1)

- Running WordPress PHP plugins or themes.
- Page builder, e-commerce, multi-tenant SaaS (one site per process / cluster).
- Requiring any external service on the default (SQLite + local media) setup.
- Redis, message brokers or any coordination service other than the database.

## Anti-goals (what would make this a failure even if it works)

- A plugin can read data it did not declare, or crash/hang a page render.
- The SQLite single-folder experience gets worse because of Postgres support (extra
  config, extra processes, slower startup).
- Postgres is a second-class backend that silently lacks features or passes fewer tests.
- An upgrade leaves a site in a state that neither the new nor the old binary can run.

## Deployment profiles

The design is organised around two supported profiles. Each feature must work in both.

| | **Solo** (default) | **Cluster** |
|---|---|---|
| Database | SQLite (WAL) in `data/ferrite.db` | PostgreSQL (managed or self-hosted) |
| Media | Local folder `data/media/` | S3-compatible bucket |
| Nodes | Exactly 1 (enforced by lock file) | 1..N stateless nodes |
| Coordination | In-process | Via Postgres (`LISTEN/NOTIFY`, `SKIP LOCKED`, advisory locks) |
| Backup | `ferrite backup` (folder/archive) | Provider snapshots/PITR + `ferrite export` |
| Typical user | Self-hoster, agency with many small sites | Bigger sites, hosting providers |

Mixed setups are allowed where they make sense: Postgres + local media with a single
node, or SQLite + S3 media with a single node. Unsafe combinations are rejected at
startup (see [04](04-cluster.md#startup-validation)).

## Decision log

Decisions taken during design discussion. "User" = decided by the project owner;
"Design" = a call made in this design, open to challenge on review.

| # | Decision | By | Doc |
|---|---|---|---|
| D1 | Support SQLite **and** PostgreSQL from M1, both first-class | User | 02 |
| D2 | Reversible migrations (`up`/`down`) with rollback tooling | User | 02, 09 |
| D3 | Rollback priority: undoing a bad upgrade first, point-in-time content restore second | User | 09 |
| D4 | S3-compatible media storage from the beginning (M1), not "later" | User | 03 |
| D5 | Design for multiple instances from the start | User | 04 |
| D6 | Admin UI: React + shadcn/ui + a good chart library + transitions | User | 07 |
| D7 | The database is the only coordination service (no Redis) | Design | 04 |
| D8 | Expand/contract migration policy: a release only adds schema; destructive changes ship one release later | Design | 02 |
| D9 | Runtime-checked sqlx queries + one repository impl per backend + shared contract tests (no compile-time `query!` macros) | Design | 02 |
| D10 | `object_store` crate as the media backend abstraction | Design | 03 |
| D11 | Plugins and themes are stored as content-addressed artifacts in the media store and recorded in the DB, so every node runs the same set | Design | 04, 05 |
| D12 | Admin stack: Vite + React + TypeScript, TanStack Router/Query, shadcn/ui, Recharts (via shadcn chart), Motion, TipTap | Design | 07 |
| D13 | Plugin hooks use a generic typed dispatch (`apply-filter(name, value)`), not one export per hook | Design | 05 |
| D14 | Portable export format (`ferrite export`/`import-site`) doubles as backup on Postgres and as SQLite↔Postgres migration | Design | 02, 09 |
| D15 | IDs are UUIDv7 | Design | 02 |
| D16 | Action hook payloads are JSON (`action-event { subject, payload }`) so new actions are additive, not breaking | Design | 05 |
| D17 | `content.read` sees published public items only (plus the item being previewed); plugin reads record cache dependencies; kv/http/clock use marks a page volatile (short TTL) | Design | 05, 06 |
| D18 | Forward upgrades may skip versions; binary rollback only to the immediately previous binary | Design | 09 |
| D20 | Cluster mode is **working and tested** in the first release (not only designed for) | User | 04, ROADMAP |
| D21 | Plugin routes (`/_p/<plugin>/…`), declarative plugin settings, `mail` (admins only) and batched `request.served` are in plugin API v0.1 | User | 05 |
| D19 | No WP password-hash verification in v1 (WXR has no hashes) — imported users reset passwords | Design | 07, 08 |

## Verified facts this design relies on

Per project rules, claims about third-party libraries were checked on 2026-10-22.

| Claim | Source | Verified |
|---|---|---|
| sqlx supports reversible migrations and `Migrator::undo(conn, target)` | docs.rs sqlx `Migrator` (via Context7) | Yes |
| sqlx migrations lock the DB by default (`set_locking`, default `true`); Postgres uses advisory locks | docs.rs sqlx `Migrator::set_locking` | Yes |
| sqlx exposes `PgAdvisoryLock` for app-level advisory locks | docs.rs `sqlx::postgres::PgAdvisoryLock` | Yes |
| wasmtime `StoreLimitsBuilder::memory_size` caps linear memory per store | docs.rs wasmtime | Yes |
| wasmtime supports fuel and epoch deadlines, incl. async yield on epoch (`epoch_deadline_async_yield_and_update`) | docs.rs wasmtime | Yes |
| `object_store` `AmazonS3Builder::with_endpoint` supports S3-compatible endpoints; conditional put works on R2/MinIO | arrow-rs-object-store docs | Yes |
| shadcn/ui `chart` component is built on Recharts v3 | shadcn-ui/ui registry (`recharts@3.8.0`) | Yes |
| shadcn-svelte `chart` is built on LayerChart (considered, not chosen) | shadcn-svelte docs | Yes |
| Latest crate versions (cargo search, 2026-10-22): wasmtime 50.0.0, sqlx 0.9.0, axum 0.8.9, minijinja 3.0.2, object_store 0.14.2, tokio 1.54.1, wit-bindgen 0.63.0, utoipa 6.0.0, rust-embed 8.13.0 | `cargo search` | Yes |
| Local toolchain: rustc 1.98.1, Node 24.18.0, pnpm 10.28.2, Docker 29.8.1, psql 18.1 | local commands | Yes |
| "~96% of WP security issues come from plugins" (Patchstack) | quoted in BRIEF | **Not re-verified** — check before any public use |
| MiniJinja 3.x has a `fuel` feature to bound template execution | memory | **Not verified** — confirm in M1 |
| Motion (formerly Framer Motion) is published as the `motion` npm package | memory | **Not verified** — confirm at admin scaffold |
| Yoast / Rank Math / ACF meta key names in 08 §3 | memory | **Not verified** — confirm against real exports in M3 |
| `wasm32-wasip2` target builds components directly; unused WIT imports are dropped | memory | **Not verified** — M0 task |

Exact API shapes (e.g. wasmtime component async entry points in v50) must be confirmed
against the pinned version when implementing; the M0 spike exists to do exactly that.

## Glossary

- **Node** — one running `ferrite serve` process.
- **Artifact** — an immutable, content-addressed blob (plugin `.wasm`, theme bundle,
  media original or variant).
- **Dependency key** — a string identifying data a rendered page used
  (`post:<id>`, `term:<id>`, `menu:<name>`, `option:<key>`, `theme`, `plugins`).
- **Hook** — a named extension point; *filter* (value in → value out) or *action*
  (event, no return).
- **Capability** — a permission declared in a plugin manifest that causes the host to
  link a WIT interface and/or allow a scope within it.
