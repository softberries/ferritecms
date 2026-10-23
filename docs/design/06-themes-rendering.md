# 06 — Themes & Rendering

Status: built in M1c-2 (`crates/ferrite-theme`, `crates/ferrite-http`, `crates/ferrite-cache`,
starter theme `themes/blog`). Sections say what is built and mark what is planned. The
controller rulings referenced as P1–P4 and R1–R31 are listed in
[`docs/superpowers/rulings/m1c2.md`](../superpowers/rulings/m1c2.md).

## 1. Theme package

```
themes/blog/
├── theme.toml
├── templates/
│   ├── base.html
│   ├── index.html            # required: final fallback
│   ├── single.html
│   ├── page.html
│   ├── archive.html
│   ├── taxonomy.html
│   ├── author.html
│   ├── date.html
│   ├── search.html
│   ├── 404.html
│   ├── 500.html              # optional, rendered with minimal context
│   └── partials/…
└── assets/                   # css, js, fonts, images — served at /_theme/<hash>/<path>
```

```toml
[theme]
name        = "blog"
version     = "0.1.0"          # semver
author      = "FerriteCMS"     # optional
license     = "MIT"            # optional
description = "…"              # optional

[menus]                                   # menu locations: name = label
primary = "Primary navigation"
footer  = "Footer"

[images]                                  # variant sizes this theme wants (03 §4)
card = { width = 640, height = 360, crop = true }

[settings]                                # declarative, rendered by the admin (M2)
accent_color = { type = "color", default = "#c2410c", label = "Accent colour" }
show_author  = { type = "bool", default = true }
```

Setting kinds are the plugin manifest's minus `secret` (`text`, `textarea`, `bool`, `number`,
`select`, `color`, `media`), because every setting value is visible to templates
(`crates/ferrite-theme/src/manifest.rs`).

**Loading (as built, plan ruling P1).** `Theme::load` reads every file into memory, validates
`theme.toml`, requires `templates/index.html` and compiles every template; template errors are
collected, each as `templates/<file>:<line>: <message>`. Limits: 2000 files, 20 MiB in total,
directory depth 16; symlinks are refused (`crates/ferrite-theme/src/package.rs`). The theme
hash is the first 16 hex digits of a SHA-256 over all files and is part of every asset URL.
Sources:

- embedded starter themes (`rust-embed` over the workspace `themes/` folder): `blog`;
- a folder `<data>/themes/<name>/`, which wins over an embedded theme of the same name.

The active theme's name comes from the `themes` table (`active` row), default `blog`
(`crates/ferrite/src/site.rs`). Theme names are `[a-z0-9_-]{1,64}`.

**Planned.** Installing themes as artifacts (tar.zst of the folder, the plugin distribution
path of [04 §8](04-cluster.md#8-plugins--themes-across-nodes-d11)) is M1.5; theme
install/activate commands and admin screens are M2. The theme is read once at startup; a
change needs a restart until hot swap on `theme.changed` lands (M1.5).

## 2. Template hierarchy

Resolution is a pure function, `resolve_template(key, has)` in
`crates/ferrite-theme/src/hierarchy.rs`, unit-tested with table tests. The first existing
candidate wins; `index.html` is always last.

| Route | Candidates (first existing wins) |
|---|---|
| Single item (non-page type) | `single-{type}-{slug}.html` → `single-{type}.html` → `single.html` → `index.html` |
| Page | `page-{slug}.html` → `page-{id}.html` → `page.html` → `single.html` → `index.html` |
| Type archive | `archive-{type}.html` → `archive.html` → `index.html` |
| Taxonomy term | `taxonomy-{tax}-{term}.html` → `taxonomy-{tax}.html` → `taxonomy.html` → `archive.html` → `index.html` |
| Author | `author-{login}.html` → `author.html` → `archive.html` → `index.html` |
| Date archive | `date.html` → `archive.html` → `index.html` |
| Search | `search.html` → `index.html` |
| Front page, static page (R17) | `front-page.html` → the full Page row (`page-{slug}` → `page-{id}` → `page` → `single`) → `index.html` |
| Front page, posts (R17) | `front-page.html` → `home.html` → `index.html` |
| Not found (404 and 410) | `404.html` → `index.html` |
| Server error | `500.html`, else the built-in page (no `index.html` fallback) |

WordPress aliases for terms: for `category`, `category-{slug}.html` is tried just before
`taxonomy-category-{slug}.html` and `category.html` just before `taxonomy-category.html`; the
same with `tag-…` for `tag` (and `post_tag`). The front page is a static page when the
`reading.front_page` option holds the id of a public item; otherwise it lists posts.

## 3. Routing

The router is a pure function, `resolve(path, query, table) -> Route`
(`crates/ferrite-http/src/route.rs`; its module docs are the full specification).

**Permalinks** (`crates/ferrite-core/src/permalink.rs`). Structures are `/`-separated
segments, each one token or one literal. Tokens: `%slug%` (required), `%year%` and `%month%`
(from `published_at`, else `created_at`, in UTC on purpose: R4), `%category%` (first
`category` term by position, `uncategorized` when none) and `%parent_path%` (ancestor slugs,
collapsing for top-level items). Defaults: posts `/%slug%/`, pages `/%parent_path%/%slug%/`,
every other public type `/{type}/%parent_path%/%slug%/`. Options `permalink.post` and
`permalink.page` override the post and page structures (the page structure must contain
`%parent_path%`). Segments are percent-encoded (UTF-8, uppercase hex, RFC 3986 unreserved
characters kept); paths end in `/`. `serve` builds the rules from the options at startup and
rejects a structure whose first literal is a type or taxonomy name, a router word or a year
(`PermalinkRules::check_types`).

**Root namespace** (R3, R13). Posts under `/%slug%/` and top-level pages share `/<slug>/`;
core allocates slugs so they do not collide (`about-2`), and the router tries a page before a
post as a safety net. Top-level slugs may not be reserved words (`admin`, `api`, `feed`,
`_media`, `_theme`, `_p`, …), type or taxonomy names, `author`, `page`, `search`, or four
digits, so static routes never hide content.

**Canonical form.** Each segment is percent-decoded once and re-encoded, so a request
compares byte for byte with stored permalinks. Fixed words match ASCII case-insensitively,
slug positions are lowercased, logins keep their case. A request whose path is not its
route's canonical path gets a 301 to it (query kept, encoded to visible ASCII). Rejected with
404: paths over 2048 bytes or 32 segments, `.`/`..` segments (raw or decoded), empty interior
segments (`//x`), malformed escapes, invalid UTF-8, control characters, `\`, and `%2F` inside a
segment.

**Static routes.** `/`, `/feed/`, `/robots.txt`, `/sitemap.xml`, `/sitemap-<kind>-<n>.xml`,
`/{taxonomy}/{slug}/`, `/{taxonomy}/{slug}/feed/`, `/author/{login}/`, `/{yyyy}/`,
`/{yyyy}/{mm}/`, `/{type}/` (types with an archive), `/_media/{hash}/{name}`,
`/_theme/{hash}/{path…}`, `/healthz`, `/readyz`. Listings take `/page/N/` with `N` in
`2..=1000`; `/page/1/` redirects to the path without it. `?s=` on `/` is a search. Whatever is
left goes to the permalink matcher, which returns the candidates in order (pages, then posts,
then other types); a hit counts only when the item's computed URL equals the request path, so
a wrong year, month or category is a 404.

**Request order** (`crates/ferrite-http/src/app/handler.rs`): methods other than GET/HEAD →
405; `/healthz`, `/readyz`, `/_media`, `/_theme` (never redirected, not in the page cache) →
redirect table (exact path, then the trailing slash toggled; before the cache) → canonical
301 → page cache → render. `410` redirect rows answer with the shared gone page.

**Feeds and sitemaps** (`crates/ferrite-http/src/app/feeds.rs`): RSS 2.0 for posts (`/feed/`)
and for terms, the latest 20 public items each; a term without public posts is 404 (R7).
`/sitemap.xml` indexes per-type files of up to 50 000 URLs with `lastmod`, published items
only. `robots.txt` disallows everything when option `reading.discourage_indexing` is `true` and
names the sitemap index. Feeds, sitemaps and `robots.txt` go through the page cache. Atom feeds
are not built (deferred, ROADMAP M4).

**Not built:** `/api`, `/admin` and plugin routes `/_p/…` (M2).

## 4. MiniJinja environment

As built (`crates/ferrite-theme/src/render.rs`, module docs):

- `minijinja` 3.0.2 (pinned in the workspace `Cargo.toml`, features include `fuel` and
  `urlencode`; no `debug` feature, so errors carry name and line but not source snippets).
- **No loader, no filesystem, no network.** Templates are compiled into the theme's
  environment at load; `include`/`extends`/`import` resolve names against those templates
  only, verbatim, so `../../etc/passwd` is just an unknown template name.
- **Auto-escape (R15):** HTML escaping for every template except names ending in `.txt`,
  `.css` or `.js`.
- **Undefined is chainable:** `post.author.login` without a `post` prints nothing.
- **Fuel:** 5 000 000 instructions per render (`FUEL`). **Output cap:** 8 MiB
  (`MAX_OUTPUT`), enforced by a capped writer. Exhausted fuel, the output cap, the recursion
  limit and MiniJinja's own size guards are `ThemeError::Limit`.
- **Known gap (R14):** fuel counts instructions, not bytes or CPU time, so a template can
  build large intermediate strings. Accepted because themes are installed by administrators;
  revisit in M1.5 when third-party theme artifacts install.
- **Preloaded data (plan ruling P2).** MiniJinja is synchronous, so everything a template
  sees is loaded into the render context first, through `SiteReader` (published-only). The
  data-backed helpers go through a `HelperBridge` that calls the async store with
  `Handle::block_on` from the blocking render thread, at most 20 calls per render
  (`MAX_BRIDGE_CALLS`); after that they return nothing and one warning is logged. Same-name
  `menu()` calls are memoised, and `image()` of media already in the context needs no call.
  The objects are not lazy (the original idea of `minijinja::value::Object` impls querying on
  access was replaced by P2).
- **Render slots (R28, R32):** at most `2 × available parallelism` pages are built at once. A
  content page takes its slot before loading data and holds it through the plugin filters,
  the head and the template render (on a blocking thread), so plugin calls are bounded by the
  same limit; error pages take a slot for their template render only. A request waits up to
  5 s for a slot, then gets a 503 with `Retry-After: 5` (never cached).

### Globals and helpers (fixed set, as built)

| Name | Description | Dependency keys recorded |
|---|---|---|
| `site` | `title`, `url`, `description`, `locale`, `settings` (every declared theme setting with its value) | `option:site.title`, `option:site.description`, `option:site.locale`, `theme` (`site.url` is config, not an option: no key); the HTTP layer adds `option:theme.<name>.settings` |
| `post` | current item: `id`, `kind`, `slug`, `title`, `excerpt`, `url`, `body` (sanitised HTML after plugin filters, marked safe), `date`, `updated` (RFC 3339, UTC), `menu_order`, `parent_id`, `fields`, `author`, `terms`, `featured` | `post:<id>`, `user:<author>`, `term:<id>` per term, `post:<featured media>` |
| `page` | same object as `post`, set when the item's kind is `page` | as `post` |
| `posts` | current listing: `items`, `page`, `per_page`, `total`, `pages`, `prev_page`, `next_page` | per item as `post`; the read adds `archive:<kind>` / `term:<id>` |
| `archive` | listing routes only (R30): `kind` (`front`, `type`, `term`, `author`, `date`, `search`), `content_type`, `term`, `author`, `year`, `month` | `term:<id>` / `user:<id>` |
| `pagination` | listing routes only (R30): `page`, `pages`, `prev_url`, `next_url` (ready-made, encoded links; `none` at either end) | — |
| `request` | `path`, `s` (the search query), `page`, `status`, `preview` | — |
| `menu(name)` | menu tree; links to non-public items, and to terms without a public post, removed. Item targets load in one batch (`SiteReader::links`: titles and URLs only, no body render); each term target is one existence probe (`SiteReader::term_link`) | `menu:<name>`, plus `post:<id>`/`term:<id>` for every target, removed ones included, and `archive:post` for term targets |
| `related(post, n=3)` | public items of the same type sharing a term with `post`, newest first per term; `n` ≤ 10 | `archive:<kind>`, `post:<item>` |
| `query(type=…, term="<tax>/<slug>", limit=10)` | public items, newest first; `limit` ≤ 50; with `term` the result is also capped at `reading.per_page` (page 1 of the term archive) | `archive:<kind>`, `term:<id>` |
| `asset(path)` | `/_theme/<theme hash>/<path>`, each segment percent-encoded | `theme` |
| `image(media, size)` | the `size` variant of a public media item (the original for an unknown size) with `width`, `height` and a `srcset` of one candidate per width | `post:<media id>` |
| `head()` | the pre-rendered `<head>` HTML (core SEO + `render.head` plugin filters, §5) | — |
| filters | `date(fmt)` (chrono strftime, UTC), `excerpt(n=55)` (plain text), `slugify` (core's slug function), `json` (safe in `<script>` and in **single-quoted** attributes only), MiniJinja's `urlencode`; `t` (i18n) is not built | — |

MiniJinja builtins that remain: `range` (≤ 100 000 items), `dict`, `namespace`, the standard
filters and tests. `debug()` is removed. `site` exposes only the four whitelisted options and
the active theme's settings, which are stored as one JSON object in option
`theme.<name>.settings` (plugin settings live under `plugin.<name>.settings.<key>`, 05 §4c).

The dependency keys of the read that produced the context (ancestors, listings, options read
such as `reading.per_page` and `reading.front_page`) are merged by the HTTP layer, which also
adds `theme` and `plugins` to every page. The full key grammar and the emitter/renderer
contract are in `crates/ferrite-core/src/events.rs`.

## 5. Rendering pipeline (as built)

```
Route ─► SiteReader loads item(s), published-only (blocks were rendered to sanitised HTML by core)
      ─► plugin filters on every item: content.body (html), content.excerpt (text)
      ─► head(): core SEO tags + render.head filter chain → structured head → HTML (host-escaped)
      ─► template by hierarchy ─► Renderer::render on a blocking thread with the HelperBridge
      ─► CachedPage { status, html, deps, ttl }
```

`crates/ferrite-http/src/app/pipeline.rs`. Plugin calls go through
`PluginHost::apply_filter_traced`, which returns the dependency keys the plugin's reads
recorded and whether a call was volatile or failed ([05 §7a](05-plugin-host.md#7a-data-visibility-and-caching)).
The core head (`crates/ferrite-http/src/head.rs`) has the title, a canonical link, Open Graph
`og:title`/`og:description`/`og:url`/`og:type` (plus `og:image` for a featured image) and a
meta description from the item's `seo.description` meta, else its excerpt cut to 160
characters, else the site tagline. `render.footer` is not wired yet.

Block rendering: blocks are JSON (`[{type:"paragraph", content:[…]}, …]`) using the
TipTap/ProseMirror JSON document model ([07](07-admin-api.md#editor)). The Rust renderer in
`ferrite-core` (`blocks`) maps node types to HTML with escaping; raw `html` blocks are
sanitised with `ammonia`.

**Previews.** `SiteReader::preview(actor, id)` and the preview request scope for plugins exist
(05 §7a), but no route serves previews yet: that needs authentication (M2). Preview responses
are never cached.

## 6. Page cache (as built)

`crates/ferrite-cache` (crate docs).

- Per node, in memory, `moka` (future cache), bounded by bytes (`cache.max_bytes`, default
  256 MiB); an entry weighs its body plus key, header, ETag and dependency-key bytes.
- Key: `(host, path, normalised allowlisted query, variant)`. The host is always the
  configured `site.url` host, never the `Host` header (R25). The only allowlisted query
  parameters are `s` and `page`. Variant: `anon` only; there is no login yet, and
  `cache.logged_in = "per-user"` is parsed but unused until M2.
- Value: status, headers, uncompressed body, ETag (16 hex digits of SHA-256 over the body),
  dependency keys, optional TTL. Compression (br, gzip) happens per response in the HTTP layer,
  so the ETag is sent weak (`W/"…"`) and every HTML response, 304 included, has
  `Vary: Accept-Encoding`. `If-None-Match` → 304 (weak comparison).
- **Dependency index**: each page's keys index its cache key; `PageCache::invalidate(keys)`
  drops every page indexed under any of them, `all` drops everything. A page whose keys were
  invalidated while it rendered is not stored (staleness guard), and concurrent misses for one
  key render once (single flight).
- **Invalidation** comes only from the `events` outbox through the polling loop
  ([04 §4](04-cluster.md#4-event-bus--cache-invalidation)): `invalidate` and `options.changed`
  drop their keys; `theme.changed`, `plugins.changed` and `plugin.disabled` flush; unknown kinds
  drop their keys. Coarse `archive:<type>` keys cover listings (correctness over hit rate).
- Not cached under the request path: error statuses (≥ 400), responses with `Set-Cookie`,
  previews, the 500 page and 503. Only GET and HEAD reach the cache.
- **Shared error pages (R26):** the 404 and 410 bodies are rendered without the request path
  and cached under internal keys with their dependency keys, so random missing URLs render
  once and a menu edit still refreshes them. Nothing is cached under a missing URL's path.
- **Short TTL**: search results and pages whose plugin calls were volatile or failed, or whose
  theme helper reads failed, are cached for 60 s (`SHORT_TTL`; a constant, not configurable
  yet).
- Scheduled publish: `serve` runs `publish_due` every 30 s; its writes emit normal events.
- **Planned:** `Cache-Control: public, max-age=0, s-maxage=<cfg>` for CDNs is not sent (HTML
  pages carry no `Cache-Control`); per-user variants (M2).

## 7. Error behaviour (as built)

- Template error or failed read: logged (`templates/<file>:<line>` for template errors), then
  500 using the theme's `500.html` with minimal context, else a built-in page; a failing
  `500.html` falls back to the built-in page. Never cached. A panicking handler gets the
  built-in 500 (tower-http `CatchPanic`). The server keeps serving.
- 404: the theme's not-found hierarchy, shared body (§6). 410 redirect rows: the same template
  with `request.status` 410.
- A theme that fails to load at startup (bad manifest, template syntax error, unknown or
  invalid name) is logged and the embedded `blog` theme is used, so a broken theme never takes
  the site down (`crates/ferrite/src/site.rs`). Flagging it in the admin and refusing a broken
  theme on activation are M2 (no admin or activation flow yet).
- Security headers on every response unless it sets them itself: `X-Content-Type-Options:
  nosniff`, `Referrer-Policy: strict-origin-when-cross-origin`, `X-Frame-Options: SAMEORIGIN`.

## 8. Starter themes

M1: `blog` (built, `themes/blog`, MIT, plain CSS, no build step, light and dark schemes).
M4: `docs`, `business`. WCAG 2.2 AA is a goal; automated checks (axe) in CI for the starter
themes are not set up yet.
