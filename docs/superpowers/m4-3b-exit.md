# M4-3b exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M4-3; plan
[`2026-10-30-m4-3b-themes-docs.md`](plans/2026-10-30-m4-3b-themes-docs.md), Task 10,
ruling P41): *the starter themes `docs` and `business` with the template helpers a docs
theme needs (page tree, previous/next, heading anchors, TOC), Markdown import, and a docs
site built with FerriteCMS itself. The exit builds the docs site with `site/build.sh` and
the release binary and looks at it in a browser (desktop and 375 px, light and dark), seeds
the business site, installs the contact-form plugin from the packed asset, submits its form
for real and checks the thank-you page, re-imports the M3 WordPress corpus, and runs the
full gates.*

**Result (2026-10-31, branch `m4-3b-exit` from `m4-3` at `ba6d738`):** every exit check
passed on macOS 26.6 (arm64) with local PostgreSQL 17.11 (port 54329), `rustc` 1.99.1,
Node 24.18.0 and pnpm 10.28.2. Nothing was found that needed a code fix. The first
workspace test run had one timing failure under load; the second, on an idle machine, had
none (§2). Nothing was pushed, tagged, published or hosted.

| Exit check | Verified by | Result |
|---|---|---|
| (a) Docs site: `site/build.sh` with the release binary, `serve`, every sitemap URL, screenshots in Chromium at 1440 px and 375 px, light and dark (and, beyond the brief, Firefox and WebKit in light) | §1 (a) | yes |
| (b) Business site: `seed demo --profile business`, `theme activate business`, the contact-form plugin from `package-plugins.sh`'s asset, `contact_form = true`, one real form submit in Chromium, the thank-you page, the mail and the inbox | §1 (b) | yes |
| (c) WordPress re-check: the M3-1 CLI run on `site-a.xml` and the real-world corpus test, counts against M3-1's | §1 (c) | yes: identical counts |
| (d) Gates: fmt, clippy and rustdoc on both workspaces, the workspace tests with Postgres, the admin gates, the `public-themes-chromium` project | §2 | yes: second run 119 test binaries, 3822 passed, 0 failed, 2 ignored, no `SKIP` (the first, under load, had 1 timing failure) |

## What shipped

Per task (merges into `m4-3`; rulings in [`rulings/m4.md`](rulings/m4.md), "M4-3b plan
rulings" P54–P65 and "M4-3b execution rulings" R-b1–R-b17; task reports and reviews in
the controller's ledger):

1. **Heading ids and `post.toc`** (`2652c46`; P54, P55): the block renderer gives every
   heading an `id` by GitHub's rule (lowercase plain text, alphanumerics, `-`, `_`, spaces
   to `-`; `section` when empty; `-1`, `-2` for repeats; seeded with the ten reserved ids
   of `ferrite_domain::html::RESERVED_FRAGMENT_IDS`); `PageItem.toc` (`TocEntry {level, id,
   text}`) is `post.toc` in templates (06 §4).
2. **Markdown to blocks** (`e0d1fe2`; P62): new crate `ferrite-import-md` — TOML front
   matter between `+++` lines, the folder scan (`index.md`/`README.md` as a folder's page,
   `NN-` order prefixes, 5000 files, 1 MiB each, symlinks never followed), CommonMark plus
   GFM tables and strikethrough through `pulldown-cmark` `=0.13.6` (and its
   `pulldown-cmark-escape`) and the WordPress crate's HTML converter (R-b4, R-b5).
3. **`page_tree()` and `query(order=)`** (`35f31c3`; P56–P58, R-b1, R-b10):
   `SiteReader::tree(kind)` with the sitemap's visibility, at most 1000 nodes
   (`truncated`), `archive:<kind>` as its cache key; the theme helper `page_tree(post=,
   kind=)` with `items`, `current`, `ancestors`, `children`, `prev`, `next`; nodes carry
   `excerpt` and `featured` (public media only); 64 levels at most; `query(order="menu")`.
4. **`ferrite import markdown`** (`11adb6f`; P62, R-b6, R-b9, R-b12, R-b13): runs as an
   import run (`markdown:<NAME>`) with the WordPress import's lock and fence, `import_map`
   re-runs (unchanged, updated, kept when edited here, not in the source), media, links
   rewritten to page paths, `--front-page` only for a public root index, future dates
   scheduled; `ImportRepo::map_list` on both backends; 08 §8, 09 §8.
5. **The `docs` theme** (`1182368`; P59, P65): `themes/docs`, page-tree sidebar (a "Pages"
   toggle under 60rem), breadcrumbs, TOC (levels 2–3), previous/next, `docs.js` (2 073
   bytes: heading anchors, Copy buttons), light and dark, contrast stated in the CSS; the
   P65 size test over every embedded theme.
6. **The `business` theme and `seed demo --profile business`** (`9930a25`; P60, P61,
   R-b15): hero, feature cards from the front page's children, latest news, CTA with a
   scheme allowlist, the contact-form plugin's form on `page-contact.html` when
   `contact_form` is on; the seed writes "Larkspur Studio" (a made-up studio) in one
   transaction; the `blog` profile is unchanged byte for byte (pinned).
7. **Docs site, part 1** (`9ba8f74`; P63, R-b14): `site/content/` getting started and
   guides, `site/build.sh`, `site/README.md`, `site/VERIFIED.md`; `/site/` is no longer
   git-ignored; `shellcheck` of `site/build.sh` in CI.
8. **Docs site, part 2 and the site test** (`ba6d738`; P63, R-b2): developers, operations
   and reference pages (26 pages in all), `crates/ferrite/tests/docs_site.rs` (SQLite: every
   sitemap URL 200 with one `<h1>`, every same-site link 200 or a redirect to 200, every TOC
   link an id on its page); doctor's `--offline` help corrected; 06's theme-name rule fixed.
9. **axe checks for the public themes** (`56ff105`; P64, R-b16, R-b17): Playwright project
   `public-themes-chromium` (`admin/e2e/public-themes.spec.ts`, one SQLite site per theme,
   WCAG 2.2 AA tags in both colour schemes), its own CI step and failure upload; code blocks
   render as `<pre tabindex="0">` from core, and the plugin sanitiser keeps exactly that
   attribute; 05, 06 §5 and §8.

29 non-merge commits, 174 files, +17 033 / −296 lines from the plan commit `041b94c` to
`ba6d738`. No migration. No WIT change (the world stays `0.1.0`). No REST API change (the
OpenAPI snapshot is unchanged).

## 1. End-to-end checks

The release binary and the admin of `ba6d738`: `pnpm -C admin install --frozen-lockfile &&
pnpm -C admin build` (`check-dist: ok`), then `cargo build --release -p ferrite` (2 m 58 s)
and `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml
--locked`. Scratch area `$E` = the session scratchpad's `m43b-exit/` (left in place);
screenshots in the controller's `.superpowers/sdd/2026-10-30-m4-3b-themes-docs/shots-exit/`
(37 PNGs; git-ignored), taken with Playwright 1.64.0 from the admin's dependencies.

### (a) The docs site

```
$ FERRITE_LOG=warn FERRITE=target/release/ferrite site/build.sh $E/docs http://localhost:8311
==> init $E/docs (http://localhost:8311)
==> admin account 'docs' (no password shown; see the README to set one)
==> import markdown site/content
created 26, updated 0, unchanged 0, kept (edited here) 0, media 0
front page: index.md
==> activate the docs theme
activated theme docs
Built. Serve it with:
  FERRITE_SERVER__LISTEN=127.0.0.1:8311 target/release/ferrite serve --data $E/docs
exit=0
```

`serve` as printed. `/sitemap.xml` names `sitemap-page-1.xml`: 26 sitemap URLs, plus `/`
fetched on its own. Each sitemap URL answered 200 with exactly one `<h1>` (the root index's
`/home/`, `/getting-started/` and its 3 pages, `/guides/` and its 7, `/developers/` and its
3, `/operations/` and its 6, `/reference/` and `/reference/cli/`); `/` 200 with one `<h1>`,
`/?s=theme` 200, `/nope/` 404. Code blocks
are `<pre tabindex="0">` (4 on the Install page); headings carry ids
(`build-from-source`, `release-binaries-not-published-yet`, …). `/` has the canonical
`http://localhost:8311/` and `/home/` the canonical `http://localhost:8311/home/` (the
duplicate front page, §7).

**Screenshots** of `/`, `/getting-started/install/` and `/operations/configuration/` at
1440 × 900 and 375 × 812, light and dark, plus the "Pages" toggle opened at 375 px; the
same in Firefox and WebKit in light (28 docs shots). Measured in each engine: no
horizontal page scroll at either width (`scrollWidth == clientWidth`), one `<h1>`, the
sidebar nav and the TOC visible at 1440 px and hidden at 375 px, 26 sidebar links in the
DOM once "Pages" is opened (the closed sections' links are in collapsed `<details>`),
`docs.js`'s anchors (3, 5 and 14 on the three pages) and one Copy button per code block
(4 and 2), no console error.

What the shots show:

- **Desktop, light** (`docs-getting-started-install-desktop-light.png`): a sticky header
  with "FerriteCMS" and the search field; the left sidebar with Introduction, then the
  sections (Getting started open with Install marked by an accent bar and tint, the
  others closed with a chevron); breadcrumbs "Getting started / Install", the title, body
  text with inline code, a code block with a Copy button at its top right; "On this page"
  on the right with h2 entries and an indented h3 ("Check a download"). The front page
  marks Introduction current and shows the blue-ruled "Status" quote.
- **Desktop, dark**: the same layout on a near-black background with light text; the
  current item and links keep the accent; the code block's background is darker than the
  page's; the text is readable throughout.
- **375 px, light and dark**: one column; the header holds the title and a full-width
  search; a closed "Pages" toggle sits above the breadcrumbs; no TOC; text wraps without
  overflow. On the configuration page the Copy button sits above the code block (under
  30rem) and the long command scrolls inside the block, not the page.
- **375 px, "Pages" open** (dark): the section list under the toggle (Introduction, then
  Getting started open on the trail with Install current, the other four sections
  closed). The first capture was taken during the 0.15 s chevron rotation and showed it still pointing right; re-taken after 0.4 s the computed
  transform is `rotate(45deg)` (pointing down) — a capture artefact, not a defect.
- **Firefox and WebKit** (light, both widths): the same layout as Chromium, sidebar and TOC
  in place at 1440 px, the toggle and its tree at 375 px. This is a visual check only; axe
  ran in Chromium only (§2, §5).

### (b) The business site and the contact form

```
$ .github/scripts/package-plugins.sh 0.1.0 $E/asset        # seo/, analytics/, contact-form/
  sha256 4612db0a9871076dd060300c62a35bf6c76956510cbfa2594be86c1511aca6f7
$ tar -xzf $E/asset/ferrite-plugins-0.1.0.tar.gz -C $E/plugins
$ ferrite init --data ./biz --url http://localhost:8312
$ ferrite seed demo --profile business --data ./biz
  Seeded business demo content: 3 posts, 6 pages, 1 term, 2 menus, 4 images (admin user: admin).
  next: ferrite theme activate business --data ./biz
$ ferrite theme activate business --data ./biz          # activated theme business
$ ferrite user create --data ./biz --login me --email me@localhost.test --password-stdin
$ (ferrite.toml) [mail] smtp_url = "smtp://127.0.0.1:2527", from = "Larkspur Studio <noreply@localhost.test>"
$ ferrite config check --data ./biz
  ok: mode=solo database=sqlite media=local workers=14 mail=configured metrics=off tls=off backup=off
$ ferrite plugin install --data ./biz --dir ./plugins/contact-form --enable --yes
  plugin contact-form 0.1.0 (new) … fingerprint: 5dd20d5a…f6800
  installed plugin contact-form 0.1.0 (enabled)
```

The SMTP sink is M4-3a's 60-line asyncio script (plain SMTP, each message appended as a
JSON line). With `serve` on `127.0.0.1:8312`: `POST /api/v1/auth/login` (`Origin:
http://localhost:8312`) 200; `PATCH /api/v1/themes/business/settings` `{"contact_form":
true}` 200 (the values now show `contact_form: true`); `PATCH
/api/v1/plugins/contact-form/settings` `{"viewers": "<me's id>"}` 200.

**The real submit**, in Chromium at `http://localhost:8312/contact/`: the form rendered by
the theme (`form.contact-form`, `action="/_p/contact-form/submit"`, a hidden `return` of
`/contact/`, the honeypot inside an `aria-hidden` block); the name, e-mail, subject and a
three-line message typed into the fields found by their labels; "Send" clicked. The
browser posted to `http://localhost:8312/_p/contact-form/submit` with `Origin:
http://localhost:8312`; the answer was 200 `text/html` under `content-security-policy:
sandbox allow-forms; form-action 'self'`, reading "Thank you / Your message was sent.
Thank you for getting in touch. / Back to the site"; its only link is `/contact/`, and
following it landed on `http://localhost:8312/contact/` ("Contact – Larkspur Studio").

The sink received one message: `MAIL FROM <noreply@localhost.test>`, `RCPT TO` both admins
(`admin@example.com` from the seed, `me@localhost.test`), `Subject: [Larkspur Studio]
[Contact] A garden for a small yard`, `text/plain`, with the name, e-mail, subject, `Page:
/contact/`, the time and the message. `GET /_p/contact-form/inbox` with the session: "1
message, newest first", the same message; anonymous: 403.

**Screenshots** (`business-{front,contact}-{desktop,375}-{light,dark}.png`,
`business-thank-you-desktop-light.png`): the front page's header with the menu (Home
current) and the "Get in touch" button; a gradient hero image with the dark-teal panel
"Gardens that grow with you", the lede and a white CTA; the intro paragraph; three cards
(Garden design, Consulting, Aftercare) each with a 3 px accent rule, a gradient image,
title and excerpt; the tinted "Latest news" band with three dated posts and "All news";
the footer menu. At 375 px the panel overlaps the bottom of a shorter hero and the header
button stays visible; no horizontal scroll at either width. The contact page shows the
body on the left and the "Send a message" panel on the right with labelled fields and a
"Send" button (stacked at 375 px). The thank-you page is the plugin's own plain page.

### (c) The WordPress corpus

M3-1's CLI run (`m3-1-exit.md` §2) repeated with the release binary on a fresh site
(`init --url http://127.0.0.1:18081`, `user create --login importer`):

```
$ ferrite import wordpress crates/ferrite-import-wp/tests/corpus/site-a.xml --data ./wp \
    --uploads crates/ferrite-import-wp/tests/corpus/site-a-uploads
  book 2, comments 4, media 4, menus 1, options 3, page 4, post 8, redirects 7,
  taxonomies 1, terms 9 (1 skipped), types 1, users 4 created; fixups 2 updated; exit=0
$ (the same command again)
  every kind unchanged (book 2, comments 4, media 4, menus 1, options 3, page 4, post 8,
  redirects 7, taxonomies 1, terms 9 (1 skipped), types 1, users 4); exit=0; ./wp/import holds only reports/
```

Identical to M3-1's counts, kind by kind. Served: `/2024/03/every-core-block/` 200
(`<title>Every core block - Corpus Site`) with `<h2 id="a-level-two-heading">` and `<h3
id="a-level-three-heading">` (the new ids; the theme's own headings are unchanged),
`/about/team/` 200, `/tag/rust/` 200, `/hero/` 301 to `/_media/d54bb56e…/hero.png`. The
heading ids are rendered, not stored, so the import report does not change.

The real-world corpus (`FERRITE_WP_CORPUS=…/.superpowers/m3-corpus cargo test -p
ferrite-import-wp --test corpus_real -- --nocapture`; each file into a fresh SQLite site
without uploads): 1 passed, and every count equals `m3-1-exit.md` §1 —
`themeunittestdata.wordpress.xml`: posts 58, pages 21, comments 29 created and 4 skipped,
terms 178 created, 1 unchanged and 6 skipped, menus 6, redirects 65 created and 1 refused,
users 1 created and 1 unchanged, options 2, media 37 missing, unknown blocks 30 and
shortcodes 2, the same dropped-formatting counts, 79 items editor-safe;
`64-block-test-data.xml`: posts 61, pages 1, comments 2, terms 11 created, 11 unchanged and
2 skipped, redirects 4, users 1 and 1, options 2, media 9 missing, unknown blocks 29, the
same dropped-formatting counts, 62 items editor-safe.

## 2. Gates

At `ba6d738` (the code is unchanged on this branch; logs in the scratchpad `m43b-exit/`):

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | ok |
| `cargo fmt --manifest-path plugins/Cargo.toml --all --check` | ok |
| `cargo clippy --workspace --all-targets -- -D warnings` | ok |
| `cargo clippy --manifest-path plugins/Cargo.toml --target wasm32-wasip2 -- -D warnings` | ok |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | ok |
| `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml --locked` | ok |
| `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --no-fail-fast` | 119 test binaries (doc-tests included), 3821 passed, 1 failed, 2 ignored; no `SKIP` line in the log. The failure: `cluster` `jobs::lease_expiry_after_crashed_node` (`attempts` 2, expected 1: node 0 missed a renewal of its 2 s test lease while the exit's three browsers, two `serve`s and the test run shared the machine). The `cluster` binary run alone three times afterwards: 13 passed each time. Neither `ferrite-cluster` nor that test changed in M4-3b. **Second full run** (after the E2E runs, nothing else on the machine): exit 0, 119 test binaries, 3822 passed, 0 failed, 2 ignored, no `SKIP` line (the first run's 3821 + 1 failed). New binaries in the first run: `docs_site` 4, `import_md` 40, `site_tree` 12, `ferrite-import-md` 25 unit, `docs` 15, `business` 8; `openapi_snapshot_is_current` and `embedded_themes_stay_small` passed |
| `FERRITE_WP_CORPUS=… cargo test -p ferrite-import-wp --test corpus_real` | 1 passed; counts as in §1 (c) |
| `pnpm -C admin lint`, `typecheck` | ok |
| `pnpm -C admin test` | 125 files, 748 tests passed |
| `pnpm -C admin build` | `check-dist: ok` |
| `pnpm -C admin bundle:check` | ok |
| `pnpm -C admin e2e --project public-themes-chromium` (the release binary of §1) | 17 passed (20.9 s) |
| `pnpm -C admin e2e --project chromium-sqlite` (not asked by the brief; Task 9 had not re-run it after R-b17's core change) | 195 passed (8.1 min), with its `-site-wide` teardown |

The workspace tests ran while the end-to-end checks used the machine (§1), as in earlier
exits; the `cluster` timing tests are the ones known to suffer from that (M4-3a §9 had
`invalidation_latency_under_1s`).

## 3. Found during the exit

Nothing that needed a code change. Docs: the execution rulings R-b4 to R-b17 copied into
[`rulings/m4.md`](rulings/m4.md) (05, 06 and `ci.yml` already cited R-b16 and R-b17; R-b1
notes Task 2's public-media check), with a pointer to the ROADMAP lists; the ROADMAP (M4-3
done, "Deferred from M4-3b", the Later items the plan left out); CLAUDE.md (status, crate
map, the docs-site and axe commands); README.md (status, the docs site); 01 §1 (layout,
`ferrite-import-md`, `site/`, the dependency graph, `pulldown-cmark`); this file and
[`m4-3-exit.md`](m4-3-exit.md).

## 4. Accepted limits

- **Heading ids are GitHub-style, not GitHub's exactly:** combining marks are dropped
  (`is_alphanumeric`), so ids for text with combining characters can differ from GitHub's
  (Task 1 review Minor 1).
- **A symlinked `index.md` next to a real `README.md`** makes the README the folder's page
  (the symlink is reported `outside` and skipped; Task 6 re-review 2 Minor 2).
- **No syntax highlighting in `docs`:** code blocks are plain text with a `language-x`
  class (P59; Later).
- `page_tree` lists at most 1000 nodes per type (the theme says so) and drops nodes deeper
  than 64 levels (R-b10); a plugin's `content.body` filter that removes a heading leaves its
  TOC entry (P55).
- Markdown import: TOML front matter only (P62); a changed image is not replaced by a
  re-run (`image-changed`); a rooted `src` is kept as written and never read; GFM column
  alignment is dropped without a note; a dry run is one transaction, so one longer than
  120 s can outlive the run's staleness window (exit 1), and on Postgres it holds the row
  locks of the rows it updates until it ends (09 §8).
- Code blocks scroll sideways with a focus stop (`tabindex="0"`, R-b17); an author's
  `<pre tabindex>` inside a raw `html` block is still stripped.
- The seeded business pages live under the front page (`/home/garden-design/`), as pages
  follow their parent's slug (WordPress does the same); `business`'s `single.html` has no
  comment thread.
- Accepted in the task reviews, recorded here: `ImportBatch::mapped`'s filter has no direct
  core test (covered by the `import_md` not-in-source tests and the store contract test);
  the `import_md` tests' store open retries a SQLite "in use" error for up to 5 s (a
  test-only retry; the CLI's own open has none); `page_tree`'s flat nodes (`ancestors`,
  `prev`, `next`, grandchildren) carry `children = []`, so a template must not use
  `node.children` on them to decide "is a section" (Task 2 concern 3).
- Pale accent colours fall under 4.5:1 in light mode in both new themes (stated in the CSS
  and the setting's label, as in `blog`).
- `query(order="menu")` (and `SiteReader::latest` with `ListOrder::Menu`) picks its rows by
  `(menu_order, id)` and sorts them by title only afterwards: with the default
  `menu_order` 0, `limit=3` gives the three oldest-created items shown by title, not the
  first three by title. This deviates from P58's "`menu_order` ascending, then title" for
  the rows picked (final review A-M2; 06 §4 says so; the store order that would fix it is
  in §7).

## 5. Not verified

| Item | Why | Covered instead by |
|---|---|---|
| Any M4-3b code on Linux: the tests, the builds and every exit run were on macOS (arm64) | macOS host; nothing pushed | CI's Linux jobs once pushed ("Deferred from M4-3b") |
| axe in Firefox and WebKit, at phone width, on a comment thread with comments, and on the contact-form plugin's own pages | P64 scoped the project to Chromium at desktop width; the demo seed writes no comments | the exit's Firefox and WebKit screenshots of the docs site (visual only, §1 a); the axe run of the contact page with `contact_form` on and the plugin absent (Task 9) |
| The docs site on Postgres | R-b2: `docs_site.rs` runs on SQLite | the tree and the Markdown import tests on both backends (Tasks 2 and 6) |
| A real SMTP provider for the contact form | a local plain-SMTP sink | M4-3a's checks; the mailer's TLS modes are M2a-2a's |
| The docs site hosted anywhere (domain, TLS, a static mirror) | the user's decision (P63) | §6 |
| 61 of the 351 claims in `site/VERIFIED.md`: one not run (the `git clone`), 60 verified with a part not run or resting on the design, a runbook or `--help` (no release or image published, no S3 bucket, no SMTP provider, no https site or Let's Encrypt, the `[backup]` schedule, the compose example, a TOTP sign-in, the interactive install prompt, error cases not provoked, admin screens not opened, tests cited but not re-run) | not runnable here; each row says what and why | the code, design docs, `--help` and tests each row cites (§8) |
| A Markdown dry run longer than 120 s | not run (read from the fence code, Task 6) | 09 §8 states it |
| The full admin E2E matrix (`firefox-sqlite`, `webkit-sqlite`, `chromium-pg`) after M4-3b | not run in this exit (no admin change in M4-3b; the brief asked for the public-themes project) | `chromium-sqlite` in full, 195 passed (§2), on the final tree with R-b17's `<pre tabindex>` |

## 6. For the user

- **Hosting the docs site** is your decision (P63): the domain, where it runs (a solo
  FerriteCMS node built with `site/build.sh`, or a static mirror of it), and whether CI
  publishes it. Nothing is hosted or published. `site/build.sh OUT_DIR URL` builds a
  served site today; a static mirror would need the Later `ferrite site export --html` or
  an outside crawler. Until it is hosted, the docs say "no hosted copy of these docs"
  (`site/content/index.md`), and the install page names the repository URL
  `https://github.com/softberries/ferritecms.git`, whose public visibility was not checked.
- **No syntax highlighting** in the `docs` theme: code blocks are plain monospace on a
  tinted block (dark in dark mode). Adding it means a highlighter in core's renderer or a
  script in the theme; it is under Later.
- Nothing new in the release gate beyond M4-2's and M4-3a's.

## 7. Open follow-ups

In [ROADMAP](../ROADMAP.md), M4-4, "Deferred from M4-3b":

- the front page answers at both `/` and its permalink (`/home/` on the docs site), each
  with its own canonical URL — duplicate content; a 301 from the permalink to `/` (as
  WordPress does) or a canonical `/` fixes it in the router; the docs sidebar's
  Introduction links to `/home/`;
- `image()`'s `srcset` mixes image shapes (core, shared with `blog`);
- rust-embed `debug-embed` does not notice a new theme folder until `cargo clean -p
  ferrite-theme`;
- the axe coverage gaps of §5 (Firefox, WebKit, phone width, a thread with comments, the
  plugin's pages);
- `query(order="menu")` ties across a `limit` follow id order (a store order with the title
  would make it exact);
- the first Linux CI runs of the new tests;
- the final review's Minors that were not fixed (§9): the tree's full-row read, tree
  excerpts bypassing `content.excerpt`, card images spending the bridge budget, the
  `page_tree()` memo past 16 pairs, `menu_order_cmp`'s allocations, the Markdown import's
  ~5 GB worst case and its folder count, a WXR sharing a Markdown source name, the stale
  binary poll message, `build.sh` and IPv6 loopback URLs, and the inline styles a future
  public CSP would need to allow.

"Later" has the items the plan left out on purpose: syntax highlighting, search snippets and
section-scoped search, a static export (`ferrite site export --html`), versioned docs,
admonitions, footnotes and task lists, YAML front matter, per-menu-item attributes and
theme-shipped content types. Next: M4-4.

## 8. Verified claims

`site/VERIFIED.md` lists every claim in `site/content/` with what it was checked against
(part 1 by Task 7, part 2 by Task 8, both re-checked in their reviews). Counted here with
one rule, applied to every row (by a scratch script, not kept); a few rows sit on the
boundary (`migrate down`'s "only its refusal", a "from the code" operations row), so the
split is a judgement at the margin, about ±2 rows:

- **Not run:** the Result starts with "**Not run**".
- **Verified, a part not run:** some part of the claim was neither executed nor read in the
  code. The "Checked against" or Result column says a step was not run, re-run, provoked,
  opened, exercised, tested, made, compiled, published, started, sent or tried (or "no …
  run"), or the Result says a part rests only on the design docs, a runbook, the code's doc
  comments or `--help` ("Verified against design", "from the help", "only 0 run").
- **Verified:** everything else: run, or read in the code or tests at a cited place.

| Section | Rows | Verified | Verified, a part not run | Not run |
|---|---|---|---|---|
| Introduction | 11 | 10 | 1 | 0 |
| Getting started | 47 | 39 | 7 | 1 |
| Guides | 88 | 74 | 14 | 0 |
| Developers | 81 | 72 | 9 | 0 |
| Operations | 99 | 72 | 27 | 0 |
| Reference | 18 | 16 | 2 | 0 |
| The site build | 7 | 7 | 0 | 0 |
| **Total** | **351** | **290** | **60** | **1** |

"Verified" means run (on 2026-10-31, debug builds of the task branches) or read in the code
with `file:line`. The one row not run is the `git clone` of the install page. No row is
marked failed: a claim that failed its check was corrected on the page before its row was
written (Task 7: the password rule, the permission-report limits, deep folders skipped,
trash restoring to draft; the reviews: the "both profiles" claim, the 100 fields per level,
doctor's `--offline` wording, `metrics_listen`'s clash rule).

Claims in this file:

| Claim (where) | Source | Result |
|---|---|---|
| The docs site imports 26 pages and every sitemap URL answers 200 with one `<h1>` (§1 a, ROADMAP) | the build output and the curl loop (§1 a); `docs_site` in §2 | verified |
| The contact form posts, mails both admins and lands in the inbox; the thank-you page links back to `/contact/` (§1 b) | the browser run, the sink's JSON line, the inbox page (§1 b) | verified |
| The WordPress counts equal M3-1's (§1 c) | `m3-1-exit.md` §1 and §2 against this run | verified |
| Each starter theme is under 150 KB (ROADMAP, 01) | `embedded_themes_stay_small` (in §2's run) | verified |
| No migration, no WIT change, no REST change (What shipped) | no new file under `crates/ferrite-store-*/migrations/` or `wit/` from `041b94c` to `ba6d738`; `openapi_snapshot_is_current` passes (§2) | verified |
| `pulldown-cmark` is the only new direct external dependency; `Cargo.lock` gains it and its `pulldown-cmark-escape` (01) | `git diff 7c24df6 ba6d738 -- Cargo.toml Cargo.lock` | verified |
| 29 commits, 174 files, +17 033 / −296 (What shipped) | `git log --no-merges 041b94c..ba6d738 \| wc -l`; `git diff --shortstat 041b94c ba6d738` | verified |

## 9. Final review

The whole-branch review (`10b71fe..5aea604`) was done in three parts: A (core and rendering),
B (the Markdown importer) and C (themes, E2E, CI). One fix round on branch
`m4-3b-final-fix` from `m4-3` at `5aea604`; each code fix had its failing test first.

**Fixed:**

| Finding | Change |
|---|---|
| A-I1: the plugin sanitiser stripped heading ids, so any non-raw `content.body` filter broke `post.toc` and deep links | `id` on `h1`–`h6` kept when `ferrite_domain::html::heading_id_allowed` accepts it (non-empty; lowercase Unicode alphanumerics, `-`, `_`; not reserved), dropped on every other element (`crates/ferrite-plugin-host/src/validate.rs`); tests `a_core_rendered_body_survives`, `hostile_heading_ids_are_dropped`, `every_given_id_survives_the_plugin_sanitiser` (anchor), `heading_ids` (domain), and `sanitised_content_filters` (`ferrite-http` `tests/plugins`, html-inject over a core body with headings and a code block, both profiles); 05 §7, 06 §4/§5 |
| A-M1: the themes' other fixed ids were not reserved | `RESERVED_FRAGMENT_IDS` (still in `ferrite-domain`, used by both crates) gains every fixed id of the three themes (30 in all); test `fixed_ids` (`ferrite-theme`) reads the templates, including `form_id`-derived ids; 06 §4 |
| A-M2: `query(order="menu")` picks rows by `(menu_order, id)` | recorded: 06 §4 and §4 above (accepted limit); the exact store order stays in §7 |
| B-I1: one front matter `description` (multi-line, over 500 characters) or a NUL in `title` failed the whole run | `front::parse` collapses whitespace/control runs to one space, trims, cuts to 500 characters on a char boundary (notice `description-cut`), checks the result with `SeoPatch::check` (a refusal skips the file as `bad-front-matter`), strips NUL from `title`; tests in `front.rs` and `front_matter_core_would_refuse_is_cleaned` (both backends); 08 §8, 09 §8, the Markdown guide |
| B-I2: no `--takeover` for `import markdown` | added, passed through `import_cmd.rs`; test `a_stale_run_is_taken_over` (exit 3, then 0 with the flag; both backends) and `exit_codes_documented`; 09 §8, the CLI reference, the guide, CLAUDE.md |
| B-M1: `front page: index.md` printed when the site's choice was kept | `FrontPage::Kept` sets no `front_page`; test `a_kept_front_page_is_not_reported_as_set` |
| B-M5: a link to a non-Markdown file in the folder was neither imported nor reported | reported `broken-link` (it would 404); test `unimported_links_and_protocol_relative_images_are_reported` |
| B-M6: a protocol-relative image was dropped silently | reported `outside`; same test |
| C-M1: the public-themes CI step ran after a setup failure | the admin step has `id: admin-e2e`; the step runs only when its outcome is `success` or `failure` |
| C-M2: the spec's `afterAll` put `blog` back for nothing | removed, header comment updated (06 §8 did not mention it) |
| C-M4: business menus without `role="list"` | added on `menu` and `menu__sub`; `business.rs` asserts both (fixture menu gains a child) |
| C-M5: the docs "Pages" disclosure was not linked to its nav | `aria-controls="nav"` on the summary; `docs.rs`; 06 §8 |
| C-M6: every heading anchor was "Link to this section" | `Link to section: <heading text>`; `docs.rs` and a new E2E check `docs_heading_anchors_have_distinct_names`; 06 §8 |

**Re-review residuals** (fixed on the same branch, tests first):

- B-I1 dates: a front matter `date` outside the years 1 to 9999 (UTC) is `bad-front-matter` at its line and the file is skipped; before, `-005000-01-01` failed the whole run on Postgres (exit 1). Tests `a_date_outside_years_1_to_9999_is_refused` (`front.rs`) and `a_date_out_of_range_skips_the_file` (both backends); 09 §8 now says no single file's *front matter* value can fail the run.
- B-M5 links: a link to an image the import made a media item points at its public file URL, and is not reported; a link to a missing file of any kind is `broken-link`; other non-page links are reported after the media pass (`Rw::href`, `linked_media`); test `unimported_links_and_protocol_relative_images_are_reported`; 08 §8, 09 §8, the guide.
- A-I1 follow-on: `comment-<uuid>`, `reply-<uuid>` and `reply-<uuid>-<field>` are reserved by pattern (`ferrite_domain::html::is_reserved_fragment_id`): the sanitiser drops them from headings and core's anchors move a heading off them with a suffix; tests `comment_anchor_ids_are_reserved`, `comment_anchor_ids_are_never_given`, `hostile_heading_ids_are_dropped`; 05 §7, 06 §4.

**Deferred** (§7 and ROADMAP "Deferred from M4-3b"): A-M3 the tree's full-row read, A-M4 tree
excerpts bypass `content.excerpt`, A-M5 card `image()` calls spend the bridge budget, A-M6 the
`page_tree()` memo past 16 pairs, A-M7 `menu_order_cmp` allocations, B-M2 the ~5 GB worst case,
B-M3 asset folders count toward the folder limit, B-M4 a WXR sharing a Markdown source, C-M3
the stale-binary poll message, C-M7 `build.sh` and IPv6 loopback URLs, C-M8 the inline styles a
future public CSP must allow.

**Gates (macOS, Postgres 17 at port 54329):** `cargo fmt --all --check`, `cargo clippy
--workspace --all-targets -D warnings`, `cargo doc --workspace --no-deps` with `-D warnings`:
clean. `cargo test -p ferrite-plugin-host -p ferrite-core -p ferrite-domain -p ferrite-theme
-p ferrite-import-md -p ferrite-import-wp`: 60 binaries, 1407 passed, 0 failed, no `SKIP`;
`-p ferrite --test import_md --test docs_site --test serve`: 71 passed; `-p ferrite-http --test
plugins`: 255 passed. `pnpm -C admin exec playwright test --project public-themes-chromium`
(after `pnpm -C admin build` and `cargo build --release -p ferrite`): 18 passed. `pnpm -C admin
typecheck` and `lint`: clean. `actionlint` and `shellcheck site/build.sh`: clean. `site/build.sh`
into a fresh folder with the release binary: `created 26`, `front page: index.md`, no problem
lines. Not re-run: the full workspace tests, the other admin E2E projects, Linux.
