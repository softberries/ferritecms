# M2b-2 — Media & Content Modelling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The admin gains a media library (grid and list, search and type filters, drag-and-drop uploads with progress and `Idempotency-Key`, alt text, variants), media in the editor (featured image, inline images, a picker), custom fields generated from the type's schema, SEO fields with a search preview, a content-type screen with a field builder, taxonomies with term trees, a drag-and-drop menu editor, the comment moderation queue and a redirects screen with CSV import and export. The public site pages long comment threads with a real total. The M2b-1 follow-ups listed in the ROADMAP are closed.

**Architecture:** Five Rust tasks close the API gaps found by mapping each screen to `crates/ferrite-http/openapi.json` (SEO meta on content and a body-root check; media search and type filters; comment item references and counts; public comment pages; redirect search and theme menu locations). Then nine admin tasks build the screens on the M2b-1 foundation (API client, kit, shell, shortcuts, editor), each with its E2E spec against the release binary, its contact sheet and axe. Drag and drop is `@dnd-kit` (classic packages) behind two kit components, `SortableList` and `SortableTree`, each with a keyboard and button alternative. A last task writes the docs and the exit.

**Tech Stack:** as M2b-1 (pins in `admin/package.json`; M2b-1 plan "Frontend pins"), plus `@dnd-kit/core` 6.3.1, `@dnd-kit/sortable` 10.0.0, `@dnd-kit/utilities` 3.2.2. Rust side unchanged (axum 0.8.9, utoipa 6.0.0, sqlx 0.9).

**Spec:** `docs/ROADMAP.md` "M2b-2" (every item and the M2b-1 follow-ups); `docs/design/07-admin-api.md` §1 (resources, Media, Comments, Redirects, Autosave) and §3 (screen rows: Media library, Content types, Taxonomies, Menus, Comments, Redirects, Editor "Planned (M2b-2)"); `docs/design/11-admin-ui.md` (design language, §6 keyboard map, §7 kit, §8 review checklist); `docs/design/03-media.md` §3–§5 (upload types, variants, URLs); `docs/design/06-themes-rendering.md` §4 (`comments()` helper, comment form) and §5 (head); `docs/design/02-storage.md` §4, §7 (`content_meta`, migrations); `docs/superpowers/m2b1-exit.md` (findings and follow-ups); rulings `docs/superpowers/rulings/m2b.md` (P1–P26, R1–R20 stay binding). The controller appends this plan's rulings P27–P40 to `m2b.md` under "M2b-2 plan rulings" at the start, and execution rulings there as R21 onwards.

## Global Constraints

- **Everything in the M2b-1 plan's Global Constraints still holds** (CLAUDE.md binding; M2a conventions such as `ApiPath<T>`, `check_limit`, `invalid_cursor()`, `CoreError::invalid_field`, the `unsafe_operations_need_auth_and_csrf` sweep with no `UNAUTHENTICATED` additions, `test-seams`; no new problem slug; Rust gates on both backends with and without `admin/dist`; admin gates `lint`, `typecheck`, `test`, `api:check`, `build`, `e2e`; the design review gate with `snap()` and the contact sheet against 11 §8; the accessibility gate `expectAccessible` in light and dark; the CSP gate; `t()` for every string; E2E data hygiene with `uid()`).
- **Capabilities stay in core.** Nav items, buttons and screens show from `me.capabilities` or a DTO's `allowed`, and the server checks again. Media items carry `allowed` like content does only if the DTO has it; it does not (P30), so media actions show by capability (`upload_media` for upload; edit/trash/delete from `author_id == me.id` or `edit_others`) and a `403` is shown as an error toast, never assumed away.
- **Untrusted text is text.** Comment bodies, author names and URLs, media titles and original names, redirect targets and CSV error lines are rendered as text nodes (no `dangerouslySetInnerHTML`). A comment's author URL is a link only for `http:`/`https:` with `rel="noopener noreferrer nofollow ugc"` and `target="_blank"`. SVG media are previewed only through `<img src>`, never inlined.
- **Last-write-wins resources** (types, taxonomies, terms, menus, redirects; 07 §1 P11) are saved through `staleGuard` (Task 6): before a write the screen re-reads the resource and, when it differs from the copy the form started from, asks "Changed by someone else" with **Reload** and **Overwrite**. The window between the check and the write is accepted (P31).
- **Migration:** `0008_autosave_seo` (Task 1) is the only migration, same name on both backends, `-- ferrite:phase expand`, with a `.down.sql`; the hard-coded schema version in CLI and migration tests moves from 7 to 8.
- **New frontend pins** (newest stable published on or before 2026-10-12, at least two weeks before 2026-10-26; `npm view <pkg> time`, checked 2026-10-26):

  | Package | Pin | Published | Licence | Context7 |
  |---|---|---|---|---|
  | @dnd-kit/core | 6.3.1 | 2024-12-05 | MIT | `/clauderic/dnd-kit`: the migration table names the classic `SortableContext`, `arrayMove`, `verticalListSortingStrategy` (classic API confirmed); `KeyboardSensor` keys `Space`/`Enter` start, `Escape` cancels |
  | @dnd-kit/sortable | 10.0.0 | 2024-12-04 | MIT | as above; peers `@dnd-kit/core ^6.3.0` |
  | @dnd-kit/utilities | 3.2.2 | 2023-11-06 | MIT | — |

  Not chosen: `@dnd-kit/react` 0.5.x (0.x; 0.5.1 is 2026-10-15, too new) (P29). Uploads use `XMLHttpRequest` for progress (no dependency; `fetch` has no upload progress in browsers).
- **Exact values:** media grid and lists page by 50; at most 3 uploads run at once from one browser tab (the server's 8 per node is shared); the media picker shows only images when opened for the featured image or an inline image, and every kind for a `media` field (the schema does not say which kinds a field takes); comment pages hold 50 top-level threads by default (`comments.per_page`, 1..=100); the comment page query parameter is `cpage`; redirect search is 1..=200 characters; SEO title at most 200 characters and description at most 500 (soft counters at 60 and 160, labelled "Often cut off in search results after about N characters", no claim about any search engine's rule); the local backup (Task 14) is written at most once a second; new shortcuts `g m` media, `g c` comments, and in the comments queue `a` approve, `u` unapprove, `s` spam, `#` trash.

## Review Focus

1. **An upload is never stored twice and never lost silently.** A retry of a failed or interrupted upload reuses that file's `Idempotency-Key`, so the server answers the first item; `413`, `415`, `422` and `503 busy` are shown on that file's row with the server's detail and a Retry where it makes sense; leaving the screen or closing the tab while uploads run asks first. *Tests: Task 7 `upload-queue.test.ts` `retry_reuses_idempotency_key`, `busy_waits_retry_after`; E2E `upload_retry_after_network_drop_creates_one_item` (the route is aborted once with `page.route`, the retry succeeds, `GET /media?q=<name>` lists one item), `rejected_type_shows_on_its_row`, `leaving_during_upload_asks`.*
2. **Changing a type's schema never leaves items unsaveable without saying why.** Core refuses unknown field keys on every write (`fields.rs`), so after a field is removed or renamed, or a required field added, an item's next save would fail. The field builder warns before saving such a change (names the removed or renamed fields and says existing values stay stored until each item is saved); the editor's fields section lists stored values that the schema no longer has ("Not in the type any more: <name> — removed when you save") and drops exactly those keys from the save, and a required empty field blocks the save with the field's own message. *Tests: Task 8 `fields-form.test.tsx` `stale_keys_are_listed_and_dropped`, `required_blocks_save`; Task 9 E2E `removing_a_field_warns_and_items_still_save`.*
3. **Untrusted text never becomes markup.** A comment body `<img src=x onerror=alert(1)>`, an author URL `javascript:alert(1)`, a media file named `"><svg onload=alert(1)>.png` and a CSV import line with HTML show as literal text; the CSP guard sees no violation and no dialog opens. *Tests: Task 12 E2E `hostile_comment_renders_as_text`; Task 7 E2E `hostile_file_name_renders_as_text`; Task 13 E2E `import_errors_are_text`.*
4. **Last-write-wins screens do not clobber someone else's change unnoticed.** Two pages open the same menu (or type, or term); A saves; B's save shows "Changed by someone else" with Reload and Overwrite, and only Overwrite writes. *Tests: Task 6 `stale-guard.test.ts`; Task 11 E2E `menu_stale_guard`; Task 9 E2E `type_stale_guard`.*
5. **Long comment threads stay reachable.** With more threads than a page holds, every approved comment is on some page, `count` is the real total, the 303 after a submit lands on the page that holds the new comment, and the page cache keeps pages apart. *Tests: Task 4 `thread_pages_cover_every_comment`, `count_is_the_real_total`, `form_redirect_lands_on_the_comments_page`, `cpage_is_part_of_the_cache_key`.*

---

### Task 1: Content API — SEO fields, the body root check, SEO in the autosave slot and the head

**Files:**
- Create: `crates/ferrite-core/src/content/seo.rs`, `crates/ferrite-core/tests/content_seo.rs`, `crates/ferrite-http/tests/api/content_seo.rs`, `crates/ferrite-store-{sqlite,postgres}/migrations/0008_autosave_seo.{up,down}.sql`
- Modify: `crates/ferrite-domain/src/content.rs` (`Autosave.seo`), both backends' `src/repos/autosave.rs`, `crates/ferrite-store-tests/src/{autosave.rs,migrations.rs}`, `crates/ferrite-core/src/{content.rs,content/admin.rs,blocks/*}` (root check), `crates/ferrite-http/src/api/{content.rs,dto…,openapi.rs}`, `crates/ferrite-http/src/head.rs`, `crates/ferrite-http/tests/api/main.rs`, `crates/ferrite-http/openapi.json`, CLI and migration tests holding schema version 7, `docs/design/07-admin-api.md` §1 (Content row, Autosave), `docs/design/06-themes-rendering.md` §5 (head), `docs/design/02-storage.md` §4, §7

**Interfaces — Produces:**
- `ferrite_core::content::seo::{Seo, SeoPatch, SEO_TITLE_META = "seo.title", SEO_DESCRIPTION_META = "seo.description", MAX_SEO_TITLE = 200, MAX_SEO_DESCRIPTION = 500}`. `Seo { title: Option<String>, description: Option<String> }` read from `content_meta` (a stored value that is not a JSON string reads as `None`). `SeoPatch { title: Option<Option<String>>, description: Option<Option<String>> }`: absent keeps, `null` or a string blank after trimming deletes the meta row, a string sets it (trimmed; no control characters, NUL refused; over the limit `422` with `errors[0].field` = `seo.title` / `seo.description`).
- `ContentDraft` / the update input gain `seo: Option<SeoPatch>`; create and update write the meta rows in the same `Tx` as the item. A write that changes only `seo` is a normal update (version + 1, a revision, events, invalidation), so `If-Match` keeps protecting it. Revisions do not carry meta (P13 unchanged).
- DTOs: `ContentDto.seo: SeoDto { title: string | null, description: string | null }` (added member); `ContentInput.seo?`, `ContentPatch.seo?: SeoPatchDto { title?: string | null, description?: string | null }`. List entries do not carry `seo`.
- **Body root check:** every content create, update, autosave `PUT` and revision restore refuses a `body` that is not an object with `"type": "doc"` (`422`, `errors[0].field` = `body`, "the body must be a document (type doc)"). A stored body is never rewritten. Built-in seed content already complies (the test checks `seed demo`).
- Migration `0008_autosave_seo` adds `autosaves.seo` (Postgres `jsonb NULL`, SQLite `TEXT NULL CHECK (seo IS NULL OR json_valid(seo))`); down drops it (`-- ferrite:lossy-down`). Domain `Autosave.seo: Option<Value>`; `AutosaveInput`/`AutosaveDto` gain `seo?: SeoPatchDto | null` (stored as given after the same text checks; `null` or absent = "unchanged", like `slug`, F11).
- Head (`head.rs`): when `seo.title` is set it is the whole `<title>` and `og:title` (no site suffix); the description order stays `seo.description`, excerpt, tagline. One extra meta read per item page, recorded under the existing `post:<id>` dependency.

**Tests:** core (both backends) `seo_roundtrip_and_delete` (set both, read back on `get`; `null` and `"  "` delete the rows); `seo_limits_and_controls` (201 characters → field `seo.title`; a NUL → refused); `seo_only_patch_bumps_version` (and writes one revision); `body_root_must_be_doc` (create with `{"type":"paragraph"}`, `[]`, `"x"` → field `body`; autosave and revision restore likewise; `seed demo` bodies pass). Contract (both backends) `migration_0008_roundtrip` (lossy listed), `autosave_keeps_seo`. API `content_dto_has_seo`, `patch_seo_with_if_match` (no `If-Match` → `428` as for any PATCH). Head (`crates/ferrite-http` unit or app test) `seo_title_replaces_title_and_og_title`, `seo_description_wins_over_excerpt`. The sweep passes; the snapshot is regenerated.

- [ ] **Step 1:** Write the tests above and register them.
- [ ] **Step 2:** Run `cargo test -p ferrite-core --test content_seo` and `cargo test -p ferrite-store-tests --test contract migration_0008 autosave` and expect compile failures.
- [ ] **Step 3:** Implement the module, migration, DTOs, the root check and the head change; regenerate the snapshot; update 07, 06, 02.
- [ ] **Step 4:** Run `cargo test -p ferrite-core`, `cargo test -p ferrite-store-tests`, `cargo test -p ferrite-http` with `FERRITE_TEST_PG_URL` set and expect PASS; `cargo test -p ferrite` passes with schema version 8.
- [ ] **Step 5:** Commit `feat(api): SEO title and description on content, body root check, SEO in the autosave slot (0008)`.

### Task 2: Media API — search, type filters, trash view and restore

**Files:**
- Create: `crates/ferrite-core/tests/media_filters.rs`, `crates/ferrite-http/tests/api/media_filters.rs`
- Modify: `crates/ferrite-domain/src/content.rs` (`MediaKind`, `ContentQuery.media_kind`), both backends' content list and count queries, `crates/ferrite-store-tests/src/content.rs`, `crates/ferrite-core/src/media.rs` (`MediaQuery`, `restore`), `crates/ferrite-http/src/api/{media.rs,openapi.rs}`, `crates/ferrite-http/openapi.json`, `docs/design/07-admin-api.md` §1 (Media row and **Media**)

**Interfaces — Produces:**
- Domain `MediaKind { Image, Video, Audio, Document }` (serde snake case). `ContentQuery.media_kind: Option<MediaKind>`: `Image` = the item's `media.mime` starts with `image/`, `Video` `video/`, `Audio` `audio/`, `Document` = none of the three. Set together with a `kind` other than `media` → `Invalid`. Both stores filter with a sub-select on `media` (`id IN (SELECT item_id FROM media WHERE …)`), in `list` and `count`, keeping cursors exact.
- `MediaQuery` gains `q: Option<String>` and `kind: Option<MediaKind>`. With `q` (trimmed, blank = none) the ids come from the search index, as content search does (relevance order; `q` with `author` → `422`, field `q`, as F19); `kind` combines with either.
- `GET /media?q&kind&author&trash&limit&cursor`; an unknown `kind` → `400` like other bad query values. `trash=true` lists only trashed items (same `readable_by` rule; with `q` the search runs over trashed items too).
- `MediaService::restore(actor, id, expected_version) -> MediaItem` through the content lifecycle's `restore` (the same R8 role rules as content, with `media:write` in place of `content:write`; restores to `published`, the status uploads have, P5 of M2a-2b) and `POST /media/{id}/restore` (`If-Match`, `200 MediaDto`; a non-trashed item `422` as the content route answers). The `/_media` URLs answer again after a restore.

**Tests:** contract (both backends) `media_kind_filters_list_and_count` (one image, one SVG (`image/svg+xml` counts as image), one PDF, one MP4, one MP3 → each kind lists exactly its items; `Document` lists the PDF; a non-media `kind` with `media_kind` → `Invalid`). Core `media_search_by_title_and_original_name` (what the search index holds for media: assert on the title; if the original name is not indexed, the test pins that and 07 says so); `media_q_with_author_is_invalid`; `readable_by_still_applies` (an author does not find another author's private upload); `media_trash_list_and_restore` (trash → listed only with `trash=true`; restore → back in the normal list, served again; a contributor's restore → `Forbidden`, R8). API `media_filters_query_params`, `media_restore_needs_if_match`; the sweep covers the new route. Snapshot regenerated.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement the query member in both stores, the service and the handler; update 07.
- [ ] **Step 4:** Run the store, core and API tests on both backends and expect PASS.
- [ ] **Step 5:** Commit `feat(api): media search, type filters, trash view and restore`.

### Task 3: Moderation, redirects and menus — comment item references and counts, redirect search, theme menu locations

**Files:**
- Create: `crates/ferrite-http/tests/api/{comments_admin.rs,redirect_search.rs,menu_locations.rs}`, `crates/ferrite-store-tests/src/comment_counts.rs`
- Modify: `crates/ferrite-store/src/repos.rs` (`CommentRepo::counts`, `RedirectRepo::list` query), both backends' comment and redirect repos, `crates/ferrite-store-tests/{src/lib.rs,src/redirects.rs,tests/contract.rs}`, `crates/ferrite-core/src/{comments.rs,redirects.rs,menu.rs}`, `crates/ferrite-http/src/api/{comments.rs,redirects.rs,menus.rs,openapi.rs}`, `crates/ferrite-http/openapi.json`, `docs/design/07-admin-api.md` §1 (Comments, Redirects, Menus rows)

**Interfaces — Produces:**
- `CommentRepo::counts(&self, item: Option<ContentId>) -> Result<BTreeMap<CommentStatus, u64>, StoreError>` (every status present, zero-filled). `CommentService::counts(actor, item) -> CommentCounts { pending, approved, spam, trash }` (`moderate_comments` + `comments:read`). `GET /comments/counts?item` → `CommentCountsDto`.
- `CommentDto.item: CommentItemDto { id, type, title, permalink: string | null, status }` (added member; `permalink` only while the item is public now). One `content().get_many` per page in core. The member is required; the comments FK makes a missing item impossible, so core treats one as an internal error rather than inventing a shape for it.
- `RedirectQuery { q: Option<String>, limit, cursor }`; `RedirectRepo::list(&RedirectQuery)` replaces `list(limit, cursor)`. `q` (trimmed, 1..=200 characters, else `422` field `q`) matches `source` or `target` as a case-insensitive substring with `%`, `_` and the escape character escaped (`LIKE … ESCAPE '\'` on both backends; Postgres `ILIKE`). Order and cursor unchanged. `GET /redirects?q`.
- `MenuList` gains `locations: MenuLocationDto[]` = `{ name, label, menu: boolean }` from the `[menus]` table of the theme the answering node serves (`menu` = a menu with that name exists). Empty when the theme declares none. Needs `menus:read` like the list.

**Tests:** contract (both backends) `comment_counts_per_status_and_item`; `redirect_search_escapes_wildcards` (`q = "50%_off"` matches only the literal row; case-insensitive match on target). API `comment_list_has_item_ref` (title, type; permalink null for a draft item's held comment); `comment_counts_endpoint` (+ `403` for an author); `redirect_q_param` (and `q` of 201 characters → 422); `menu_list_has_theme_locations` (the embedded `blog` theme → `primary` and `footer`; after `PUT /menus/primary`, `menu: true`). Snapshot regenerated.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement; update 07.
- [ ] **Step 4:** Run the store, core and API tests on both backends and expect PASS.
- [ ] **Step 5:** Commit `feat(api): comment item references and counts, redirect search, theme menu locations`.

### Task 4: Public comment pages and the real total

**Files:**
- Create: `crates/ferrite-store-tests/src/comment_threads.rs`, `crates/ferrite-http/tests/app/comment_pages.rs`
- Modify: `crates/ferrite-store/src/{repos.rs,paging.rs}`, both backends' comment repos, `crates/ferrite-store-tests/{src/lib.rs,tests/contract.rs}`, `crates/ferrite-core/src/{comments.rs,site.rs,settings.rs}`, `crates/ferrite-theme/src/…` (the `comments()` helper's value), `crates/ferrite-http/src/app/{pipeline.rs,comment_form.rs}`, `crates/ferrite-cache/src/…` (query allowlist), `themes/blog/templates/partials/comments.html`, `docs/design/06-themes-rendering.md` §4 and §6, `docs/design/07-admin-api.md` §1 (Comments), `docs/ROADMAP.md` (tick the item in Task 15)

**Interfaces — Produces:**
- Thread model (P34): a **root** is an approved comment whose `parent_id` is null or whose parent is not approved; roots are ordered `(created_at, id)` ascending; a page holds `per_page` roots, each followed by its approved descendants depth first, oldest first (the existing thread order). Page 1 holds the oldest roots.
- `CommentRepo::approved_page(&self, item: ContentId, offset: u32, roots: u32) -> Result<Vec<Comment>, StoreError>` (roots `offset..offset+roots` and their approved descendants, through a recursive CTE on both backends; at most `MAX_ITEM_COMMENTS` rows, the cut falls after a whole root where possible), `CommentRepo::approved_totals(&self, item) -> Result<ApprovedTotals { comments: u64, roots: u64 }, StoreError>`, `CommentRepo::root_position(&self, item, comment: CommentId) -> Result<Option<u64>, StoreError>` (the 0-based index of the root whose thread holds `comment`; `None` if not approved). `approved_for_item` is removed once unused (no dead code).
- Setting `comments.per_page` (integer 1..=100, default 50, in the allowlist; `null` until set, like the other comment settings, F21).
- `SiteReader::comments(id, page: u32)` → `CommentThread { open, items, total /* approved comments */, page, pages, per_page }`; a `page` past the last is clamped to the last (pages ≥ 1). The helper value gains `total`, `page`, `pages`, `prev_url`, `next_url` (the item's permalink with `?cpage=N`, page 1 without a query); `count` becomes the real total (06 §4 updated: no longer capped at 500).
- The pipeline reads `cpage` (a positive integer, anything else = 1) from single-item requests and passes it to the helper through the render context; `cpage` joins `s` and `page` in the page cache's query allowlist, so each comment page is its own cache entry.
- `comment_form.rs`: an approved comment's `303` goes to `<permalink>?cpage=<root_position / per_page + 1>#comment-<id>` (no query on page 1); held and discarded comments keep `#comment-pending`.
- `themes/blog/templates/partials/comments.html`: the count heading uses `total`; "Older comments" / "Newer comments" links (`prev_url`/`next_url`) with `rel="prev"`/`"next"` when `pages > 1`.

**Tests:** contract (both backends) `thread_pages_cover_every_comment` (120 roots, some with replies 3 deep, a reply under a spam parent → as a root; pages of 50 → 3 pages, the union equals all approved comments exactly once, order within a page = thread order); `root_position_of_reply_is_its_root`; `approved_totals_counts_all`. App (both profiles) `count_is_the_real_total` (600 approved → the page shows 600, not 500); `form_redirect_lands_on_the_comments_page` (with `comments.per_page = 2` and 5 existing roots, a new approved comment → `Location` ends `?cpage=3#comment-<id>`, and that page's HTML contains `id="comment-<id>"`); `cpage_is_part_of_the_cache_key` (page 1 and page 2 bodies differ after both are cached; `?cpage=abc` = page 1); `cpage_past_the_end_shows_the_last_page`.

- [ ] **Step 1:** Write the tests.
- [ ] **Step 2:** Run them and expect FAIL.
- [ ] **Step 3:** Implement the store queries, the service, the setting, the helper, the pipeline, the form redirect and the theme partial; update 06 and 07.
- [ ] **Step 4:** Run `cargo test --workspace` on both backends and expect PASS.
- [ ] **Step 5:** Commit `feat(site): page public comment threads (cpage) with the real total`.

### Task 5: Admin kit for M2b-2 — sortable list and tree, file drop zone, nav sections

**Files:**
- Create: `admin/src/components/kit/{sortable-list.tsx,sortable-tree.tsx,tree-model.ts,drop-zone.tsx,media-thumb.tsx}`, `admin/src/components/kit/{tree-model.test.ts,sortable.test.tsx}`, `admin/src/i18n/en/kit.json`
- Modify: `admin/package.json` + lockfile (the three `@dnd-kit` pins), `admin/src/shell/nav.ts` (+ `nav.test.ts`), `admin/src/routes/_app/-styleguide/patterns.tsx` (the new components in the styleguide), `admin/e2e/styleguide.spec.ts`, `docs/design/11-admin-ui.md` §6 (`g m`, `g c`, the queue keys) and §7 (the new kit rows)

**Interfaces — Produces:**
- `tree-model.ts` (pure): `type TreeNode<T> = { id: string; parentId: string | null; data: T }`; `flatten<T>(nodes: TreeNode<T>[]): FlatNode<T>[]` (`FlatNode = TreeNode & { depth: number; index: number /* among siblings */ }`, depth-first in sibling order); `moveNode(nodes, id, to: { parentId: string | null; index: number }): TreeNode[]` (refuses a move under its own subtree by returning the input unchanged); `indent(nodes, id)` (becomes the last child of its previous sibling), `outdent(nodes, id)` (becomes the next sibling of its parent); `projectDrop(flat, activeId, overId, offsetX, indentPx = 24, maxDepth)` → `{ parentId, index, depth }` (the dnd-kit sortable-tree projection: depth from the horizontal offset, clamped between the next row's depth and the previous row's depth + 1, and to `maxDepth`).
- `SortableList<T>({ items, getId, render, onReorder, label })`: `DndContext` with `PointerSensor` (activation distance 4 px) and `KeyboardSensor` (`sortableKeyboardCoordinates`), `verticalListSortingStrategy`, a drag handle button per row (`aria-label` "Reorder <label>"), Move up / Move down in the row menu, announcements through `DndContext`'s `accessibility.announcements` using `t()` and the item's label (never the raw id).
- `SortableTree<T>({ nodes, render, onChange, maxDepth, label, canNest? })`: the same sensors over `flatten(nodes)`, the projection above while dragging (an indent guide shows the projected depth), and per-row Move up / Move down / Indent / Outdent buttons in a menu (the keyboard and touch alternative to dragging; E2E drives these). Motion: rows use the layout spring from 11 §5; reduced motion → none.
- `DropZone({ onFiles, accept?, children })`: whole-area `dragenter`/`dragover`/`drop` handling with a visible overlay ("Drop files to upload"), plus a "Choose files" button wired to a hidden `<input type="file" multiple>`; ignores drags that carry no files.
- `MediaThumb({ media: MediaDto, size: "sm" | "md" | "lg" })`: the smallest variant at least the box size, else the original for images (SVG via `<img>`); a type icon (lucide) with the extension for other kinds; `alt` = the media alt, or `""` with the title as visible text when no alt.
- `nav.ts` sections (only when the capability is held): **Media** (`read_admin`; contributors see it read-only), **Comments** (`moderate_comments`, with a pending-count badge from `GET /comments/counts`), **Structure**: Content types and Taxonomies (`manage_settings`; Taxonomies also for `manage_terms`, which manages terms only), Menus (`manage_menus`), Redirects (`manage_redirects`). Items for screens not built yet are absent until their task registers the route (each later task adds its own nav entry and test row).

**Tests:** `tree_model_test` (flatten order; `moveNode` into own subtree refused; indent/outdent at the edges are no-ops; `projectDrop` clamps to `maxDepth` and to the neighbours); `sortable_test` (keyboard reorder with Space, ArrowDown, Space calls `onReorder` once with the new order; the Move down menu item does the same; the announcement text uses the label). `nav_test` gains the role rows. Styleguide E2E: `styleguide_screens` covers the new pattern section.

- [ ] **Step 1:** Add the pins (`pnpm -C admin add -E @dnd-kit/core@6.3.1 @dnd-kit/sortable@10.0.0 @dnd-kit/utilities@3.2.2`); write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement the components and nav sections; update 11.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): sortable list and tree, drop zone, media thumbnail, structure nav`.

### Task 6: Admin API modules and the stale guard

**Files:**
- Create: `admin/src/api/{comments.ts,menus.ts,redirects.ts,terms.ts,upload.ts}`, `admin/src/lib/stale-guard.ts`, `admin/src/components/kit/stale-dialog.tsx`, `admin/src/lib/stale-guard.test.ts`, `admin/src/api/upload.test.ts`
- Modify: `admin/src/api/{schema.d.ts (regenerated),keys.ts,media.ts,types.ts,taxonomies.ts,content.ts}`

**Interfaces — Consumes:** Tasks 1–4 endpoints.

**Interfaces — Produces:**
- `qk` gains `media: { list: (filters), item: (id) }`, `comments: { list: (filters), counts: (item?) }`, `menus: { list, item: (name) }`, `redirects: { list: (q) }`, `terms: (taxonomy)` (existing), `type: (name)`.
- Typed wrappers: `listMedia({ q?, kind?, author?, trash?, cursor? })`, `getMedia`, `patchMedia(id, version, { title?, alt? })`, `trashMedia(id, version)`, `restoreMedia(id, version)` (`POST /media/{id}/restore`, Task 2), `deleteMedia(id, version)`; `listComments`, `commentCounts`, `moderateComment(id, status)`, `deleteComment`, `replyComment(type, itemId, { parent_id, body })`; `listMenus`, `getMenu`, `putMenu`, `deleteMenu`; `listRedirects`, `upsertRedirect`, `patchRedirect`, `deleteRedirect`, `importRedirects(csv: string)`, `exportRedirectsUrl = "/api/v1/redirects/export"`; `listTerms`, `createTerm`, `patchTerm`, `deleteTerm`; `getType`, `createType`, `patchType`, `deleteType`; `createTaxonomy`, `patchTaxonomy`, `deleteTaxonomy`.
- `upload.ts`: `uploadMedia(file: File, opts: { key: string; title?: string; alt?: string; onProgress(loaded: number, total: number): void; signal: AbortSignal }): Promise<MediaDto>` over `XMLHttpRequest` (`POST /api/v1/media`, `withCredentials`, `X-CSRF-Token`, `Idempotency-Key: key`, multipart with `title`, `alt` then `file` last); a problem response rejects with `ApiError` (same parsing as `problem.ts`, `Retry-After` kept); a network error or abort rejects with `UploadNetworkError` / `AbortError`; a `401` goes through the same global handler as the client.
- `stale-guard.ts`: `staleGuard<T>(opts: { reread: () => Promise<T>; base: T; equal?: (a: T, b: T) => boolean /* default: stable JSON equality */; ask: (theirs: T) => Promise<"reload" | "overwrite" | "cancel"> }): Promise<{ proceed: true } | { proceed: false; reloaded?: T }>`. `stale-dialog.tsx` is the `ask` UI (`AlertDialog`, "Changed by someone else", the resource name, **Reload** / **Overwrite** / Cancel; never closes on outside click).

**Tests:** `upload_test` (fake XHR: headers incl. `Idempotency-Key` and CSRF; progress callbacks; `415` problem → `ApiError` with slug; abort → `AbortError`; `503` keeps `retryAfter`); `stale_guard_test` (equal → proceeds without asking; differs → asks; reload returns the fresh copy; key order does not matter to the default equality). `pnpm api:check` passes after regeneration.

- [ ] **Step 1:** Regenerate `schema.d.ts` (`pnpm -C admin api:gen`); write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates.
- [ ] **Step 4:** Commit `feat(admin): API modules for media, comments, menus, redirects, terms; XHR uploads; stale guard`.

### Task 7: Media library

**Files:**
- Create: `admin/src/routes/_app/media.index.tsx`, `admin/src/media/{library.tsx,media-grid.tsx,media-table.tsx,media-search.ts,media-detail.tsx,upload-queue.ts,upload-tray.tsx,variants-list.tsx}`, `admin/src/media/{media-search.test.ts,upload-queue.test.ts,media-detail.test.tsx}`, `admin/src/i18n/en/media.json`, `admin/e2e/media.spec.ts`, `admin/e2e/support/files.ts` (fixture files built in memory: a PNG of a given size, an SVG, a PDF header, a `.exe`-like binary)
- Modify: `admin/src/shell/nav.ts` (Media, `g m`), `admin/src/palette/palette.tsx` (Go to Media; Upload)

**Interfaces — Consumes:** Task 5 `DropZone`, `MediaThumb`; Task 6 `listMedia`, `uploadMedia`, `patchMedia`, `trashMedia`, `deleteMedia`; M2b-1 `DataTable`, `PageHeader`, `EmptyState`, `RelativeTime`, `useLeaveGuard`.

**Interfaces — Produces:**
- Route `/admin/media` with `MediaSearch { view?: "grid" | "list" /* default grid, remembered per viewer */; q?: string; kind?: MediaKind; mine?: boolean; trash?: boolean; id?: string /* open detail */ }` (Zod, defaults omitted from the URL; `q` disables `mine` as in the content list).
- `upload-queue.ts`: `createUploadQueue(opts: { upload: typeof uploadMedia; concurrency: 3; onDone(m: MediaDto): void }): UploadQueue` with `add(files: File[])`, `retry(id)`, `cancel(id)`, `clearDone()`, `subscribe(fn)`, `active(): boolean`. Each entry `{ id, file, key /* crypto.randomUUID(), fixed for the entry's life */, state: "queued" | "uploading" | "done" | "failed" | "cancelled", loaded, total, error?: ApiError | Error, media?: MediaDto }`. A `503 busy` or network failure retries automatically once after `Retry-After` (default 2 s) with the same key; other failures wait for **Retry**. Review Focus 1.
- `upload-tray.tsx`: a bottom-right panel listing the queue (name as text, a progress bar with percent, the error detail, Retry/Cancel), collapsible, `aria-live="polite"` for completions. `useLeaveGuard(queue.active())` plus `beforeunload` while active.
- `library.tsx`: page header (Media, count of loaded items, **Upload** primary), filter bar (search `/`, kind chips All/Images/Video/Audio/Documents, Mine), grid (`media-grid.tsx`: responsive square tiles with `MediaThumb md`, title and type below, selection checkbox on hover/focus, `j`/`k`/`x`/`enter` like the `DataTable`) or list (`media-table.tsx`: thumb, title, type, size (`Intl.NumberFormat` in kB/MB), dimensions, author, uploaded). Infinite paging by `next_cursor`. The whole page is a `DropZone`. New uploads appear at the top with a layout animation.
- `media-detail.tsx`: a right sheet for `?id=` with a large preview (image, or the icon with a "Download" link for other kinds; SVG noted "sanitised on upload" when `sanitized`), editable **Title** and **Alt text** (saved on blur and `mod+s` with `If-Match`; `412` → reload the item and show the server's value with a toast), facts (type, size, dimensions, original name as text, uploaded by and when, URL with copy), **Variants** (`variants-list.tsx`: name, size, a link; "Variants are being made" while a raster image has none, polling `GET /media/{id}` every 3 s up to 60 s), and **Move to trash** with Undo / **Delete permanently** in Trash only (confirm). Actions show by capability as Global Constraints say.
- Trash view: a "Trash" toggle (`MediaSearch.trash`) lists trashed items (`trash=true`) with Restore and Delete permanently (confirm); the trash toast's Undo restores.
- Bulk: select several → Move to trash (fan-out 4, per-item `If-Match`, Undo restores all) with the M2b-1 `runBulk`.

**Tests:** `media_search_test`; `upload_queue_test` (concurrency 3; `retry_reuses_idempotency_key`; `busy_waits_retry_after` with fake timers; cancel aborts; `active()`); `media_detail_test` (alt save sends `If-Match`; `412` path). E2E `media.spec.ts` (author): `upload_by_drop_shows_progress_and_variants` (a 1200×800 PNG dropped through `DataTransfer` → the row reaches done, the tile appears, the detail lists variants after the job); `upload_retry_after_network_drop_creates_one_item` (Review Focus 1); `rejected_type_shows_on_its_row` (the binary → `415` detail on the row); `leaving_during_upload_asks` (a slowed route); `edit_alt_text`; `search_and_kind_filters`; `trash_with_undo`; `trash_view_restore_and_delete`; `hostile_file_name_renders_as_text` (Review Focus 3); `contributor_sees_no_upload`; `screens` (grid, list, empty, detail of an image and of a PDF, tray uploading/failed, mobile).

- [ ] **Step 1:** Write the tests and fixtures; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): media library with drag-and-drop uploads, filters, details and variants`.

### Task 8: Media in the editor, custom fields and SEO

**Files:**
- Create: `admin/src/media/{media-picker.tsx,picker-state.ts}`, `admin/src/editor/panel/{featured-image.tsx,seo-section.tsx,search-preview.tsx}`, `admin/src/editor/fields/{fields-section.tsx,field-input.tsx,repeater-field.tsx,relation-field.tsx,media-field.tsx,rich-text-field.tsx,date-field.tsx,fields-model.ts}`, `admin/src/editor/fields/{fields-model.test.ts,fields-form.test.tsx}`, `admin/src/editor/insert-image.ts`, `admin/e2e/editor-media.spec.ts`, `admin/e2e/editor-fields.spec.ts`
- Modify: `admin/src/editor/{schema.ts (image insert command; `ferrite:media/<id>`),slash-menu.tsx ("Image"),image-node.tsx (alt editing popover),editor-state.ts,conflict.ts,autosave.ts}`, `admin/src/editor/panel/side-panel.tsx`, `admin/src/editor/revisions/revision-model.ts` (fields and SEO in the comparison; the null-field follow-up), `admin/src/i18n/en/editor.json`

**Interfaces — Consumes:** Task 1 `seo`; Task 2 filters; Task 6 media wrappers, `uploadMedia`; Task 7 `createUploadQueue`, `MediaThumb`, grid; M2b-1 editor draft, autosave, conflict and revisions modules.

**Interfaces — Produces:**
- `media-picker.tsx`: `openMediaPicker(opts: { kind?: MediaKind; title: string }): Promise<MediaDto | null>` (a dialog: search, the grid of Task 7 with single selection, an Upload tab using the same queue with a drop area; selecting a just-uploaded item works before its variants exist; `enter` picks, `esc` cancels).
- **Featured image** (`featured-image.tsx`, panel section for every type): empty → "Set featured image" (picker, kind image); set → the thumb, Replace and Remove. Stored as `fields.featured_image` (UUID or `null`, F17).
- **Inline images:** the slash menu's "Image" and a toolbar button open the picker (kind image) and insert `image { src: "ferrite:media/<id>", alt: <media alt>, title: null }`; the node view's popover edits `alt` (empty allowed, marked "decorative"). A drop of image files onto the editor uploads through the queue and inserts each at the drop position when done. External `https:` images already in a body stay as they are.
- `fields-model.ts`: `fieldsFor(type: ContentTypeDto): FieldDef[] | null` (null = no schema → no section, the raw `fields` object is kept untouched except `featured_image`); `emptyValue(def)`; `staleKeys(defs, value): string[]` (top-level keys not in the schema, `featured_image` excepted); `toSave(defs, value): Value` (drops exactly `staleKeys`, keeps `null` for cleared optional fields); `validateFields(defs, value): Record<string /* path like fields.price */, string>` mirroring core's rules for immediate feedback (required, number, select options, RFC 3339 dates, UUIDs, repeater rows ≤ 200, text ≤ 65 536); the server's `422` `errors[].field` paths map onto the same inputs.
- Field inputs (`field-input.tsx`): `text` (an auto-growing textarea, since the schema does not say single- or multi-line), `rich_text` a compact TipTap with `editorExtensions` (same schema, P19; the value is the `doc` JSON), `number` (`inputmode="decimal"`, stored as a JSON number), `date` (calendar + 24 h time as in the schedule picker, stored RFC 3339 UTC), `bool` (switch), `select` (shadcn `Select` over `options`; an empty choice for optional), `relation` (a combobox searching `GET /content/{type}?q=` across the types the nav shows, showing type and status; stored UUID; an unreadable or missing target shows "Unknown item <short id>" and keeps the value), `media` (picker, any kind; thumb + Replace/Remove), `repeater` (rows as cards with the sub-fields, `SortableList` reorder, Add row / Remove row, at most 200, nesting as the schema allows (depth ≤ 2)). Every input has a label, the field name in mono as a hint, and its error.
- `fields-section.tsx`: a "Fields" section in the side panel (on wide screens a "Fields" card under the body when the type has more than 4 fields — decided by count), the stale-keys notice (Review Focus 2), and the values joining the editor's one draft (they autosave, enter the 412 conflict list as "fields", and PATCH only when changed).
- `seo-section.tsx`: SEO title and description inputs with counters (soft limits 60/160 in the hint; hard 200/500), a "Search result preview" (`search-preview.tsx`: the title or the item title, the permalink, the description or the excerpt, cut visually with an ellipsis like a result snippet) and "Reset to defaults". Values join the draft as `seo` (PATCH and autosave `seo`; conflict field "seo").
- Revisions (`revision-model.ts`): `fields` changes are listed per field (name: before → after, as text; `null` and absent are different, closing the M2b-1 Task 17 M5 residual); SEO is not in revisions (P13) and the drawer says so in its footer note.

**Tests:** `fields_model_test` (`staleKeys`, `toSave`, every kind's validation, repeater depth); `fields_form_test` (`stale_keys_are_listed_and_dropped`, `required_blocks_save`, server `422` path → the right input shows the message). E2E `editor-media.spec.ts`: `set_featured_image_from_picker`; `insert_inline_image_and_render` (insert → save → publish → the public page's `<img>` points at a `/_media/` variant URL with the alt); `drop_image_into_editor_uploads_and_inserts`; `edit_image_alt`. E2E `editor-fields.spec.ts` (a type created over the API with every kind): `fill_every_field_kind_and_reload`; `repeater_reorder_by_keyboard`; `relation_picks_a_post`; `seo_fields_reach_the_head` (published page `<title>` and meta description); `field_conflict_shows_in_412_dialog`; `screens` (fields section with every kind, picker dialog, featured image set, SEO preview, stale-keys notice).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement the picker, featured image, inline images, fields and SEO.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): media picker, featured and inline images, custom fields and SEO in the editor`.

### Task 9: Content types and the field builder

**Files:**
- Create: `admin/src/routes/_app/structure.types.index.tsx`, `admin/src/routes/_app/structure.types.$name.tsx`, `admin/src/structure/types/{types-list.tsx,type-form.tsx,field-builder.tsx,field-editor.tsx,builder-model.ts,schema-change.ts}`, `admin/src/structure/types/{builder-model.test.ts,schema-change.test.ts}`, `admin/src/i18n/en/structure.json`, `admin/e2e/types.spec.ts`
- Modify: `admin/src/shell/nav.ts` (Content types), `admin/src/palette/palette.tsx`

**Interfaces — Consumes:** Task 5 `SortableList`; Task 6 type wrappers, `staleGuard`.

**Interfaces — Produces:**
- List: every type (built-ins marked and read-only for name/hierarchy; the `media` type is not listed), with label, name (mono), hierarchical, archive, field count, and a "New type" primary (dialog: label, name derived as `[a-z][a-z0-9_]{0,…}` and editable until created, hierarchical, has archive).
- Type screen: label, hierarchical, archive (the server decides what a built-in allows; refused changes show the field error), and the **field builder**: a `SortableList` of fields, each row showing label, name (mono), kind badge and required; selecting opens `field-editor.tsx` (label, name, kind (a kind change on a saved field is allowed with a warning "existing values may no longer be valid"), required, options for `select` (a `SortableList` of strings, 1..=100, no duplicates), sub-fields for `repeater` (the same builder nested, depth ≤ 2)). Save sends the whole `fields` (`{ "fields": [...] }`) through `PATCH /types/{name}` after `staleGuard`.
- `builder-model.ts`: `toSchema(rows)`, `fromSchema(value)`, `nameFromLabel(label)` (lowercase ASCII, other characters to `_`, collapse repeats, strip to start with a letter, ≤ 64), `checkBuilder(rows): Record<string, string>` mirroring `fields.rs` (`MAX_FIELDS`, names, duplicates, `featured_image` reserved, options, repeater rules).
- `schema-change.ts`: `describeChange(before: FieldDef[], after: FieldDef[]): { removed: string[]; renamed: [string, string][] /* same position and kind, new name */; newlyRequired: string[]; kindChanged: string[] }`; a non-empty result opens a confirm listing them with the Review Focus 2 explanation before the save.
- Delete type: only when the API allows (built-ins and types with content are refused by core; the button stays and shows the server's reason) with a confirm (a type is not undoable).

**Tests:** `builder_model_test` (round trip of a schema with every kind and a nested repeater; `nameFromLabel` table; `checkBuilder` mirrors core's error cases); `schema_change_test`. E2E `types.spec.ts` (admin): `create_type_with_fields_then_write_an_item` (the new type appears in the nav and its editor shows the fields); `reorder_fields_by_keyboard`; `removing_a_field_warns_and_items_still_save` (Review Focus 2); `type_stale_guard` (Review Focus 4); `delete_type_with_content_is_refused_with_reason`; `editor_role_has_no_types_screen`; `screens` (list, type with builder, field editor for select and repeater, schema-change confirm).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): content types screen with the field builder`.

### Task 10: Taxonomies and term trees

**Files:**
- Create: `admin/src/routes/_app/structure.taxonomies.index.tsx`, `admin/src/routes/_app/structure.taxonomies.$name.tsx`, `admin/src/structure/taxonomies/{taxonomy-list.tsx,taxonomy-form.tsx,term-tree.tsx,term-table.tsx,term-form.tsx,terms-model.ts}`, `admin/src/structure/taxonomies/terms-model.test.ts`, `admin/e2e/taxonomies.spec.ts`
- Modify: `admin/src/shell/nav.ts` (Taxonomies), `admin/src/i18n/en/structure.json`

**Interfaces — Consumes:** Task 5 `SortableTree`, `tree-model`; Task 6 term and taxonomy wrappers, `staleGuard`.

**Interfaces — Produces:**
- Taxonomy list: label, name, hierarchical, content types, term count (from `GET /taxonomies/{t}/terms` length, loaded per row lazily). Creating and editing a taxonomy (label, name, hierarchical, content types as checkboxes over the types) needs `manage_settings`; with only `manage_terms` the list links to the terms and hides those controls.
- Hierarchical taxonomy → `term-tree.tsx`: a `SortableTree` with `canNest` and no sibling order (terms have no order column; siblings sort by name, P35), so a drop only changes the parent (`PATCH /terms/{id}` `{ parent_id }`, optimistic, rolled back with the server's message on `422`, e.g. a cycle). Row menu: Edit, Add child, Move to… (a parent combobox, the keyboard path), Delete. Flat taxonomy → `term-table.tsx` (`DataTable`: name, slug, description, client-side filter; the API returns all terms).
- `term-form.tsx` (sheet): name, slug (derived when empty, the server suffixes `-2`), description, parent (hierarchical). Writes go through `staleGuard` on that term.
- Delete: a confirm naming how many child terms move up (the server's behaviour; the implementer reads `crates/ferrite-core/src/taxonomy.rs` and words the confirm from it) — terms are not undoable (P21 exception, recorded in P35).
- `terms-model.ts`: `toTree(terms: TermDto[]): TreeNode<TermDto>[]` (siblings by name with `Intl.Collator`; a term whose parent is missing becomes a root), `descendants(id)`.

**Tests:** `terms_model_test`. E2E `taxonomies.spec.ts`: `editor_builds_a_category_tree` (create three, nest by Move to…, then by keyboard drag; reload shows the tree); `cycle_is_refused_and_rolled_back` (drag a parent under its child via the API-backed move); `flat_tags_table_filter`; `admin_creates_taxonomy_for_a_custom_type` (the editor's terms section then shows it); `screens` (list, tree, tags table, term sheet, mobile).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): taxonomies and term trees`.

### Task 11: Menus editor

**Files:**
- Create: `admin/src/routes/_app/structure.menus.index.tsx`, `admin/src/routes/_app/structure.menus.$name.tsx`, `admin/src/structure/menus/{menus-list.tsx,menu-editor.tsx,add-items.tsx,menu-item-form.tsx,menu-model.ts}`, `admin/src/structure/menus/menu-model.test.ts`, `admin/e2e/menus.spec.ts`
- Modify: `admin/src/shell/nav.ts` (Menus), `admin/src/i18n/en/structure.json`

**Interfaces — Consumes:** Task 3 `locations`; Task 5 `SortableTree`; Task 6 menu wrappers, `staleGuard`; M2b-1 `useLeaveGuard`.

**Interfaces — Produces:**
- List: the theme's **locations** first (label, name, "Create menu" when `menu` is false, else a link), then other menus. New menu: name (`[a-z0-9_-]`, whatever core accepts; field errors from the server) and label.
- Editor: a two-column layout (one column on mobile). Left **Add items**: tabs Content (search across the nav's types, multi-select, Add), Terms (taxonomy select + term search), Custom link (URL + label; `http(s)`, `mailto:` and site-relative paths as the server allows). Right: the menu's `SortableTree` (`maxDepth` = the core limit, read from `crates/ferrite-core/src/menu.rs` and written into `menu-model.ts` with a comment naming it); each row shows its label (or the target's title in muted text when the label is empty), kind and target, with Edit (label, URL for links), Remove (with Undo within the unsaved draft) and the move buttons. A dirty state, **Save** (`mod+s`; `PUT` the whole tree after `staleGuard`, Review Focus 4), Discard, the leave guard, and Delete menu (Undo re-`PUT`s the snapshot).
- `menu-model.ts`: `fromDto(menu): TreeNode<MenuEntry>[]`, `toInput(nodes, label): MenuInput` (`position` = sibling index, `parent_id` from the tree, new entries get `crypto.randomUUID()` ids), `checkMenu(nodes)` (≤ 500 entries, depth).

**Tests:** `menu_model_test` (round trip; positions; new ids; limits). E2E `menus.spec.ts` (editor): `build_primary_menu_and_see_it_on_the_site` (create `primary` from the location, add a page, a category, a custom link, nest one by keyboard, save; the public home page's nav shows them in order); `drag_reorder_with_pointer` (one pointer drag test with `page.mouse`); `remove_with_undo_before_save`; `leave_with_unsaved_changes_asks`; `menu_stale_guard` (Review Focus 4); `screens` (list with locations, editor empty and filled, add-items tabs, mobile).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): drag-and-drop menu editor with theme locations`.

### Task 12: Comment moderation queue

**Files:**
- Create: `admin/src/routes/_app/comments.index.tsx`, `admin/src/comments/{queue.tsx,comment-row.tsx,comment-search.ts,moderate.ts,reply-box.tsx}`, `admin/src/comments/{comment-search.test.ts,moderate.test.ts,comment-row.test.tsx}`, `admin/src/i18n/en/comments.json`, `admin/e2e/comments.spec.ts`
- Modify: `admin/src/shell/nav.ts` (Comments with the pending badge, `g c`), `admin/src/dashboard/needs-attention.tsx` (pending comments for `moderate_comments`, linking to the queue)

**Interfaces — Consumes:** Task 3 `item`, counts; Task 6 comment wrappers; M2b-1 `DataTable`, `runBulk`, shortcuts.

**Interfaces — Produces:**
- Route `/admin/comments` with `CommentSearch { status?: "pending" | "approved" | "spam" | "trash" /* default pending */; item?: string }`; tabs with counts from `GET /comments/counts` (with `item` when filtered; a chip shows the item's title and clears the filter).
- Rows (`comment-row.tsx`): author name, e-mail (mono, `mailto:` link), URL (Global Constraints rule), a short `ip_hash` (first 8 hex, mono, full in `title`), the body as plain text with line breaks preserved (`white-space: pre-wrap`, clamped to 6 lines with "Show more"), "on <item title>" linking to the editor and, when `permalink` is set, "View" to the public page, `RelativeTime`, and a "Reply to" line when `parent_id` is set.
- Actions by status: pending → Approve, Spam, Trash; approved → Unapprove (to pending), Reply, Spam, Trash; spam → Not spam (to pending), Delete permanently; trash → Restore (to pending), Delete permanently. Status changes are optimistic with an Undo toast that re-applies the previous status (P21); Delete permanently confirms (it also removes replies, the server's rule, so the confirm says so).
- `moderate.ts`: `nextStatuses(status)`, `runModeration(ids, status)` over `runBulk` (fan-out 4; failures listed). Keys in the queue: `j`/`k`/`x`/`enter` (open the item), `a` approve, `u` unapprove, `s` spam, `#` trash, acting on the selection or the focused row (registered with the shortcuts module so `?` lists them).
- `reply-box.tsx`: an inline textarea under an approved comment; sends `POST /content/{type}/{item}/comments` `{ parent_id, body }` with the session (CSRF; the moderator's reply is approved, 07 §1); a pending comment offers **Approve and reply** (approve first, then reply).

**Tests:** `comment_search_test`; `moderate_test` (`nextStatuses` table; bulk failure collection); `comment_row_test` (`javascript:` URL → no link; body text not parsed as HTML). E2E `comments.spec.ts` (editor; comments made through the public endpoint as a guest with distinct `X-Forwarded-For` like the auth spec): `approve_from_queue_shows_on_site`; `spam_and_undo`; `bulk_trash_with_partial_failure` (one comment deleted over the API meanwhile); `reply_as_moderator`; `keyboard_moderation`; `hostile_comment_renders_as_text` (Review Focus 3); `author_has_no_queue` (nav item absent, `/admin/comments` shows the forbidden state); `screens` (each tab, empty pending, row expanded, reply box, mobile).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): comment moderation queue with undo, bulk actions, keys and replies`.

### Task 13: Redirects

**Files:**
- Create: `admin/src/routes/_app/structure.redirects.tsx`, `admin/src/structure/redirects/{redirects-table.tsx,redirect-form.tsx,import-dialog.tsx,csv-preview.ts}`, `admin/src/structure/redirects/csv-preview.test.ts`, `admin/e2e/redirects.spec.ts`
- Modify: `admin/src/shell/nav.ts` (Redirects), `admin/src/i18n/en/structure.json`

**Interfaces — Consumes:** Task 3 `q`; Task 6 redirect wrappers.

**Interfaces — Produces:**
- Table (`DataTable`, infinite paging): source and target (mono, truncated with `title`), code, hits (tabular), last hit (`RelativeTime`, "never"), origin badge (manual, import, slug change). Search `q` (debounced 200 ms, `/`). Row actions Edit, Delete (Undo re-creates it with `POST` and its code; hits restart at 0, said in the toast).
- `redirect-form.tsx` (dialog): source (a path starting with `/`), target (a path or an absolute `http(s)` URL), code (the codes core accepts, read from `crates/ferrite-core/src/redirects.rs` into the select); server field errors shown inline (reserved prefixes, toggled-slash duplicates, R21/R26). Create uses `POST`, which overwrites a row for the same source; the form says so when the source exists (looked up with `q` = the source before saving).
- `import-dialog.tsx`: choose or drop a `.csv` (≤ 1 MiB checked client-side, else the server's `413` anyway); `csv-preview.ts` `previewCsv(text): { rows: number; header: boolean; sample: string[][] /* first 5 */ }` (a small RFC 4180 parser: quotes, doubled quotes, CRLF) shows what will be sent; **Import** posts `text/csv`; a `422` lists each problem as text (line number and message, at most 100, the server's list); success shows the count and refreshes the table. **Export** is a link to `/api/v1/redirects/export` with `download="redirects.csv"` (same-origin GET with the session cookie).

**Tests:** `csv_preview_test` (header detection, quoted commas, CRLF, empty file). E2E `redirects.spec.ts` (admin): `create_edit_delete_with_undo`; `search_narrows`; `import_and_export_roundtrip` (import 3 rows, export contains them); `import_errors_are_text` (Review Focus 3: a row with `<b>x</b>` and a reserved source); `redirect_works_on_the_site` (the source answers with the code and `Location`); `editor_has_no_redirects`; `screens` (table, empty, form with errors, import preview, import errors).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E; review the contact sheet.
- [ ] **Step 4:** Commit `feat(admin): redirects screen with search, CSV import and export, hits`.

### Task 14: Editor follow-ups from M2b-1

**Files:**
- Create: `admin/src/editor/local-backup.ts`, `admin/src/editor/local-backup.test.ts`, `admin/e2e/editor-followups.spec.ts`
- Modify: `admin/src/editor/{editor-state.ts,restore-banner.tsx,handoff.ts,autosave.ts}`, `admin/src/editor/panel/publish-dialog.tsx`, `admin/src/routes/_app/content.$type.{new,$id}.tsx`, `admin/src/auth/session.ts` (logout clears backups), `docs/design/07-admin-api.md` §3 (Editor row)

**Interfaces — Produces:**
- `local-backup.ts` (closes R13): `writeBackup(key: { item: string; user: string }, draft: Draft): void` (at most once a second, `localStorage` key `ferrite:backup:<item>:<user>`, JSON with `saved_at`, inside try/catch; a quota error is ignored and logged with `console.warn`, not `error`), `readBackup(key): { draft: Draft; saved_at: string } | null`, `clearBackup(key)`, `clearAllBackups(user)`. It is written only while a kept copy (Take theirs) occupies the slot and the working draft has unsaved edits; it is cleared on a successful save, on Discard and on logout. On load, a backup that differs from both the item and the slot is offered in the restore banner as a second choice ("Unsaved edits from this browser, <time>": Restore / Discard).
- Publish dialog on a new item (the M2b-1 exit finding): `mod+shift+p` on `/new` while the first save is moving the item to `/$id` is carried across the navigation in the editor handoff (`handoff.ts` gains `openPublishDialog: boolean`), so the dialog opens on the new route instead of closing.
- Note: the M2b-1 follow-up "core validates the body root" is Task 1; "a revision that differs only by a null-valued custom field" is Task 8.

**Tests:** `local_backup_test` (throttle; storage throwing; clear on logout). E2E `editor-followups.spec.ts`: `kept_copy_edits_survive_a_crash` (two editors make a 412, Take theirs, type new text, close the page without saving (`page.close({ runBeforeUnload: false })`), reopen → the banner offers the browser backup and restores the text); `publish_dialog_survives_first_save` (type a title on `/new`, press `mod+shift+p` immediately → the dialog is open on `/$id`).

- [ ] **Step 1:** Write the tests; run them and expect FAIL.
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Run the admin gates and the E2E (the editor specs of M2b-1 included).
- [ ] **Step 4:** Commit `fix(admin): local backup while a kept copy holds the slot; publish dialog survives the first save`.

### Task 15: Docs, ROADMAP and the M2b-2 exit check

**Files:**
- Create: `docs/superpowers/m2b2-exit.md`
- Modify: `docs/ROADMAP.md` (M2b-2: link this plan, tick each item; known limits), `CLAUDE.md` (status: M2b-2 done, next M2b-3), `docs/design/07-admin-api.md` §3 (each screen row "Built (M2b-2)" with an as-built summary), `docs/design/11-admin-ui.md` (§6, §7 as built), `docs/design/10-testing.md` (new specs)

**Exit check** (each line recorded with the command or steps and what was seen; anything not verified under "Not verified"):
1. The workspace tests on both backends, with and without `admin/dist`.
2. The admin gates and `pnpm -C admin e2e` on SQLite, and the smoke project on Postgres.
3. Automated walk-through on a release build (Playwright driving Google Chrome against a fresh `init` + `seed demo` + `serve`, as R19): upload images and a PDF by drag and drop, set alt text, see variants; set a featured image and an inline image and publish; define a type with every field kind, write an item of it, change the schema and save the item again; build a category tree; build the primary menu and see it on the site; moderate comments from the queue and see the public page; page a thread past 50 threads; import and export redirects and follow one.
4. The contact sheet reviewed against 11 §8 in full, findings fixed or recorded as rulings.
5. Bundle sizes compared with the M2b-1 baseline (the entry chunk must not grow by more than 10 %: `@dnd-kit` and the screens live in route chunks); the numbers recorded for M2b-3's budget.
6. Safari: still not verified unless the user checks by hand (recorded).

- [ ] **Step 1:** Run the exit checks and write `m2b2-exit.md`.
- [ ] **Step 2:** Update ROADMAP, CLAUDE.md and the design docs.
- [ ] **Step 3:** Commit `docs: M2b-2 exit, roadmap and status`.

## Plan rulings

- **P27 — M2b-2 is 15 tasks:** five Rust API tasks first (the gaps every screen needs), then the kit, the API modules, eight screens or screen groups, the follow-ups and the exit. Cost: screens wait for the API tasks.
- **P28 — SEO fields are `content_meta` keys `seo.title` and `seo.description`, exposed as a `seo` member on the content DTOs**, not custom fields: the head already reads `seo.description`, SEO plugins write `seo.*` (05 §2), and the WP importer maps Yoast/Rank Math onto the same keys (BRIEF). A write that changes only `seo` is a full update (version, revision, events) so `If-Match` protects it. `seo.title` replaces the whole `<title>`. Cost: a revision per SEO-only save, and revisions do not restore SEO (P13).
- **P29 — `@dnd-kit` classic (core 6.3.1, sortable 10.0.0, utilities 3.2.2), not `@dnd-kit/react` 0.x.** The classic packages are stable and documented; the new API is pre-1.0 and its latest release is too new for the two-week rule. Every drag has a button/keyboard alternative, which is also what E2E drives. Cost: a later migration to the new API.
- **P30 — No `allowed` member on media DTOs in M2b-2.** Media actions show by capability and ownership, and the server's `403` is the authority. Adding `allowed` would duplicate the content rules for a screen where every action is already undoable or confirmed. Cost: a contributor-like edge case may show a button that fails with a clear message.
- **P31 — Last-write-wins resources get a client-side stale check (`staleGuard`), not versions.** Adding version columns to types, terms, menus and redirects is a storage change across four tables for screens edited rarely and by admins/editors. Cost: a race inside the check-then-write window loses one edit, as today.
- **P32 — Uploads use `XMLHttpRequest`** for progress and a client-side queue of 3; each file keeps one `Idempotency-Key` for all its retries. Cost: a second request path next to `openapi-fetch` (shared problem parsing).
- **P33 — Media get a restore route and a trash list (`POST /media/{id}/restore`, `GET /media?trash=true`)** because the content routes answer `404` for type `media` and the library's Undo and Trash need them; restore reuses the content lifecycle and R8. Cost: two more media endpoints.
- **P34 — Comment pages are pages of top-level threads** (roots, oldest first, 50 per page by `comments.per_page`), each with all its approved replies; a reply whose parent is not approved is a root. `?cpage=N` (allowlisted in the cache key); a page past the end shows the last page. Cost: one very large thread can make a page long (bounded by `MAX_ITEM_COMMENTS` rows).
- **P35 — Terms have no manual order** (no column); siblings sort by name and a drag only changes the parent. Term deletes and type deletes confirm instead of offering Undo (re-creating a term would change its id and lose its assignments). Cost: no custom category order until a `term_order` column exists.
- **P36 — Custom fields of a type with no schema are not editable in the admin** (the raw object is kept; only `featured_image` is shown). Editing free-form JSON is a developer task for the API. Cost: types created over the API without a schema show no fields section.
- **P37 — Relation fields search every type the nav shows**, because `FieldDef` has no target type. Cost: a relation can point at an item of an unintended type; a `target_types` member would be a schema change (later).
- **P38 — Menu, type and redirect deletes:** a menu delete offers Undo (re-`PUT` of the snapshot, same name); a redirect delete offers Undo (hits restart at 0); a type delete confirms. Cost: an undone redirect loses its hit count.
- **P39 — The stale-keys rule (Review Focus 2) drops only top-level keys the schema no longer has, and only when the user saves.** The values stay stored until then, so re-adding a removed field before saving recovers them. Cost: a save after a field removal deletes that field's value from the item (shown in the notice beforehand).
- **P40 — The local backup (R13 follow-up) uses `localStorage` per item and user, only while a kept copy occupies the slot**, cleared on save, discard and logout. Cost: unsaved text sits in the browser's storage until then (a shared computer that never logs out keeps it).

## Self-review notes

- Spec coverage (ROADMAP M2b-2): media library → Task 7 (+ API Task 2, kit Task 5, upload Task 6); media in the editor (featured image, inline `ferrite:media/<id>`, picker) → Task 8; custom fields from the schema → Task 8; SEO fields → Tasks 1, 8; content types and field builder → Task 9; taxonomies and term trees → Task 10; menus with `@dnd-kit` → Tasks 5, 11 (+ locations Task 3); comment moderation queue → Task 12 (+ API Task 3); redirects with CSV and hits → Task 13 (+ `q` Task 3); comment pagination → Task 4; M2b-1 follow-ups: body root → Task 1, local backup → Task 14, null-valued field revision → Task 8, publish dialog during first save → Task 14. 07 §3 rows for Media, Content types, Taxonomies, Menus, Comments, Redirects and the Editor's planned part are all covered; Users, Themes, Plugins, System and Settings stay in M2b-3.
- API gaps found by mapping screens to endpoints: SEO meta (Task 1); media search/kind (Task 2); comment item titles and tab counts, redirect search, menu locations (Task 3); comment pages (Task 4); media restore and the trash list (Task 2, P33). Everything else exists in `openapi.json` (types, taxonomies, terms, menus `PUT`, redirects CRUD/import/export, comments list/patch/delete and the public submit used for replies, media CRUD).
- Interfaces defined once: `Seo`, `SeoPatch`, 0008 (T1); `MediaKind`, `ContentQuery.media_kind`, `MediaService::restore` (T2); `CommentRepo::counts`, `CommentItemDto`, `RedirectQuery`, `MenuLocationDto` (T3); `approved_page`, `approved_totals`, `root_position`, `comments.per_page`, `cpage` (T4); `tree-model`, `SortableList`, `SortableTree`, `DropZone`, `MediaThumb` (T5); API wrappers, `uploadMedia`, `staleGuard` (T6); `createUploadQueue`, `MediaSearch` (T7); `openMediaPicker`, `fields-model`, SEO section (T8); `builder-model`, `describeChange` (T9); `toTree` (T10); `menu-model` (T11); `nextStatuses`, `runModeration` (T12); `previewCsv` (T13); `local-backup` (T14).
- Points the implementer must read from code and not guess are named in their tasks: the limits core enforces for menu names and depth, redirect codes and what a term delete does to children (Tasks 10, 11, 13).
