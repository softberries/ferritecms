# 01 — Architecture

## 1. Workspace layout

The brief's layout (§7) is extended with storage, media and cluster crates. Dependencies
point downwards only; no cycles.

Status after M1c-2: crates marked **(built)** exist in `crates/` and are workspace members
(`Cargo.toml`); the others are planned and carry their milestone.

```
ferritecms/
├── Cargo.toml                    # workspace, shared [workspace.dependencies], lints
├── rust-toolchain.toml           # stable, edition 2024
├── crates/
│   ├── ferrite/                  # (built) BIN: clap CLI (`init`, `migrate`, `serve`, `seed demo`),
│   │                             #   logging, startup check, shutdown; anyhow at the top level
│   ├── ferrite-config/           # (built) ferrite.toml + env parsing, validation, init template
│   ├── ferrite-domain/           # (built) pure types: ids, Post, Term, User, Role, Block, errors
│   ├── ferrite-store/            # (built) repository traits + Store facade (no SQL)
│   ├── ferrite-store-sqlite/     # (built) SQLite impl + migrations/
│   ├── ferrite-store-postgres/   # (built) Postgres impl + migrations/
│   ├── ferrite-store-tests/      # (built) contract test suite run against both backends;
│   │                             #   other crates reuse its harness as a dev-dependency
│   ├── ferrite-media/            # (built) MediaStore over object_store, sniffing, image variants
│   ├── ferrite-cluster/          # (built, M1.5) job runner, periodic scheduler, built-in
│   │                             #   prune jobs; node registry (M1.5, planned)
│   ├── ferrite-core/             # (built) services: content (+ lifecycle, revisions, redirects on
│   │                             #   permalink change, search indexing), taxonomy, menus, options,
│   │                             #   redirects; permalinks; the published-only read facade
│   │                             #   `SiteReader` (+ preview); blocks rendering, slugs, events and
│   │                             #   the dependency-key contract, roles/capabilities. Depends on
│   │                             #   domain + store only; users/auth and media services are M2
│   ├── ferrite-cache/            # (built, M1c-2) page cache (moka) + dependency index + outbox
│   │                             #   invalidation loop
│   ├── ferrite-theme/            # (built, M1c-2) theme packages, hierarchy resolution, sandboxed
│   │                             #   MiniJinja renderer and helpers
│   ├── ferrite-plugin-host/      # (built) wasmtime engine, capability linker, limits, registry,
│   │                             #   output validation + ammonia sanitising, call tracing
│   ├── ferrite-http/             # (built, M1c-2: public site, router, head, plugin wiring;
│   │                             #   M2: REST API v1, auth, admin assets)
│   ├── ferrite-import-wp/        # (M3) WXR importer
│   └── ferrite-sdk/              # (built, M0) guest SDK for Rust plugins (`export_plugin!` macro
│                                 #   lives in the crate; there is no separate macros crate)
├── wit/                          # WIT packages (source of truth for plugin API)
│   └── ferrite-plugin@0.1.0/
├── admin/                        # (M2) React SPA (pnpm workspace), built into dist/
├── themes/                       # (built) starter themes embedded in the binary: `blog`
├── plugins/examples/             # (built) hello, order-a, order-b, clock, html-inject,
│                                 #   meta-writer, reader, hostile/*; seo, analytics,
│                                 #   contact-form are M4
├── tests/                        # cross-crate integration tests (planned; per-crate tests/ exist)
└── docs/
```

> Migrations live in each store crate (`crates/ferrite-store-*/migrations/`); CLAUDE.md
> says the same.

### Dependency graph

As built after M1c-2 (normal dependencies, from each crate's `Cargo.toml`; crates not yet
built are marked with their milestone):

```
ferrite (bin) ── ferrite-http, ferrite-cache, ferrite-theme, ferrite-core, ferrite-plugin-host,
 │               ferrite-media, ferrite-config, ferrite-store, ferrite-store-{sqlite,postgres},
 │               ferrite-domain
 ├─ ferrite-http ─┬─ ferrite-cache ─── ferrite-store, ferrite-domain
 │                ├─ ferrite-theme ─── ferrite-core, ferrite-domain
 │                ├─ ferrite-core ──── ferrite-store (traits) ◄── ferrite-store-{sqlite,postgres}
 │                │                    └─ ferrite-domain
 │                ├─ ferrite-media ─── ferrite-domain
 │                ├─ ferrite-plugin-host (no ferrite-* dependencies)
 │                └─ ferrite-store, ferrite-domain
 ├─ ferrite-config ─── ferrite-domain
 ├─ ferrite-cluster ─── ferrite-store (traits), ferrite-domain (not yet used by the binary)
 └─ ferrite-import-wp (M3) ─── ferrite-core
```

`ferrite-core` does not use `ferrite-media` (media URLs reach `SiteReader` as a closure built
by the binary from `MediaStore::public_url`). `ferrite-plugin-host` keeps no dependency on
core; `ferrite-http` adapts between them (`StoreHostServices`). `ferrite-cache` uses
`ferrite-core` only in its tests.

Only the binary crate depends on concrete store crates; everything else sees
`Arc<dyn Store>` (see [02](02-storage.md#repository-layer)). Both store crates are always
compiled in; Cargo features to leave one out (`sqlite`, `postgres`) are planned but do not
exist yet (no `[features]` section in any crate).

## 2. Runtime components

One `ferrite serve` process contains (as built after M1c-2; planned parts marked):

| Component | Responsibility | Crate |
|---|---|---|
| Config | Load `ferrite.toml`, env overrides (`FERRITE_*`), validate profile | ferrite-config |
| Store | Pooled DB access, transactions, migrations | ferrite-store-* |
| MediaStore | Blob get/put/list/delete, public URLs | ferrite-media |
| Cluster *(M1.5)* | Node heartbeat, `LISTEN/NOTIFY` event bus, job queue, distributed locks | ferrite-cluster |
| Core services | Business rules, permission checks, revisions, emitting events; `SiteReader` for public reads | ferrite-core |
| Plugin host | Load components, run filters/actions under limits | ferrite-plugin-host |
| Theme engine | Resolve template, render with MiniJinja | ferrite-theme |
| Page cache | URL → rendered response, dependency-based invalidation | ferrite-cache |
| Invalidation loop | Polls the `events` outbox every `cache.poll_interval_ms` (default 250) and applies events to the page cache | ferrite-cache |
| Scheduled publishing | `publish_due` every 30 s (a tokio task; the job queue replaces it in M1.5) | ferrite (bin) + core |
| HTTP | Public routes, `/_media`, `/_theme`, `/healthz`, `/readyz`; later `/api/v1`, `/admin`, `/_p/<plugin>`, `/metrics` | ferrite-http |
| Workers *(M1.5)* | Job queue consumers (actions, variants, imports) | ferrite-cluster + core |

Startup order as built (`crates/ferrite/src/serve.rs`; any failure exits 1): config → open
the database → migration check (pending migrations applied when allowed, an incompatible
schema refused) → media store write/read/delete probe → permalink rules from options,
checked against types and taxonomies → active theme (falls back to the embedded `blog`) →
plugins from the `plugins` table + artifacts (a broken row is skipped) → route table →
invalidation loop (cursor set before any page is cached) → scheduled-publish ticker →
signal handlers → bind `server.listen` and print `Listening on http://<addr>`. Planned
additions: cluster register (M1.5), workers (M1.5).

Shutdown on SIGINT or SIGTERM, as built: stop accepting → drain open requests for up to 10 s
→ stop the background tasks (up to 1 s) → close the database pools (up to 2 s) → the
runtime gets a 1 s grace, then remaining connection tasks are dropped. The process is gone
about 14 s after the first signal at most; a second signal during the drain exits at once
(130 for SIGINT, 143 for SIGTERM). Planned: stop workers after the current job, deregister
the node (M1.5).

## 3. Request lifecycle (public page)

As built (`crates/ferrite-http/src/app/handler.rs`, `pipeline.rs`):

```
request ─► GET/HEAD? ──no──► 405
              │
              ▼
          router (pure) ─► /healthz, /readyz, /_media, /_theme ──► answered directly
              │
              ▼
          redirect table? ──yes──► 301/302/307/308 (410: shared gone page)
              │no
              ▼
          non-canonical path? ──yes──► 301 to the canonical path
              │no
              ▼
          page cache lookup (key: site host + path + allowlisted query + variant)
              │hit ──► respond (weak ETag, 304 support)
              │miss (single flight)
              ▼
          SiteReader (published-only) ─► dependency keys recorded
              ▼
          plugin filters: content.body, content.excerpt, render.head (traced)
              ▼
          theme: resolve template by hierarchy → render (MiniJinja, blocking thread)
              ▼
          store in cache with the dependency keys (unless an error status) → respond
```

- **Cache variant**: anonymous only in M1. There is no login yet, so every request is
  anonymous; logged-in bypass or per-user variants arrive with auth (M2).
- **Single-flight**: concurrent misses for the same key wait on one render
  (per-node; cross-node duplication is acceptable).
- **Failure**: a plugin hook failure skips that hook ([05](05-plugin-host.md#8-failure-isolation-p5-p6))
  and gives the page a short TTL. A theme render error returns a 500 rendered via the theme's
  `500.html` if present, otherwise a built-in minimal page; the server keeps serving. Admin
  reachability with a broken theme applies once the admin exists (M2).
- Details: [06 §3](06-themes-rendering.md#3-routing) (routing), §5 (pipeline), §6 (cache).

## 4. Configuration

Implemented in `crates/ferrite-config` (`load(data_dir, env)`, `Config`); the binary calls it
through `load_config` (`crates/ferrite/src/lib.rs`). Keys marked *planned* are in the design
but rejected today (`unknown key`).

**Sources and precedence.** `<data>/ferrite.toml` (a missing file means all defaults, but
`site.url` has no default, so the site URL must then come from the environment), then
environment variables, which win.

**Env rules** (`load.rs`, `apply_env`):

- Only variables starting with `FERRITE_` count. `FERRITE_LOG` (log filter) and
  `FERRITE_TEST_*` are ignored.
- `FERRITE_<SECTION>__<KEY>`: a double underscore nests, keys are lowercased
  (`FERRITE_DATABASE__MAX_CONNECTIONS` is `database.max_connections`). A variable with an
  empty segment (`FERRITE___X`, `FERRITE_A__`) is a problem.
- The value is parsed as a TOML scalar or array (`16`, `true`, `["10.0.0.0/8"]`), and
  otherwise taken as a raw string. `server.trusted_proxies` also accepts a comma-separated
  string.
- The binary rejects a `FERRITE_*` variable whose value is not valid UTF-8 (naming only the
  key); non-UTF-8 variables without the prefix are skipped (`utf8_env`).
- Problems caused by an env var are reported as `env:FERRITE_...`, not by TOML path.

**Secrets** may be given inline or as `<key>_file` (path relative to the data folder unless
absolute; one trailing newline is stripped; an empty or unreadable file is a problem; setting
both the inline key and `_file` is a problem). This applies to `database.url` and to
`media.s3.access_key_id` / `media.s3.secret_access_key`. `Config` holds the database URL and
the S3 secret key as `ferrite_domain::Secret`, and messages never contain secret values.

**Value formats.** Sizes: `"256MB"`, `"32MiB"`, `"512KB"`, `"1GB"` or bytes; KB/KiB = 1024,
MB/MiB = 1024², GB/GiB = 1024³ (`cx.rs`, `parse_size`). Durations: seconds, `"90s"`,
`"10m"`, `"1h"`, must be above zero.

```toml
[site]
url        = "https://example.com"     # required; absolute http(s) URL
title      = ""                        # default empty
locale     = "en"                      # default "en"; must not be empty
timezone   = "UTC"                     # default "UTC"; must not be empty (not validated as a zone)

[server]
listen     = "127.0.0.1:8080"          # default; a socket address
trusted_proxies = []                   # CIDRs or bare IPs
# tls = ...                            # planned (M4: ACME); rejected as an unknown key today

[database]
url        = "sqlite://ferrite.db"     # default; sqlite:… (relative to the data folder) or postgres[ql]://…
# url_file = "/run/secrets/db_url"     # instead of url
max_connections = 16                   # > 0
auto_migrate    = true                 # run pending migrations at startup (see 02 §8)
allow_contract  = false                # Postgres: also allow contract (destructive) migrations at startup
# schema = "ferrite"                   # Postgres only: keep all tables in this schema; [A-Za-z0-9_]+
# SQLite URLs accept no ?query options.

[media]
backend    = "local"                   # "local" | "s3"
# local:
root         = "media"                 # default; relative to the data folder
# local_shared = false                 # operator asserts the folder is a shared volume
# s3 (required when backend = "s3"):
# [media.s3]
# bucket = "my-site"                   # required
# region = "eu-central-1"              # required
# endpoint = "https://s3.eu-central-1.amazonaws.com"   # optional, http(s) URL
# access_key_id / secret_access_key (or *_file)        # both or neither; neither = environment credentials
# allow_http = false
# virtual_hosted_style = false
# prefix = ""
# public_base_url = "https://cdn.example.com"          # optional, http(s) URL

[cluster]
mode       = "auto"                    # "solo" | "cluster" | "auto"
# node_name = ""                       # optional; empty means unset (no hostname-pid default yet)

[cache]
max_bytes  = "256MB"                   # 0 allowed
logged_in  = "bypass"                  # "bypass" | "per-user"
poll_interval_ms = 250                 # > 0; how often the invalidation loop reads the outbox

[plugins]
default_memory = "32MB"                # > 0
default_fuel   = 50_000_000            # > 0
render_deadline_ms = 200               # > 0
action_deadline_ms = 5_000             # > 0
disable_after_failures = 20            # > 0, within failure_window
failure_window = "10m"
```

`media.root` / `media.local_shared` are rejected with `backend = "s3"`, and `media.s3` with
`backend = "local"`.

**Cluster mode** (`Config::effective_cluster_mode`, `validate_cluster`): `auto` resolves to
`cluster` only when the database is Postgres **and** media is shareable between nodes (S3,
or local with `local_shared = true`); otherwise `solo`, so single-node Postgres with local
media works with defaults. An explicit `mode = "cluster"` is rejected with SQLite, and with
local media unless `local_shared = true`.

**Validation.** Unknown keys are errors. `load` returns `ConfigError` listing *all* problems
at once with the offending key path (one line per problem); a TOML syntax error is reported
as `ferrite.toml` with line and column but without the source text, since it may contain
secrets.

`ferrite-config` also renders the template that `ferrite init` writes
(`render_default_toml`, `InitMedia`); see [09](09-operations.md#8-cli-summary).

## 5. Error model

- Library crates define `thiserror` enums per crate (`StoreError`, `MediaError`,
  `PluginError`, `ThemeError`, `CoreError`…). `CoreError` wraps the lower ones.
- `ferrite-http` maps `CoreError` → HTTP status + RFC 9457 problem JSON
  (`application/problem+json`) for the API; HTML error pages for the public site.
- Only `crates/ferrite` uses `anyhow`.
- No `unwrap()`/`expect()` in non-test code except on invariants documented with a
  comment; clippy `unwrap_used` = warn in libraries.

## 6. Concurrency model

- `tokio` multi-thread runtime.
- DB access is async (sqlx). SQLite: one writer — sqlx's SQLite driver serialises on a
  worker thread per connection; we use a **single-connection write pool** plus a read
  pool to avoid `SQLITE_BUSY` storms. Postgres: one pool.
- Plugin calls are async wasmtime calls with epoch-based yielding so a CPU-bound plugin
  cannot starve the runtime ([05](05-plugin-host.md#limits)).
- Template rendering is CPU-bound but short; rendered on the runtime directly, with
  `spawn_blocking` for pages over a size threshold (measure first, M1).

## 7. Observability

- `tracing` with JSON output in production, pretty in dev. Spans: `http.request`,
  `render`, `plugin.hook{plugin,hook}`, `db.query{repo,op}`, `job{kind}`.
- `/metrics` (Prometheus text): request latency histograms by route class, cache
  hit/miss, plugin hook duration/fuel/failures by plugin+hook, job queue depth/latency,
  DB pool usage, node count.
- `/healthz` (process alive) and `/readyz` (DB reachable, migrations compatible, theme
  loaded) for load balancers.

## 8. Security baseline

- All admin/API mutations require auth + CSRF protection ([07](07-admin-api.md#auth)).
- Strict security headers by default (CSP for admin, `X-Content-Type-Options`,
  `Referrer-Policy`, HSTS when TLS).
- Uploaded files: content-type sniffed, SVG sanitised or served with
  `Content-Disposition: attachment` and a sandboxing CSP; never executed.
- Plugins: see [05](05-plugin-host.md). Themes: no code execution.
- Rate limiting on login and API token auth (DB-backed counters so it works across
  nodes).
