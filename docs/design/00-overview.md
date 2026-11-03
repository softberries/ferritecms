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
| 11 | [Admin UI design language](11-admin-ui.md) | Admin study notes, typography, colour tokens and contrast gate, density, motion, keyboard map, components, review checklist |
| 12 | [AI assistants: the MCP server](12-mcp.md) | `/mcp` (Streamable HTTP, stateless), the built-in OAuth 2.1 authorization server and audience-bound tokens, scopes, tools/resources/prompts, what stays human, the off switch, admin screens, the product story |
| 13 | [Multilingual content](13-multilingual.md) | `site.locales`, translation groups, locale prefixes in permalinks and the router, locale-scoped reads, hreflang and sitemaps, theme helpers, the API, MCP, admin and import (M6-1) |

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

Implementation-level rulings taken while building each milestone (smaller than these
decisions, each with its reason and cost) are recorded per milestone in
[`docs/superpowers/rulings/`](../superpowers/rulings/): `m1a.md`, `m1b.md`, `m1c1.md`,
`m1c2.md`, `m15.md`, `m2a.md`. Two M1c-2 rulings narrowed decisions above for M1: themes loaded from
the embedded starter themes and `data/themes/` folders, with artifact distribution (D11)
moved to M1.5 (plan ruling P1); plugins loaded from the `plugins` table and their artifacts
at startup only, with hot reload in M1.5 (plan ruling P3). M1.5 delivered both
([`m15.md`](../superpowers/rulings/m15.md)) and refines D7 and D11:

- **M1.5 P1:** the coordination mechanism follows the *database backend*, not
  `cluster.mode`. Postgres always uses `SKIP LOCKED` job claims, advisory locks and
  `NOTIFY` (even for a solo node, because `ferrite migrate` or a second node can reach the
  same database); SQLite uses the single-writer claim, an in-process mutex and no
  subscription. `cluster.mode` only decides whether a node listens for notifications, the
  solo single-live-node rule and whether `data/themes/` folders are read.
- **M1.5 P17:** theme artifacts (D11) are plain deterministic ustar bundles
  (`application/x-tar`), not `tar.zst`; in cluster mode `data/themes/` is ignored so every
  node runs the artifact recorded in the database.

M2a-1 (the REST API core and auth; [`m2a.md`](../superpowers/rulings/m2a.md)) adds three
rulings at this level:

- **M2a P1:** M2 is split into M2a-1 (auth, API conventions, users, content, types,
  taxonomies, menus, redirects, settings endpoints, previews, logged-in requests), M2a-2
  (extensions and plugin API extras, media, comments, system endpoints, hardening) and M2b
  (the admin SPA, D6/D12), each with its own plan.
- **M2a P4 (refines D7):** the site key that seals secrets (TOTP, then password-reset tokens and plugin
  secrets in M2a-2a) is the one piece of shared state that is **not** in the database:
  `auth.secret_key[_file]`, required in cluster mode; a solo site generates
  `<data>/secret.key`. A database dump or export therefore never carries the key that
  opens the secrets in it.
- **M2a P5 (applies D7):** rate limits are fixed windows counted in the database
  (`rate_limits`, one upsert on the database clock), not an in-process limiter such as
  `tower_governor`, so limits hold across nodes; a fixed window admits up to 2× the limit
  across its edge.

M2a-2a (extensions and the plugin platform; [`m2a2.md`](../superpowers/rulings/m2a2.md))
adds two at this level:

- **M2a-2 P1:** M2a-2 is split into M2a-2a (extension APIs and CLI, install approval,
  plugin routes, settings, mail, password reset, action jobs) and M2a-2b (media,
  comments, system API, hardening).
- **M2a-2 P3 (applies principle 2, "plugins are untrusted"):** an admin approves a plugin by the fingerprint
  of the exact permission report it saw (plugin, version, artifact hash, grant), so an
  approval never covers other bytes or a wider grant, and a widening update without a
  new approval is refused while the old version keeps running.

M2a-2b (media, comments, system API, hardening; [`m2a2b.md`](../superpowers/rulings/m2a2b.md))
adds two at this level:

- **M2a-2b R1 (applies WordPress familiarity):** uploads accept common documents (docx,
  xlsx, pptx, odt, ods, odp, zip, txt, csv) next to images, audio, video, PDF and sanitised
  SVG; the type comes from the content, and documents are always served download-only
  (`attachment` + `nosniff`) so they cannot run in the site's origin (03 §3).
- **M2a-2b P16 (applies principle 2):** the plugin `http` import connects only to the
  addresses its own DNS lookup checked (every address public, `https` on 443, redirects
  re-checked against the plugin's list per hop), so an allowlisted name cannot be pointed
  at internal services (05 §6).

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
| Motion (formerly Framer Motion) is published as the `motion` npm package, React API under `motion/react` | npm registry (`motion@14.1.0`) + Context7 (motion.dev upgrade guide), 2026-10-25 | Yes |
| Admin toolchain installs and builds with the exact pins (Vite 8.3.4, React 19.3.0, TypeScript 5.9.3, Tailwind 4.3.3, shadcn 4.21.4 `radix-nova`, TanStack Router 1.170.42, Vitest 5.0.3, ESLint 10.12.0); `vite build` emits no inline script and no `data:` URL | `admin/package.json` + `pnpm-lock.yaml`, `pnpm -C admin build` (`scripts/check-dist.mjs`), 2026-10-25 | Yes |
| Yoast / Rank Math / ACF meta key names in 08 §3 | the plugins' source and exports A (Yoast, ACF) and B (Rank Math) from a scratch WordPress 7.1.3 (`crates/ferrite-import-wp/tests/corpus/`), M3-1, 2026-10-28; re-checked against the corpus files, 2026-10-29 | Yes |
| `wasm32-wasip2` target builds components directly; unused WIT imports are dropped | M0 spike + tests (`hello` imports exactly `log`; hostile `undeclared-import`) | Yes |

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
