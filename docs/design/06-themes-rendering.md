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
homepage    = "https://github.com/softberries/ferritecms"  # optional; http:// or https:// only
description = "…"              # optional

[menus]                                   # menu locations: name = label
primary = "Primary navigation"
footer  = "Footer"

[images]                                  # variant sizes this theme wants (03 §4)
card = { width = 640, height = 360, crop = true }

[settings]                                # declarative; API M2a-2a, admin form M2b
accent_color = { type = "color", default = "#c2410c", label = "Accent colour" }
show_author  = { type = "bool", default = true }
```

`author`, `license`, `homepage` and `description` are free text shown by the admin
(`GET /themes`, 07 §1) as text; `homepage` must be an `http://` or `https://` URL with a
host part, no whitespace and at most 2048 bytes, else `theme.toml` is refused (M2b-3), so
the admin can link it. The rule is `ferrite_domain::is_web_url`, shared with the plugin
manifest's `plugin.homepage` (05 §3).

Setting kinds are the plugin manifest's minus `secret` (`text`, `textarea`, `bool`, `number`,
`select`, `color`, `media`), because every setting value is visible to templates
(`crates/ferrite-theme/src/manifest.rs`). **Settings as built (M2a-2a P13):** one schema and
validator for themes and plugins (`ferrite_core::ext_settings`, 05 §4c). The theme row
stores the schema in `manifest.settings` (an array of definitions; a row stored before
M2a-2a falls back to its artifact's `theme.toml`, an embedded theme without a row to its
files). Values are one option `theme.<name>.settings`, an object of the set keys, written
by `PATCH /themes/{name}/settings` (07 §1) under the `extensions` lock with
`options.changed`; templates see every declared key (`site.settings`), the stored value or
the default. Every page of the theme records `option:theme.<name>.settings`, so a change
flushes them on every node; values are read per cache miss (no option cache, P14).

**Loading (as built, plan ruling P1).** `Theme::load` reads every file into memory, validates
`theme.toml`, requires `templates/index.html` and compiles every template; template errors are
collected, each as `templates/<file>:<line>: <message>`. Limits: 2000 files, 20 MiB in total,
directory depth 16; symlinks are refused (`crates/ferrite-theme/src/package.rs`). The theme
hash is the first 16 hex digits of a SHA-256 over all files and is part of every asset URL.
Sources (`ThemeSource`):

- embedded starter themes (`rust-embed` over the workspace `themes/` folder): `blog`;
- a folder `<data>/themes/<name>/` (solo mode only, see below);
- a **bundle** (`ThemeSource::Bundle`), the artifact a theme is installed as.

**Bundles (as built, M1.5, ruling P17).** `ferrite_theme::pack` writes a theme's files as a
plain, deterministic ustar archive: regular-file entries only, sorted by path, mode `0644`,
mtime 0, uid/gid 0, empty user/group names, no PAX/GNU extension headers, paths at most 255
bytes (ustar prefix + name). The same files give the same bytes whatever the folder's
metadata, so the artifact hash (SHA-256 of the bundle, kind `theme`, mime
`application/x-tar`) identifies the content. Not `tar.zst`: zstd needs a C library and
themes are at most 20 MiB; the mime type leaves room for a compressed format later.
`Theme::load(Bundle)` unpacks in memory, never to disk, treating the bytes as hostile: it
reads the raw entry stream (no PAX/GNU processing, so no header can rename or resize the
next entry) and refuses, naming the entry, anything but ustar regular files — symlinks,
hard links, devices, FIFOs, directories, extension headers — plus absolute paths,
`..`/`.`/empty segments, backslashes, duplicate paths, paths over 255 bytes, depth over 16,
and more than 2000 entries or 20 MiB. Sizes are checked against the header before an entry
is read, and a bundle larger than the limits allow (`MAX_BUNDLE_BYTES`, 20 MiB + 1 KiB per
entry) is refused unparsed. Hidden files are skipped as in a folder; the theme hash is
computed over the files exactly as for a folder, so the same files give the same
`Theme::hash()` from any source (`crates/ferrite-theme/src/bundle.rs`).

**Packing a folder (as built, M2b-3 P42).** `ferrite theme pack --dir DIR --out FILE`
writes `ferrite_theme::pack` of a folder (hidden files, any path segment starting with
`.`, left out), loads the result as an upload would (unpacked and compiled; a theme that
does not load is refused, exit 1, and nothing is written) and prints `packed <n> files,
<bytes> bytes, sha256 <hex>`. It needs no data folder, config or database (09 §8), so an
API user can produce the file `POST /themes` takes; a `tar cf` archive is refused (it has
directory entries and pads to a 10 KiB record). The admin packs a dropped folder in the
browser with a TypeScript port of `pack`; both packers are pinned byte for byte to
`admin/src/appearance/__fixtures__/blog-min.tar`, the command's output for the
`blog-min/` folder next to it (a nested asset, a path over 100 bytes that needs the ustar
prefix field, a hidden file that is skipped; `crates/ferrite/tests/theme_pack.rs`).

**Install and selection (as built, M1.5).** `ferrite::extensions::install_theme_dir` packs
a folder, loads the bundle (a theme that does not compile is refused before anything is
written), uploads it with `MediaStore::put_artifact` and records the artifact and theme rows
through `ExtensionService::install_theme` (one transaction under the `extensions` lock,
P21). At startup `load_theme` (`crates/ferrite/src/site.rs`) uses the active row's artifact
(fetched, SHA-256 checked against the row, unpacked, its manifest name must match the row);
if that fails, or there is no row (name `blog`), it tries `<data>/themes/<name>/` (**solo
only**), then the embedded theme of that name, then the embedded `blog` theme, so a broken
theme never takes the site down; the failure is logged and kept in `LoadedTheme::error`. In
**cluster** mode `<data>/themes/` is never read (a warning says so if it exists): every node
must run the same theme (C5). `reload_theme` is the hot-swap counterpart (P18): no change
when the active row names the artifact already running, an error (the caller keeps the
current theme) when the new artifact fails, never a fallback to `blog` at runtime.
Theme names are `[a-z0-9_-]{1,64}`.

**Install, activation and the fallback flag (as built, M2a-2a P8, P12).** Uploads (`POST
/themes`, a raw bundle) and `ferrite theme install --dir` go through
`ferrite_http::extensions::Installer::stage_theme`: the bundle is bounded
(`MAX_BUNDLE_BYTES`) before it is read, unpacked in memory with the bundle rules above and
every template compiled before anything is stored. Activation (`POST
/themes/{name}/activate`, `ferrite theme activate`) loads the stored artifact first
(fetched, SHA-256 checked, unpacked, compiled, name checked); a theme that does not load
is refused (`422`, CLI exit 1) and the active theme, and what nodes serve, stay as they
are, with no `theme.changed`. The activation then records only if the row still names
the artifact just loaded (checked under the `extensions` lock; else `409 conflict`, R10).
An embedded theme without a row is installed from the embedded files when activated; an
upload named like an embedded theme becomes that theme's row. Each node reports in
`nodes.meta.theme` the theme it serves plus `fallback`: `true` when there is an active row
and the node does not serve its artifact (a startup fallback, or a failed hot swap that
keeps the previous theme), `false` otherwise (also when no row is active and the embedded
`blog` serves). `GET /themes` shows it per live node on the active entry. A node's flag
can lag a change by up to one heartbeat (10 s). There is no theme delete (P30): an unused
theme row and its artifact stay until the artifact GC (M4).

**Hot swap (as built, M1.5, P18).** On every node the reloader task
(`crates/ferrite/src/reload.rs`) calls `reload_theme` after a `theme.changed` event (and
after lost event history), off the invalidation loop. A new theme replaces the old one
with `AppState::swap_theme` and the page cache is flushed again after the swap (the loop
already flushed on the event). The replaced theme is kept as `previous_theme`, so
`/_theme/<old hash>/…` URLs in pages rendered just before the swap keep working until the
next swap. A theme that fails to load (missing or tampered artifact, template error)
leaves the current theme serving, is reported in `nodes.meta.theme.error`, and is retried
after 5 s, doubling to 5 min (R14). A startup fallback (to the folder, embedded or `blog`
theme) after the active row's artifact failed is retried the same way (R17). The themes API and the `ferrite theme` commands are built
(M2a-2a, above); the admin screens are M2b.

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
`_media`, `_theme`, `_preview`, `_p`, `_comment`, …), type or taxonomy names, `author`, `page`, `search`, or four
digits, so static routes never hide content. A child item (one with a parent) may not take
`page` (`/{path}/page/{n}/` is a listing page). A caller slug that breaks these rules is
refused (`reserved slug`); a generated one gets `-2`. An update that keeps an item's slug and
parent never refuses or renames it, even when a later rule (or a row written another way)
makes the stored slug reserved. Both checks read the registered types once per write.

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
`/_theme/{hash}/{path…}`, `/_preview/{id}` (`id` a lowercase hyphenated UUID, no trailing
slash; anything else under `/_preview/` is 404), `/healthz`, `/readyz`. Listings take `/page/N/` with `N` in
`2..=1000`; `/page/1/` redirects to the path without it. `?s=` on `/` is a search. Whatever is
left goes to the permalink matcher, which returns the candidates in order (pages, then posts,
then other types); a hit counts only when the item's computed URL equals the request path, so
a wrong year, month or category is a 404.

**Term archives** (P25). `/{taxonomy}/{slug}/` lists the public items of every public type the
taxonomy applies to (its `content_types` but media, at most 20, through `ContentQuery.kinds`),
newest first, so a taxonomy used only by `book` has archives too. The read records `term:<id>`
and `archive:<kind>` for each listed type. A term without public items, or a taxonomy without a
public type, is 404 (R7); the menu probe `SiteReader::term_link` applies the same rule.
Renaming a term (`TaxonomyService::update_term` with a new slug) stores a `301` (origin
`slug-change`) from the old archive path to the new one in the same transaction, with the
content-move rules: redirects to the old path are re-targeted (no chains), redirects from the
new path (either slash variant) are deleted, and the `redirect:` keys are emitted. It does so
only when the term had a public archive (a public item with the term), so no redirect reveals a
term the site does not show (R7). Because redirect rows outrank every route, creating a term and
every slug change (with or without a public archive) delete the redirects from the term's new
archive path, either slash variant and any origin, in the same transaction, as publishing does
for content; the `redirect:` keys are emitted. A taxonomy applies to at most 20 content types
(`content_types`, refused beyond). Not handled: items whose permalink contains `%category%` move
with the term without redirects (permalink structure redirects are M3, P26).

**Request order** (`crates/ferrite-http/src/app/handler.rs`): methods other than GET/HEAD →
405; `/healthz`, `/readyz`, `/_media`, `/_theme` (never redirected, not in the page cache) →
`/_preview/<id>` (before the redirect rows, so none can shadow it; never cached) → redirect
table (exact path, then the trailing slash toggled; before the cache) → canonical 301 →
session cookie (logged in or anonymous) → page cache (or none: logged-in bypass) → render.
`410` redirect rows answer with the shared gone page. A hit on a redirect row is counted in
the node's bounded buffer (`app/hits.rs`, §6), never with I/O on the request.

**Previews** (P20). `GET /_preview/<id>` takes the session cookie only (`__Host-ferrite_session` on an `https` site, 07 §2) (no bearer
tokens) of an account that need not reset its password, and loads the item with
`SiteReader::preview(actor, id)`: any status but trash, readable by the actor under the API's
rule (R17/R23 `can_read`: own items, items publicly visible now (published with
`published_at <= now`), or `edit_others`). A contributor previewing
another author's draft, no or a bad session, a trashed or missing item and a malformed id all
get the shared 404 page. The item is rendered through the single-item pipeline with the
plugin preview scope (05 §7a) and `request.preview = true` (the blog theme shows a notice),
never through the cache; every preview answer carries `Cache-Control: private, no-store` and
`X-Robots-Tag: noindex`, and no `ETag` (`If-None-Match` is ignored).

**Logged-in requests** (P21). A request whose session cookie resolves to a session
is logged in; without the cookie no session lookup happens, and a cookie that resolves to
nothing (unknown, expired, logged out, disabled user) or a failed lookup is ignored (the
request is anonymous, no `Set-Cookie`, never a 401). Plugins see `is_logged_in = true`.
`cache.logged_in = "bypass"` (default): the page renders on every request without reading or
filling the cache. `"per-user"`: cached under the user's own variant (§6). Either way the
answer is `Cache-Control: private, no-store`, and logged-in requests never read or fill the
anonymous entries.

**Feeds and sitemaps** (`crates/ferrite-http/src/app/feeds.rs`): RSS 2.0 for posts (`/feed/`)
and for terms, the latest 20 public items each; a term without public posts is 404 (R7).
`/sitemap.xml` indexes per-type files of up to 50 000 URLs with `lastmod`, published items
only. `robots.txt` disallows everything when option `reading.discourage_indexing` is `true` and
names the sitemap index. Feeds, sitemaps and `robots.txt` go through the page cache. Atom feeds
are not built (deferred, ROADMAP M4).

**Plugin routes** `/_p/<plugin>/…` are axum routes in front of the public fallback (M2a-2a,
05 §4b): never cached, never seen by redirects. **Not built:** `/admin` (M2b). The REST API lives under
`/api/v1` in the same process (07 §1); `/api` itself and other `/api/…` paths are ordinary
public paths (canonical slash, then the shared 404).

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
- **Known gap (M1c-2 R14, decided in M1.5 by plan ruling P23):** fuel counts
  instructions, not bytes or CPU time, so a template can build large intermediate strings
  and run the process out of memory. It stays open until M4 (render worker): a per-thread
  counting allocator cannot fail one allocation gracefully (a `GlobalAlloc` returning null
  aborts the process), and MiniJinja has no check point other than fuel where a budget
  could stop a render; the fix is rendering in a separate worker process with an OS memory
  limit. M1.5 added no untrusted install path; M2a-2a adds the upload API (`POST /themes`,
  `manage_themes`, so admins only) and `ferrite theme install`. A theme is compiled, not
  rendered, before it is stored or activated, so a template that only exhausts memory at
  render time still passes. With hot swap a hostile theme reaches every node at once.
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
| `request` | `path`, `s` (the search query), `page`, `status`, `preview` (the comment page `?cpage=N` is not exposed; `comments()` reads it) | — |
| `menu(name)` | menu tree; links to non-public items, and to terms without a public item, removed. Item targets load in one batch (`SiteReader::links`: titles and URLs only, no body render); each term target is one existence probe (`SiteReader::term_link`) | `menu:<name>`, plus `post:<id>`/`term:<id>` for every target, removed ones included, and `archive:post` plus `archive:<kind>` per type of the taxonomy for term targets |
| `related(post, n=3)` | public items of the same type sharing a term with `post`, newest first per term; `n` ≤ 10 | `archive:<kind>`, `post:<item>` |
| `query(type=…, term="<tax>/<slug>", limit=10)` | public items, newest first; `limit` ≤ 50; with `term`, exactly up to `limit` items of `type` with the term in one query (`SiteReader::term_items`, any type the taxonomy applies to, not capped by `reading.per_page`); a type the taxonomy does not list gets none | `archive:<kind>`, `term:<id>` |
| `asset(path)` | `/_theme/<theme hash>/<path>`, each segment percent-encoded | `theme` |
| `image(media, size)` | the `size` variant of a public media item (the original for an unknown size) with `width`, `height` and a `srcset` of one candidate per width (`MediaPublic::srcset_candidates`, shared with inline images below: of several files of one width the requested size wins, then the original) | `post:<media id>` |
| `head()` | the pre-rendered `<head>` HTML (core SEO + `render.head` plugin filters, §5) | — |
| `footer()` | the plugins' footer HTML (`render.footer`, §5; sanitised like `content.body`); `""` without footer plugins and on error pages. `blog` prints it just before `</body>` | — |
| `comments(post)` | one page of the public thread of a **public post** (other kinds, drafts, unknown ids: `none`), through `SiteReader::comments` (M2b-2 P34): the page is the request's `?cpage=N` on a single item (a positive integer, anything else is 1; past the last page shows the last). A page holds `comments.per_page` top-level threads (default 50, 1..=100), oldest first, each with all its approved replies; a **root** is an approved comment whose parent is null or not approved (a reply under a spam or held comment starts its own thread). A page always shows every one of its top-level threads; its rows are capped at 2,000 (`MAX_PAGE_ROWS`), and past that only replies are cut: the roots first, then replies in thread order (depth first, oldest first) until the budget is used, so the latest replies of a page's later threads may not show (a shown reply's parent always shows). `open` (`comments.enabled`, default on; a closed thread still lists its comments, as WordPress does, and the theme hides the form), `count` = `total` (every approved comment of the post, not only this page's), `page`, `pages` (at least 1), `per_page`, `prev_url` / `next_url` (the post's permalink, `?cpage=N` past page 1, without a fragment: the theme adds its section's anchor, `blog` links to `…#comments`; `none` at either end), `items` in thread order (each reply right after its parent, siblings oldest first), `action` (`/_comment`), `honeypot` (`website`). Each item: `id`, `parent_id`, `depth` (0 at the top level), `author_name` (plain text), `author_url` (`http(s)` only, else `none`), `html` (the plain-text body escaped by core into `<p>`/`<br>`, marked safe), `date` (RFC 3339), `can_reply` (a reply would be accepted: core's own depth rule over the stored chain, so it holds when a parent is hidden). Approved comments only; never an e-mail or address hash. One bridge call | `post:<id>`, `option:comments.enabled`, `option:comments.per_page`; with more than one page the permalink's term and ancestor keys |
| `plugin_route(name, path)` | `/_p/<name><path>` (05 §4b), each path segment percent-encoded; `""` unless `name` is a plugin name (`[a-z0-9-]{2,48}`) and `path` starts with `/` and has no `.`/`..` segment | — |
| filters | `date(fmt)` (chrono strftime, UTC), `excerpt(n=55)` (plain text), `slugify` (core's slug function), `json` (safe in `<script>` and in **single-quoted** attributes only), MiniJinja's `urlencode`; `t` (i18n) is not built | — |

**Inline images (M2b-3 Task 4, the follow-up of M2b-2 R28).** A body `image` node whose
`src` is `ferrite:media/<id>` of a public media item renders as `image()` does
(`Hydrator::inline_image` in `ferrite-core`), when the item is a raster image (JPEG, PNG,
GIF, WebP) whose dimensions the `media.variants` job has recorded:

- `srcset` lists the original and every ready variant of the same shape, one per width
  with `w` descriptors (`MediaPublic::srcset_candidates`). A variant cropped to another
  aspect ratio (the core `thumbnail`, the `blog` theme's `card` and `hero`) is left out:
  a browser picking it would show a different picture.
- `src` is the widest of those no wider than the theme's largest size (the widest
  `[images]` entry of the served theme, `SiteReader::set_image_width`, set by the node
  whenever it starts serving a theme), else the original; without theme sizes the
  original's width is the largest size.
- `sizes="(max-width: <largest>px) 100vw, <largest>px"`, `width`/`height` from the media
  item's dimensions, `loading="lazy"`, `decoding="async"`, after `alt` (and `title`).

An SVG, any other type and an image not measured yet keep the plain `<img src alt>` of
the original; a missing or non-public item drops the image, as before. The media item's
`post:<id>` is already a dependency of the page, and `record_variants` invalidates it, so
a page rendered before the variants were ready is refreshed when they are. A theme swap
flushes the page cache, so pages follow the new theme's largest size.

**The comment form (M2a-2b P12, P13).** Themes post a plain HTML form (no script) to
`POST /_comment` (`crates/ferrite-http/src/app/comment_form.rs`), form-encoded, with the
fields `post`, `parent` (a reply), `author`, `email`, `url`, `comment` and the honeypot
`website`, which the theme hides from people (off screen, `aria-hidden`, `tabindex="-1"`,
`autocomplete="off"`). The form carries no per-visitor token, so cached pages stay shared;
what protects it is the origin check (`Origin`, else `Referer`, must be `site.url`'s origin,
else `403`) and that every form comment is **anonymous** (the session cookie is ignored).
The body is at most 64 KiB (`413`), must be `application/x-www-form-urlencoded` (`415`),
and the same per-IP limits as the API submit apply (3 a minute, 20 an hour, `429` with
`Retry-After`; no client address `503`). Success is a `303` to the post's permalink (never a
client-supplied URL) with `?cpage=N#comment-<id>` when approved (`N` the comment page that
holds it: its thread's root position divided by `comments.per_page`, plus one; no query on
page 1), or `#comment-pending` when held or
dropped by the honeypot (the same answer, so a bot cannot tell); the `blog` theme shows its
"awaiting moderation" notice under `:target`. Validation errors are a `422` and the limit a
`429`, each a small built-in HTML page with the escaped message and a link back; an unknown,
non-public or closed item is `404`. Every answer is `Cache-Control: no-store`. A pending
comment is never shown, not even to its author (later); an approval, moderation or delete
invalidates `post:<item>`, so cached pages show the change after the next poll.
Moderation follows `comments.moderation` (07 §1 "Comments"): with the default `known`
(M2a-2b R2, WordPress's default) a guest's first comment is held, and once an admin or
editor approves it the same name and e-mail post directly (at most 2 links); `all` holds
every form comment. Since form comments are anonymous, a moderator commenting through the
theme is moderated like a guest.

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
      ─► footer(): render.footer filter chain over "" (html), once per page
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
characters, else the site tagline. The title is the item's `seo.title` meta when set (the
whole `<title>` and `og:title`, no site suffix; M2b P28), else `<item title> – <site
title>` (`og:title` the item title), else the site title. Blank or non-string meta values
count as unset. Both keys come from one meta read per item page, covered by the page's
`post:<id>` dependency (every content write, an SEO-only one included, invalidates it);
the keys and limits are core's (`ferrite_core::content::seo`).

`render.footer` (Task 14) runs once per page, after the item filters and before the head,
with the input `""`; every filter of the page (items, footer, head) shares the page's render
budget (P22). Its output is `html` and sanitised by the host like `content.body` (unless a
plugin is `render.html = "raw"`), and a failing plugin is skipped, so the footer is what the
others returned. The result reaches templates as `footer()` (marked safe). Error pages get an
empty footer (no plugin calls).

Block rendering: blocks are JSON (`[{type:"paragraph", content:[…]}, …]`) using the
TipTap/ProseMirror JSON document model ([07](07-admin-api.md#editor)). The Rust renderer in
`ferrite-core` (`blocks`) maps node types to HTML with escaping; raw `html` blocks are
sanitised with `ammonia`.

**Previews.** `/_preview/<id>` (§3) renders `SiteReader::preview(actor, id)` with the
preview request scope for plugins (05 §7a) and `request.preview = true`. Preview responses
are never cached.

## 6. Page cache (as built)

`crates/ferrite-cache` (crate docs).

- Per node, in memory, `moka` (future cache), bounded by bytes (`cache.max_bytes`, default
  256 MiB); an entry weighs its body plus key, header, ETag and dependency-key bytes.
- Key: `(host, path, normalised allowlisted query, variant)`. The host is always the
  configured `site.url` host, never the `Host` header (R25). The only allowlisted query
  parameters are `s`, `page` and `cpage` (the comment page, M2b-2 P34; `cpage=1` and
  anything that is not an integer of 2 or more are dropped, so they share page 1's entry;
  it is kept only on routes that can show one item, a content path or the front page,
  `Route::may_show_item`, and dropped from every other route's key, M2b-3 Task 4),
  normalised by `NormalQuery` in `ferrite-http`. Variant: `anon`, or `user(<id>)` for logged-in requests
  under `cache.logged_in = "per-user"` (P21; under the default `"bypass"` logged-in requests
  are not cached at all). Dependency invalidation ignores the variant, so an edit drops every
  user's copy too. **Invariant:** the variant is the user id alone, which is safe only
  because nothing user-specific reaches the render (plugins see only `is_logged_in`;
  templates get no user, role or capability), so every user's copy is identical to what
  the render would give anyone logged in. Per-user output added later (an admin bar, the
  current user in templates) must add role/capabilities to the key or bypass the cache,
  and emit a `user:<id>` invalidation on role change and logout. Entries are not evicted
  on logout (an ended session no longer resolves, so they are unreachable); `per-user`
  stores one copy per active user in the shared `cache.max_bytes` budget.
- Value: status, headers, uncompressed body, ETag (16 hex digits of SHA-256 over the body),
  dependency keys, optional TTL. Compression (br, gzip) happens per response in the HTTP layer,
  so the ETag is sent weak (`W/"…"`) and every HTML response, 304 included, has
  `Vary: Accept-Encoding`. `If-None-Match` → 304 (weak comparison).
- **Dependency index**: each page's keys index its cache key; `PageCache::invalidate(keys)`
  drops every page indexed under any of them, `all` drops everything. A page whose keys were
  invalidated while it rendered is not stored (staleness guard), and concurrent misses for one
  key render once (single flight).
- **Invalidation** comes only from the `events` outbox through the invalidation loop
  ([04 §4](04-cluster.md#4-event-bus--cache-invalidation)), which polls every
  `cache.poll_interval_ms` and, in cluster mode, is also woken by Postgres `NOTIFY`:
  `invalidate` and `options.changed` drop their keys; `theme.changed`, `plugins.changed` and
  `plugin.disabled` flush; unknown kinds drop their keys. Coarse `archive:<type>` keys cover
  listings (correctness over hit rate).
- **Hot swap (M1.5):** after a theme, plugin or permalink reload the node flushes the cache a
  second time, because pages rendered during the swap may mix old and new state (P18, R13);
  the cache refuses in-flight renders that started before a flush.
- Not cached under the request path: error statuses (≥ 400), responses with `Set-Cookie`,
  previews, the 500 page and 503. Only GET and HEAD reach the cache.
- **Shared error pages (R26):** the 404 and 410 bodies are rendered without the request path
  and cached under internal keys with their dependency keys, so random missing URLs render
  once and a menu edit still refreshes them. Nothing is cached under a missing URL's path.
- **Short TTLs** (Task 14): search results are cached for `cache.search_ttl` (default 60 s, at most 1 h;
  `0` = search pages are not cached, each search renders), and pages whose plugin calls were
  volatile or failed, or whose theme helper reads failed, for `plugins.volatile_ttl` (default
  60 s, 1 s..=1 h; [05 §7a](05-plugin-host.md#7a-data-visibility-and-caching)); a volatile
  search page gets the shorter of the two. Both map to `HttpConfig.search_ttl` /
  `volatile_ttl` (default `SHORT_TTL`, 60 s).
- **Redirect hits** (Task 14, `crates/ferrite-http/src/app/hits.rs`): a redirect answered
  from a row is counted in the node's in-memory buffer (at most 10 000 distinct redirects;
  hits on further new ones are dropped and counted, hits on buffered ones always count). A
  flusher writes every 10 s, and once more when `serve` stops (after the HTTP drain, before
  the database pools close): one `RedirectRepo::add_hits` (`hits = hits + n`, `last_hit_at`
  never moved back) per redirect. A failed write goes back into the buffer for the next flush
  (within the cap), and so does what a final flush cut short by the shutdown bound did not
  write, which `serve` then logs (ids and hits). Best effort: a crash loses the buffer. Each
  cluster node keeps its own buffer.
- Scheduled publish (M1.5, P12): scheduling an item enqueues a `content.publish_scheduled`
  job due at `scheduled_at`; the periodic `content.publish_due` job (every 30 s, one per
  period cluster-wide) is the safety net. Their writes emit normal events.
- **Planned:** `Cache-Control: public, max-age=0, s-maxage=<cfg>` for CDNs is not sent
  (anonymous HTML pages carry no `Cache-Control`; logged-in pages and previews are
  `private, no-store`).

## 7. Error behaviour (as built)

- Template error or failed read: logged (`templates/<file>:<line>` for template errors), then
  500 using the theme's `500.html` with minimal context, else a built-in page; a failing
  `500.html` falls back to the built-in page. Never cached. A panicking handler gets the
  built-in 500 (tower-http `CatchPanic`). The server keeps serving.
- 404: the theme's not-found hierarchy, shared body (§6). 410 redirect rows: the same template
  with `request.status` 410.
- A theme that fails to load at startup (bad manifest, template syntax error, unknown or
  invalid name) is logged and the embedded `blog` theme is used, so a broken theme never takes
  the site down (`crates/ferrite/src/site.rs`). As built in M2a-2a (§1): activation refuses a
  theme that does not load (`422`, nothing changes), and a node serving anything but the
  active row's artifact reports `nodes.meta.theme.fallback = true`, shown per node by `GET
  /themes`; `GET /system/status` (M2a-2b) will read the same key, the admin view is M2b.
- Security headers on every response unless it sets them itself: `X-Content-Type-Options:
  nosniff`, `Referrer-Policy: strict-origin-when-cross-origin`, `X-Frame-Options: SAMEORIGIN`.

## 8. Starter themes

M1: `blog` (built, `themes/blog`, MIT, plain CSS, no build step, light and dark schemes).
M4: `docs`, `business`. WCAG 2.2 AA is a goal; automated checks (axe) in CI for the starter
themes are not set up yet.
