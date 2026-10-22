# 01 — Architecture

## 1. Workspace layout

The brief's layout (§7) is extended with storage, media and cluster crates. Dependencies
point downwards only; no cycles.

```
ferritecms/
├── Cargo.toml                    # workspace, shared [workspace.dependencies], lints
├── rust-toolchain.toml           # stable, edition 2024
├── crates/
│   ├── ferrite/                  # BIN: CLI (clap), wiring, anyhow at the top level
│   ├── ferrite-config/           # ferrite.toml + env parsing, validation
│   ├── ferrite-domain/           # pure types: ids, Post, Term, User, Role, Block, errors
│   ├── ferrite-store/            # repository traits + Store facade (no SQL)
│   ├── ferrite-store-sqlite/     # SQLite impl + migrations/sqlite/
│   ├── ferrite-store-postgres/   # Postgres impl + migrations/postgres/
│   ├── ferrite-store-tests/      # contract test suite run against both backends
│   ├── ferrite-media/            # MediaStore over object_store, variants, artifacts
│   ├── ferrite-cluster/          # node registry, event bus, job queue, locks
│   ├── ferrite-core/             # services: content, taxonomy, users, options, redirects,
│   │                             #   search, scheduling; orchestrates store+media+cluster
│   ├── ferrite-cache/            # page cache + dependency index
│   ├── ferrite-theme/            # theme loading, hierarchy resolution, minijinja env
│   ├── ferrite-plugin-host/      # wasmtime engine, capability linker, limits, registry
│   ├── ferrite-http/             # axum: public site, REST API v1, auth, admin assets
│   ├── ferrite-import-wp/        # WXR importer
│   └── ferrite-sdk/              # guest SDK for Rust plugins (+ ferrite-sdk-macros)
├── wit/                          # WIT packages (source of truth for plugin API)
│   └── ferrite-plugin@0.1.0/
├── admin/                        # React SPA (pnpm workspace), built into dist/
├── themes/                       # starter themes (blog first)
├── plugins/examples/             # hello, seo, analytics, contact-form, hostile/*
├── tests/                        # cross-crate integration tests
└── docs/
```

> CLAUDE.md says migrations live in `crates/ferrite-core/migrations/`. With two
> backends they move to each store crate (`crates/ferrite-store-*/migrations/`).
> CLAUDE.md must be updated when this design is approved.

### Dependency graph

```
ferrite (bin)
 ├─ ferrite-http ─┬─ ferrite-core ─┬─ ferrite-store (traits) ◄── ferrite-store-{sqlite,postgres}
 │                │                ├─ ferrite-media
 │                │                ├─ ferrite-cluster ─── ferrite-store (traits)
 │                │                └─ ferrite-domain
 │                ├─ ferrite-cache
 │                ├─ ferrite-theme
 │                └─ ferrite-plugin-host ─── ferrite-domain
 ├─ ferrite-import-wp ─── ferrite-core
 └─ ferrite-config
```

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

`ferrite.toml` in the data folder. Every key can be overridden by an env var
`FERRITE_<SECTION>__<KEY>` (double underscore = nesting), which is how cluster nodes are
normally configured (12-factor). Secrets may be given as `*_file` paths.

```toml
[site]
url        = "https://example.com"     # canonical base URL
title      = "Example"
locale     = "en"
timezone   = "Europe/Warsaw"

[server]
listen     = "0.0.0.0:8080"
trusted_proxies = ["10.0.0.0/8"]       # for X-Forwarded-For / -Proto
# tls = { acme = true, domains = ["example.com"], email = "ops@example.com" }

[database]
url        = "sqlite://ferrite.db"     # relative to data folder; or postgres://…
max_connections = 16
# url_file = "/run/secrets/db_url"

[media]
backend    = "local"                   # "local" | "s3"
# [media.s3]
# bucket   = "my-site"
# region   = "eu-central-1"
# endpoint = "https://s3.eu-central-1.amazonaws.com"   # any S3-compatible endpoint
# access_key_id_file = "/run/secrets/s3_key"
# secret_access_key_file = "/run/secrets/s3_secret"
# public_base_url = "https://cdn.example.com"         # optional: serve media from CDN

[cluster]
mode       = "auto"                    # "solo" | "cluster" | "auto" (derived from backends)
node_name  = ""                        # default: hostname-pid

[cache]
max_bytes  = "256MB"
logged_in  = "bypass"                  # "bypass" | "per-user"

[plugins]
default_memory = "32MB"
default_fuel   = 50_000_000
render_deadline_ms = 50
action_deadline_ms = 5_000
disable_after_failures = 20            # within failure_window
failure_window = "10m"
```

`ferrite-config` produces a validated `Config` struct; validation errors list *all*
problems at once with the offending key path.

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
