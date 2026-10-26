# M2b-2 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M2b-2; plan Task 15): *a fresh `init` + `seed demo` +
`serve` on a release build: upload images and a PDF by drag and drop, set alt text, see
variants; set a featured image and an inline image and publish; define a type with every
field kind, write an item, change the schema and save the item again; build a category tree;
build the primary menu and see it on the site; moderate comments from the queue and see the
public page; page a thread past 50 threads; import and export redirects and follow one; the
workspace tests on both backends with and without `admin/dist`; the admin gates and E2E
(SQLite, Postgres smoke); the contact sheet against 11 §8; bundle sizes against the M2b-1
baseline; Safari recorded.*

**Result (2026-10-26, branch `m2b2-media` at `52612bb`, plus this docs commit):** met in
Chrome, with one concern: the first full E2E run had one failure, a race between parallel
tests (§2); the second full run passed. Safari was not checked (ruling R19).

| Exit check | Verified by | Result |
|---|---|---|
| 1. Workspace tests, both backends, with and without `admin/dist` | §1: two full runs with `CI=true` and PostgreSQL 17 | yes |
| 2. Admin gates and `pnpm -C admin e2e` (SQLite + Postgres smoke) | §2 | yes on the second run; the first run failed one test (a cross-test race, recorded, not fixed) |
| 3. The flows on a release build | §3: a Playwright script driving Google Chrome against a fresh site | yes in Chrome |
| 4. Contact sheet against 11 §8 | §4: the 74 M2b-2 screens of the first full E2E run | yes, findings recorded (none fixed here; no doc-only defect) |
| 5. Bundle sizes against the M2b-1 baseline | §5 | entry +3.4 % (within the 10 % budget); `@dnd-kit` only in lazy chunks |
| 6. Safari | — | **not verified** |

No code changed in this task. Docs changed: this file, `docs/superpowers/rulings/m2b.md`
(M2b-2 pre-flight findings F-A, F-B, F-D; controller rulings R21–R36; P34 amended by R25),
ROADMAP, CLAUDE.md, README (status line), 01 §1 (admin layout), 07 §3 (every M2b-2 screen row
as built; the Media row's retry wording), 10 §1 (new specs), 11 §6 and §7 (as built).

## 1. Workspace tests

macOS 26.6 (Darwin 25.6.0) arm64, PostgreSQL 17.11 at `localhost:54329` (CI uses
`postgres:18`). `target/` had been cleaned before the task.

```
$ pnpm -C admin build            # dist embedded by the first run
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace
  with admin/dist:    81 test result lines, 2377 passed, 0 failed, 1 ignored; no SKIP
$ mv admin/dist <scratch>/dist-aside && touch crates/ferrite-http/build.rs   # then the same command
  without admin/dist: 81 test result lines, 2377 passed, 0 failed, 1 ignored; no SKIP
  (dist moved back and build.rs touched again afterwards)
$ cargo fmt --all --check                          # ok
$ cargo clippy --all-targets -- -D warnings        # ok
$ cargo clippy -p ferrite -- -D warnings           # ok
```

The ignored test is the `ferrite-sdk` `export_plugin` doc test, as before. Both runs passed
`site::admin_index_on_real_binary`, `site::admin_placeholder_on_real_binary` and
`admin::placeholder_without_dist` (solo, cluster). M2b-1 ended at 2298 passed; M2b-2 adds 79.

## 2. Admin gates and E2E

On `52612bb`, Node 24.18.0, pnpm 10.28.2, Vite 8.3.4, Playwright 1.64.0:

```
$ pnpm -C admin lint            # ok
$ pnpm -C admin typecheck       # ok
$ pnpm -C admin test            # 61 files, 438 passed
$ pnpm -C admin api:check       # src/api/schema.d.ts is current
$ pnpm -C admin build           # check-dist: ok
$ cargo build --release -p ferrite
$ FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test pnpm -C admin e2e
  run 1: 148 passed, 1 failed (5.4 min): 145 + 1 failed chromium-sqlite, 3 chromium-pg
  run 2: 149 passed (5.5 min): 146 chromium-sqlite, 3 chromium-pg (shell.smoke)
```

**The failure (a race between tests, not fixed here).** Run 1 failed
`editor-fields.spec.ts` › `fill_every_field_kind_and_reload` in the guard fixture:

```
console.error: Failed to load resource: the server responded with a status of 404 (Not Found)
(http://127.0.0.1:55996/api/v1/content/t-mvpn3dwb-4c744158?q=Related%20e2e-…&limit=8:0)
```

A relation field searches every type the sidebar shows, one `GET /content/{type}?q=…` per
type (P37), from the types list the page loaded. Another worker's test had deleted its type
`t-mvpn3dwb-…` in the meantime, so that request answered `404`; Chrome logs it as a console
error and the guard fails the test. The field itself copes (each type is its own query; a
failed one only adds no hits). Running `editor-fields.spec.ts` alone three times
(`--repeat-each 3`, SQLite) reproduced it once more (17 passed, `relation_picks_a_post`
failed the same way on a `bk…` type deleted by a parallel repeat). The full run 2 passed.
Fix options for the controller: re-read the types list when a type answers `404`, or let
the specs that search across types allow `404`s from `/api/v1/content/<other type>`.

The screenshots reviewed in §4 are from run 1 (every screen spec passed in it; the failing
test takes no screenshots), copied aside before the reruns.

## 3. The flows (automated)

No human hands in this run (ruling R19). A Playwright script (`flows.ts`, kept in the session
scratchpad, not committed; run with `node`, Playwright's `chromium.launch({ channel:
"chrome" })`) drove **Google Chrome 156.0.8078.48** (headless, 1440×900) against the release
binary (`ferrite 0.1.0`, built from `52612bb` with the `admin/dist` of §1). Each run used a
fresh data folder:

1. `ferrite init --data <dir>/site --url http://127.0.0.1:<port>`, `seed demo` ("12 posts, 3
   pages, 4 terms, 2 menus, 1 image (admin user: admin)"), `user reset-password --login admin
   --password-stdin`, `user create --login editor --role editor --password-stdin`.
2. `serve` with `FERRITE_SERVER__LISTEN=127.0.0.1:<port>`; the admin and the editor signed
   in through the sign-in form in two browser contexts; guests in fresh contexts.

Runs 1–5 failed steps on script errors (a seeded category sorting between the test's terms,
selectors, typing before an option input had focus, an assertion read before the page cache
had caught up, see below); run 6 passed every step:

| Step (as scripted) | Seen |
|---|---|
| Media (admin): drop two PNGs (1600×1000, 1200×900) and a PDF on `/admin/media` through a real `DataTransfer`; Documents chip; open each image, type the alt text, blur; open the first image's detail | tray "3 files uploaded"; the API has three items, the PDF `application/pdf`; the Documents chip lists the PDF; each alt `PATCH` 200 and stored; the sheet lists 4 variants: thumbnail 150 × 150, card 640 × 360, medium 768 × 480, hero 1280 × 640, and shows 1600 × 1000 |
| Category tree (editor) on `/admin/structure/taxonomies/category`: New term ×3 (Fruit, Apples, Gooseberries); Move to… Apples under Fruit; a keyboard drag of Gooseberries one row up; reload | announcement "Gooseberries … is now under Fruit …"; after the reload the tree reads Fruit > Apples, Gooseberries; the API has both children's `parent_id` = Fruit |
| Editor (admin): new post, title and a paragraph, `⌘S`; Set featured image → picker → search → Select; slash menu `/image` → picker → the second image; tick Fruit; `⌘S`; Publish… → Publish now | "Published" toast; API `status: published`, `fields.featured_image` = the first image, the body's image `src` `ferrite:media/<second image id>`; the public page has two `<img>`: the featured image (`alt="A harbour at dusk"`, with a `srcset`, from the theme's hero partial) and the inline one `/_media/<hash>/meadow-….png` with `alt="A green meadow"` (the original file, R28) |
| Primary menu (editor): `/admin/structure/menus` (the Primary navigation location shows the seeded menu, Edit menu), open `primary`, add the post (Content search), Fruit (Terms → Categories), a custom link; row menu Indent on Fruit; `⌘S`; open `/` as a guest | seeded entries Home, About, News, Contact; status "Fruit … is now under Harbour notes …"; `PUT /menus/primary` 200; the home page's Primary nav: top Home, About, News, Contact, Harbour notes …, Elsewhere …; sub-menu Fruit … |
| Content types (admin): New type "Books …" (name derived `books_…`), then Add field nine times: Subtitle (Text), Blurb (Rich text), Price (Number), Release (Date and time), In print (Yes / no), Genre (Choice: fiction, essay), Related (Link to item), Cover (Media), Chapters (Repeater with the sub-field Chapter title); Save changes | the API schema: `subtitle:text, blurb:rich_text, price:number, release:date, in_print:bool, genre:select, related:relation, cover:media, chapters:repeater[chapter_title]` |
| An item of the type: every field filled (rich text typed, 12.5, a calendar day at 10:30, the switch, essay, the post as Related, the PDF as Cover from the picker, one Chapters row), `⌘S` | stored `fields`: `blurb` a doc, `chapters [{chapter_title: "Opening"}]`, `cover` the PDF's id, `genre "essay"`, `in_print true`, `price 12.5`, `related` the post's id, `release "2026-10-15T08:30:00Z"` (10:30 on the machine's UTC+2 clock), `subtitle "First edition"` |
| Schema change: remove Price, Save changes; open the item; change the title, `⌘S` | the confirm: "Save changes to the fields? … existing values stay stored until each item is saved, and are then removed. Removed price"; after it the item still stores `price 12.5`; the editor shows "Not in the type any more: price, removed when you save"; after the save the item has no `price` and keeps the other eight keys |
| Comments: a guest posts through the theme's form on the published post; the editor opens `/admin/comments` and approves it; then 51 more top-level comments as the admin through `POST /api/v1/content/post/{id}/comments` | the guest lands on `…/#comment-pending` with the notice; the sidebar link reads "Comments (1 waiting)"; Approve → "Comment approved"; the public post shows the comment; all 51 answer `201` (a moderator: approved, not rate limited, R34) |
| Paging the thread | the public page said "52 comments" 366 ms after the last insert (the page cache's invalidation); page 1: 50 threads, the guest's among them, a "Newer comments" link to `?cpage=2`; page 2: 2 threads (Thread 50, Thread 51) and a link back to the post's address; `?cpage=9` shows the last page (2 threads) |
| Redirects (admin): Import a 3-row CSV (header, a 301 to the post, a 410, a 302 to an external URL); Export; `GET` the 301 and the 410 without following; follow the 301 in a guest browser; reload the table | preview "3 rows · The header row is skipped"; "Imported 3 redirects"; the export `redirects.csv` starts `source,target,code` and holds the 3 rows; `301` with `Location: /harbour-notes-…/`; `410`; the browser landed on the post ("Harbour notes … – FerriteCMS Demo"); right after the visits the row read 0 hits, "Never"; 6.2 s later 2 hits, "just now" (hits are flushed every 10 s, `HIT_FLUSH_EVERY`, 06 §6) |

Also seen:

- No browser console errors in run 6 (the script recorded every `console.error` and page
  error of both signed-in pages).
- The two waits above are the documented asynchronous parts: the page cache catches up after
  a comment insert (run 1 read the page too early and saw no next link), and redirect hits
  arrive with the next flush.
- `serve` stopped on SIGTERM and logged "shut down".
- The scratch data folders under the session scratchpad were left in place (`rm -rf` is
  blocked here); they hold only the test sites.

## 4. Contact sheet against 11 §8

Reviewed: the 74 M2b-2 screens of E2E run 1 (`admin/test-results/screens/`, 722 PNGs in
all; media 19, editor 8, types 8, taxonomies 11, menus 8, comments 11, redirects 5,
styleguide ordering 3, dashboard-attention 1), each as a 4-up composite (light/dark desktop,
light/dark mobile; first viewport), built with headless Chrome from the PNGs.

| 11 §8 item | Result |
|---|---|
| 4 px grid | no misalignment seen |
| One primary action per view | holds: Upload, New type, New taxonomy, New term, New menu, Save (menu, type), New redirect, Publish… in the editor; the comments queue has no filled button outside an open reply box; dialogs have one filled (or destructive) button |
| Consistent page header | title, count, description, actions on every list; see finding 6 for widths |
| Loading, empty, error, permission-denied states | media has all four (`media-loading`, `-none`, `-empty`, `-error`, `-forbidden`); comments: empty per tab and filtered, forbidden; redirects, taxonomies, menus: empty; types: refused delete. Loading and error states of the structure screens and the queue are not snapped (deferred minor, Tasks 10 and 12) |
| Dark mode at parity | yes on every screen; `no_raw_colours_outside_tokens` passes in the unit suite |
| Mobile without horizontal scroll, ≥ 44 px targets | `snap` asserts no horizontal overflow on every shot (passed); targets measured only by the tasks' tests (`styleguide_touch_targets`), not here |
| Text truncated only with a tooltip or `title` | truncated media titles, redirect sources and targets, menu labels seen; their `title`s were checked in the task reviews, not again here |
| Numbers tabular | counts, hits, codes, sizes and `n / 60` counters look tabular; not measured |
| Focus visible | focused inputs in dialogs show the ring (e.g. `menus-item-dialog`, `taxonomies-settings`) |
| Motion within §5 | not judged from stills |
| `expectAccessible` and the guard on every visit | every screen spec passed in run 1 (axe WCAG 2.2 AA in both schemes, no CSP violation, page error or `console.error`) |

Findings (recorded for the controller; none is a doc defect, so none was fixed here):

1. `media-detail-image-end`, dark mobile: not scrolled to the variants (the other three are).
2. `media-delete-confirm`: the previous step's "1 file restored" toast is still on screen
   behind the dialog.
3. `media-forbidden`: the forbidden state renders under the library's own header with Upload,
   search and chips still live (the shot fakes a `403` for an author, who can upload); a
   viewer really without access never gets there, but the screen is inconsistent with
   `comments-forbidden`, which shows the state alone.
4. A trashed item's detail still shows its URL with Copy, although the URL answers `404`
   (deferred minor, Task 7).
5. `editor-fields-invalid` on phones: the "Not saved: check the highlighted fields" toast
   covers a field (deferred minor, Task 8).
6. Page width: the menus list and the comments queue sit in a narrower centred column on
   desktop, while media, types, taxonomies and redirects use the full content width.
7. The content type screen on phones puts the Settings card before the Fields card, so the
   builder (the screen's main work) starts below the first viewport; the menu editor orders
   its cards for phones (Items second).
8. `menus-editor-empty` on phones: the "… created" toast covers the bottom of the Add items
   card (transient).
9. `comments-row-expanded` on phones: the first viewport does not show the expanded row
   (deferred minor, Task 12).
10. The redirects import dialog keeps "Import 6 redirects" live after a `422` (deferred
    minor, Task 13); `redirects-empty` offers New redirect but not Import (deferred minor).

## 5. Bundle sizes (against the M2b-1 baseline)

`pnpm -C admin build` on `52612bb` (Vite 8.3.4, `assetsInlineLimit: 0`, no source maps).
Raw bytes and `gzip -9` bytes; the M2b-1 column is from `m2b1-exit.md` §5:

| What | M2b-1 raw / gzip -9 | M2b-2 raw / gzip -9 | Change |
|---|---|---|---|
| Entry `index-*.js` | 296 697 / 92 323 | 306 918 / 95 474 | +10 221 (+3.4 %) / +3 151 (+3.4 %) |
| Entry + the modulepreloaded chunks `index.html` names (M2b-1: 15, M2b-2: 17) | 642 779 / 208 013 | 691 212 / 222 694 | +7.5 % / +7.1 % |
| Main stylesheet `index-*.css` | 166 083 / 25 603 | 175 735 / 26 870 | +5.8 % / +4.9 % |
| Editor chunk `editor-*.js` (TipTap; lazy) | 619 303 / 187 232 | 656 406 / 197 276 | +6.0 % / +5.4 % |
| Editor stylesheet | 6 697 / 1 941 | 7 217 / 2 020 | +7.8 % |
| Chart chunk `chart-*.js` (Recharts; lazy) | 361 961 / 104 240 | 361 961 / 104 241 | same raw size |
| Styleguide chunk (lazy) | 127 792 / 34 769 | 97 485 / 27 042 | −23.7 % (parts now in shared chunks) |
| Fonts (20 woff2) | 781 088 | 781 088 | same |
| Whole `admin/dist` | 93 files, 3 344 933 | 172 files, 3 727 396 | +11.4 % |

The entry chunk grew 3.4 %, inside the 10 % budget. `@dnd-kit` is only in
`sortable-list-*.js` (49 341 / 16 347; its "Draggable item …" strings appear in no other
chunk), which no modulepreload names; TipTap stays out of the entry graph. The 17 preloads
are `react`, `zod`, `button`, `circle-alert`, `keys`, `auth`, `link`, `preload-helper`,
`preferences`, `content`, `session`, `next`, `types`, `list-search`, `handoff`, `menus`
(460 bytes) and `taxonomies` (841 bytes); which two are new against M2b-1 was not checked
(the M2b-1 list is not recorded). New lazy route chunks, raw / gzip -9: `media.index` 25 147 / 8 289,
`comments.index` 21 286 / 7 488, `structure.menus.$name` 23 590 / 7 355,
`structure.menus.index` 8 437 / 3 109, `structure.types.$name` 23 249 / 7 213,
`structure.types.index` 6 697 / 2 618, `structure.taxonomies.$name` 19 985 / 6 447,
`structure.taxonomies.index` 4 421 / 1 810, `structure.redirects` 18 205 / 6 053, plus
shared `use-upload-queue` 18 560 / 6 957, `media-picker` 7 490 / 3 048, `sortable-tree`
4 927 / 2 006. These are the numbers for M2b-3's budget.

## Verified (claims in this task's doc changes)

| Claim (file) | How verified | Result |
|---|---|---|
| 07 §3 Media: Retry for network, 429, 503, other 5xx and unrecognised failures; Dismiss for 403/413/415/422 | `media/upload-errors.ts` (`REFUSALS`, `uploadFailure`) | true |
| 07 §3 Media: variants polled every 3 s for up to 60 s | `media/media-detail.tsx` `VARIANTS_POLL_MS`, `VARIANTS_POLL_FOR_MS` | true |
| 07 §3 Menus: 16 levels, 500 entries; no `mailto:` | `structure/menus/menu-model.ts` (`MENU_MAX_DEPTH = 15` zero-based, `MAX_MENU_ITEMS = 500`); `ferrite-http/src/app/bridge.rs` `MAX_MENU_DEPTH = 16` | true |
| 07 §3 Menus: targets named through `GET /content/refs` and `GET /terms/{id}` | `structure/menus/targets.ts` | true |
| 07 §3 Taxonomies: up to 64 levels; delete explained, no delete while built-in or with terms | `term-tree.tsx` `TERM_MAX_DEPTH = 63`; `taxonomy-form.tsx` `deleteBlocked` | true |
| 07 §3 Comments: 50 a page; badge refreshed every 60 s | `comments/comment-search.ts` `COMMENT_PAGE_SIZE`; `routes/_app.tsx` `refetchInterval: 60_000` | true |
| 07 §3 Redirects: 50 a page, Undo 10 s, 1 MiB / 10 000 rows; hits after the 10 s flush | `redirects-table.tsx` `PAGE_SIZE`, `UNDO_TOAST_MS`; `import-dialog.tsx` `MAX_BYTES`, `MAX_ROWS`; `app/hits.rs` `HIT_FLUSH_EVERY`; §3 | true |
| 07 §3 Types: palette entries, limits 100 fields / 64 / 200 | `i18n/en/palette.json` `create.*`; Task 9 report (constants read from `fields.rs`) | true (limits from the report and its review) |
| 11 §6: `/` focuses search in media and redirects; `mod+s` in the type form, menu editor, media detail | `content/filters.tsx` `SearchField` (`keys: "/"`), used by `media/library.tsx` and `redirects-table.tsx`; `useShortcut` `mod+s` in `type-form.tsx`, `menu-editor.tsx`, `media-detail.tsx` | true |
| 11 §7: kit additions (`unordered`, `actions`, `leadingActions`/`trailingActions`, `rowClassName`, `RowAction.overlay`, `openOnClick`, `align`, `data-row-id`, `single`, `labels`, `StaleDialog`, `useStaleAsk`, `tab-styles.ts`, `SelectionBar`) | grep in `components/kit/` and `content/bulk-bar.tsx` | true |
| 11 §7: the badge's sr-only "(n waiting)" | `i18n/en/shell.json` `badge_one`/`badge_other` | true |
| 10 §1: the new spec files and support modules; pointer drags in `menus` and `taxonomies` | `ls admin/e2e admin/e2e/support`; `page.mouse` in both specs | true |
| Rulings R24, R25, R32, R36 constants | `ferrite-core/src/media.rs` `list` (kind on loaded hits); `ferrite-store/src/paging.rs` `MAX_PAGE_ROWS = 2_000`; `bridge.rs`; `ferrite-core/src/types.rs` `API_WORDS = ["refs"]` | true |
| ROADMAP/CLAUDE.md: migration `0008_autosave_seo` | `ls crates/ferrite-store-*/migrations` | true |
| §1 counts, §2 counts, §5 sizes | the command output above | true |

## Not verified (limits)

1. **Safari.** Nothing ran in Safari or WebKit. For the user, by hand, on a release build:
   the flows of §3, especially drag and drop of files onto the library and the editor body,
   the keyboard drags (Space, arrows), and `⌘S` in the type form and menu editor.
2. **The flows ran headless**, scripted, on one machine, SQLite only, in Chrome on macOS.
   The Postgres site is covered by the smoke project only (P22).
3. **The contact sheet** was judged from first-viewport composites of the screenshots;
   motion, touch-target sizes, tabular figures and `title`s were not measured here.
4. **The E2E race** of §2 is recorded, not fixed.
5. **`docker build`** was not run in this task, and the M2b-2 ledger records no local run
   (CI's `docker` job builds the image).
6. **CI** has not run on this commit at the time of writing.

## Open review minors

Deferred in the task reviews (ledger) and not fixed by a later task as far as checked here;
the controller's final review decides which stay open:

- Must go before merging (user rule, dead code): `qk.comments.lists` (`api/keys.ts`) and the
  i18n keys `comments:row.edit`, `comments:list.end_one`/`end_other` are unused (checked).
- Rust: `seo_of` re-reads the item (three item reads per response); no app-level test of the
  head after an SEO-only `PATCH`; `MediaKindParam` duplicates `MediaKind`, `MEDIA_KIND` is
  defined in store and core, `media_kind_sql` per backend; moderation reads the item after
  commit (a `500` after a committed change is possible); blank `q` is `422` on `/redirects`
  but no search on `/media`; `RedirectQuery::default()` has `limit` 0; the redirect `q`
  search scans the table; `CommentItemDto.permalink` docs omit media; `cpage` is in the cache
  key on every route; prev/next comment URLs lack `#comments`; a wrong comment "Inserted
  newest first." in `ferrite-store-tests/src/comment_threads.rs` (checked, still there);
  `peer()` is `pub(crate)` in `tests/app/comments.rs` (checked); comment totals and page are
  read outside one `Tx`; no SQL test for a short page under a tight row budget.
- Admin: `LAYOUT_SPRING` duplicated; `canNest` has two signatures; the tree announces a move
  for a no-op drop; styleguide truncation without `title`; `upload.ts` duplicates the
  problem check; an unparsable `2xx` upload body becomes `ApiError` 200; `useStaleAsk` leaves
  a promise pending on a second ask or unmount; the stale dialog's Cancel/Escape cases are
  untested; `moderateComment`'s status is typed `string`; the library re-registers `onDone`
  per render; "Leave and cancel" wording; `library.tsx` (617 lines) and `queue.tsx` (849
  lines) are long; Undo forces `restore: true` without a comment; the invisible tile check is
  clickable on fine pointers; `insert-image.ts`'s header comment still says the site
  resolves inline images to a variant URL (checked; R28 says the original); `RichEditor`
  reads its value only on mount; the search preview uses `break-all`; the schema-change
  confirm does not warn about removed select options and reads the same for every change
  kind; built-in type delete confirm wording; the schema-change dialog's last chip can clip;
  term dialogs morph while closing; the parent combobox filter matches UUIDs; focus falls to
  `body` after a term delete; menu rows with an empty label repeat the title; custom-link
  hint and error duplicate; the menu Content tab loads forever with no nav types; the queue's
  `j`/`k` are not pressed in E2E; while a local backup is on offer, new edits are neither
  autosaved nor backed up (deliberate); the `/new` publish gate is not pinned by a unit test;
  `restore.undoFailed` is reused for a failed backup restore.
- New here: the E2E race of §2 and the contact-sheet findings 1–3 and 6–8 of §4.
