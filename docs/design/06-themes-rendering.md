# 06 — Themes & Rendering

## 1. Theme package

```
themes/blog/
├── theme.toml
├── templates/
│   ├── base.html
│   ├── index.html            # required: final fallback
│   ├── single.html
│   ├── single-post.html
│   ├── page.html
│   ├── archive.html
│   ├── taxonomy.html
│   ├── search.html
│   ├── 404.html
│   ├── 500.html              # optional, rendered with minimal context
│   └── partials/…
└── assets/                   # css, js, fonts, images — served at /_theme/<hash>/…
```

```toml
[theme]
name    = "blog"
version = "0.1.0"
author  = "FerriteCMS"
license = "MIT"

[menus]
primary = "Primary navigation"
footer  = "Footer"

[images]                                  # variant sizes this theme wants (03 §4)
card  = { width = 640, height = 360, crop = true }

[settings]                                # declarative, rendered by admin
accent_color = { type = "color", default = "#c2410c", label = "Accent colour" }
show_author  = { type = "bool", default = true }
```

Installed as an artifact (tar.zst of the folder) — same distribution path as plugins
([04 §8](04-cluster.md#8-plugins--themes-across-nodes-d11)). Starter themes are also
embedded in the binary so a fresh site works offline.

## 2. Template hierarchy

Resolution is a pure function `resolve(route, theme_files) -> template_name`, unit-tested
with table tests.

| Route | Candidates (first existing wins) |
|---|---|
| Single item | `single-{type}-{slug}.html` → `single-{type}.html` → `single.html` → `index.html` |
| Page | `page-{slug}.html` → `page-{id}.html` → `page.html` → `single.html` → `index.html` |
| Type archive | `archive-{type}.html` → `archive.html` → `index.html` |
| Taxonomy term | `taxonomy-{tax}-{term}.html` → `taxonomy-{tax}.html` → `taxonomy.html` → `archive.html` → `index.html` (`category-*`/`tag-*` aliases like WP) |
| Author | `author-{login}.html` → `author.html` → `archive.html` → `index.html` |
| Date archive | `date.html` → `archive.html` → `index.html` |
| Search | `search.html` → `index.html` |
| Front page | `front-page.html` → `home.html` (if posts page) / `page.html` → `index.html` |
| Not found | `404.html` → `index.html` |

## 3. Routing

- Permalink structure option (WP-style tokens): `/%year%/%month%/%slug%/`,
  `/%category%/%slug%/`, `/%slug%/` (default `/%slug%/` for posts, hierarchical
  `/%parent%/%slug%/` for pages).
- Router compiles the active structure to a matcher; a route resolves to
  `Route::{Single, Page, Archive, Term, Author, Date, Search, Front, Feed, NotFound}`.
- Lookup order: exact redirect → static core routes (`/_media`, `/_theme`, `/api`,
  `/admin`, `/feed`, `/sitemap.xml`, `/robots.txt`) → permalink matcher → 404.
- Feeds: RSS 2.0 + Atom for posts and terms; `sitemap.xml` index + per-type sitemaps.
  Core features (SEO basics shouldn't need a plugin).

## 4. MiniJinja environment

- `minijinja` 3.x, templates loaded from the theme artifact (in memory), compiled once
  per theme version. Auto-escaping on for `.html`.
- **No filesystem, no network, no `include` outside the theme.** Loader resolves only
  names inside the theme archive.
- Fuel: MiniJinja has a `fuel` feature to cap template execution — enable it with a
  generous limit to stop runaway loops in templates (verify feature in minijinja 3.x).
- Context objects are **lazy** (`minijinja::value::Object` impls) so `post.related(3)`
  etc. only query when used, and every access records its dependency key.

### Globals and helpers (fixed set)

| Name | Description | Dependency recorded |
|---|---|---|
| `site` | title, url, locale, description, settings of active theme | `option:site.*`, `theme` |
| `post` / `page` | current item: title, body (rendered blocks, post-filter), excerpt, url, date, author, terms, fields, featured image | `post:<id>` |
| `posts` | current listing (archive/search) with pagination | `archive:<type>` / `term:<id>` |
| `menu(name)` | menu tree | `menu:<name>` |
| `related(post, n)` | same-term posts | `archive:<type>` |
| `query(type=…, term=…, limit=…)` | read-only listing, limit ≤ 50 | `archive:<type>` |
| `asset(path)` | hashed theme asset URL | `theme` |
| `image(media, size)` | variant URL + width/height/srcset | `post:<media id>` |
| `head()` | renders structured head (core SEO + plugin `render.head` filters) | — |
| filters | `date(fmt)`, `excerpt(n)`, `slugify`, `markdown`? (no), `json`, `t` (i18n, later) | — |

## 5. Rendering pipeline

```
Route ─► core loads item(s) ─► blocks → HTML (Rust renderer, per block type)
      ─► plugin filters: content.body (html), content.excerpt (text)
      ─► build Context { site, post|posts, request } with DependencyRecorder
      ─► head(): core SEO head + render.head filter chain → structured tags → HTML
      ─► template render ─► render.footer filter on footer slot
      ─► Response { html, status, deps, cacheable }
```

Block rendering: blocks are JSON (`[{type:"paragraph", content:[…]}, …]`) using the
TipTap/ProseMirror JSON document model ([07](07-admin-api.md#editor)). Rust renderer
maps node types → HTML with escaping; unknown node types render their text content and
log once. Imported raw HTML is stored as an `html` block, sanitised on import.

## 6. Page cache

- Per node, in memory, `moka` (async, size-bounded by bytes) or an equivalent — chosen
  in M1.
- Key: `(host, path, normalized allowlisted query params, variant)`; variant =
  `anon` | `user:<id>` (when `cache.logged_in = "per-user"`).
- Value: body (compressed once with brotli + gzip variants stored lazily), headers,
  status, `deps: SmallVec<DepKey>`, created_at.
- **Dependency index**: `HashMap<DepKey, HashSet<CacheKey>>` maintained on insert/evict.
  Invalidation event with keys → remove all entries indexed under those keys.
- Coarse keys for listings: `archive:<type>` is invalidated whenever any item of that
  type is published/unpublished/changed in a listed field. Correctness over hit-rate.
- `theme.changed`, `plugins.changed`, `options.changed` (for non-trivial options) flush
  everything.
- Not cached: non-GET, logged-in (default), responses with `Set-Cookie`, 5xx, previews.
- Search results are cached with a 60 s TTL (configurable, `0` disables).
- Pages marked volatile by plugins ([05 §7a](05-plugin-host.md#7a-data-visibility-and-caching)) are cached with a short TTL.
- Scheduled publish: the publish job emits normal invalidation events.
- HTTP caching for downstream CDNs: `Cache-Control: public, max-age=0, s-maxage=<cfg>`
  default off; `ETag` from body hash, 304 support.

## 7. Error behaviour

- Template error in production: log with template name/line, respond 500 using
  `500.html` with minimal context, else built-in page. Never cached.
- Theme fails to load (bad archive / syntax error) on activation → activation refused
  with errors listed; on node startup → fall back to the embedded default theme and
  flag in admin.

## 8. Starter themes

M1: `blog`. M4: `docs`, `business`. All MIT, no build step required (plain CSS), WCAG
2.2 AA as a goal checked with automated tools (axe) in CI for the starter themes.
