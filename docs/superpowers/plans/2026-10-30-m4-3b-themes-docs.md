# M4-3b — Docs and Business Themes, Markdown Import and the Docs Site Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FerriteCMS ships two more built-in themes, `docs` and `business`. The template
helpers a documentation site needs land in core: a page tree with breadcrumbs and
previous/next, heading anchors and a table of contents. The milestone also adds
`ferrite import markdown`, which brings a folder of Markdown files in as a page tree. The
project's own documentation site is written as Markdown in `site/` and runs on FerriteCMS
with the `docs` theme.

**Architecture:**
- **Heading ids and TOC:** the block renderer gives every heading a GitHub-compatible `id`
  and returns the headings, so `PageItem` gets a `toc`.
- **Page tree:** `SiteReader::tree(kind)` lists one type's public items, using the
  sitemap's query ordered by `menu_order`. A new `HelperBridge::tree` exposes it, and a
  renderer function `page_tree()` turns it into nested nodes with the current trail,
  ancestors, children and depth-first previous/next.
- **Themes:** plain-CSS embedded themes under `themes/`, discovered automatically.
- **Markdown import:** a new crate `ferrite-import-md`. It renders Markdown to HTML
  (pulldown-cmark), then reuses the WordPress importer's HTML-to-blocks converter and
  writes through `ImportService` / `ImportBatch`, so re-runs, the run lock and the
  edited-here rule come for free.
- **No migration, no WIT change, no API change.**

**Tech Stack:** Rust (MiniJinja 3, sqlx 0.9, tokio), `pulldown-cmark` 0.13 (new; 0.13.6
is the newest on crates.io as of 2026-10-30; confirm the API with Context7), `toml` (front
matter), plain CSS, Playwright with `@axe-core/playwright` (already a devDependency of
`admin/`).

**Spec:**
- **ROADMAP M4-3** (lines 462-463): starter themes `docs` and `business` with the template
  helpers a docs theme needs (page tree, previous/next, heading anchors, TOC) and
  Markdown import (P41); the docs site, built with FerriteCMS itself.
- **docs/BRIEF.md** §starter themes: "Ship 2–3 starter themes: a blog, a
  docs/knowledge-base, a small business site."
- **Design docs:**
  - `docs/design/06-themes-rendering.md`: §1 package, §2 hierarchy, §4 environment and
    helpers, §8 starter themes ("WCAG 2.2 AA is a goal");
  - 08 (WordPress import: run, map and re-run rules that the Markdown import shares);
  - 09 §8 (CLI reference).
- **Ruling:** P41 in `docs/superpowers/rulings/m4.md`.
- **Recon** (git-ignored, every fact with file:line): `.superpowers/m4-3-recon/themes.md`.
  Its "Gaps" section is the input for the rulings below.

## Plan rulings (written to docs/superpowers/rulings/m4.md, section "M4-3b plan rulings", at Task 1)

Format: `- Plan ruling Pn: decision — reason — cost if wrong: …`. Numbering continues
from M4-3a (P41–P53).

**P54: heading ids.**
- The block renderer gives every `heading` node an `id`, using GitHub's rule, so links
  written for GitHub (`#install-the-binary`) keep working after an import:
  1. Take the heading's text (all text descendants, marks ignored) and lowercase it
     (Unicode `to_lowercase`).
  2. Keep characters that are alphanumeric (Unicode), `-`, `_` or a space. Drop the rest.
  3. Turn each space into `-`. Do not collapse runs of `-`.
- **Empty result:** an empty id becomes `section`.
- **Duplicates:** a repeated id within one document gets `-1`, `-2`, … appended, like
  GitHub.
- **Reserved ids:** the counter is seeded with `content`, `comments`, `respond`,
  `search`, `menu`, `top`, `main`, `toc`, `nav`, `footer`. A heading "Content" therefore
  becomes `content-1` and cannot collide with a theme landmark. The list is
  `ferrite_domain::html::RESERVED_FRAGMENT_IDS` and is documented in 06 §4.
- **Out of scope:** `html` blocks keep ammonia's rules (ids stripped). No anchor link is
  added to the body.
- Reason: a TOC and deep links need ids, and a theme cannot add them.
- Cost if wrong: ids change if the rule changes later, which breaks deep links.

**P55: `post.toc`.**
- `PageItem` gets `toc: Vec<TocEntry { level: u8, id: String, text: String }>`, in
  document order, for every heading the body renders (levels 1–6).
- Templates see it as `post.toc`, a list of `{level, id, text}`. The theme picks the
  levels it shows; `docs` shows 2–3.
- **Filtered bodies:** when a plugin's `content.body` filter changes the HTML, `toc` is
  left as computed. A filter that removes a heading leaves a dead TOC link. This is
  documented.
- Reason: parsing HTML in the template is not possible, and the renderer already walks
  the headings.
- Cost if wrong: a theme gets a TOC entry for a heading a plugin removed.

**P56: the content tree.**
- `SiteReader::tree(kind) -> SiteRead<Option<ContentTree>>` lists the public items of
  one type, with exactly the sitemap's visibility and paths: `published(now)`, the same
  ancestor and term loading, the same `rules.path`. Each node carries
  `{id, parent: Option<ContentId>, title, url, menu_order}`.
- **No tree:** `None` for `media`, or for a type that is unknown or has no public pages.
- **Size cap:** at most `MAX_TREE_NODES = 1000` nodes, fetched with
  `ContentOrder::MenuOrderAsc`. Past the cap `truncated = true`, and the template says so.
- **Cache keys:** `archive:<kind>`. Every content write emits it (06 §6, sitemap comment
  in `site.rs`), so publishing, moving or reordering anything in the type refreshes every
  page that drew the tree. This matches the sitemap's rule.
- **Bridge:** `HelperBridge::tree(kind) -> Bridged<Option<ContentTree>>`. The default
  returns `(None, empty)`, so `NoBridge` and the test bridges keep compiling.
- Reason: it is one query per page instead of N calls for children and ancestors.
- Cost if wrong: a site with more than 1000 pages of one type gets a truncated sidebar,
  and the note says so.

**P57: `page_tree()`.**
- **Signature:** `page_tree(post=none, kind=none)`.
  - With `post`, it reads `post.kind`'s tree and marks `post` as current.
  - With only `kind`, it reads that tree with no current node (search, 404 and archive
    pages).
  - With neither, or with an invalid `kind`, it returns `none` and makes no call.
- **Cost:** one bridge call per distinct kind per render, memoised. It counts against
  `MAX_BRIDGE_CALLS`.
- **Return value:** `{items, current, ancestors, children, prev, next, truncated}`.
  - Each node is `{id, title, url, depth, current, open, children}`. `depth` starts at 0.
    `open` is true for the current node and its ancestors.
  - `items` holds the root nodes, nested.
  - `ancestors` runs root first, excluding `current`.
  - `children` are `current`'s children.
- **Sibling order:** `menu_order`, then title compared case-insensitively
  (`to_lowercase`), then id.
- **Previous and next:** depth-first pre-order over the whole tree, as mdBook and
  Docusaurus do.
- **Hidden parents:** a node whose parent is not in the tree (a draft, trashed or
  truncated parent) is dropped together with its subtree. The page still renders at its
  URL; only the navigation hides it.
- **Missing current:** `current`, `prev` and `next` are `none` when `post` is not in the
  tree (a preview of a draft).
- Reason: everything a docs sidebar, breadcrumbs and pager need comes from one read.
- Cost if wrong: a hidden subtree under a draft parent surprises an editor. 06 §4
  documents it.

**P58: `query(order=)`.**
- `query()` gets `order="date"` (the default, newest first, as today) or `"menu"`
  (`menu_order` ascending, then title).
- Anything else is an error, like an unknown kwarg.
- `SiteReader::latest` and `term_items` take a `ListOrder { Date, Menu }`.
- Reason: a business front page lists services in a chosen order.
- Cost if wrong: one more kwarg to keep.

**P59: the `docs` theme** (`themes/docs`, MIT, plain CSS, no build step).
- **Layout:**
  - sticky header: site title, primary menu, search form;
  - left sidebar: `page_tree`, with sections as `<details>`, open on the current trail;
  - centre: breadcrumbs, title, body, "updated" date, previous/next;
  - right: TOC, levels 2–3, on wide screens only.
- **Mobile:** one column. The sidebar sits in a `<details>` labelled "Pages".
- **Settings:**
  - `accent_color` (color, `#2563eb`);
  - `nav_type` (text, default `page`: the content type whose tree the sidebar shows);
  - `show_toc` (bool, true);
  - `show_updated` (bool, true).
- **Menus:** `primary`, `footer`.
- **Templates:**
  - `base`, `index`, `page`, `single`, `archive`, `search`, `404`, `500`;
  - partials `sidebar`, `toc`, `breadcrumbs`, `pager`, `search-form`, `accent`, `menu`.
- **JavaScript:** one asset, `docs.js` (under 3 KB, progressive enhancement only). It adds
  a "link to this section" anchor after each heading that has an id, and a "Copy" button
  on code blocks. The pages work fully without it. There is no syntax highlighting
  (Later).
- **Fonts and colours:** system font stacks only, no font files. Light and dark come from
  `prefers-color-scheme`. Contrast ratios are stated in the CSS header as in blog.
- Reason: a docs theme that matches what readers expect from mdBook or Docusaurus,
  without a build step.
- Cost if wrong: no highlighting until Later.

**P60: the `business` theme** (`themes/business`, MIT, plain CSS, no JS).
- **`front-page.html`** shows, in order:
  - a hero: the front page's featured image through `image(.., "hero")`, its title and
    excerpt, and a CTA button from the settings;
  - the front page's children from `page_tree(post).children`, as feature cards (featured
    image `card`, title, excerpt);
  - an optional "latest news" strip, `query(type="post", limit=3)`, with setting
    `show_news`.
- **`page-contact.html`:** the page body, then, when setting `contact_form` is true, the
  contact-form plugin's form. The markup follows `plugins/examples/contact-form/README.md`
  ("Embedding the form") and posts to `plugin_route("contact-form", "/submit")`.
- **Settings:**
  - `accent_color` (color, `#0f766e`);
  - `cta_label` (text, "Get in touch");
  - `cta_url` (text, `/contact/`);
  - `show_news` (bool, true);
  - `contact_form` (bool, false).
- **Images:** `hero` 1600×700 crop, `card` 640×400 crop.
- **Menus:** `primary`, `footer`.
- **Other templates:** `base`, `index`, `page`, `single`, `archive`, `search`, `404`,
  `500`.
- Reason: what BRIEF calls "a small business site" needs a front page and a contact path.
- Cost if wrong: the form shows a 404 when the plugin is missing; the setting defaults
  to off.

**P61: `seed demo --profile blog|business`.**
- `blog` is the default and is unchanged byte for byte. The existing tests and the admin
  E2E rely on it.
- `business` writes, in one transaction like `DemoBatch`:
  - pages Home (the front page, with a featured image); Services, Consulting and Support
    as children of Home with `menu_order` 1–3; About; Contact;
  - 3 posts;
  - menus `primary` (Home, About, News → `/post/`, Contact) and `footer`;
  - `reading.front_page` set to Home;
  - `site.title` "Larkspur Studio" (a made-up name).
- There is no `docs` profile: the docs site's own Markdown is the docs demo (P63).
- The profile writes content only and does not activate a theme. The command prints
  `next: ferrite theme activate business --data DIR`.
- Reason: the business theme needs a front page tree to show.
- Cost if wrong: one more seed to keep.

**P62: `ferrite import markdown DIR --data DATA`.**

Options: `[--as LOGIN] [--type page] [--front-page] [--dry-run] [--source NAME]`.

- **Source and map:**
  - It runs as an import run with source `markdown:<NAME>`; `NAME` defaults to the
    folder's file name.
  - It takes the same lock and run fence as the WordPress import, so exit 3 means another
    import is running.
  - The map key (`import_map.wp_id`) is the first 8 bytes of SHA-256 of the file's path
    relative to `DIR` (with `/` separators), as a big-endian `i64` masked to be
    non-negative.
  - The map hash is SHA-256 over the file's bytes plus the import format version `"md1"`.
- **Re-runs** follow `ImportBatch` rules:
  - unchanged → nothing;
  - changed → updated;
  - edited here since → kept;
  - a renamed file becomes a new page, the old one stays, and the report lists it as not
    in the source.
- **Tree:**
  - `DIR/a/index.md` (or `README.md`) is the page for folder `a`, and `DIR/a/b.md` is its
    child.
  - A folder without an index gets a synthesized page with an empty body, titled from the
    folder name ("getting-started" → "Getting started"). It is reported.
  - The root `DIR/index.md` is a top-level page, slug from front matter, else `home`.
    With `--front-page` it becomes `reading.front_page` (a dry run only reports it).
- **Slug and order:**
  - A file or folder name `NN-name` (digits, then `-`) gives slug `name` and order `NN`.
  - Front matter `slug` and `order` win.
  - Otherwise the slug is the file stem and the order is 0.
- **Front matter:** optional TOML between a first line `+++` and the next `+++` line.
  - Keys: `title`, `slug`, `order` (integer), `draft` (bool), `date` (RFC 3339),
    `description` (the excerpt and the SEO description).
  - An unknown key, invalid TOML or a `---` YAML block is an error that names the file
    and line. The file is skipped and the run continues; exit 2.
- **Title and headings:**
  - The title is front matter `title`, else the first `# H1` (removed from the body),
    else the stem, as "Getting started" above.
  - If an `h1` is left in the body, every heading level is shifted by +1, capped at 6, so
    the body holds editor-safe levels 2–6.
- **Markdown:**
  - CommonMark plus GFM tables and strikethrough (pulldown-cmark `ENABLE_TABLES |
    ENABLE_STRIKETHROUGH`). Other extensions stay off.
  - A UTF-8 BOM and CRLF are accepted.
  - The HTML goes through `ferrite_import_wp::convert::html::convert` with a `Rewrite`
    for links and images.
- **Images:**
  - A relative `src` resolving inside `DIR` (after canonicalising; no symlink may leave
    `DIR`) becomes a media item, checked as an upload.
  - Any other relative `src` is reported missing.
  - `http(s)` is kept as a URL.
- **Links:**
  - A relative link to a `.md` file or folder in the import is rewritten to that page's
    path, keeping `#fragment`.
  - A link to a `.md` not in the import is reported broken and kept.
  - Other links are kept.
- **Pass order:**
  1. media;
  2. content;
  3. link fixup, through `ImportBatch::set_body_and_fields` with paths from
     `ImportBatch::path`.
- **Exit codes and report:**
  - Exit: 0 done; 2 done with skipped files, missing images or broken links; 3 another
    import running; 1 error.
  - The report is printed as text: counts per outcome, then each problem with its file.
    There is no HTML report.
- **Size and file limits:** at most 5000 files; each file at most 1 MiB; symlinks are not
  followed; hidden files and folders (`.`-prefixed) are skipped.
- Reason: the docs site needs it, and authors keep docs in git as Markdown.
- Cost if wrong: TOML-only front matter turns away Hugo/Jekyll YAML files; the error says
  how to convert.

**P63: the docs site.**
- `site/content/` holds the documentation as Markdown.
- `site/build.sh` builds a local site:
  - `init` with `FERRITE_SITE__TITLE="FerriteCMS"`;
  - `seed` is not used;
  - `import markdown site/content --front-page`;
  - `theme activate docs`;
  - then it prints the `serve` command.
- `site/README.md` explains how to build and run it.
- **Hosting is the user's call:** no deploy, no domain and no CI publish in this plan. It
  is listed in the exit.
- **Claims:** every claim in the pages is checked against the code or the design docs
  (the user's rule for articles and READMEs). `site/VERIFIED.md` holds the table of what
  was checked and how.
- **Test:** `crates/ferrite/tests/docs_site.rs` imports `site/content` into a fresh site,
  serves it with `docs` active, then fetches every sitemap URL. Every page must:
  - answer 200;
  - have one `<h1>`;
  - have every internal link answer 200 or a redirect to 200;
  - have every TOC link point to an id on the same page.
- Reason: "built with FerriteCMS itself" (ROADMAP), and the test keeps the docs from
  rotting.
- Cost if wrong: the docs are only as current as their last edit; the test catches
  broken links, not stale facts.

**P64: accessibility checks for the public themes.**
- A Playwright spec `admin/e2e/public-themes.spec.ts` runs axe with the admin's tags
  (`wcag2a`, `wcag2aa`, `wcag21a`, `wcag21aa`, `wcag22aa`), in both colour schemes.
- **Pages checked:**
  - `blog` on `seed demo`: front, a single post, a page, a category archive, search,
    404;
  - `business` on `seed demo --profile business`: front page, a child page, contact,
    search, 404;
  - `docs` on the imported `site/content`: front, a nested page, search, 404.
- **Runs:** in the `chromium-sqlite` project only, in its own CI step. The spec activates
  each theme through the API and restores `blog` in `afterAll`.
- This closes the M4-4 item "Automated accessibility checks (axe) in CI for the starter
  themes" (06 §8).
- Reason: 06 §8 sets WCAG 2.2 AA as the goal, and three themes untested in a browser
  would be a claim without proof.
- Cost if wrong: one more CI step of about a minute.

**P65: embedded theme size.**
- Each starter theme stays under 150 KB on disk: no images, no font files.
- `crates/ferrite-theme/tests/load.rs` asserts the total per embedded theme.
- Reason: the themes ship inside every binary.
- Cost if wrong: a test to adjust.

## Global Constraints

- Rust stable, edition 2024. `cargo fmt` and `cargo clippy --all-targets -- -D warnings`
  pass; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` passes.
- `thiserror` in libraries, `anyhow` only in the `ferrite` binary crate. No
  `allow(dead_code)`.
- No migration, no WIT change, no REST API change. `openapi_snapshot_is_current` must
  stay green without regeneration.
- Themes don't execute code on the server (principle 3). Client JS is allowed only as
  progressive enhancement (P59), and `business` and `blog` ship none.
- Storage-facing reads go through `SiteReader`, published-only. Dependency keys follow
  `ferrite_core::events`.
- Postgres tests:
  `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test`. A test that
  touches the store runs on both backends.
- Commits are small and end with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Implementers branch from `refs/heads/m4-3` (`git switch -c <branch> refs/heads/m4-3`)
  and dispatch no subagents.
- Docs: every claim written into `site/content`, a README or a design doc is checked
  against code or tests. Name what could not be checked.

## Review Focus

1. **Headings whose text is punctuation-only, emoji, non-Latin (Polish, Cyrillic, CJK) or
   repeated:** ids must be non-empty, unique per page and never one of the reserved ids.
   Tests are in Task 1.
2. **A page tree of 1000 nodes, or 64 levels deep, in the `docs` theme:** it renders
   within the default fuel and MiniJinja's recursion limit, and the truncated note shows
   at 1001. Tests are in Task 3.
3. **A published child of a draft parent:** the child's page renders, the navigation
   hides the subtree, and previous/next skip it. Tests are in Task 2.
4. **Markdown that points outside the folder** (a `../../etc/passwd` image, a symlinked
   file or folder, an absolute path): it is never read. It is reported missing, and other
   files still import. Tests are in Task 6.
5. **Re-running the Markdown import:**
   - after editing a page in the admin, the page is kept;
   - after editing the file, the page is updated;
   - with nothing changed, nothing is written and no revision is added.

   Tests are in Task 6.

---

### Task 1: Heading ids and `post.toc`

**Files:**
- Create: `crates/ferrite-core/src/blocks/anchor.rs` (id rule, `TocEntry`)
- Modify:
  - `crates/ferrite-core/src/blocks/render.rs` (heading `id`, collect the TOC);
  - `crates/ferrite-core/src/blocks/mod.rs` (export);
  - `crates/ferrite-domain/src/html.rs` (`RESERVED_FRAGMENT_IDS`);
  - `crates/ferrite-core/src/site/model.rs` (`PageItem.toc`);
  - `crates/ferrite-core/src/site/hydrate.rs` (fill it);
  - `crates/ferrite-theme/src/view.rs` (`ItemObject.toc`);
  - `docs/design/06-themes-rendering.md` §4;
  - `docs/superpowers/rulings/m4.md` ("M4-3b plan rulings": P54–P65 as written above).
- Test: the unit tests in `anchor.rs` and `render.rs`; `crates/ferrite-theme/tests/theme/render.rs`.

**Interfaces:**
- Produces:
  - `ferrite_core::blocks::TocEntry { pub level: u8, pub id: String, pub text: String }`
    (Serialize, Clone, Debug, PartialEq);
  - `pub fn render_html_toc(doc: &Value, ctx: &RenderCtx) -> (String, Vec<TocEntry>)`.
    `render_html` stays and returns the HTML only;
  - `PageItem.toc: Vec<TocEntry>`;
  - template `post.toc`;
  - `ferrite_domain::html::RESERVED_FRAGMENT_IDS: [&str; 10]`.

- [ ] **Step 1: Write the failing tests** (`anchor.rs` tests, through a `Anchors` counter type you define):
  - `"Install the binary"` → `install-the-binary`;
  - `"What's new?"` → `whats-new`;
  - `"Zażółć gęślą"` → `zażółć-gęślą`;
  - `"Привет мир"` → `привет-мир`;
  - `"日本語"` → `日本語`;
  - `"🎉"` → `section`;
  - `"a  b"` → `a--b`;
  - `"snake_case"` → `snake_case`;
  - two `"Setup"` → `setup`, `setup-1`;
  - `"Content"` → `content-1`;
  - `"section"` after an empty heading → `section-1`.

  In `render.rs` tests:
  - the existing `<h2>H</h2>` expectation becomes `<h2 id="h">H</h2>`;
  - a heading with a bold mark and a link mark yields the id from the plain text;
  - `render_html_toc` returns entries in order with levels;
  - an invalid level (rendered as `<p>`) gets no id and no entry;
  - an id holding `"` or `<` cannot occur (only the kept characters remain), and it is
    written escaped anyway.
- [ ] **Step 2:** `cargo test -p ferrite-core blocks` → FAIL (no `anchor` module).
- [ ] **Step 3: Implement** the rule exactly as P54 states, in `anchor.rs`. Thread one
  `Anchors` through the render, so ids are unique per document. Headings nested deeper
  than `MAX_DEPTH` render text-only and get no id. Hydrate uses `render_html_toc` and
  stores `toc`. Every other caller keeps `render_html`.
- [ ] **Step 4: Theme test:** in `tests/theme/render.rs`, add `toc_is_in_the_context`.
  It renders `{% for h in post.toc %}{{ h.level }}:{{ h.id }}:{{ h.text }};{% endfor %}`
  with a `PageItem` holding two entries and asserts the exact string.
- [ ] **Step 5:** `cargo test -p ferrite-core -p ferrite-theme`, then `cargo test -p
  ferrite-http --test app` (blog pages: update any exact-HTML expectation that included a
  heading). All pass.
- [ ] **Step 6:** In 06 §4, describe the id rule, the reserved ids, `post.toc` and the
  plugin-filter caveat (P55). Add P54–P65 to the rulings file. Commit:
  `feat(blocks): GitHub-style heading ids and post.toc (M4-3b P54, P55)`.

### Task 2: The content tree, `page_tree()` and `query(order=)`

**Files:**
- Create: `crates/ferrite-core/src/site/tree.rs` (`ContentTree`, `TreeNode`, `SiteReader::tree`)
- Modify:
  - `crates/ferrite-core/src/site.rs` (`ListOrder` on `latest` and `term_items`; mod
    `tree`; `MAX_TREE_NODES`);
  - `crates/ferrite-theme/src/bridge.rs` (`tree`, `query` signature);
  - `crates/ferrite-theme/src/render.rs` (`page_tree` function and the `order` kwarg);
  - `crates/ferrite-http/src/app/bridge.rs` (`BlockingBridge::tree`, `query` order);
  - the test bridges (`tests/blog.rs`, `tests/theme/common.rs`) where their `query`
    signature changes;
  - 06 §4.
- Test:
  - `crates/ferrite-core/tests/site_tree.rs` (both backends, through the existing
    store-test harness that `crates/ferrite-core/tests` uses);
  - `crates/ferrite-theme/tests/theme/page_tree.rs`;
  - `crates/ferrite-http/tests/app/tree.rs`.

**Interfaces:**
- Consumes: `ContentOrder::MenuOrderAsc` and the sitemap's path code (`load_ancestors`,
  `chain`, `primary_term`, `rules.path`). Factor out the shared part; don't copy it.
- Produces:
  - `pub const MAX_TREE_NODES: usize = 1000;`
  - `pub struct TreeNode { pub id: ContentId, pub parent: Option<ContentId>, pub title: String, pub url: String, pub menu_order: i32 }`;
  - `pub struct ContentTree { pub kind: String, pub nodes: Vec<TreeNode>, pub truncated: bool }`;
  - `SiteReader::tree(&self, kind: &str) -> Result<SiteRead<Option<ContentTree>>, CoreError>`;
  - `pub enum ListOrder { Date, Menu }`;
  - `HelperBridge::tree(&self, kind: &str) -> Bridged<Option<ContentTree>>` (default
    `(None, DepKeys::default())`);
  - `HelperBridge::query(&self, kind, term, limit, order: ListOrder)`;
  - template `page_tree(post=, kind=)` returning P57's object;
  - template `query(..., order="date"|"menu")`.

- [ ] **Step 1: Core tests, both backends** (`site_tree.rs`):
  - **`tree_matches_the_sitemap`:** pages A(order 2) and B(order 1), each with children,
    and one draft. The node set equals the sitemap entries' URLs. Nodes come in
    `menu_order` order; parents are set.
  - **`tree_of_media_or_unknown_is_none`.**
  - **`tree_deps_are_archive_kind`:** deps equal `{archive:page}`.
  - **`tree_truncates_at_1000`:** create 1001 published pages. 1000 nodes come back
    with `truncated == true`. Use a batch insert helper; keep the test under 10 s on
    SQLite.
  - **`query_menu_order`:** three services with `menu_order` 3, 1, 2 and publish dates in
    reverse. `latest(.., ListOrder::Menu)` returns order 1, 2, 3, and
    `ListOrder::Date` is unchanged.
- [ ] **Step 2: Theme tests** (`page_tree.rs`, `FakeBridge` returning a fixed tree):
  - **`nests_and_marks_the_trail`:** for current = a grandchild:
    - `ancestors` titles are root then parent;
    - `open` is true on the trail only;
    - `children` are the current node's children;
    - `depth` values are 0, 1, 2.
  - **`prev_next_are_preorder`:** in the tree R1(C1, C2(G1)), R2, `next` of C2 is G1,
    `next` of G1 is R2, `prev` of R2 is G1, `prev` of R1 is none, and `next` of R2 is
    none.
  - **`siblings_sort_by_order_then_title_then_id`:** equal `menu_order`, titles "b" and
    "A" → "A" first.
  - **`orphans_are_dropped`:** a node whose parent id is absent is gone, with its
    children.
  - **`current_absent_gives_no_trail`.**
  - **`kind_only_has_no_current`.**
  - **`no_args_is_none_and_costs_no_call`.**
  - **`memoised_per_kind`:** two calls make one bridge call.
  - **`counts_against_the_call_cap`.**
  - **`query_rejects_unknown_order`:** `order="x"` gives a render error.
- [ ] **Step 3: HTTP test** (`tests/app/tree.rs`, both profiles; `test-minimal` fixture
  theme plus a template using `page_tree(post)`):
  - **`draft_parent_hides_the_subtree`:** the child page is 200. Its rendered sidebar
    lacks the child, and `prev`/`next` skip it.
  - **`publishing_refreshes_the_tree`:** a cached page shows a new sibling after it is
    published (the outbox invalidation).
- [ ] **Step 4:** Run them → FAIL.
- [ ] **Step 5: Implement.**
  - `tree` reuses the sitemap's query in pages of `MAX_PAGE` until 1000.
  - `page_tree` builds the nested structure in Rust (`IndexMap` by parent, then sort),
    computes pre-order once, and returns MiniJinja values. Each node in `items`,
    `ancestors`, `children`, `prev` and `next` is the same shape. Avoid exponential
    copies: build `items` once, and give `prev`, `next`, `ancestors` and `children` the
    flat `{id, title, url, depth}` without `children`, except `children` of current,
    which keeps one level of its own `children`.
  - Record P57's memo key `tree:<kind>`.
- [ ] **Step 6:** `cargo test -p ferrite-core -p ferrite-theme -p ferrite-http` with
  `FERRITE_TEST_PG_URL` → pass. Clippy.
- [ ] **Step 7:** Document `page_tree`, the node shape, the order, the orphan rule, the
  1000 cap and `query(order=)` in 06 §4 ("Globals and helpers"). Commit:
  `feat(theme): page_tree() and query(order=) over SiteReader::tree (M4-3b P56–P58)`.

### Task 3: The `docs` theme

**Files:**
- Create: `themes/docs/` (`theme.toml`, `templates/*.html`, `templates/partials/*.html`,
  `assets/style.css`, `assets/docs.js`), `crates/ferrite-theme/tests/docs.rs`
- Modify:
  - `crates/ferrite-theme/tests/theme/load.rs` (P65 size test over every embedded theme);
  - 06 §8;
  - `docs/design/01-architecture.md` (`themes/` line);
  - `CLAUDE.md` (starter themes).

**Interfaces:**
- Consumes: `post.toc` (Task 1), `page_tree()` (Task 2), the existing helpers.
- Produces: embedded theme `docs` with P59's settings, menus and templates.

- [ ] **Step 1: Write `tests/docs.rs`**, modelled on `tests/blog.rs`: a `FakeBridge` with
  a tree and `assert_frame` (lang, skip link, one `<main>`, one `<h1>`, landmarks, accent
  style). Tests:
  - **`loads_cleanly`:** manifest, settings, menus, and CSS needles
    `prefers-color-scheme: dark`, `:focus-visible`, `prefers-reduced-motion`,
    `forced-colors`.
  - **`hierarchy_resolves_the_real_files`.**
  - **`every_page_template_renders_with_an_empty_context`.**
  - **`page_shows_sidebar_breadcrumbs_toc_and_pager`:** the current link has
    `aria-current="page"`, the trail's `<details>` are `open`, the breadcrumbs `<nav
    aria-label="Breadcrumb">` end in the current title without a link, the TOC lists h2
    and h3 only, and prev/next links carry `rel="prev"`/`rel="next"`.
  - **`settings_toggle_toc_and_updated`.**
  - **`nav_type_setting_selects_the_tree`:** `nav_type = "guide"` calls
    `tree("guide")`.
  - **`search_and_404_show_the_sidebar_without_current`.**
  - **`truncated_tree_says_so`.**
  - **`a_thousand_node_tree_renders_within_the_default_fuel`:** 10 roots × 10 × 10.
  - **`a_64_deep_chain_renders`.**
  - **`docs_js_is_progressive`:** the page HTML has no inline script; `docs.js` is
    referenced with `defer`.
- [ ] **Step 2:** Run → FAIL (no theme).
- [ ] **Step 3: Write the theme per P59.**
  - Accessibility patterns as in blog: skip link, landmarks, one `<h1>`, `aria-label` on
    each `<nav>`, visible focus, ≥ 4.5:1 text contrast in both schemes, stated in the CSS
    header.
  - The sidebar is a recursive macro over `page_tree(post).items`.
  - The TOC renders only when `post.toc` has two or more entries at levels 2–3.
  - The search form uses `role="search"`.
  - `docs.js` adds anchors (`<a class="heading-anchor" href="#id" aria-label="Link to
    this section">#</a>`) and copy buttons (`<button type="button">Copy</button>`, using
    `navigator.clipboard`, with a hidden status text for screen readers). No external
    requests.
- [ ] **Step 4: P65 size test:** in `load.rs`, add `embedded_themes_stay_small`. For
  every `Theme::embedded_names()`, the total of the embedded files is under 150 000
  bytes.
- [ ] **Step 5:** `cargo test -p ferrite-theme` → pass. Also run
  `cargo test -p ferrite-http --test api themes` (the themes list now has `docs`; fix any
  exact count).
- [ ] **Step 6:** Update the docs: 06 §8 (built: `docs`, with layout and settings), 01
  §1 and CLAUDE.md. Commit: `feat(themes): the docs starter theme (M4-3b P59, P65)`.

### Task 4: The `business` theme and `seed demo --profile business`

**Files:**
- Create: `themes/business/…`, `crates/ferrite-theme/tests/business.rs`
- Modify:
  - `crates/ferrite/src/seed.rs` and `seed/demo.rs` (`--profile`);
  - `crates/ferrite-core/src/seed.rs` (a `DemoBatch` writer for the business content,
    if the batch needs new calls);
  - `crates/ferrite/tests/cli.rs` (seed tests);
  - 06 §8;
  - 09 §8 (`seed demo`);
  - CLAUDE.md.

**Interfaces:**
- Consumes: `page_tree()` and `query(order=)` (Task 2).
- Produces: embedded theme `business`;
  `ferrite seed demo --data DIR [--profile blog|business]`.

- [ ] **Step 1: Theme tests** (`business.rs`, following `docs.rs`'s pattern):
  - **`loads_cleanly`.**
  - **`front_page_has_hero_cta_cards_and_news`:** the hero `<img>` has `srcset`, `sizes`
    and `fetchpriority="high"`; CTA text and href come from settings; cards are in
    children order; the news strip renders only with `show_news`.
  - **`contact_page_form_follows_the_plugin_readme`:** with `contact_form = true`, the
    form's `action` is `/_p/contact-form/submit` and it has fields `name`, `email`,
    `subject`, `message`, the hidden honeypot `website` (`tabindex="-1"`,
    `autocomplete="off"`, inside an `aria-hidden` wrapper) and `return` =
    `request.path`; each input has a `<label>`. With `contact_form = false` there is no
    form.
  - **`every_page_template_renders_with_an_empty_context`.**
  - **`cta_url_is_escaped`:** `cta_url = "\" onclick=x"` renders escaped.
- [ ] **Step 2: Seed tests** (`cli.rs`, both backends):
  - **`seed_business_profile`:** `reading.front_page` names Home; Home has 3 children
    ordered 1–3; menus `primary` and `footer` exist; the output has the `next:` line.
  - **`seed_default_profile_is_unchanged`:** the counts and titles of today's blog seed
    assertions still hold.
  - **`seed_unknown_profile_is_a_usage_error`:** clap exit 2.
- [ ] **Step 3:** Run → FAIL.
- [ ] **Step 4:** Write the theme per P60 and the profile per P61. The featured image
  reuses the seed's generated PNG approach.
- [ ] **Step 5:** `cargo test -p ferrite-theme --test business` and
  `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test cli seed` → pass.
- [ ] **Step 6:** Update the docs (06 §8, 09 §8 `seed demo --profile`, CLAUDE.md "Run a
  site locally"). Commit: `feat(themes): the business starter theme and seed profile
  (M4-3b P60, P61)`.

### Task 5: Markdown to blocks (`ferrite-import-md`, conversion only)

**Files:**
- Create:
  - `crates/ferrite-import-md/` (`Cargo.toml`, `src/lib.rs`, `src/front.rs` (front
    matter), `src/scan.rs` (folder walk to `MdDoc`s), `src/convert.rs` (Markdown → HTML
    → blocks), `src/error.rs`);
  - `crates/ferrite-import-md/tests/fixtures/` (a small tree).
- Modify: root `Cargo.toml` (member, `pulldown-cmark = "0.13"` in workspace deps).
- Test: unit tests per module; `tests/scan.rs`.

**Interfaces:**
- Consumes: `ferrite_import_wp::convert::{html::convert, doc::to_json, Rewrite, ImageRef, Notes}` (all public
  already; no change in that crate).
- Produces:
  - `pub struct FrontMatter { pub title: Option<String>, pub slug: Option<String>, pub order: Option<i32>, pub draft: bool, pub date: Option<jiff::Timestamp>, pub description: Option<String> }`;
  - `pub fn split_front_matter(text: &str, file: &Path) -> Result<(FrontMatter, &str), MdError>`;
  - `pub struct MdDoc { pub rel: String /* "/"-separated, relative to DIR */, pub parent: Option<String> /* rel of the parent doc */, pub slug: String, pub order: i32, pub front: FrontMatter, pub source: Option<String> /* None for a synthesized folder page */, pub hash: String }`;
  - `pub struct Scan { pub docs: Vec<MdDoc> /* parents before children */, pub problems: Vec<Problem> }`;
  - `pub fn scan(dir: &Path) -> Result<Scan, MdError>`;
  - `pub fn map_id(rel: &str) -> i64` (P62);
  - `pub struct Converted { pub title: Option<String> /* from the first H1 */, pub body: serde_json::Value, pub images: Vec<String> /* relative srcs, resolved inside DIR */, pub links: Vec<String> /* relative .md targets */ }`;
  - `pub fn convert(markdown: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Converted`;
  - `pub enum Problem { BadFrontMatter { file, line, message }, TooLarge { file }, OutsideDir { file, target }, MissingImage { file, src }, BrokenLink { file, href }, SynthesizedSection { dir }, NotUtf8 { file } }`.

- [ ] **Step 1: Write the failing tests.**
  - **Front matter:**
    - a valid TOML block with every key;
    - no block;
    - `---` YAML → `BadFrontMatter` naming line 1;
    - an unknown key → `BadFrontMatter`;
    - an unterminated `+++` → error;
    - a BOM and CRLF are accepted.
  - **Scan:**
    - the fixture with `index.md`, `01-install.md`, `02-configure.md`, a folder
      `guides/` with `README.md` and `writing.md`, and a folder without an index;
    - `rel`, `parent`, slugs `install` and `configure`, orders 1 and 2, and the
      synthesized section;
    - hidden files skipped;
    - a symlink (unix) not followed and reported `OutsideDir`;
    - a 1 MiB + 1 file → `TooLarge`;
    - the order is parents before children.
    - `map_id` is stable for a fixed input (pin the value), non-negative, and different
      for two paths.
  - **Convert:**
    - a heading, a paragraph with bold/italic/code/link, lists (nested), a blockquote, a
      fenced code block with `rust` (→ `codeBlock` with `language: "rust"`), a GFM table
      (→ `table`), strikethrough, an `hr`, a hard break and raw HTML (sanitised, as WP
      import does);
    - the first `# Title` becomes `title` and leaves the body;
    - a second `# H1` shifts all levels +1, capped at 6;
    - `![a](img/x.png)` is passed to `rw.image("img/x.png")` and listed in `images`;
    - `[x](../other.md#sec)` is listed in `links`;
    - `rw.href` is applied.
- [ ] **Step 2:** `cargo test -p ferrite-import-md` → FAIL.
- [ ] **Step 3: Implement** with `pulldown-cmark` (`Options::ENABLE_TABLES |
  Options::ENABLE_STRIKETHROUGH`). Render to HTML with `pulldown_cmark::html::push_html`,
  then call the WP converter. Detect and strip the first H1 on the event stream before
  rendering. The heading shift also happens on the events. `scan` never follows
  symlinks (`symlink_metadata`). Paths resolve with `canonicalize` and must stay under
  `canonicalize(dir)`.
- [ ] **Step 4:** Tests and clippy pass. Commit:
  `feat(import-md): Markdown to blocks with front matter and a folder scan (M4-3b P62)`.

### Task 6: `ferrite import markdown`

**Files:**
- Create:
  - `crates/ferrite-import-md/src/run.rs` (the passes over `ImportService`);
  - `crates/ferrite/src/import_cmd.rs` (subcommand `Markdown(MarkdownArgs)`);
  - `crates/ferrite/tests/import_md.rs`.
- Modify:
  - `crates/ferrite-core/src/import/content.rs` and `batch.rs`: `ImportContent.id_meta:
    &'static str`. The WordPress passes give `WP_ID_META`, Markdown gives
    `MD_ID_META = "import.md.id"`, and `check_meta_key` accepts both;
  - `crates/ferrite/Cargo.toml`;
  - 09 §8 (`import markdown`);
  - 08 (a short "Markdown import" section: shared run and map rules);
  - CLAUDE.md.

**Interfaces:**
- Consumes:
  - Task 5's `scan`, `convert`, `map_id`, `Problem`;
  - `ImportService::start(actor, "markdown:<name>", dry_run, takeover=false, None)`;
  - `ImportBatch::{content, media, path, set_body_and_fields}`;
  - `ImportHandle::finish`;
  - the importer actor lookup in `import_cmd.rs` (`importer`).
- Produces:
  - `pub struct MdReport { pub created: u32, pub updated: u32, pub unchanged: u32, pub kept_edited: u32, pub media: u32, pub problems: Vec<Problem>, pub front_page: Option<String> }`;
  - `pub async fn run(svc: &ImportService, handle: &ImportHandle, dir: &Path, opts: &MdOptions) -> Result<MdReport, MdError>`;
  - `MdOptions { kind: String, front_page: bool, dry_run: bool }`;
  - the CLI `ferrite import markdown DIR --data DATA [--as LOGIN] [--type page] [--front-page] [--dry-run] [--source NAME]`, with exit codes 0, 2, 3 and 1.

- [ ] **Step 1: Write the failing tests** (`import_md.rs`, both backends; build each
  fixture tree in a tempdir):
  - **`imports_a_tree`:**
    - pages with parents, slugs and `menu_order` as Task 5's fixture;
    - the root `index.md` page set as `reading.front_page` with `--front-page`;
    - a relative image imported as media, its body node pointing at
      `ferrite:media/<id>`;
    - `[x](02-configure.md#sec)` rewritten to `/configure/#sec` (the page's real path).
  - **`a_rerun_with_nothing_changed_writes_nothing`:** the revision count is unchanged
    and the report says `unchanged`.
  - **`an_edited_file_updates`:** the page body changes.
  - **`an_admin_edit_is_kept`:** edit the page through `ContentService::update`, change
    the file, re-run → `kept_edited`, and the body is unchanged.
  - **`a_renamed_file_makes_a_new_page`:** the old one stays.
  - **`outside_dir_is_never_read`:**
    - an image `../../secret.png` (the file exists outside DIR);
    - a symlinked `.md`;
    - an absolute-path image.

    Each is reported, nothing is imported from it, the other pages import, and the exit
    is 2.
  - **`bad_front_matter_skips_the_file_exit_2`.**
  - **`dry_run_writes_nothing`.**
  - **`busy_exit_3`:** while another run holds the lock.
  - **`exit_codes_documented`:** the `--help` text names 0, 2, 3 and 1.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3: Implement** `run`:
  1. pass 1: media for every image `convert` reported, under `ImportMedia` with
     `wp_id = map_id(rel_of_image)`, staged the way the WP media pass stages a local
     upload;
  2. pass 2: content, parents first, with `ImportContent` (author = actor's user,
     `created_at`/`published_at` = front matter `date` or the run start, `updated_at` =
     run start, `excerpt` and `seo.description` = `description`);
  3. pass 3: link fixup for docs whose `links` resolved, with
     `set_body_and_fields(id, body_with_paths)`.

  Use one batch per pass, committed, as the WP driver does (see `ferrite-import-wp/src/run.rs`).
  Set the front page after pass 2 through the settings write the WP options pass uses.
  Print the report as `created N, updated N, unchanged N, kept (edited here) N, media N`,
  then one line per problem: `<kind>  <file>  <detail>`.
- [ ] **Step 4:** `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test import_md` and
  `cargo test -p ferrite-import-wp` (id_meta change) → pass.
- [ ] **Step 5:** Docs: 09 §8 (synopsis, options, exit codes), 08 ("Markdown import":
  map key, hash, re-run table shared with WP) and CLAUDE.md. Commit:
  `feat(import): ferrite import markdown (M4-3b P62)`.

### Task 7: Docs site, part 1 (getting started and guides) and `site/build.sh`

**Files:**
- Create:
  - `site/README.md`, `site/build.sh` (P63), `site/VERIFIED.md`;
  - `site/content/index.md` (what FerriteCMS is, its principles, the two deployment
    profiles);
  - `site/content/01-getting-started/{index.md, 01-install.md (release binary with the
    `SHA256SUMS` and minisign check, Docker image, from source), 02-first-site.md (init,
    user create, serve, the admin), 03-concepts.md (types, items, terms, menus, themes,
    plugins)}`;
  - `site/content/02-guides/{index.md, 01-writing.md (the editor, publishing,
    scheduling, revisions, autosave, previews), 02-media.md, 03-content-types.md (types,
    fields, taxonomies, menus), 04-themes.md (the three built-in themes, activating,
    customising, installing a bundle), 05-plugins.md (installing with the permission
    report, settings, the three example plugins), 06-import-wordpress.md,
    07-import-markdown.md}`.
- Modify: root `README.md` (link to `site/`).

**Interfaces:**
- Consumes: `ferrite import markdown` (Task 6); the `docs` theme (Task 3).
- Produces: `site/build.sh OUT_DIR [URL]`. It uses `FERRITE` (default `cargo run -q -p
  ferrite --`). It exits non-zero on any failed step and on import exit 2 (broken links
  are a build failure).

- [ ] **Step 1: Write the pages.** Each is short and task-oriented, and uses headings
  that read well as a TOC. Commands are copy-pasteable and taken from 09 §8 or the
  `--help` output, never invented. Link between pages with relative `.md` links
  (rewritten on import).
- [ ] **Step 2: Verify every claim.** For each page, add rows to `site/VERIFIED.md`:
  `| Page | Claim | Checked against (file:line, test name, or command run) | Result |`.
  Run every command shown, in a scratch data folder, except release-download and Docker
  steps; mark those "not run" with the reason. Any claim you cannot check is removed or
  marked as such on the page.
- [ ] **Step 3: Build.** Run `site/build.sh <scratch>/docs-site http://localhost:8099`,
  then `serve` it. Fetch each page with curl and confirm 200 and the sidebar order.
  Paste the build output into the report.
- [ ] **Step 4:** Commit: `docs(site): getting started and guides as Markdown, build
  script (M4-3b P63)`.

### Task 8: Docs site, part 2 (developers, operations, reference) and the site test

**Files:**
- Create:
  - `site/content/03-developers/{index.md, 01-theme-authoring.md (manifest, hierarchy,
    context, every helper incl. `page_tree`, `post.toc`, `query(order=)`), 02-plugin-
    authoring.md (the world, the manifest, capabilities, limits, the SDK, building for
    `wasm32-wasip2`), 03-rest-api.md (auth, CSRF, tokens and scopes, the OpenAPI
    spec)}`;
  - `site/content/04-operations/{index.md, 01-configuration.md (every `ferrite.toml`
    section and the `FERRITE_*` overrides), 02-backups.md, 03-upgrades.md,
    04-https.md, 05-monitoring.md (metrics, doctor), 06-cluster.md}`;
  - `site/content/05-reference/{index.md, 01-cli.md (every command, synopsis and exit
    codes)}`;
  - `crates/ferrite/tests/docs_site.rs`.
- Modify: `site/VERIFIED.md`.

**Interfaces:**
- Consumes: Tasks 6 and 7.

- [ ] **Step 1: Write the site test** (`docs_site.rs`, SQLite only; it is about content,
  not storage).
  - In a tempdir: `init`, then `import markdown <repo>/site/content --front-page`, then
    `theme activate docs`, then `serve` on a free port.
  - Read `/sitemap.xml` and its files, then for every URL check:
    - status 200;
    - exactly one `<h1>`;
    - every same-site `href` (excluding `/_theme/`, `/_media/` and `/admin/`) answers
      200, or a 301/308 to a 200;
    - every `href="#x"` inside the TOC `nav` has a matching `id="x"`.
  - Also assert the import exited 0 and the sitemap lists at least 20 URLs.
- [ ] **Step 2:** Run → FAIL (part 2's pages are missing, so the 20-URL assertion fails).
- [ ] **Step 3: Write the pages.** Same rules as Task 7, Steps 1 and 2.
  - The configuration page lists each key with its default from
    `crates/ferrite-config/src`, cited in `VERIFIED.md`.
  - The CLI page is generated by hand from `ferrite <cmd> --help`. Run each command and
    paste the commands checked into `VERIFIED.md`.
- [ ] **Step 4:** `cargo test -p ferrite --test docs_site` → pass. Commit: `docs(site):
  developer, operations and reference pages; the site test (M4-3b P63)`.

### Task 9: Accessibility checks for the public themes

**Files:**
- Create: `admin/e2e/public-themes.spec.ts`
- Modify:
  - `admin/e2e/support/` (a seed for the business profile and the imported docs content,
    if global setup needs them; prefer a per-spec data folder if the harness supports
    one, else seed through the CLI before `serve` in a dedicated project);
  - `admin/playwright.config.ts` (project `public-themes-chromium`);
  - `.github/workflows/ci.yml` (run that project in the `admin-e2e` job);
  - 06 §8;
  - ROADMAP (M4-4 axe item → done).

**Interfaces:**
- Consumes: the three themes, `seed demo --profile business`, `import markdown`.

- [ ] **Step 1:** Write the spec per P64. Use `expectAccessible` from
  `support/fixtures.ts` (both colour schemes). For each theme:
  - activate it with `POST /api/v1/themes/{name}/activate`;
  - wait for the node state as `themes.spec.ts` does;
  - visit the P64 pages and run axe;
  - restore `blog` in `afterAll`.

  Keyboard check on `docs`: Tab from the top reaches the skip link first, and Enter moves
  focus to `<main>`.
- [ ] **Step 2:** `pnpm -C admin exec playwright test --project public-themes-chromium`.
  Fix every violation in the theme CSS or templates; never weaken the rule set. Re-run →
  pass.
- [ ] **Step 3:** Wire CI. The step runs after the admin build, like the other E2E
  projects. Commit: `test(themes): axe checks for the blog, docs and business themes
  (M4-3b P64)`.

### Task 10: Exit

**Files:**
- Create: `docs/superpowers/m4-3b-exit.md` and `docs/superpowers/m4-3-exit.md` (a short
  M4-3 summary pointing at both exits).
- Modify:
  - ROADMAP (M4-3 items done; "Deferred from M4-3b");
  - CLAUDE.md ("Current status", crate map with `ferrite-import-md`, run instructions);
  - `README.md`;
  - 01 §1.

- [ ] **Step 1: End-to-end, by hand, with the release build:**
  - **docs site:** `site/build.sh`, `serve`, open in Chromium (screenshots of desktop and
    375 px wide, light and dark).
  - **business site:** `seed demo --profile business`, activate `business`, install the
    contact-form plugin from the packed asset with `contact_form = true`, submit the form
    and see the thank-you page.
  - **WordPress re-check:** run `ferrite import wordpress` on the M3 corpus file. The
    report counts are unchanged except for the new heading ids.
- [ ] **Step 2: Gates.**
  - `cargo fmt`, clippy and rustdoc on both workspaces;
  - `FERRITE_TEST_PG_URL=… cargo test --workspace --no-fail-fast`: 0 failed, no `SKIP`;
  - the admin's `pnpm -C admin test`, `pnpm -C admin build`, `bundle:check` and the new
    Playwright project.
- [ ] **Step 3: Write the exit.**
  - **Sections:** what shipped, the end-to-end checks, gates, found during the exit,
    accepted limits, not verified, for the user, open follow-ups, and a "Verified claims"
    table (the site's `VERIFIED.md` summarised, with its count of claims checked and not
    checked).
  - **For the user:**
    - hosting the docs site (domain, where it runs, whether to publish a static mirror);
    - the `docs` theme's lack of syntax highlighting.
  - Commit: `docs: M4-3b exit`.

## Self-review notes

- **Spec coverage:** every ROADMAP M4-3 item has a task.
  - page tree, prev/next, breadcrumbs: Task 2;
  - heading anchors, TOC: Task 1;
  - `docs`: Task 3;
  - `business`: Task 4;
  - Markdown import: Tasks 5–6;
  - the docs site: Tasks 7–8;
  - BRIEF's "docs/knowledge-base, small business site": Tasks 3–4;
  - 06 §8 WCAG goal: Task 9.
- **Left out on purpose (Later):**
  - syntax highlighting;
  - search snippets and section-scoped search;
  - static export (`ferrite site export --html`);
  - versioned docs;
  - admonitions, footnotes, task lists;
  - YAML front matter;
  - per-menu-item attributes;
  - theme-shipped content types.

  Task 10 adds them to ROADMAP "Later".
- **Parallel work:**
  - Tasks 1 and 5 touch disjoint files and can run in parallel.
  - Task 2 needs Task 1's view changes only for `post.toc` (no shared file).
  - Tasks 3 and 4 need Task 2.
  - Task 6 needs Task 5.
  - Tasks 7–8 need Tasks 3 and 6.
  - Task 9 needs Tasks 3, 4 and 6.
