# 01 — Architecture

## 1. Workspace layout

The brief's layout (§7) is extended with storage, media and cluster crates. Dependencies
point downwards only; no cycles.

Status after M1c-1: crates marked **(built)** exist in `crates/` and are workspace members
(`Cargo.toml`); the others are planned and carry their milestone.

```
ferritecms/
├── Cargo.toml                    # workspace, shared [workspace.dependencies], lints
├── rust-toolchain.toml           # stable, edition 2024
├── crates/
│   ├── ferrite/                  # (built) BIN: clap CLI (`init`, `migrate`), logging, startup
│   │                             #   check; anyhow at the top level. No `serve` yet (M1c-2)
│   ├── ferrite-config/           # (built) ferrite.toml + env parsing, validation, init template
│   ├── ferrite-domain/           # (built) pure types: ids, Post, Term, User, Role, Block, errors
│   ├── ferrite-store/            # (built) repository traits + Store facade (no SQL)
│   ├── ferrite-store-sqlite/     # (built) SQLite impl + migrations/sqlite/
│   ├── ferrite-store-postgres/   # (built) Postgres impl + migrations/postgres/
│   ├── ferrite-store-tests/      # (built) contract test suite run against both backends;
│   │                             #   ferrite-core reuses its harness as a dev-dependency
│   ├── ferrite-media/            # (built) MediaStore over object_store, sniffing, image variants
│   ├── ferrite-cluster/          # (M1.5) node registry, event bus, job queue, locks
│   ├── ferrite-core/             # (built) services: content (+ lifecycle, revisions, redirects on
│   │                             #   permalink change, search indexing), taxonomy, menus, options,
│   │                             #   redirects; blocks rendering, slugs, events, roles/capabilities.
│   │                             #   Depends on domain + store only; users/auth services (M2),
│   │                             #   media wiring (M1c-2) are not there yet
│   ├── ferrite-cache/            # (M1c-2) page cache + dependency index
│   ├── ferrite-theme/            # (M1c-2) theme loading, hierarchy resolution, minijinja env
│   ├── ferrite-plugin-host/      # (built, M0) wasmtime engine, capability linker, limits, registry
│   ├── ferrite-http/             # (M1c-2 public site; M2 REST API v1, auth, admin assets)
│   ├── ferrite-import-wp/        # (M3) WXR importer
│   └── ferrite-sdk/              # (built, M0) guest SDK for Rust plugins (`export_plugin!` macro
│                                 #   lives in the crate; there is no separate macros crate)
├── wit/                          # WIT packages (source of truth for plugin API)
│   └── ferrite-plugin@0.1.0/
├── admin/                        # (M2) React SPA (pnpm workspace), built into dist/
├── themes/                       # (M1c-2) starter themes (blog first)
├── plugins/examples/             # (built) hello, order-a, order-b, hostile/*; seo, analytics,
│                                 #   contact-form are M4
├── tests/                        # cross-crate integration tests (planned; per-crate tests/ exist)
└── docs/
```

> Migrations live in each store crate (`crates/ferrite-store-*/migrations/`); CLAUDE.md
> says the same.

### Dependency graph

Target graph (crates not yet built are marked with their milestone):

```
ferrite (bin)
 ├─ ferrite-http (M1c-2/M2) ─┬─ ferrite-core ─┬─ ferrite-store (traits) ◄── ferrite-store-{sqlite,postgres}
 │                           │                ├─ ferrite-media
 │                           │                ├─ ferrite-cluster (M1.5) ─── ferrite-store (traits)
 │                           │                └─ ferrite-domain
 │                           ├─ ferrite-cache (M1c-2)
 │                           ├─ ferrite-theme (M1c-2)
 │                           └─ ferrite-plugin-host (built; no ferrite-* dependencies yet)
 ├─ ferrite-import-wp (M3) ─── ferrite-core
 └─ ferrite-config
```

As built today: `ferrite` depends on `ferrite-config`, `ferrite-domain`, `ferrite-store`
and both store crates (`crates/ferrite/Cargo.toml`); `ferrite-core` depends on
`ferrite-domain` and `ferrite-store`; `ferrite-config` and `ferrite-media` depend on
`ferrite-domain`; `ferrite-plugin-host` depends on no other `ferrite-*` crate and the
binary does not use it yet. `ferrite-core` does not yet use `ferrite-media`.

Only the binary crate depends on concrete store crates; everything else sees
`Arc<dyn Store>` (see [02](02-storage.md#repository-layer)). Both store crates are
compiled in by default; Cargo features `sqlite` and `postgres` (both default-on) allow
slimmer builds.

## 2. Runtime components

One `ferrite serve` process contains:

| Component | Responsibility | Crate |
|---|---|---|
| Config | Load `ferrite.toml`, env overrides (`FERRITE_*`), validate profile | ferrite-config |
| Store | Pooled DB access, transactions, migrations | ferrite-store-* |
| MediaStore | Blob get/put/list/delete, public URLs | ferrite-media |
| Cluster | Node heartbeat, event bus, job queue, distributed locks | ferrite-cluster |
| Core services | Business rules, permission checks, revisions, emitting events | ferrite-core |
| Plugin host | Load components, run filters/actions under limits | ferrite-plugin-host |
| Theme engine | Resolve template, render with MiniJinja | ferrite-theme |
| Page cache | URL → rendered response, dependency-based invalidation | ferrite-cache |
| HTTP | Public routes, `/api/v1`, `/admin`, `/_media`, `/_p/<plugin>` (later), `/metrics`, `/healthz`, `/readyz` | ferrite-http |
| Workers | Job queue consumers (scheduled publish, actions, variants, imports) | ferrite-cluster + core |

Startup order: config → store connect → migration check/run → cluster register →
media check → load active theme + plugins (from artifacts) → start workers → start event
listener → bind HTTP → mark ready.

Shutdown (SIGTERM): stop accepting → drain in-flight requests (configurable, default 10s)
→ stop workers after current job (leases expire otherwise) → deregister node → close pool.

## 3. Request lifecycle (public page)

```
request ─► router ─► redirect table? ──yes──► 301/302
                │no
                ▼
          page cache lookup (key: host+path+query-allowlist+variant)
                │hit ──► respond (ETag/304 support)
                │miss
                ▼
          resolve route → content query (core) ─► DependencySet records keys
                ▼
          filter hooks (plugin host): content.body, render.head, render.footer …
                ▼
          theme: resolve template by hierarchy → render (MiniJinja)
                ▼
          store in cache with DependencySet → respond
```

- **Cache variant**: anonymous vs. logged-in. Logged-in requests bypass the cache by
  default (configurable).
- **Single-flight**: concurrent misses for the same key wait on one render
  (per-node; cross-node duplication is acceptable).
- **Failure**: a plugin hook failure skips that hook ([05](05-plugin-host.md#failure-isolation)).
  A theme render error returns a 500 rendered via the theme's `500.html` if present,
  otherwise a built-in minimal page. The admin can always be reached even if the theme
  is broken.

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
