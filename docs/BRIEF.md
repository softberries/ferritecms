# FerriteCMS — Product & Technical Brief

> Working name: **FerriteCMS** (binary: `ferrite`). Name is provisional; `ferritecms.dev`
> showed as available in a GoDaddy check but must be confirmed at the registrar (GoDaddy
> was wrong about `quench.dev`).

## 1. One-line pitch

A WordPress-style CMS shipped as **one Rust binary**, where **every plugin runs in a
WebAssembly sandbox** and can only do what its manifest declares. Imports WordPress
*content*; does not run WordPress *plugins*.

## 2. Why this exists

- WordPress powers a huge share of the web, but its plugin model is the security problem:
  core, theme and every plugin share one PHP process, the same DB credentials and full
  filesystem access. Patchstack's State of WordPress Security attributes ~96% of WP
  security issues to plugins.
- The fix is architectural, not "write better PHP": plugins must be **isolated** and
  **capability-scoped**.
- Rust's value here is not mainly memory safety (PHP is already memory-safe). It is:
  a single static binary, low memory, no per-request bootstrap, and a first-class WASM
  host (wasmtime) for sandboxing.

## 3. Positioning

**For:** developers, agencies running many small client sites, self-hosters, and EU
organisations that want content on their own servers with a small attack surface.

**Not for (yet):** people who need WooCommerce, Elementor or a large plugin marketplace
next week.

**Closest competitor:** Cloudflare **EmDash** (April 2026, TypeScript/Astro, runs on
Cloudflare Workers, sandboxed plugins via Dynamic Workers, MIT). Differentiation:

| | EmDash | FerriteCMS |
|---|---|---|
| Runtime | Cloudflare Workers (serverless) | Self-hosted single binary, any VPS / bare metal |
| Lock-in | Tied to CF platform primitives | Copy one folder, run anywhere |
| Plugin sandbox | V8 isolates | WASM components (wasmtime), any source language |
| Data | CF D1/R2 | Local SQLite, or Postgres + S3-compatible storage for multi-node setups |

Similar spirit: Ghost (single-purpose, opinionated), PocketBase (single binary).

## 4. Explicit non-goals

- **No WordPress plugin/theme compatibility.** No PHP runtime, no `$wpdb` shim.
- No page builder in v1.
- No e-commerce in v1.
- No multi-tenant SaaS in v1 (single site per process; multi-site later).

## 5. Core concepts

### 5.1 One binary, one data folder

```
$ ferrite serve --data ./site
```

Compiled in: HTTP server, router + page cache, admin panel (embedded SPA), theme engine,
plugin host, SQLite.

Everything a site owns lives in the data folder:

```
site/
├── ferrite.db          # SQLite: content, users, settings, plugin KV
├── ferrite.toml        # site config
├── media/artifacts/    # content-addressed blobs: uploads, image variants, plugin .wasm, theme bundles
├── backups/            # pre-migration snapshots, scheduled backups
└── cache/              # compiled plugins etc. (safe to delete)
```

Bigger setups can instead use **PostgreSQL + S3-compatible storage** and run several
nodes behind a load balancer (see `docs/design/04-cluster.md`).

Backup = copy this folder (or `ferrite backup`). Move hosts = copy folder, run binary.

### 5.2 Request lifecycle

1. **Router** maps URL → content id (permalink rules, redirects table).
2. **Page cache** (in-memory, keyed by URL + locale + auth state) answers most requests.
3. On miss: **load content** from SQLite → run **plugin hooks** (filters in WASM) →
   **render theme** template → store in cache → respond.
4. Saving content in admin invalidates only the cached pages that depend on it
   (dependency tracking: post id, taxonomy terms, menus, options).

Design targets on a small VPS (targets, not measurements): cache hit < 1 ms, uncached
render < 10 ms, idle RSS < 50 MB.

### 5.3 Plugin model (the core differentiator)

- A plugin = **WASM component** + **manifest** (`ferrite-plugin.toml`).
- Interface defined in **WIT** (WebAssembly Interface Types / Component Model).
  Plugins can be written in anything that targets WASM components: Rust first
  (official SDK crate `ferrite-sdk`), later JS (ComponentizeJS), Go (TinyGo), Python.
- **Capability-based:** the host links only the imports the manifest declares. A plugin
  that didn't declare `http` has no HTTP import at all — not "denied at runtime", absent.
- **Resource limits per call:** memory cap (e.g. 32 MB), wasmtime fuel (instruction
  budget), epoch-based wall-clock deadline (e.g. 50 ms for render hooks).
- **No shared memory, no globals.** Data is copied in, validated on the way out. Plugins
  can't see each other.
- **Failure isolation:** a trap/timeout in a hook is logged with plugin + hook name and
  the hook is skipped; the page still renders. Repeated failures auto-disable the plugin.
- Each plugin gets its own namespaced KV store (in SQLite) instead of raw DB access.

Example manifest:

```toml
[plugin]
name    = "seo-pro"
version = "1.4.0"
publisher = "example.dev"

[capabilities]
content.read       = ["post", "page"]
content.meta.write = ["seo.*"]
http               = ["api.indexnow.org"]   # allow-listed hosts only
kv                 = true

[hooks]
"render.head" = 10      # priority, lower runs first (like WP)
"post.saved"  = 50
```

Example plugin (Rust SDK, illustrative API):

```rust
use ferrite_sdk::{filter, Head, Ctx, Result};

// same idea as add_filter('wp_head', ..., 10)
#[filter("render.head", priority = 10)]
fn head(mut h: Head, ctx: &Ctx) -> Result<Head> {
    let post = ctx.current_post()?;          // requires content.read
    h.meta("description", &post.excerpt);
    h.og_image(post.featured_image());
    Ok(h)
}
```

Hook types (mirrors WP mental model):

- **Filters** — take a typed value, return a modified value (`render.head`,
  `content.body`, `render.footer`).
- **Actions** — fire-and-forget events (`post.saved`, `post.published`, `user.created`),
  may run async/off the request path.
- **Routes** (later) — plugin-owned endpoints under `/_p/<plugin>/...`.
- **Admin UI extensions** (later) — declarative settings schema rendered by core admin;
  no arbitrary JS injected into admin in v1.

Install UX: plugin install shows declared permissions (like a phone app permission
prompt). Signed plugins from a registry show "verified publisher".

### 5.4 Themes

- A theme = folder of templates + assets + `theme.toml`. **No code execution in themes.**
- Template engine: **MiniJinja** (Jinja2 syntax, familiar to many devs).
- Template hierarchy like WordPress — most specific wins:
  `single-{type}-{slug}.html` → `single-{type}.html` → `single.html` → `index.html`;
  similar chains for `archive`, `taxonomy`, `page`, `404`, `search`.
- Templates get only data core hands them + a fixed set of safe helper functions
  (`related(post, n)`, `menu("primary")`, `asset("style.css")`, date/format filters).
- Theme needs logic → ship a small plugin alongside.
- Ship 2–3 starter themes: a blog, a docs/knowledge-base, a small business site.

Example:

```jinja
{% extends "base.html" %}
{% block content %}
  <article>
    <h1>{{ post.title }}</h1>
    <time>{{ post.published | date("%d %b %Y") }}</time>
    {{ post.body | safe }}
  </article>
{% endblock %}
```

### 5.5 Content model

- Built-in types: `post`, `page`, `media`, plus **custom content types defined in admin**
  (fields: text, rich text, number, date, bool, select, relation, media, repeater).
  Avoids the "need ACF plugin for everything" problem.
- Taxonomies: categories, tags, custom taxonomies.
- Revisions on every save; drafts, scheduled publish.
- Rich text stored as structured JSON blocks (block editor), rendered to HTML server-side.
- Users & roles: admin, editor, author, contributor (WP-compatible names); passkeys +
  password; optional TOTP.

### 5.6 Admin panel

- Embedded SPA served from the binary (assets compiled in with `rust-embed` or similar).
- Talks to a versioned JSON REST API (`/api/v1/...`) — same API available to headless use.
- MVP screens: dashboard, posts/pages list + editor, media library, content types,
  taxonomies, menus, users, themes, plugins (with permission view), settings, import.

### 5.7 WordPress migration

```
$ ferrite import wordpress export.xml --uploads ./wp-content/uploads --data ./site
```

- Parses **WXR** export: posts, pages, attachments, categories, tags, custom taxonomies,
  authors, comments, post meta.
- Copies media, rewrites media URLs in content.
- Generates **301 redirects** for every old permalink so SEO survives.
- Users imported with forced password reset (WP hashes are phpass/bcrypt — could verify
  on first login as a nice-to-have).
- Maps data from popular plugins to built-in fields: Yoast / Rank Math → SEO fields,
  Contact Form 7 → built-in forms (later), ACF → custom content type fields.
- Converts Gutenberg block comments / classic HTML into FerriteCMS blocks; unknown
  shortcodes reported in `import-report.html`.
- Custom plugins: not ported; documented as rewrite work (agency service opportunity).

### 5.8 Operations

- `ferrite backup --to <path|s3://...>` — SQLite `VACUUM INTO` snapshot + media sync, run
  with `serve` stopped (the CLI needs the data-folder lock); a live site is backed up by the
  scheduled `[backup]` job inside `serve`. Postgres: provider backups/PITR + `ferrite export`.
- `ferrite self-update` — replace binary, keep previous as `ferrite.prev`; migrations run
  on start; `ferrite rollback`.
- TLS: built-in ACME (Let's Encrypt) via rustls, or run behind a reverse proxy.
- Observability: structured logs (`tracing`), `/metrics` (Prometheus), per-plugin hook
  timings.
- Packaging: static binaries for linux x86_64/aarch64 + macOS; Docker image; systemd unit.

## 6. Proposed tech stack (to validate)

| Area | Choice | Notes |
|---|---|---|
| HTTP | `axum` + `tokio` + `tower` | |
| TLS/ACME | `rustls`, `rustls-acme` or `instant-acme` | |
| DB | `sqlx` 0.9: SQLite (default) and PostgreSQL | Repository traits, one impl per backend, shared contract tests |
| Migrations | `sqlx migrate`, reversible (up/down) | Expand/contract policy, see design 02 |
| Templates | `minijinja` | |
| WASM host | `wasmtime` with Component Model + `wasmtime-wasi` (minimal) | fuel + epoch interruption |
| WIT bindings | `wit-bindgen` (guest), `wasmtime::component::bindgen!` (host) | |
| Admin UI | Vite + React + TS, shadcn/ui, Recharts, Motion, TipTap | Decided (design 07) |
| Embedding assets | `rust-embed` | |
| Search | SQLite FTS5 | |
| Images | `image` crate, resize on upload | |
| Auth | argon2 passwords, `webauthn-rs` passkeys | |
| WXR parsing | `quick-xml` | |
| Logging | `tracing` | |

## 7. Repository layout (proposal)

```
ferritecms/
├── Cargo.toml                 # workspace
├── crates/
│   ├── ferrite/               # binary: CLI (serve, import, backup, update)
│   ├── ferrite-core/          # content model, storage, cache, routing
│   ├── ferrite-http/          # axum server, REST API, auth
│   ├── ferrite-theme/         # template resolution + minijinja env
│   ├── ferrite-plugin-host/   # wasmtime host, capability linker, limits
│   ├── ferrite-sdk/           # guest SDK for Rust plugins (+ macros)
│   └── ferrite-import-wp/     # WXR importer
├── wit/                       # WIT interface definitions (source of truth)
├── admin/                     # admin SPA
├── themes/                    # starter themes
├── plugins/examples/          # example plugins (seo, analytics, hello)
└── docs/
```

## 8. Business model

- Open-source core, licensed `AGPL-3.0-only` with an additional permission for plugins and themes
  (`LICENSE-EXCEPTION.md`); decided 2026-11-01, before the first release, replacing a brief
  `MIT OR Apache-2.0` that was never released. A commercial license is offered by the
  copyright holder, SOFTBERRIES Krzysztof Grajek (softberries.dev; `LICENSING.md`), so
  contributors sign a CLA through CLA assistant (`CONTRIBUTING.md`).
- Managed hosting: tiny per-site footprint → high density, good margins.
- Plugin registry with signing + verified publishers (paid publisher accounts / revenue share).
- Paid WordPress → FerriteCMS migrations for agencies.
- Support contracts for EU public sector / organisations wanting self-hosted.

## 9. Risks & open questions

- **Ecosystem cold start** — the real moat of WP. Mitigation: excellent built-ins (SEO,
  forms, custom fields, redirects) so fewer plugins are needed; great SDK + docs.
- **EmDash** has Cloudflare's distribution. Our angle: self-hosted, no platform lock-in.
- **WIT/Component Model maturity** for non-Rust guests — verify current state of JS/Go/
  Python toolchains before promising them.
- **Hook API design** is the hardest part — too narrow and plugins can't do useful things,
  too wide and the sandbox is meaningless. Prototype with 3 real plugins first.
- Block editor in admin is a big UI project; consider adopting an existing editor
  (TipTap/ProseMirror, Editor.js) rather than building from scratch.
- License choice affects hosting competitors and contributors.
- Name/trademark: check crates.io, GitHub, EUIPO for "ferrite"/"ferritecms" conflicts.
