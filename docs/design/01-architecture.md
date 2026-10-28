# 01 — Architecture

## 1. Workspace layout

The brief's layout (§7) is extended with storage, media and cluster crates. Dependencies
point downwards only; no cycles.

Status after M2a-1: crates marked **(built)** exist in `crates/` and are workspace members
(`Cargo.toml`); the others are planned and carry their milestone.

```
ferritecms/
├── Cargo.toml                    # workspace, shared [workspace.dependencies], lints
├── rust-toolchain.toml           # stable, edition 2024
├── crates/
│   ├── ferrite/                  # (built) BIN: clap CLI (`init`, `migrate`, `serve`, `seed demo`,
│   │                             #   `config check`, `healthcheck`, `user`, `theme` (with `pack`,
│   │                             #   M2b-3), `plugin`), logging, startup check,
│   │                             #   site key file, node wiring (hot swap, jobs), shutdown; anyhow
│   │                             #   at the top level
│   ├── ferrite-config/           # (built) ferrite.toml + env parsing, validation, init template
│   ├── ferrite-domain/           # (built) pure types: ids, Post, Term, User, Role, Block, errors
│   ├── ferrite-store/            # (built) repository traits + Store facade (no SQL)
│   ├── ferrite-store-sqlite/     # (built) SQLite impl + migrations/
│   ├── ferrite-store-postgres/   # (built) Postgres impl + migrations/
│   ├── ferrite-store-tests/      # (built) contract test suite run against both backends;
│   │                             #   other crates reuse its harness as a dev-dependency
│   ├── ferrite-media/            # (built) MediaStore over object_store, sniffing, image variants
│   ├── ferrite-cluster/          # (built, M1.5) job runner, periodic scheduler, built-in
│   │                             #   prune jobs, node registry (join checks, heartbeat)
│   ├── ferrite-core/             # (built) services: content (+ lifecycle, revisions, redirects on
│   │                             #   permalink change, search indexing), taxonomy, menus, options,
│   │                             #   redirects, content types + field schemas, settings allowlist;
│   │                             #   users (`UserService`) and auth (`AuthService`: site key,
│   │                             #   sealing, argon2id, sessions, tokens, TOTP, rate limits; M2a-1);
│   │                             #   permalinks; the published-only read facade `SiteReader`
│   │                             #   (+ preview); blocks rendering, slugs, events and the
│   │                             #   dependency-key contract, roles/capabilities + token scopes.
│   │                             #   M2a-2a: extension install/delete/purge (`ExtensionService`,
│   │                             #   `ExtensionAdmin`), theme/plugin settings (`ext_settings`),
│   │                             #   mail (`Mailer`, `MailQueue`), password reset, action jobs.
│   │                             #   M2a-2b: `MediaService`, `CommentService`, `SystemService`,
│   │                             #   `diagnostics::scrub`. M2b-3: user filters, sessions, an
│   │                             #   admin's TOTP reset, retained plugin data, audit filters,
│   │                             #   the test mail, relation `types`, inline image `srcset`.
│   │                             #   Depends on domain + store only
│   ├── ferrite-cache/            # (built, M1c-2) page cache (moka) + dependency index + outbox
│   │                             #   invalidation loop
│   ├── ferrite-theme/            # (built, M1c-2) theme packages, hierarchy resolution, sandboxed
│   │                             #   MiniJinja renderer and helpers
│   ├── ferrite-plugin-host/      # (built) wasmtime engine, capability linker, limits, registry,
│   │                             #   output validation + ammonia sanitising, call tracing;
│   │                             #   M2a-2a: grants, permission report, call cap, render budget
│   ├── ferrite-http/             # (built, M1c-2: public site, router, head, plugin wiring;
│   │                             #   M2a-1: REST API v1 in `src/api/`, previews, logged-in requests;
│   │                             #   M2a-2a: extensions/settings/reset endpoints, the install
│   │                             #   pipeline `extensions::Installer`, plugin routes `/_p/`,
│   │                             #   SMTP mailer, `request.served` buffer; M2a-2b: media,
│   │                             #   comments and system endpoints, the uploader `media::Uploader`,
│   │                             #   `Idempotency-Key`, the theme comment form `/_comment`, the
│   │                             #   plugin `http` egress; M2b-1: the embedded admin (`admin`
│   │                             #   module, rust-embed of `admin/dist`, its CSP and headers;
│   │                             #   M2b-3: the `ferrite_admin_theme` cookie's class on
│   │                             #   `index.html`))
│   ├── ferrite-import-wp/        # (M3) WXR importer
│   └── ferrite-sdk/              # (built, M0) guest SDK for Rust plugins (`export_plugin!` macro
│                                 #   lives in the crate; there is no separate macros crate)
├── wit/                          # WIT packages (source of truth for plugin API)
│   └── ferrite-plugin@0.1.0/
├── admin/                        # (built, M2b-1 to M2b-3) admin SPA: one pnpm package (not a
│                                 #   workspace), Vite + React + TS; `pnpm build` writes dist/,
│                                 #   which ferrite-http embeds (placeholder page without it;
│                                 #   07 §3). src/ (routes/, api/ with the generated
│                                 #   schema.d.ts, editor/, components/ui + kit, design/, shell/,
│                                 #   palette/, dashboard/, content/, media/, comments/,
│                                 #   structure/, account/, users/, settings/, appearance/,
│                                 #   plugins/, system/, auth/, lib/, i18n/), e2e/ (Playwright
│                                 #   + support), scripts/ (dist and bundle checks, API drift
│                                 #   check, the E2E wrapper), bundle-budget.json
├── themes/                       # (built) starter themes embedded in the binary: `blog`
├── plugins/examples/             # (built) hello, hello-v2 (manifest only), order-a, order-b,
│                                 #   clock, html-inject, meta-writer, reader, router, notifier,
│                                 #   counter, settings-probe, fetcher, admin-e2e (the admin
│                                 #   E2E's plugin, M2b-3), hostile/*; seo, analytics,
│                                 #   contact-form are M4
├── tests/                        # cross-crate integration tests (planned; per-crate tests/ exist)
└── docs/
```

> Migrations live in each store crate (`crates/ferrite-store-*/migrations/`); CLAUDE.md
> says the same.

### Dependency graph

As built in M2a-2a (normal dependencies, from each crate's `Cargo.toml`; crates not yet
built are marked with their milestone):

```
ferrite (bin) ── ferrite-http, ferrite-cache, ferrite-cluster, ferrite-theme, ferrite-core,
 │               ferrite-plugin-host, ferrite-media, ferrite-config, ferrite-store,
 │               ferrite-store-{sqlite,postgres}, ferrite-domain
 ├─ ferrite-http ─┬─ ferrite-cache ─── ferrite-store, ferrite-domain
 │                ├─ ferrite-theme ─── ferrite-core, ferrite-domain
 │                ├─ ferrite-core ──── ferrite-store (traits) ◄── ferrite-store-{sqlite,postgres}
 │                │                    └─ ferrite-domain
 │                ├─ ferrite-media ─── ferrite-domain
 │                ├─ ferrite-plugin-host ─── ferrite-domain (M2a-2a: the shared setting rules)
 │                └─ ferrite-config (M2a-2a: `MailConfig`), ferrite-store, ferrite-domain
 ├─ ferrite-config ─── ferrite-domain
 ├─ ferrite-cluster ─── ferrite-store (traits), ferrite-domain
 └─ ferrite-import-wp (M3) ─── ferrite-core
```

`ferrite-core` does not use `ferrite-media` (media URLs reach `SiteReader` as a closure built
by the binary from `MediaStore::public_url`). `ferrite-plugin-host` keeps no dependency on
core (its one domain dependency is `ferrite_domain::setting`, the value rules it shares
with core and themes); `ferrite-http` adapts between them (`StoreHostServices`). `ferrite-cache` uses
`ferrite-core` only in its tests.

Only the binary crate depends on concrete store crates; everything else sees
`Arc<dyn Store>` (see [02](02-storage.md#repository-layer)). Both store crates are always
compiled in; Cargo features to leave one out (`sqlite`, `postgres`) are planned but do not
exist yet (no `[features]` section in any crate).

## 2. Runtime components

One `ferrite serve` process contains (as built after M2a-2a; planned parts marked):

| Component | Responsibility | Crate |
|---|---|---|
| Config | Load `ferrite.toml`, env overrides (`FERRITE_*`), validate profile | ferrite-config |
| Store | Pooled DB access, transactions, migrations | ferrite-store-* |
| MediaStore | Blob get/put/list/delete, public URLs | ferrite-media |
| Cluster | Node registry and heartbeat, job runner, periodic scheduler, built-in prune jobs (`LISTEN/NOTIFY`, locks and the job queue are in the store) | ferrite-cluster |
| Core services | Business rules, permission checks (role + token scope), revisions, emitting events; users, sessions, tokens, rate limits, TOTP (`AuthService`, `UserService`); `SiteReader` for public reads and previews | ferrite-core |
| Plugin host | Load components, run filters/actions/routes under limits, a per-node call cap and a per-render budget | ferrite-plugin-host |
| Mail | `MailQueue` (16 sends in flight, 10 s each) over the SMTP mailer when `[mail]` is set: password reset, plugin `notify-admins` and the admin's test mail (M2b-3) | ferrite-core + ferrite-http |
| Served buffer | `request.served` events buffered per node, flushed as `plugin.action` jobs every 10 s or 1 000 events (05 §4a) | ferrite-http |
| Redirect hits | Redirect hits counted per node (at most 10 000 redirects buffered), one `add_hits` write per redirect every 10 s and on stop (06 §6) | ferrite-http |
| Theme engine | Resolve template, render with MiniJinja | ferrite-theme |
| Page cache | URL → rendered response, dependency-based invalidation | ferrite-cache |
| Invalidation loop | Polls the `events` outbox every `cache.poll_interval_ms` (default 250; woken by `LISTEN` in cluster mode) and applies events to the page cache; its hook drives the hot swap | ferrite-cache |
| Reloader | Hot swap of theme, plugin registry and permalink rules/route table on events (P18) | ferrite (bin) |
| Scheduled publishing | `content.publish_scheduled` job per scheduled item, `content.publish_due` periodic every 30 s (P12) | ferrite (bin) + core |
| HTTP | Public routes, `/_media`, `/_theme`, `/_preview/<id>`, `/healthz`, `/readyz`, the REST API `/api/v1` (M2a-1, same process and layers, never cached), plugin routes `/_p/<plugin>/…` (M2a-2a, never cached), the embedded admin `/admin/` (M2b, 07 §3); later `/metrics` (M4) | ferrite-http |
| Workers | Job queue consumers (`cluster.workers` per node): prune jobs incl. `sessions.prune`, publishing, `plugin.action`, `plugin.purge` and `plugin.purge_sweep` (M2a-2a); variants and imports later | ferrite-cluster + core |

Startup order as built (`crates/ferrite/src/serve.rs`, `ferrite::start`; any failure exits
1; rulings P15, P22): config → open the database → migration check under the migrate lock
(pending migrations applied when allowed, an incompatible schema refused) → join the node
registry → media store write/read/delete probe → page cache + invalidation cursor (**set
before any node state is read**) → permalink rules from options, checked against types and
taxonomies → active theme (falls back to the embedded `blog` at startup only) → site key
(`auth.secret_key[_file]`, else solo `<data>/secret.key`; a missing or bad key exits 1) and,
with `[mail]`, the SMTP mailer and mail queue (a value lettre refuses exits 1) → plugins
from the `plugins` table + artifacts (a broken row is skipped) → route table → the API's
services (`AuthService`, users, content types, options, the node's extension
`Installer`) → bind
`server.listen` → background tasks: invalidation loop (`LISTEN` in cluster mode) with the
reloader hook, plugin-failure flusher, `request.served` flusher, job runner, periodic
scheduler, heartbeat → serve the
public site with the API merged in front of it (`build_site`) and print
`Listening on http://<addr>`. Signal handlers are installed before all of this; a signal
during startup exits at once (130/143), after deregistering the node (bounded at 1 s) if it
had already joined.

Shutdown on SIGINT or SIGTERM, as built: stop accepting **and** stop the job runner → drain
open requests for up to 10 s while running jobs get 9 s, then the rest are released without
using an attempt (runner abandoned after 12 s) → stop the background tasks (final failure
flush) and deregister the node (together up to 1 s) → close the database pools (up to 2 s,
cut to end within 13 s of the signal) → the runtime gets a 1 s grace, then remaining
connection tasks are dropped. The process is gone at most 14 s after the first signal; a
second signal during startup or shutdown exits at once (130 for SIGINT, 143 for SIGTERM).

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

- **Cache variant** (M2a-1, P21): a request whose session cookie (07 §2) resolves to a
  session is logged in. `cache.logged_in = "bypass"` (default) renders it without the cache;
  `"per-user"` caches it under the user's own variant. Both answer `Cache-Control: private,
  no-store`. Everything else is anonymous. `/_preview/<id>` is handled right after
  `/_media`/`/_theme`, before the redirect table, and never cached ([06 §3](06-themes-rendering.md#3-routing)).
- **Single-flight**: concurrent misses for the same key wait on one render
  (per-node; cross-node duplication is acceptable).
- **Failure**: a plugin hook failure skips that hook ([05](05-plugin-host.md#8-failure-isolation-p5-p6))
  and gives the page a short TTL. A theme render error returns a 500 rendered via the theme's
  `500.html` if present, otherwise a built-in minimal page; the server keeps serving. The
  API does not use the theme, so it keeps working with a broken one; admin reachability
  applies once the admin exists (M2b).
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
both the inline key and `_file` is a problem). This applies to `database.url`,
`media.s3.access_key_id` / `media.s3.secret_access_key`, `auth.secret_key` and
`mail.smtp_url`. `Config` holds the database URL, the S3 secret key, the site key and the
SMTP URL as `ferrite_domain::Secret`, and messages never contain secret values.

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
# either backend:
# max_upload = "256MB"                 # M2a-2b: largest uploaded file, 1 byte..=1GiB; default 256MB
# image_workers = 1                    # M2a-2b: image variant jobs at once per node, 1..=16; default 1
# upload_idle_timeout = "30s"          # M2a-2b: an upload receiving nothing this long is aborted
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
# node_name = "web-1"                  # optional, trimmed, 1..=64 bytes; unset: $HOSTNAME-<pid>, else node-<pid>
# workers  = 4                         # concurrent jobs per node, 1..=256; default max(2, CPU count)

[cache]
max_bytes  = "256MB"                   # 0 allowed
logged_in  = "bypass"                  # "bypass" | "per-user"
poll_interval_ms = 250                 # > 0; how often the invalidation loop reads the outbox
search_ttl = "60s"                     # M2a-2b: TTL of search result pages, 0 ..= 1h; 0 = not cached

[auth]
# secret_key = "<64 hex characters>"   # the site key (32 bytes); or secret_key_file
                                       # unset: solo uses <data>/secret.key; required in cluster mode

[plugins]
default_memory = "32MB"                # > 0
default_fuel   = 50_000_000            # > 0
render_deadline_ms = 200               # > 0
action_deadline_ms = 5_000             # > 0
disable_after_failures = 20            # > 0, within failure_window
failure_window = "10m"
# max_concurrent_calls = 32            # M2a-2a: plugin calls at once per node, > 0;
                                       #   unset: max(8, 4 × available parallelism)
render_budget_ms = 1_000               # M2a-2a: all plugin filters of one page together;
                                       #   render_deadline_ms ..= 60_000
volatile_ttl = "60s"                   # M2a-2b: TTL of pages with volatile or failed plugin
                                       #   calls; 1s ..= 1h

[mail]                                 # optional; unset: no outgoing mail
# smtp_url = "smtps://user:pass@mail.example.com"   # or smtp_url_file; a secret
#   smtps:// implicit TLS; smtp://…?tls=required STARTTLS; plain smtp:// (credentials only to a loopback relay)
# from = "Site <noreply@example.com>"  # required with smtp_url; "a@b" or "Name <a@b>"
```

**Mail** (`[mail]`, M2a-2 P17). The SMTP password lives only in the URL, so only in the
config file, a `smtp_url_file` or `FERRITE_MAIL__SMTP_URL`, never in the database. `load`
checks the shape (scheme `smtp`/`smtps`, a host, `tls` only `required` and only on
`smtp` — there is no opportunistic mode, which would fall back to plaintext; credentials
in the URL only with `smtps` or `tls=required`, or on plain `smtp` to a loopback relay
(`localhost`, `127.0.0.0/8`, `::1`), since lettre authenticates on whatever connection it
has, R16; `from` a plain address or `Name <address>` without control characters, required
with `smtp_url`, refused without it) without echoing the URL; `serve` then parses both
with lettre (`ferrite_http::mail::SmtpMailer`, 10 s per SMTP step), so a value lettre
refuses stops startup. Mail leaves through the node's `ferrite_core::mail::MailQueue`: at
most 16 sends in flight (more are refused), each bounded by 10 s, run in background tasks
so no plugin call or request waits for SMTP, and cancelled at shutdown inside the
background-stop second (only an explicit shutdown cancels; a dropped queue lets its sends
finish). Plugin mail subjects are prefixed with `[<site.title>] `, the title cut to 100
characters, and left out when it is empty or contains control characters (checked once
when `serve` builds the services). Every mailer refuses (never folds) control characters, CR/LF and
Unicode line separators in subjects and addresses. Without `[mail]` the plugin
`mail.notify-admins` import answers "mail is not configured".

**Plugins** (`[plugins]`): `default_memory` and `default_fuel` are the per-call defaults a
manifest may lower; above them it needs the admin's approval, up to 256 MiB and 10 ×
`default_fuel` (05 §3). `max_concurrent_calls` (M2a-2a P22) bounds the filter, action and
route calls running at once on a node; a call that gets no permit within its deadline is
skipped, never counted as a failure. `render_budget_ms` (P22) is the time all plugin
filters of one page share, from the render slot on; it must lie between
`render_deadline_ms` and 60 000 (05 §6). `volatile_ttl` (M2a-2b Task 14) is how long a page
whose plugin calls were volatile or failed stays cached (05 §7a): a duration (integer
seconds or `"90s"`/`"10m"`/`"1h"`) from 1 s to 1 h, default 60 s.

**Cache** (`[cache]`): `search_ttl` (M2a-2b Task 14) is how long search result pages stay
cached, a duration from 0 to 1 h (`0`: search pages are never cached), default 60 s
(06 §6).

`media.root` / `media.local_shared` are rejected with `backend = "s3"`, and `media.s3` with
`backend = "local"`.

**Uploads** (`[media] max_upload`, `image_workers`; M2a-2b P8/P9, `Config.uploads:
UploadConfig`). Both keys are valid with either backend. `max_upload` is a size (0 is
refused, above 1 GiB is refused: the S3 copy ceiling of 5 GB stays out of reach). `POST
/api/v1/media` refuses a declared `Content-Length` above `max_upload` + 1 MiB before
reading the body and cuts the file at `max_upload` while it streams (`413 too-large`);
the route's own body limit is the ceiling, 1 GiB + 1 MiB (pre-flight F12). At most 8
uploads stream at once per node (`ferrite_http::media::MEDIA_UPLOAD_SLOTS`), more are
`503 busy`. An upload that receives no bytes for `upload_idle_timeout` (a duration > 0,
default 30 s) is aborted with `400 malformed-body` and frees its slot, so slow clients
cannot hold the slots. **Memory per node:** an upload holds at most 8 MiB in memory
before the store switches to a staging object (2 MiB for an SVG, which is read whole to
be sanitised), so uploads use at most about 64 MiB per node (8 slots × 8 MiB) besides
the 8 KiB head each; size small VMs for it. The file name is checked (1..=255 bytes, no
control or format characters) before the file is read. `image_workers` bounds the `media.variants` jobs a node runs at once (Task 6);
variants are made only for raster originals of at most 64 MiB.

**Cluster mode** (`Config::effective_cluster_mode`, `validate_cluster`): `auto` resolves to
`cluster` only when the database is Postgres **and** media is shareable between nodes (S3,
or local with `local_shared = true`); otherwise `solo`, so single-node Postgres with local
media works with defaults. An explicit `mode = "cluster"` is rejected with SQLite, and with
local media unless `local_shared = true`.

**Site key** (`[auth]`, P4, R1): 32 bytes written as 64 hex characters (either case; any
other length or a non-hex character is a problem). It never lives in the database, so a
database dump does not carry the key that opens the secrets in it. When the effective mode
is `cluster` (explicit, or `auto` resolving to it) and no key is configured, `load` reports
`auth.secret_key: required in cluster mode (every node must use the same key)`, so
`ferrite config check` shows it. In solo mode without a key the binary uses
`<data>/secret.key` (`crates/ferrite/src/keys.rs`): 64 lowercase hex characters and a
newline, read if present (a malformed file is an error naming it), otherwise created with
`create_new` and mode `0600`. `ferrite init` creates it only for a solo config without a
configured key (and removes it if `init` fails); the key is never logged or printed.

**Node startup checks** (`ferrite_cluster::Registry::join`, 04 §3, P15) need the database,
so they run when a node joins the registry (`serve`, after the migration check), not in
`load`: a *live* node (heartbeat within 30 s by the database clock) with another `site_id`
or another `site.url` refuses the start; in solo mode another live node is waited for up
to 35 s, then refused. On SQLite (the data-folder lock keeps other processes out) rows of
other nodes are from dead processes and are deleted instead, with no wait or comparison
(R17). The waiting is unlocked; the final check-and-register pass runs
under the `cluster.join` lock, so nodes starting together are checked one at a time. `startup_check`
(and `ferrite migrate up|down`) hold the `migrate` lock on both backends, waiting up to
5 minutes and logging every 10 s.

**Validation.** Unknown keys are errors. `load` returns `ConfigError` listing *all* problems
at once with the offending key path (one line per problem); a TOML syntax error is reported
as `ferrite.toml` with line and column but without the source text, since it may contain
secrets.

`ferrite config check --data DIR` runs exactly this `load` (file + `FERRITE_*` environment)
without opening the database or media, and prints
`ok: mode=<solo|cluster> database=<sqlite|postgres> media=<local|s3> workers=<n> mail=<configured|off>` (exit 0)
or every problem (stderr, exit 1); see [09 §8](09-operations.md#8-cli-summary).

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

- Every API endpoint except `GET /api/v1/meta`, `GET /api/v1/openapi.json`, login (both
  steps) and the two password-reset endpoints (M2a-2a) needs a session cookie or a bearer
  token; cookie-authenticated mutations also need the CSRF token and a same-origin
  `Origin`/`Referer`, and login and password reset check the origin (M2a-1, P8; M2a-2 P28;
  [07 §2](07-admin-api.md#2-auth)); a route sweep test enforces the list. API responses
  are `Cache-Control: no-store`. Extension endpoints check `manage_themes` /
  `manage_plugins` with the `extensions` scope in core before reading an upload.
- Strict security headers by default (CSP for admin, `X-Content-Type-Options`,
  `Referrer-Policy`, HSTS when TLS).
- Uploaded files: content-type sniffed, SVG sanitised or served with
  `Content-Disposition: attachment` and a sandboxing CSP; never executed.
- Plugins: see [05](05-plugin-host.md): an install needs the admin's approval of the
  exact upload (a fingerprint of the permission report); plugin routes under `/_p/` run
  in a sandboxed opaque origin (`Content-Security-Policy: sandbox allow-forms; form-action
  'self'`), never see the session cookie or `Authorization`, cannot set cookies, and are
  rate-limited per client across nodes (05 §4b). Themes: no code execution; a theme that
  does not compile is refused on upload and on activation.
- Rate limiting on login, TOTP codes, unknown API tokens, password-reset requests, plugin
  routes and plugin mail (DB-backed fixed windows, so it works across nodes; P5, 07 §2,
  05 §4b, 05 §6 "Mail capability details").
- One site key ([§4](#4-configuration)) outside the database. Every use gets its own
  subkey, HMAC-SHA256(site key, label) (`ferrite_core::auth`): `ferrite:seal:v1` for sealed
  values, `ferrite:ip-hash:v1` for client IP hashes, `ferrite:login-bucket:v1` for the
  per-login rate-limit buckets, `ferrite:challenge-pw:v1` for the password fingerprint
  inside a TOTP login challenge, and (M2a-2a) `ferrite:reset-bucket:v1` for the per-input
  password-reset buckets, `ferrite:reset-pw:v1` for the password fingerprint inside a
  reset token and `ferrite:plugin-ip:v1` for the per-plugin client hash passed to plugin
  routes. A sealed value is
  `v1.` + base64url(no padding) of a random 24-byte nonce followed by the
  XChaCha20-Poly1305 ciphertext and tag, with the purpose as associated data, so it opens
  only for the same purpose and key (TOTP secrets and stateless TOTP challenges in M2a-1;
  password-reset tokens, purpose `password-reset`, and plugin `secret` settings, purpose
  `plugin-setting:<plugin>:<key>`, in M2a-2a). Client IPs are stored only as the first 32 hex characters of
  an HMAC over the canonical address (an IPv4-mapped IPv6 address hashes like the IPv4
  address). A lost or changed key makes sealed values unreadable (users re-enroll TOTP, admins
  re-enter plugin secrets, outstanding reset links stop working).
