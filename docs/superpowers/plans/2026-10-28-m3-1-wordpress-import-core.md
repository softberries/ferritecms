# M3-1 — WordPress Import: Core, Conversion and CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ferrite import wordpress export.xml --uploads DIR --data SITE [--fetch-media] [--dry-run] [--report DIR]`
imports a WordPress WXR export into a FerriteCMS site on either backend. It covers:
- authors, terms, custom types and taxonomies (ACF fields included);
- media, posts, pages and custom items, with Gutenberg and classic bodies converted to the
  stored block tree and media URLs and internal links rewritten;
- Yoast and Rank Math SEO, comments and menus;
- 301 redirects from the old URLs;
- a JSON and HTML report.

A re-run updates instead of duplicating. A crashed run can be resumed. A second import that
is still running is fenced out in the data.

**Architecture:** The import is split three ways.
- **Core** (`ferrite-core::import`) gets an import writer: `ImportService` with `ImportBatch`, one
  store transaction per batch of at most 500 items. It sets what the public services cannot:
  - explicit status, including private;
  - dates;
  - author;
  - exact slugs;
  - generic meta;
  - comments with status and date.

  Each batch emits one invalidate event and checks a run token, so a second run is fenced out.
- **Store** gets two tables in migration `0009_import`:
  - `import_runs` (runs and their fence token);
  - `import_map` (WP id → ferrite id, per source site). The map persists across runs, which is
    what makes re-runs idempotent.
- **`ferrite-import-wp`** (new crate) does everything WordPress-specific. It never touches SQL.
  - A streaming quick-xml WXR parser writes an IR into a temp SQLite file.
  - Four passes plus a link-fixup pass call `ImportBatch`.
  - A converter turns Gutenberg blocks and classic HTML into the stored TipTap JSON.
  - It writes the report.

The CLI in `crates/ferrite` wires it to the site. The HTTP media fetcher reuses the
SSRF-checked client from `ferrite-http`.

**Tech Stack:** Rust 2024; quick-xml `=0.41.0` (workspace); sqlx 0.9 sqlite (IR file); html5ever
0.40.1, the version ammonia 4.2.4 already pulls in (no second copy; see Task 11); ammonia
4.2.4; clap 4.6.7. TipTap 3.31.4 in the admin (Task 6 only).

**Spec:**
- `docs/design/08-wp-import.md` (all sections).
- `docs/ROADMAP.md` "M3": every item except those this plan moves to M3-2 (ruling P2).
- `docs/design/02-storage.md` §7–8 (migrations), §on comments (`by_account` on a new insert path, 02:423).
- `docs/design/04-cluster.md` §7 (locks are not fences).
- `docs/design/06-themes-rendering.md` §4 (blocks, `image()`).
- `docs/design/07-admin-api.md` §1 (Import resource; the endpoints are M3-2).
- Rulings: `docs/superpowers/rulings/m15.md` R2, R17 and P9, P13; `m2b.md` P18 (body headings 2–6).

**Research inputs (git-ignored, read before planning a task):**
- `.superpowers/m3-corpus/`. Its contents:
  - `site-a.xml`: Gutenberg, Yoast 27.x, ACF 6.8.10 free, CPT `book` and taxonomy `genre`,
    menus, comments, every status.
  - `site-b.xml`: classic editor, shortcodes, Rank Math 1.0.280.
  - Their `site-*-uploads/` folders.
  - The builder scripts `site-a.php` and `site-b.php`.
  - `themeunittestdata.wordpress.xml` and `64-block-test-data.xml` from
    github.com/WordPress/theme-test-data (GPL-2.0, **not** checked in, ruling P15).

  Sites A and B were exported from a real WordPress 7.1.3 with real plugins on 2026-10-28.
- Recon facts used below were taken from the code on `develop` at d94275c.

## Plan rulings (written to `docs/superpowers/rulings/m3.md` at the start)

- **P1: The import writes through a new core API, not the public services.** `ferrite-core::import`
  holds `ImportService`/`ImportBatch`.
  - Why: every public service method commits its own transaction and emits its own events.
    None can set `published_at`/`created_at`, the author, the `private` status, generic meta, or
    a comment's status and date (recon of 2026-10-28).
  - The batch reuses the crate-private `*_in` helpers. Capability `Import` (admin only, scope
    `import:write`, already defined and unused) is checked once in `ImportService::start`.
  - Cost: a second write path whose invariants must match the services'. Task 3's tests compare
    an imported item with a service-created one.
- **P2: M3 is two sub-plans.**
  - M3-1 (this plan) is the core, the conversion and the CLI.
  - M3-2 is everything else:
    - the `import.wordpress` job and `POST /api/v1/import/wordpress` with `Idempotency-Key`;
    - `GET` on runs, and the report download;
    - the admin import screen and the admin fields for canonical and robots;
    - redirects for `permalink.*` structure changes (M2a-2b P26);
    - the Postgres job-claim `EXPLAIN` (R17);
    - the M3 exit.
  - Cost: the importer is CLI-only until M3-2.
- **P3: Idempotency lives in `import_map`, a persistent table in the site database.** It is keyed
  `(source, kind, wp_id)` and stores `ferrite_id`, `hash` (SHA-256 of the item's IR record) and
  `version` (the item's version after the import wrote it). The temp IR file is only a cache.
  - `source` is the WXR `<wp:base_blog_url>`, normalised: lower-case host, no trailing slash.
  - Item meta `import.wp.id` (a JSON number) is also written, for people and plugins, but it is
    never read back.
  - Why: the design's map lived only in the temp file, so a re-run from a fresh upload would
    duplicate everything.
  - Cost: one more table, and rows that outlive deleted items (a stale row is treated as "not
    mapped").
- **P4: What a re-run does with an item already mapped.**
  - Same `hash`: skip it (`unchanged`).
  - Changed `hash`, and the item's current `version` still equals the mapped `version`: update it
    (`updated`).
  - Changed `hash`, but the item was edited since: keep it (`kept: edited locally`, reported).
  - The mapped item is gone: create it again.
  - Cost: an edit made in FerriteCMS blocks later fixes from WordPress for that item.
- **P5: The fence.**
  - `ImportService::start` inserts an `import_runs` row with a random 128-bit `token` and
    `state = running`. It refuses with `Busy` while another run is `running` with a heartbeat
    newer than 120 s.
  - With `takeover = true`, it marks a stale run `abandoned` first.
  - Every `ImportBatch` starts its transaction with
    `UPDATE import_runs SET heartbeat_at = now WHERE id = $run AND token = $token AND state = 'running'`.
    It aborts with `CoreError::Conflict("import run was taken over")` unless exactly one row
    changed.
  - The row lock this takes on Postgres serialises the batch against a takeover's `UPDATE`.
    SQLite has a single writer anyway.
  - The `import` advisory lock (`LOCK_IMPORT`) is still taken for the whole run, best-effort, for
    a fast "already running" answer (R17 M3: it is not relied on).
  - Cost: one extra `UPDATE` per batch.
- **P6: Batches.**
  - At most 500 items per batch (an item is anything that makes a row the report counts).
  - One `invalidate` event per batch, with the deduplicated keys. If there are more than 1 000
    keys, it is `[all]` instead (R2: Postgres serialises the commits of transactions that ran
    `NOTIFY`).
  - Imported items write one revision each and their search index entry.
  - They enqueue **no** `post.saved`/`post.published` plugin actions, but scheduled items get
    their `content.publish_scheduled` job.
  - Cost: plugins do not see imported content as saved. The report says so.
- **P7: Status mapping.**
  - publish → published; draft → draft; pending → pending; private → private; trash → trash.
  - future → scheduled with its job when the date is still ahead. When the date is past, it
    becomes published (WP missed the schedule) and the report says so.
  - Skipped and counted: auto-draft, inherit outside attachments, and request-* statuses.
  - The dates:
    - `published_at` is `post_date_gmt`. If that is `0000-00-00 00:00:00`, `post_date` is read
      as UTC.
    - `created_at` is the same, and `updated_at` is `post_modified_gmt`.
  - Cost: a WP site without GMT dates shifts by its offset.
- **P8: Slugs are kept exactly where core allows them.**
  - The batch writes the WP `post_name`. If it is taken or reserved, it falls back to core's
    `-N` and reports the change.
  - An empty `post_name` (drafts) derives one from the title.
  - Cost: none beyond the reported renames.
- **P9: Users.**
  - A WXR `<wp:author>` matches an existing user by e-mail, then by login (reported as "matched by
    login"). Otherwise it is created with `password: None`, `must_reset: true` and role `author`,
    because WXR carries no roles (verified on sites A and B).
  - The WP login is kept when it passes `UserService`'s login rules. Otherwise it is slugified,
    with `-N` on a clash.
  - Content whose author is not in the WXR goes to the importing admin.
  - Cost: every imported user is an author until an admin changes roles.
- **P10: SEO gets `canonical` and `robots`.**
  - `SeoPatch` and the item's SEO meta add `seo.canonical` (an absolute http(s) URL, at most 2 048
    characters) and `seo.robots`. `seo.robots` is a sorted, deduplicated subset of `noindex`,
    `nofollow`, `noarchive`, `nosnippet` and `noimageindex`, stored as a JSON array.
  - The public head emits them. The REST DTOs carry them (snapshot regenerated). The admin
    fields come in M3-2.
  - Source keys, verified on 2026-10-28 in the plugin source and in exports A and B:
    - Yoast: `_yoast_wpseo_title`, `_yoast_wpseo_metadesc`, `_yoast_wpseo_canonical`,
      `_yoast_wpseo_meta-robots-noindex` (`1` = noindex, `2` = index, `0` = type default),
      `_yoast_wpseo_meta-robots-nofollow` (`1` = nofollow), `_yoast_wpseo_meta-robots-adv`
      (comma list).
    - Rank Math: `rank_math_title`, `rank_math_description`, `rank_math_canonical_url`,
      `rank_math_robots` (PHP-serialised array).
  - Title variables are substituted:
    - Yoast `%%title%%`, `%%sitename%%`, `%%sep%%`; Rank Math `%title%`, `%sitename%`, `%sep%`.
    - `sep` is `-`, the plugins' default separator.
    - Any other variable is removed and counted in the report.
  - A canonical equal to the item's old `<link>` is dropped.
  - Cost: per-type title templates (WP options) are not in WXR and are lost.
- **P11: Two block-tree additions, and nothing else.**
  - An `embed` node: `attrs.url` plus `attrs.provider`, one of `youtube` or `vimeo`. It renders a
    privacy-mode `<iframe>` (`https://www.youtube-nocookie.com/embed/<id>`,
    `https://player.vimeo.com/video/<id>`).
  - An optional `caption` on `image`, rendered as `<figure>…<figcaption>`.
  - Embeds from other providers become a paragraph with a link.
  - Both get admin TipTap support so imported bodies stay editable.
  - Everything else follows these rules:
    - Columns, groups, covers, media-text and buttons are flattened (buttons become a paragraph
      with a link).
    - A gallery becomes a run of images.
    - Underline, sub, sup, colour and alignment are dropped with the text kept, and counted.
  - Cost: some layout is lost, and the report counts it.
- **P12: Unknown blocks and shortcodes.**
  - An unknown Gutenberg block becomes an `html` node with its inner HTML. A block whose inner
    HTML is empty after trimming, such as dynamic FSE blocks like `wp:latest-posts`, is dropped.
  - An unknown shortcode stays as text in its paragraph.
  - Both are counted with up to 5 example items each.
  - The `html` node is sanitised at render time, as today.
  - Cost: an html node opens as a preview-only block in the editor.
- **P13: Media.**
  - Files come from `--uploads DIR` by `_wp_attached_file`. With `--fetch-media`, a file missing
    from the folder is fetched from `<wp:attachment_url>` through the existing SSRF-checked HTTP
    client: 30 s per file, `media.max_upload` as the cap.
  - A file that is neither in the folder nor fetchable is reported as missing. The items that
    reference it keep the old URL.
  - Files go through the same sniffing and SVG sanitising as uploads, after the refactor in Task 7.
  - Variants are left to the `media.variants` jobs, which the next `serve` runs. The CLI says so.
  - Cost: no srcset until a server has run the jobs.
- **P14: URL rewrite.**
  - Every URL form of an attachment maps to its media item:
    - `attachment_url`;
    - each size file in `_wp_attachment_metadata` (`sizes.*.file`, same directory);
    - the `-scaled` original.
  - The match is on host-insensitive paths under the source's `/wp-content/uploads/`.
  - An `img` src becomes `ferrite:media/<uuid>`; a link `href` becomes the media's public URL.
  - Links to an old item URL (its `<link>`, `?p=N`, `?page_id=N`) become the new item's path in a
    fix-up pass after all content exists.
- **P15: The corpus.**
  - The two synthetic exports A and B, written by us and exported from a scratch WordPress, are
    checked in under `crates/ferrite-import-wp/tests/corpus/`. So are their uploads (PNG and
    JPEG files of flat colour, 1.4 MB in total) and the builder scripts.
  - The two WordPress theme-test files are GPL-2.0. They are not checked in: a test reads them
    from `FERRITE_WP_CORPUS` (a directory) and prints `SKIP` without it.
  - Cost: CI does not run the large files.
- **P16: ACF.**
  - Field definitions come from the export itself: the `acf-field-group` and `acf-field` items.
    In an `acf-field`, `post_name` is the key, `post_excerpt` the name, and `content:encoded` is a
    PHP-serialised config with `type`.
  - For an auto-created custom type, the fields map as follows:
    - text, textarea, email, url, password → `text`;
    - wysiwyg → `rich_text` (converted HTML);
    - number, range → `number`;
    - true_false → `bool`;
    - date_picker (`Ymd`) and date_time_picker (`Y-m-d H:i:s`) → `date` (UTC RFC 3339);
    - select, radio and button_group without `multiple` → `select` with the choices;
    - image and file → `media`;
    - post_object and page_link without `multiple` → `relation`;
    - relationship → `repeater` of one `relation` field `item`;
    - gallery → `repeater` of one `media` field `image`;
    - checkbox → `repeater` of one `text` field `value`;
    - repeater (ACF Pro) → `repeater` with mapped sub-fields.
  - Everything else (group, flexible_content, clone, maps, multiple selects) stays as raw meta
    `wp.<name>` and is reported.
  - ACF names are normalised to `[a-z][a-z0-9_]{0,63}`.
  - On the built-in `post` and `page` types, ACF values stay as `wp.<name>` meta (reported).
  - Cost: ACF on built-ins needs manual modelling.
- **P17: Permalinks.**
  - WXR carries no options (verified: no `permalink_structure`). The old URL of each item is its
    `<link>`.
  - With `--keep-permalinks` (the default), the importer infers a structure: the post structure
    is set when every published post's `<link>` path fits one pattern over FerriteCMS tokens
    (`%year%`, `%month%`, `%slug%`, `%category%`, lower-case literals). It is set before content
    is written, so most URLs stay the same.
  - Redirects are then made for every item whose new path differs from its old one, and for
    attachment pages (to the media URL).
  - Tags also get one: `/tag/<slug>/` → the term URL. So do categories when the term URL differs
    from `/category/<slug>/`. `--category-base` and `--tag-base` override these defaults.
  - Old == new is skipped. Rows `RedirectService` validation would refuse are reported, not
    fatal.
  - `--no-keep-permalinks` leaves the site's structure and only redirects.
- **P18: Comments.**
  - WP comment HTML becomes plain text: tags are stripped, `<p>` and `<br>` become blank lines
    and newlines, and entities are decoded.
  - Status: `1` → approved, `0` → pending, `spam` → spam, `trash` → trash.
  - Pingbacks and trackbacks are skipped and counted.
  - Threading is kept. A parent that was skipped makes the child top-level.
  - A comment with a WP `user_id` that maps to a user sets `user_id` and `by_account` (02:423).
  - Cost: comment formatting is lost.
- **P19: Menus.**
  - `nav_menu_item` items become `Menu` trees, one per `nav_menu` term (name = the term slug).
  - Kinds: `post_type` → item, `taxonomy` → term, `custom` → url.
  - The order is `menu_order`. The parent comes from `_menu_item_menu_item_parent`. `_blank` is
    kept as the target where the domain type has one (else dropped and counted).
  - Theme locations are not in WXR (verified), so none are assigned.
- **P20: Site options.**
  - Channel `<title>` → `site.title`, `<description>` → `site.description`, unless
    `--no-site-options`.
  - Nothing else: `front_page` and `per_page` are not in WXR.
- **P21: The IR.**
  - A temp SQLite file at `<data>/import/<run-id>/ir.sqlite`.
  - Tables: `items(seq, kind, wp_id, record_json)`, `terms`, `authors`, `channel`,
    `passes(name, done_at)`.
  - Pass 1 reads from the IR, never the XML. A resumed run with the same file skips the parse and
    any finished passes. Within a pass, `import_map` makes redoing a batch harmless.
  - The folder is deleted when the run finishes `done`.
- **P22: XML safety.**
  - quick-xml never expands external entities; a DOCTYPE with an internal subset is refused
    (`ParseError::Doctype`).
  - A single text node over 64 MiB is refused.
  - Invalid UTF-8 is refused with the byte offset.
  - The parse is streaming: memory is bounded by the largest single item.

## Global Constraints

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (and the same for the admin's `pnpm lint`, `pnpm typecheck` in Task 6).
- Errors: `thiserror` in libraries (`ferrite-import-wp` included), `anyhow` only in `crates/ferrite`.
- No `#[allow(dead_code)]`; remove unused code instead (user rule).
- Migrations: `0009_import.up.sql` / `.down.sql` on **both** backends, same name; expand-only.
- Storage changes need contract tests in `ferrite-store-tests` on both backends; Postgres via
  `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test`.
- Plugins are not involved; Principle 2 untouched. No plugin actions fire for imported items (P6).
- No new dependency beyond: `html5ever = "=0.40.1"` and `markup5ever_rcdom`'s replacement decision in Task 11; `sha2`, `quick-xml`, `sqlx` are workspace deps already. Each new pin is checked with Context7 / `cargo info` and recorded in the rulings file.
- Store rejects NUL in JSON strings: the converter strips `\0` from every string it emits.
- Body headings 2–6: the converter demotes h1 to h2 (m2b P18).
- Batch size 500, heartbeat stale after 120 s, takeover only with `--takeover` (P5–P6).
- Library code never prints; the CLI prints. `tracing` for logs.
- Commits small, per task; messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A big real export** (tens of thousands of items, 500 MB): memory must stay bounded and a kill at any point must resume without duplicates — Task 9 test kills the run mid-pass (by an injected failure after batch N) and re-runs; Task 17 asserts counts after the re-run.
2. **Hostile or broken XML** (entity bombs, DOCTYPE, truncated file, CDATA containing `]]>` splits, invalid UTF-8): a clear error, nothing written — Task 8 tests.
3. **Body content that would make the admin open read-only** (unknown node types, h1, marks outside the supported set, NUL): every converted body must pass the admin's `validateBody` — Task 12 runs every corpus body through a Rust port of the node/mark allowlist (`blocks::is_editor_safe`, Task 6) and asserts true.
4. **A second import started while one runs, or a takeover**: the old run's next batch fails with Conflict and writes nothing — Task 1 (store contract, Postgres concurrency) and Task 2 (core) tests.
5. **Re-running after editing an imported post in FerriteCMS**: the edit is kept and reported — Task 15 test.

---

### Task 1: Store — `import_runs`, `import_map`, explicit timestamps

**Files:**
- Create: `crates/ferrite-store-sqlite/migrations/0009_import.{up,down}.sql`, `crates/ferrite-store-postgres/migrations/0009_import.{up,down}.sql`
- Create: `crates/ferrite-domain/src/import.rs` (+ `lib.rs` export)
- Modify: `crates/ferrite-store/src/repos.rs` (new `ImportRepo`, `Repos::imports()`), both backends' `repos/import.rs` (new), `NewContent` (domain `content.rs:85`) and the comment insert input
- Test: `crates/ferrite-store-tests/src/import.rs` (+ registered in both backends' contract runners)

**Interfaces:**
- Produces (domain):
  - `ImportRunId(Uuid)`, `ImportRunState { Running, Done, Failed, Abandoned }` (`snake_case` serde/text)
  - `ImportRun { id, source: String, token: [u8; 16] (stored hex), state, started_by: UserId, started_at, heartbeat_at, finished_at: Option<Timestamp>, stats: serde_json::Value, dry_run: bool }`
  - `ImportMapKind { User, Term, Media, Content, Comment, Menu }`
  - `ImportMapRow { source, kind: ImportMapKind, wp_id: i64, ferrite_id: Uuid, hash: String, version: i64 }`
- Produces (store `ImportRepo`, on `Repos`, so usable in a `Tx`):
  - `create_run(&ImportRun) -> Result<(), StoreError>`
  - `fence(run: ImportRunId, token: &[u8;16], now: Timestamp) -> Result<bool, StoreError>` — the P5 `UPDATE`; true iff one row changed
  - `get_run(id) -> Option<ImportRun>`, `running() -> Option<ImportRun>`, `list_runs(limit: u32) -> Vec<ImportRun>` (newest first)
  - `set_state(id, state, stats: &Value, now) -> Result<(), StoreError>` (sets `finished_at` for non-running)
  - `map_get(source, kind, wp_id) -> Option<ImportMapRow>`, `map_put(&ImportMapRow)` (upsert), `map_list(source, kind) -> Vec<ImportMapRow>`
- Produces: `NewContent.created_at: Option<Timestamp>` and `updated_at: Option<Timestamp>` (None = now, as today); the comment insert input gains `created_at: Option<Timestamp>` if it does not have one.

- [ ] **Step 1: Write the failing contract tests** in `ferrite-store-tests/src/import.rs`:
  - `import_run_roundtrip_and_running`: create, `running()` returns it, `set_state(Done)` → `running()` is None and `finished_at` set.
  - `fence_true_only_for_matching_running_token`: wrong token → false; after `set_state(Abandoned)` → false; matching → true and `heartbeat_at` moved.
  - `fence_serialises_with_takeover` (Postgres only; SQLite prints `SKIP` with reason "single writer"): tx A runs `fence` (true) and holds; tx B on another connection runs `set_state(Abandoned)` in a spawned task; assert B has not finished after 200 ms; commit A; B finishes; a new tx's `fence` with the old token → false.
  - `import_map_upsert_and_list`: put, put again with new hash/version → one row with the new values; `map_list` filters by source and kind; unique on `(source, kind, wp_id)`.
  - `content_insert_keeps_explicit_timestamps`: `NewContent{created_at: Some(t1), updated_at: Some(t2)}` round-trips exactly (micros).
- [ ] **Step 2: Run** `FERRITE_TEST_PG_URL=… cargo test -p ferrite-store-sqlite -p ferrite-store-postgres import` — Expected: FAIL (compile errors: missing types).
- [ ] **Step 3: Write `0009_import`** on both backends: `import_runs(id PK, source TEXT NOT NULL, token TEXT NOT NULL, state TEXT NOT NULL CHECK in the four values, started_by → users ON DELETE SET NULL?` — no: `started_by` is NOT NULL with `ON DELETE RESTRICT` like other audit references; check `audit` in `0001` and copy its choice), `started_at, heartbeat_at, finished_at NULL, stats JSON NOT NULL DEFAULT '{}', dry_run BOOL NOT NULL)`, partial unique index "at most one running" `ON import_runs(state) WHERE state = 'running'`; `import_map(source, kind, wp_id BIGINT, ferrite_id UUID/TEXT, hash TEXT, version BIGINT, PRIMARY KEY(source, kind, wp_id))`. Down drops both. Timestamps and UUIDs in each backend's existing column conventions (copy from `0007_autosaves`).
- [ ] **Step 4: Implement the domain types and `ImportRepo`** on both backends; `NewContent` timestamps (update every existing `NewContent { .. }` construction with `created_at: None, updated_at: None`).
- [ ] **Step 5: Run** the Step 2 command and `cargo test --workspace` (with the PG URL) — Expected: PASS.
- [ ] **Step 6: Commit** `feat(store): import runs, import map and explicit content timestamps (0009_import)`.

### Task 2: Core — `ImportService`, runs, fence and the batch shell (users, terms, types, meta, options)

**Files:**
- Create: `crates/ferrite-core/src/import/mod.rs`, `import/batch.rs`, `import/users.rs`, `import/terms.rs`
- Modify: `crates/ferrite-core/src/lib.rs` (exports), `types.rs`, `taxonomy.rs`, `options.rs`, `users.rs` (expose `pub(crate) *_in(tx, …)` variants of create where missing, reusing their validation)
- Test: `crates/ferrite-core/tests/import_batch.rs` (both backends via the existing core test harness)

**Interfaces:**
- Consumes: Task 1 `ImportRepo`, `ImportRun`.
- Produces:
  - `ImportService::new(store: Arc<dyn Store>, content: Arc<ContentService>, media: Arc<MediaService>, permalinks: Arc<dyn Permalinks>)`
  - `ImportService::start(&self, actor: &Actor, source: &str, dry_run: bool, takeover: bool) -> Result<ImportHandle, CoreError>` — `require(actor, Capability::Import)`; P5; also takes `LOCK_IMPORT` with `try_lock` and holds it in the handle (absent lock = continue, logged).
  - `ImportHandle { run: ImportRun, actor: Actor }`; `ImportHandle::batch(&self, svc: &ImportService) -> Result<ImportBatch<'_>, CoreError>`; `ImportHandle::finish(self, svc, state: ImportRunState, stats: Value) -> Result<(), CoreError>` (releases the lock).
  - `ImportBatch` (owns a `Box<dyn Tx>`, a `BTreeSet<String>` of keys, an item counter):
    - `fn len(&self) -> usize` (items written), `const BATCH_ITEMS: usize = 500`
    - `async fn lookup(&self, kind: ImportMapKind, wp_id: i64) -> Result<Option<Mapped>, CoreError>` where `Mapped { id: Uuid, hash: String, version: i64 }` and a stale row (target gone) returns None
    - `async fn remember(&mut self, kind, wp_id, id: Uuid, hash: &str, version: i64)`
    - `async fn user(&mut self, u: ImportUser) -> Result<UserOutcome, CoreError>`; `ImportUser { wp_id, login, email, display_name }`; `UserOutcome { id: UserId, how: Matched::{Email, Login, Created} }` (P9)
    - `async fn taxonomy(&mut self, t: Taxonomy) -> Result<(), CoreError>` (create if absent; existing with different `hierarchical` → keep and return `Ok` + warn via return `bool` "differs")
    - `async fn content_type(&mut self, t: NewType) -> Result<(), CoreError>` (create if absent; existing: add missing fields to its schema)
    - `async fn term(&mut self, t: ImportTerm) -> Result<TermId, CoreError>`; `ImportTerm { wp_id, taxonomy, slug, name, description, parent: Option<TermId> }` — exact slug where free (P8)
    - `async fn meta(&mut self, item: ContentId, key: &str, value: &Value)`
    - `async fn options(&mut self, rows: &[(String, Option<Value>)])` (allowlisted, as `OptionService::set_many`)
    - `async fn commit(self) -> Result<(), CoreError>` — appends one invalidate event (P6) and commits; on any error the tx is rolled back.
  - `ImportService::batch` begins the tx and runs `fence` first; false → `CoreError::Conflict("import run was taken over")`.

- [ ] **Step 1: Write failing tests** (`tests/import_batch.rs`, both backends):
  - `start_requires_import_capability` — an editor actor → `Forbidden`.
  - `second_start_is_busy_and_takeover_fences_the_first` — start A; start B → Busy; age A's heartbeat 121 s (direct store `fence` with an old `now`, or a test clock seam already used by the job tests); start B with takeover → Ok; `A.batch()` → Conflict.
  - `batch_emits_one_event` — 3 terms in one batch; exactly one new `invalidate` event row containing all three `term:<id>` keys.
  - `user_matching_order` — existing user with email `x@e` → `Matched::Email`; login match → `Matched::Login`; new → `Created` with `must_reset` and no password, role author.
  - `term_keeps_slug_and_parent`; `rolled_back_batch_writes_nothing` (drop without commit).
- [ ] **Step 2: Run** `cargo test -p ferrite-core --test import_batch` — Expected: FAIL (missing module).
- [ ] **Step 3: Implement** as in Interfaces; reuse validation from the services' `*_in` helpers (make them `pub(crate)` where they are private). No `post.saved` actions.
- [ ] **Step 4: Run** the tests on both backends — Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): import service with run fence and batched writes`.

### Task 3: Core — imported content items

**Files:**
- Create: `crates/ferrite-core/src/import/content.rs`
- Modify: `crates/ferrite-core/src/content.rs`, `content/lifecycle.rs` (factor the shared parts of `create_in`/`update_in`/`after_save` so the batch reuses validation, slug, permalink root checks, revision and search indexing without actions)
- Test: `crates/ferrite-core/tests/import_content.rs`

**Interfaces:**
- Produces:
  - `ImportContent { wp_id: i64, hash: String, kind: String, title, slug: Option<String>, parent: Option<ContentId>, author: UserId, status: ContentStatus, published_at: Option<Timestamp>, created_at: Timestamp, updated_at: Timestamp, excerpt: Option<String>, body: Value, fields: Value, menu_order: i32, terms: Vec<TermId>, seo: Option<SeoPatch>, meta: Vec<(String, Value)> }`
  - `ImportBatch::content(&mut self, c: ImportContent) -> Result<ContentOutcome, CoreError>`; `ContentOutcome { id: ContentId, result: Written::{Created, Updated, Unchanged, KeptEdited}, slug_changed: Option<String> /* the slug actually used */, schedule_past: bool }` — P4, P7, P8; writes `import.wp.id` meta; `remember(Content…)`.
  - `ImportBatch::set_body(&mut self, id: ContentId, body: Value) -> Result<(), CoreError>` (link fix-up; bumps version and updates `import_map.version` so P4 still sees it as not edited locally)

- [ ] **Step 1: Write failing tests:**
  - `imported_item_matches_service_item` — create the same post via `ContentService::create` + `publish` and via the batch; compare search hit, revision count (1), invalidation keys, permalink; dates differ only as set.
  - `explicit_dates_author_status` — private and published items keep `published_at`/`created_at`/`updated_at` exactly and `author_id`.
  - `future_date_schedules_with_job`, `past_future_is_published_and_flagged`.
  - `exact_slug_or_reported_suffix` — a free slug is kept; a taken one returns `slug_changed: Some("x-2")`.
  - `rerun_semantics` — same hash → Unchanged; new hash → Updated; after a `ContentService::update` → KeptEdited and body untouched; deleted item → Created anew.
  - `no_plugin_actions` — no `plugin.action` jobs after a batch of published items.
- [ ] **Step 2: Run** `cargo test -p ferrite-core --test import_content` — Expected: FAIL.
- [ ] **Step 3: Implement.** Publishing inside the batch must do what `publish_in` does except actions: `reclaim_path`, search index, `published_at` as given.
- [ ] **Step 4: Run** both backends — Expected: PASS; also `cargo test -p ferrite-core` (no regressions).
- [ ] **Step 5: Commit** `feat(core): batched import of content items with explicit dates and status`.

### Task 4: Core — imported media, comments, menus and redirects

**Files:**
- Create: `crates/ferrite-core/src/import/{media,comments,menus,redirects}.rs`
- Modify: `media.rs`, `comments.rs`, `menu.rs`, `redirects.rs` (`pub(crate)` tx variants: `record_upload_in`, `MenuService::put_in`, `RedirectService::import_in`)
- Test: `crates/ferrite-core/tests/import_media_comments.rs`

**Interfaces:**
- Consumes: Task 7's `StagedFile` (the batch takes `NewUpload` — already the core input — so this task does not depend on Task 7's code; tests build `NewUpload` from an `ArtifactRef` written with `MediaStore::put_artifact`).
- Produces:
  - `ImportMedia { wp_id, hash, upload: NewUpload, author: UserId, created_at: Timestamp, parent: Option<ContentId> }` → `ImportBatch::media(&mut self, m) -> Result<ContentOutcome, CoreError>` (published, variants job when raster, dates/author as given)
  - `ImportComment { wp_id, item: ContentId, parent: Option<CommentId>, user: Option<UserId>, author_name, author_email, author_url, body: String, status: CommentStatus, created_at: Timestamp }` → `ImportBatch::comment(&mut self, c) -> Result<CommentId, CoreError>` (any item status; sets `by_account` when `user` is Some — 02:423; no rate limit, no moderation rules; `ip_hash` empty/None as the column allows)
  - `ImportBatch::menu(&mut self, m: &Menu) -> Result<(), CoreError>` (same validation as `MenuService::put`)
  - `ImportBatch::redirects(&mut self, rows: &[(String, String, u16)]) -> Result<RedirectOutcome, CoreError>`; `RedirectOutcome { created, updated, refused: Vec<(String, String /*reason*/)> }` — validates row by row, writes the valid rows with `origin = import`, in chunks of `MAX_IMPORT_ROWS`.

- [ ] **Step 1: Write failing tests:** `media_keeps_dates_author_and_queues_variants`; `comment_statuses_dates_threading_and_by_account`; `comment_on_draft_item_allowed`; `menu_put_in_batch`; `redirects_partial_refusal_reports_and_writes_rest` (a router-owned source like `/api/x` refused, a valid one written with origin `import`); `redirect_from_published_item_path_refused_or_reclaimed` — choose: refused with reason "path is a published item" (assert that).
- [ ] **Step 2: Run** `cargo test -p ferrite-core --test import_media_comments` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** both backends — Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): batched import of media, comments, menus and redirects`.

### Task 5: SEO `canonical` and `robots`

**Files:**
- Modify: `crates/ferrite-core/src/content/seo.rs` (keys `seo.canonical`, `seo.robots`; `SeoPatch { title, description, canonical: Option<Option<String>>, robots: Option<Option<Vec<Robots>>> }`; `Robots` enum `noindex|nofollow|noarchive|nosnippet|noimageindex`), `crates/ferrite-http/src/head.rs`, `crates/ferrite-http/src/api/dto/content.rs`, `crates/ferrite-http/openapi.json` (regenerated), `admin/src/api/schema.d.ts` (regenerated by `pnpm api:gen` or the project's script — check `admin/package.json`)
- Test: `crates/ferrite-core/tests/content_seo.rs`, `crates/ferrite-http/tests/api/content_seo.rs`, a head test next to `head.rs`'s existing tests

**Interfaces:**
- Produces: `Seo { title, description, canonical: Option<String>, robots: Vec<Robots> }` from `Seo::load`; the DTO fields `canonical` and `robots` on read and write (absent = unchanged, null = clear).

- [ ] **Step 1: Write failing tests:** canonical must be absolute http(s), ≤2048, else `InvalidField("seo.canonical")`; robots sorted/deduped; unknown robots value refused at the API (422); head emits `<link rel="canonical" href="…">` replacing the default and `<meta name="robots" content="noindex, nofollow">`; `reading.discourage_indexing` still wins (both noindex — check what head does today and keep one robots tag merged).
- [ ] **Step 2: Run** `cargo test -p ferrite-core --test content_seo` and `cargo test -p ferrite-http --test api content_seo` — Expected: FAIL.
- [ ] **Step 3: Implement**; regenerate `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`; regenerate the admin's API types and run `pnpm -C admin typecheck`.
- [ ] **Step 4: Run** the Step 2 commands and `pnpm -C admin test` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(seo): canonical URL and robots directives per item`.

### Task 6: Block tree — `embed` node, image `caption`, `is_editor_safe`

**Files:**
- Modify: `crates/ferrite-core/src/blocks/render.rs`, `blocks/mod.rs` (`pub fn is_editor_safe(doc: &Value) -> bool` — the Rust twin of `admin/src/editor/schema.ts` `SUPPORTED_NODES`/`SUPPORTED_MARKS` + heading 2–6), `blocks/url.rs` (embed URL → provider id)
- Modify: `admin/src/editor/schema.ts`, new `admin/src/editor/embed-node.tsx`, `admin/src/editor/image-node.tsx` (caption input in the node view), slash command entry "Embed"
- Test: `crates/ferrite-core/src/blocks/render.rs` tests, `admin/src/editor/*.test.ts(x)`

**Interfaces:**
- Produces: node `{"type":"embed","attrs":{"provider":"youtube"|"vimeo","url":"<canonical watch URL>"}}`; `blocks::embed_id(provider, url) -> Option<String>` (YouTube: `watch?v=`, `youtu.be/`, `/embed/`, `/shorts/`; 11 chars `[A-Za-z0-9_-]`; Vimeo: numeric path id); image `attrs.caption: Option<String>` (plain text).
- Render: embed → `<figure class="embed embed-<provider>"><iframe src="…" title="<Provider> video" loading="lazy" allowfullscreen referrerpolicy="strict-origin-when-cross-origin" sandbox="allow-scripts allow-same-origin allow-presentation allow-popups"></iframe></figure>`; unparseable id → a paragraph with a link to `url` (if `safe_href`). Image with caption → `<figure><img …><figcaption>escaped</figcaption></figure>`, without → unchanged output (existing snapshot tests must not change).

- [ ] **Step 1: Write failing tests:** Rust: youtube/vimeo variants → iframe src; `javascript:` url → nothing; caption escaped; `is_editor_safe` false for h1, unknown node, `underline` mark; true for every node the admin lists. Admin: `validateBody` accepts embed and captioned image; a test that the TS lists equal a JSON fixture the Rust test also reads (`crates/ferrite-core/tests/fixtures/editor-schema.json`) so the twins cannot drift.
- [ ] **Step 2: Run** `cargo test -p ferrite-core blocks` and `pnpm -C admin test editor` — Expected: FAIL.
- [ ] **Step 3: Implement** both sides. Embed node view: shows provider + URL, an input to change the URL, no live iframe in the admin (admin CSP has no frame-src; keep it).
- [ ] **Step 4: Run** Step 2 commands plus `pnpm -C admin lint && pnpm -C admin typecheck` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(blocks): embed node and image captions, editor-safety check`.

### Task 7: Media ingest from a stream

**Files:**
- Create: `crates/ferrite-media/src/ingest.rs`
- Modify: `crates/ferrite-http/src/media.rs` (`Uploader::stage` becomes a wrapper; `classify` moves to `ingest`), `crates/ferrite-media/src/lib.rs`
- Test: `crates/ferrite-media/src/ingest.rs` tests; existing upload API tests unchanged

**Interfaces:**
- Produces: `pub async fn stage_stream<S>(store: &MediaStore, file_name: &str, stream: S, max_bytes: u64) -> Result<StagedFile, MediaError> where S: Stream<Item = Result<Bytes, io::Error>> + Send + Unpin`; `StagedFile { artifact: ArtifactRef, mime: String, bytes: u64, original_name: String, sanitized: bool, raster: bool }` — same classification (sniff + office ZIP + txt/csv by extension) and SVG sanitising as uploads today. `Uploader::stage` keeps its slot logic and calls it.

- [ ] **Step 1: Write failing tests:** a PNG file stream → `raster: true`, mime `image/png`; an SVG with `<script>` → `sanitized: true`, script gone; a `.csv` → `text/csv`; over `max_bytes` → `TooLarge`.
- [ ] **Step 2: Run** `cargo test -p ferrite-media ingest` — Expected: FAIL.
- [ ] **Step 3: Implement** by moving code, not copying it.
- [ ] **Step 4: Run** `cargo test -p ferrite-media` and `cargo test -p ferrite-http --test api media` — Expected: PASS.
- [ ] **Step 5: Commit** `refactor(media): stage uploads from any byte stream`.

### Task 8: `ferrite-import-wp` — crate, WXR parser, PHP unserialize, corpus

**Files:**
- Create: `crates/ferrite-import-wp/{Cargo.toml,src/lib.rs,src/wxr.rs,src/php.rs,src/model.rs,src/error.rs}`; add to workspace members
- Create: `crates/ferrite-import-wp/tests/corpus/{site-a.xml,site-b.xml,site-a-uploads/,site-b-uploads/,site-a.php,site-b.php,corpus-types.php,README.md}` copied from `.superpowers/m3-corpus/` (README: how they were made — WordPress 7.1.3, Yoast/Rank Math/ACF versions, `wp eval-file`, synthetic content, 2026-10-28)
- Test: `crates/ferrite-import-wp/tests/wxr.rs`

**Interfaces:**
- Produces (`model.rs`, all `Serialize + Deserialize` so the IR stores them as JSON):
  - `Channel { title, link, description, base_site_url, base_blog_url, wxr_version, language }`
  - `Author { wp_id: i64, login, email, display_name, first_name, last_name }`
  - `Term { wp_id: i64, taxonomy: String, slug, name, description, parent_slug: Option<String> }` (from `<wp:category>`, `<wp:tag>`, `<wp:term>`)
  - `Item { wp_id: i64, post_type, status, title, link, guid, creator_login, content: String, excerpt: String, post_name, parent_wp_id: i64, menu_order: i32, date_local: String, date_gmt: String, modified_gmt: String, comment_status, is_sticky: bool, attachment_url: Option<String>, terms: Vec<ItemTerm{taxonomy (domain attr), slug (nicename), name}>, meta: Vec<(String, String)>, comments: Vec<Comment> }`
  - `Comment { wp_id, author, author_email, author_url, date_gmt, date_local, content, approved: String, comment_type: String, parent_wp_id: i64, user_id: i64 }`
  - `Record { Channel(Channel) | Author(Author) | Term(Term) | Item(Item) }`
- Produces: `wxr::parse<R: BufRead>(r: R, sink: impl FnMut(Record) -> Result<(), ImportError>) -> Result<ParseStats, ImportError>`; `ImportError` (thiserror) with `Xml{offset, msg}`, `Doctype`, `TooLarge{offset}`, `Utf8{offset}`, `Missing{what, offset}`.
- Produces: `php::unserialize(s: &str) -> Result<PhpValue, PhpError>`; `PhpValue { Null, Bool(bool), Int(i64), Float(f64), Str(String), Array(Vec<(PhpValue, PhpValue)>) }`; objects (`O:`) → `PhpError::Object` (never instantiated). Depth ≤ 32, length checked against the input.

- [ ] **Step 1: Write failing tests:** `site_a_counts` — 35 items: post 8, page 4, attachment 4, nav_menu_item 5, book 2, acf-field-group 1, acf-field 11; authors 4 (`admin`, `erin`, `arlo`, `cass`); `site_b_counts` — 7 items; item "Every core block" has meta `_yoast_wpseo_meta-robots-noindex = "1"` and 4 comments; `cdata_split_is_joined` (`]]]]><![CDATA[>`); `doctype_refused`; `invalid_utf8_reports_offset`; `truncated_file_errors`; php: `a:2:{i:0;a:1:{s:4:"text";s:9:"Quote one";}…}` from site A parses; `s:` length counts bytes (UTF-8 `é`); `O:8:"stdClass"…` → `Object`; depth 33 → error.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp --test wxr` — Expected: FAIL.
- [ ] **Step 3: Implement** with `quick_xml::Reader` events (`trim_text(false)`), namespaced local names (`wp:`, `content:`, `excerpt:`, `dc:`), one `Item` buffered at a time. Verify the quick-xml 0.41 API with Context7 first.
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): streaming WXR parser, PHP unserializer and the test corpus`.

### Task 9: IR file and the run driver

**Files:**
- Create: `crates/ferrite-import-wp/src/ir.rs`, `src/run.rs`
- Test: `crates/ferrite-import-wp/tests/ir.rs`

**Interfaces:**
- Produces: `Ir::create(path: &Path) -> Result<Ir, ImportError>`, `Ir::open(path)`, `push(&Record)` (batched inserts in one tx per 1 000), `channel() -> Channel`, `authors()`, `terms()`, `items(post_types: &[&str]) -> impl Stream<Item = Result<Item, ImportError>>` (by `seq`), `item(wp_id) -> Option<Item>`, `pass_done(name) -> bool`, `mark_pass(name)`.
- Produces: `ImportOptions { uploads: Option<PathBuf>, fetch_media: bool, dry_run: bool, keep_permalinks: bool, site_options: bool, category_base: String /*"category"*/, tag_base: String /*"tag"*/, takeover: bool }`; `trait MediaSource: Send + Sync { async fn open(&self, rel_path: &str, url: &str, max: u64) -> Result<Option<ByteStream>, ImportError> }` (None = missing); `struct DirSource`; `pub async fn run(ctx: ImportContext, wxr: &Path, opts: ImportOptions) -> Result<Report, ImportError>` where `ImportContext { import: Arc<ImportService>, media_store: Arc<MediaStore>, actor: Actor, work_dir: PathBuf, media_source: Arc<dyn MediaSource>, site_url: String }`. `run` = start (P5) → parse to IR unless `pass_done("parse")` → passes 1–4 + fix-up (Tasks 13–15 fill them; here they are empty functions returning `Ok`) → report → finish. A test-only failure seam: `ImportOptions::fail_after_batches: Option<u32>` behind `#[cfg(any(test, feature = "test-seams"))]` (the repo already has a `test-seams` feature pattern — follow it).
- Produces: `Report` (Task 16 defines its fields; here a `Default` struct with `counts: BTreeMap<String, Counts>`).

- [ ] **Step 1: Write failing tests:** IR round-trip of site A (items by type, by id); `resume_skips_parse_and_done_passes` (mark a pass, reopen, the driver does not call it — count via a test sink); `dry_run_starts_a_dry_run_and_writes_nothing` (no content rows after).
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp --test ir` — Expected: FAIL.
- [ ] **Step 3: Implement** (sqlx sqlite, `journal_mode=WAL`, `synchronous=NORMAL`; the file lives under `work_dir/<run-id>/`). Dry run: passes run with a batch that is rolled back instead of committed (`ImportBatch::rollback`, add it in this task if Task 2 did not).
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): IR file, run driver and resume`.

### Task 10: Gutenberg block parser

**Files:**
- Create: `crates/ferrite-import-wp/src/convert/mod.rs`, `convert/gutenberg.rs`
- Test: unit tests in `gutenberg.rs`

**Interfaces:**
- Produces: `gutenberg::parse(content: &str) -> Vec<Block>`; `Block { name: String /* "core/paragraph"; bare names get "core/" */, attrs: serde_json::Map, inner: Vec<Block>, inner_html: Vec<HtmlPart> }`, `HtmlPart { Html(String) | Child(usize) }` (keeps interleaving, like WP's `innerContent`); free HTML between blocks → `Block { name: "core/freeform", … }`. `gutenberg::has_blocks(content) -> bool` (`<!-- wp:` present).
- Grammar as WordPress's `parse_blocks`: `<!-- wp:ns/name {json} -->`, closer `<!-- /wp:ns/name -->`, void `<!-- wp:name {json} /-->`; invalid JSON attrs → empty map (counted later); unbalanced closer → treated as text.

- [ ] **Step 1: Write failing tests:** nested list-in-list from site A; void block; gallery with two child images; attrs `{"level":3}`; freeform around blocks; unbalanced comment; namespaced unknown `acme/fancy-callout`.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp gutenberg` — Expected: FAIL.
- [ ] **Step 3: Implement** a hand-written scanner (no regex dependency).
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): Gutenberg block comment parser`.

### Task 11: HTML → block tree, `wpautop`, shortcode scanner

**Files:**
- Create: `crates/ferrite-import-wp/src/convert/{html.rs,autop.rs,shortcode.rs,doc.rs}`
- Modify: `crates/ferrite-import-wp/Cargo.toml` (`html5ever = "=0.40.1"` + a DOM sink: prefer `markup5ever_rcdom` at the version html5ever 0.40.1 documents; if none is published for it, use `scraper = "0.29"` only if `cargo tree -d -p ferrite-import-wp` shows no duplicate html5ever; record the choice as a ruling)
- Test: unit tests per file

**Interfaces:**
- Produces: `doc.rs` builders: `Node` enum with exactly the supported node set (paragraph, heading(2..=6), bulletList, orderedList{start}, listItem, blockquote, codeBlock{language}, horizontalRule, hardBreak, image{src, alt, title, caption}, table/tableRow/tableHeader/tableCell{colspan,rowspan}, html{html}, embed{provider,url}, text{text, marks}) and `Mark` (bold, italic, strike, code, link{href, target}); `fn to_json(&[Node]) -> Value` (root `doc`, NUL stripped).
- Produces: `trait Rewrite { fn image(&self, src: &str) -> ImageRef; fn href(&self, href: &str) -> String; }` where `ImageRef { Media(Uuid) /* → src "ferrite:media/<uuid>" */, Url(String), Missing(String) }`; `Notes` (counters: dropped marks by name, h1 demoted, unsupported tags, missing media, unknown shortcodes by name with item ids).
- Produces: `autop::wpautop(text: &str) -> String` (port of WordPress `wpautop` core behaviour: double newline → `<p>`, single → `<br>`, block-level tags not wrapped, `<pre>` untouched); `shortcode::scan(text) -> Vec<Piece { Text(String) | Code { name, attrs: BTreeMap<String,String>, inner: Option<String> } }>` (WP shortcode grammar incl. `[[escaped]]`, self-closing, quoted/unquoted attrs).
- Produces: `html::convert(html: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Vec<Node>`.

- [ ] **Step 1: Write failing tests:** autop on site B's classic body: first two paragraphs, `<br>` for the single break, `<ul>` not wrapped; marks `<strong><em>` → bold+italic; `<u>`, `<sup>`, `<span style>` → text kept, counted; `<h1>` → heading 2, counted; `<a href="javascript:…">` → text without link; `<img>` inside `<a>` → image node (link dropped, counted); `<table>` without `<tbody>`; `<pre>` keeps whitespace; nested lists; `<p>` inside `<li>`; NUL stripped; shortcode `[caption id="attachment_5" align="aligncenter" width="300"]<img …/> The harbour[/caption]` scanned with inner; `[[gallery]]` is literal text.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp convert` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): HTML to block tree, wpautop and shortcode scanner`.

### Task 12: Block and shortcode mapping, the body converter

**Files:**
- Create: `crates/ferrite-import-wp/src/convert/blocks.rs`, `convert/shortcodes.rs`, `convert/body.rs`
- Test: unit tests + `crates/ferrite-import-wp/tests/convert_corpus.rs`

**Interfaces:**
- Consumes: Tasks 10, 11; `ferrite_core::blocks::is_editor_safe` (Task 6).
- Produces: `body::convert(content: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Value` — Gutenberg when `has_blocks`, else autop → shortcodes → html. Mapping (P11, P12):
  - core/paragraph, heading, list(+list-item), quote(+pullquote, verse → blockquote/codeBlock-free paragraph), code → codeBlock (language from `className` `language-x` if any), preformatted → codeBlock without language, table, separator → horizontalRule, image (caption from `figcaption`; media via `attrs.id` first, then src), gallery → images, embed (youtube/vimeo → embed node; others → paragraph link), video/audio/file → paragraph link to the media URL, buttons/button → paragraph link(s), columns/column/group/cover/media-text/details → flattened children (cover/media-text image kept), html → html node, shortcode block → shortcode handling, freeform → classic path, spacer → dropped, everything else → html node with inner HTML or dropped when empty (counted with example item ids).
  - Shortcodes: `[caption]` → image with caption; `[gallery ids]` → images by attachment id; `[embed]URL[/embed]` and a paragraph that is only a URL on its own line (WP auto-embed) → embed node or link; `[video src]`/`[audio src]` → paragraph link; others → text, counted.
- Produces: `Notes::merge`, item ids recorded per note kind (≤5 examples).

- [ ] **Step 1: Write failing tests:** site A's "Every core block" converts to the expected top-level node sequence (assert node types in order: paragraph, heading(2), heading(3), bulletList(with nested bulletList), orderedList{start:3}, blockquote, image{caption:"The hero, medium size"}, image, image, codeBlock, codeBlock, table, horizontalRule, embed{youtube}, paragraph(link "Sign up"), paragraph("Left column."), paragraph("Right column."), html, paragraph(shortcode text), html(acme), paragraph(links)); site B's "Harbour walk" converts: image with caption from `[caption]`, two gallery images, embed{vimeo}, embed{youtube} from the bare URL line, video link, unknown shortcode counted; `convert_corpus`: every body in sites A and B and (when `FERRITE_WP_CORPUS` is set) both theme-test files passes `is_editor_safe` and contains no NUL.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp --test convert_corpus` and the unit tests — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** (also once with `FERRITE_WP_CORPUS=.superpowers/m3-corpus`) — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): Gutenberg and shortcode mapping to the block tree`.

### Task 13: Passes 1–2 — authors, taxonomies, terms, types, media

**Files:**
- Create: `crates/ferrite-import-wp/src/pass/{mod.rs,people.rs,terms.rs,types.rs,media.rs}`, `src/acf.rs`, `src/urls.rs`
- Test: `crates/ferrite-import-wp/tests/passes.rs` (an in-process site on SQLite and Postgres, built like `ferrite-core`'s test harness)

**Interfaces:**
- Consumes: Tasks 2, 4, 7, 9.
- Produces:
  - pass 1: authors → users (P9) mapped `User`; taxonomies: `category`, `post_tag` built-in; others created with `hierarchical = any term has a parent`, `content_types` = the post types whose items use it; terms in parent-first order (topological; a cycle → parent dropped, counted); custom post types (not post/page/attachment/nav_menu_item/revision/acf-*/wp_*/custom_css/customize_changeset/oembed_cache/user_request/wp_block) → `NewType { name (normalised to the type-name rules), label (title case of the name), hierarchical: any item has a parent, has_archive: true, fields: from ACF (P16) }`.
  - `acf::field_defs(ir) -> AcfModel` (`groups: Vec<{key, title, post_types: Vec<String> /* from the location rules in the group's serialised content */, fields: Vec<AcfField{key, name, label, kind, config: PhpValue, sub: Vec<AcfField>}>}>`), `acf::to_field_def(&AcfField) -> Result<FieldDef, Unmapped>`.
  - pass 2: attachments → `MediaSource::open` → `stage_stream` → `ImportBatch::media` (alt from `_wp_attachment_image_alt`, title from the item); `urls::MediaIndex` built from `attachment_url` + `_wp_attachment_metadata` (`file`, `sizes.*.file`, `original_image`) → `path → (Uuid, public_url)` (P14), persisted in the IR (`media_urls` table) so a resumed run has it.
  - `HttpSource` is not here (Task 17).

- [ ] **Step 1: Write failing tests:** site A pass 1: 4 users (admin matched by e-mail if the test site has `a@example.test`, else created), category `local` has parent `news`, `genre` taxonomy hierarchical with `book`, type `book` with fields `isbn:text, pages:number, in_print:bool, released:date, blurb:text, cover:media, format:select[hardback,paperback], site:text, review:rich_text, related:repeater[item:relation], quotes:repeater[text:text]` (if the free-ACF export has no `repeater` sub-field rows, assert `quotes` reported unmapped instead — check the corpus and pin whichever it is); pass 2: 4 media items with alt texts, `MediaIndex` resolves `hero-768x432.png`-style size names from the metadata and `-scaled`; a missing file is reported and skipped.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp --test passes pass1 pass2` — Expected: FAIL.
- [ ] **Step 3: Implement**, batches of `ImportBatch::BATCH_ITEMS`.
- [ ] **Step 4: Run** both backends — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): people, terms, types and media passes`.

### Task 14: Pass 3 — content, meta mapping, link fix-up

**Files:**
- Create: `crates/ferrite-import-wp/src/pass/content.rs`, `pass/fixup.rs`, `src/seo.rs`, `src/meta.rs`
- Test: extend `tests/passes.rs`

**Interfaces:**
- Consumes: Tasks 3, 12, 13.
- Produces:
  - pass 3 in parent-first order per type (pages and hierarchical custom types topologically by `parent_wp_id`): `ImportContent` from `Item` (P7, P8; excerpt `None` when empty; `featured_image` from `_thumbnail_id` via the media map; author via the user map else the actor; terms via the term map; ACF values per P16 incl. date and relation/media id mapping — relations to items not yet created are queued for fix-up; SEO per P10 via `seo::map(item, channel) -> (Option<SeoPatch>, Vec<Note>)`; meta: every other non-underscore key and `_`-prefixed keys not in the drop list (`_edit_lock`, `_edit_last`, `_pingme`, `_encloseme`, `_wp_old_slug`, `_wp_old_date`, `_wp_trash_meta_*`, `_wp_desired_post_slug`, `_oembed_*`, `_wp_page_template` → kept as `wp.page_template`, ACF `_name` reference keys, `_menu_item_*`) → `wp.<key>` as a JSON string; `is_sticky` → `wp.sticky = true`).
  - `Rewrite` impl over `MediaIndex` + an old-URL index (`<link>` paths, `?p=`, `?page_id=` → wp id) that returns already-imported paths and records unresolved ones; fix-up pass re-converts bodies with unresolved links once all content exists and calls `ImportBatch::set_body` (only for items `Created`/`Updated` in this run).

- [ ] **Step 1: Write failing tests:** site A: "Every core block" is published with `published_at = 2024-03-15T10:30:00Z` (site timezone UTC in the scratch WP — check `post_date_gmt` in the XML and pin it), author `arlo`, terms news+local+rust+wasm, featured image = hero, SEO `{title: "Every core block - Corpus Site", description, canonical, robots: [noarchive, nofollow, noindex, nosnippet]}`, meta `wp.some_unrelated_meta`, its link to `/about/` rewritten to the new about path and `?page_id=<team>` to the new team path; "Team" has parent "About" and `menu_order 2`; "From the future" scheduled with a job at 2030-06-01T12:00:00Z; "Private thoughts" private; "Gone soon" trash; book "The First Book" fields as in Task 13 with `cover` = the cover media id and `related[0].item` = "The Second Book"; site B: Rank Math SEO `{title: "Harbour walk - Classic Corpus", robots: [nofollow, noindex]}`, `%category%` posts.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp --test passes pass3` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** both backends — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): content pass with SEO, ACF and meta mapping, link fix-up`.

### Task 15: Pass 4 — comments, menus, permalinks, redirects, options; re-runs

**Files:**
- Create: `crates/ferrite-import-wp/src/pass/{comments.rs,menus.rs,redirects.rs,options.rs}`, `src/permalinks.rs`
- Test: extend `tests/passes.rs`; `tests/rerun.rs`

**Interfaces:**
- Consumes: Tasks 2–4, 14.
- Produces:
  - `permalinks::infer(posts: &[(&Item, Option<&str> /*primary category slug*/)]) -> Option<String>` (P17; runs **before** pass 3, writes `permalink.post` through `ImportBatch::options` when `keep_permalinks`) — returns e.g. `"/%year%/%month%/%slug%/"` for site A and `"/%category%/%slug%/"` for site B.
  - comments (P18), menus (P19), redirects (P17: items, attachment pages, tag and category bases; batched; refusals into notes), options (P20).
  - Re-run: the whole pipeline with an existing map follows P4; counts per kind `created/updated/unchanged/kept/skipped`.

- [ ] **Step 1: Write failing tests:** `infer` on site A → `/%year%/%month%/%slug%/`, site B → `/%category%/%slug%/`, mixed → None; site A after pass 4: 3 comments (spam included with status spam? — P18 says spam → spam: assert 4 comments: 2 approved threaded, 1 pending, 1 spam), menu `main` with Home(url) / About(item) > Team(item) / All news(term) / Every core block(item); redirects: `/books/the-first-book/` → `/book/the-first-book/`, `/tag/rust/` → the tag term URL, attachment page → media URL, none where old == new; options `site.title = "Corpus Site"`, `site.description`; site B: pingback skipped and counted, comment HTML → `"Lovely photos!\n\nSecond paragraph bold."`; `rerun.rs`: import site A twice → second run all `unchanged`, no new rows (count content, comments, redirects, terms); edit one post via `ContentService::update`, change its title in a copy of the XML, re-run → `kept` with note, title unchanged; `resume_after_failure`: `fail_after_batches = Some(2)` with batch size forced to 3 via a test seam → error; re-run with `takeover` → final counts equal a clean run.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp --test passes pass4 --test rerun` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** both backends — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): comments, menus, permalinks, redirects and idempotent re-runs`.

### Task 16: Report — JSON and self-contained HTML

**Files:**
- Create: `crates/ferrite-import-wp/src/report.rs`, `src/report.html.j2` is **not** used — render with `format!`/`html_escape` (no template engine dependency)
- Test: unit tests in `report.rs`

**Interfaces:**
- Produces: `Report { run_id, source, dry_run, started_at, finished_at, counts: BTreeMap<String /*users|terms|media|post|page|<type>|comments|menus|redirects*/, Counts{created, updated, unchanged, kept, skipped, failed}>, unknown_blocks: Vec<Example{name, count, items: Vec<ItemRef{wp_id, title, new_path: Option<String>}>}>, unknown_shortcodes: Vec<Example>, dropped_formatting: BTreeMap<String, u64>, missing_media: Vec<{wp_id, path, url}>, unmapped_meta: Vec<(String, u64)> /* top 50 */, unmapped_acf: Vec<{field, kind, items}>, users_must_reset: Vec<{login, email, how}>, redirects: {created, refused: Vec<{source, target, reason}>}, slug_changes: Vec<{wp_id, from, to}>, scheduled_in_past: Vec<ItemRef>, permalink_structure: Option<String>, notes: Vec<String> /* e.g. "variants run on the next serve", "no plugin actions fired" */ }` (`Serialize`); `Report::to_json() -> String`, `Report::to_html() -> String` (one file, inline CSS, no scripts, no external URLs; every value escaped; `dark` via `prefers-color-scheme`).

- [ ] **Step 1: Write failing tests:** JSON round-trips; HTML escapes a title `<script>x</script>` and a source URL with `"`; contains no `<script` and no `http` except inside escaped values and item links built from `site_url`; `unmapped_meta` capped at 50 sorted by count desc then key.
- [ ] **Step 2: Run** `cargo test -p ferrite-import-wp report` — Expected: FAIL.
- [ ] **Step 3: Implement**; the passes already fill `Notes` — wire them into `Report` in `run.rs`.
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(import-wp): import report as JSON and HTML`.

### Task 17: CLI `ferrite import wordpress`, HTTP media source, end-to-end tests

**Files:**
- Create: `crates/ferrite/src/import_cmd.rs`; `crates/ferrite-http/src/import_fetch.rs` (`HttpSource: MediaSource` over the SSRF-checked client used by the plugin `http` import — find it under `crates/ferrite-http/src/plugins/` and reuse its resolver/client builder, do not copy)
- Modify: `crates/ferrite/src/cli.rs`, `lib.rs`; `crates/ferrite/Cargo.toml`
- Test: `crates/ferrite/tests/import_cli.rs`; `crates/ferrite-import-wp/tests/corpus_real.rs` (env-gated)

**Interfaces:**
- Produces: `ferrite import wordpress <FILE> --data DIR [--uploads DIR] [--fetch-media] [--dry-run] [--report DIR (default: <data>/import/reports/<run-id>/)] [--no-keep-permalinks] [--no-site-options] [--category-base S] [--tag-base S] [--as LOGIN (default: the first admin)] [--takeover]`. Exit codes: 0 done, 1 error, 2 done with refusals/missing media (report has them), 3 busy. Prints counts, report paths, and "media variants are made by the next `ferrite serve`" when media was imported. Opens the site like `seed demo` (`load_config`, `open_store(MustExist)`, `startup_check`, `open_media`, permalink rules); `store.close()` on every path.
- `HttpSource`: 30 s per file, `config.media.max_upload` cap, only http/https, SSRF rules as the plugin client (private/loopback refused) — with an `allow_private` test seam under `test-seams` so tests can serve files from `127.0.0.1`.

- [ ] **Step 1: Write failing tests** (`import_cli.rs`, runs the binary like the existing CLI tests; both backends): `import_site_a_end_to_end` — exit 0, report files exist, `GET /` of a served site (start `serve` on `127.0.0.1:0` after the import) renders the imported post at its old URL `/2024/03/every-core-block/` with an `<iframe src="https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"`, `GET /books/the-first-book/` → 301 to `/book/the-first-book/`; `import_busy_exit_3` (Postgres: a run row `running` with fresh heartbeat inserted directly); `dry_run_writes_nothing`; `fetch_media_from_local_server` (serve `site-b-uploads` with a tiny axum server; `--fetch-media` without `--uploads`; media count 4). `corpus_real.rs`: with `FERRITE_WP_CORPUS` set, imports both theme-test files into a fresh SQLite site: exit 0 or 2, report JSON counts > 0, every body `is_editor_safe`; prints `SKIP` otherwise.
- [ ] **Step 2: Run** `cargo test -p ferrite --test import_cli` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** both backends, then `FERRITE_WP_CORPUS=$PWD/.superpowers/m3-corpus cargo test -p ferrite-import-wp --test corpus_real` — Expected: PASS; record the theme-test report counts for the exit doc.
- [ ] **Step 5: Commit** `feat(cli): ferrite import wordpress`.

### Task 18: Docs, rulings and the M3-1 exit

**Files:**
- Modify: `docs/design/08-wp-import.md` (§3 table with the verified keys and the P-rulings' mappings; §2 IR/`import_map`; §4 embed/caption; mark what is M3-2), `docs/design/07-admin-api.md` §1 (Import row status), `docs/design/01-architecture.md` §1 (crate), `docs/design/02-storage.md` (0009 tables), `docs/design/09-operations.md` (CLI), `docs/ROADMAP.md` (tick M3 items done in M3-1, split the rest into "M3-2"), `CLAUDE.md` (status, crate map, how to run an import), `docs/superpowers/rulings/m3.md` (execution rulings R1…)
- Create: `docs/superpowers/m3-1-exit.md` (what was built, gate results with commands, theme-test corpus counts, what was **not** verified)

- [ ] **Step 1:** Write the docs; every claim checked against code or a command run (user rule: table of verified claims in the exit doc).
- [ ] **Step 2:** Gates: `rustup update stable`; `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `FERRITE_TEST_PG_URL=… cargo test --workspace`; `cargo test --workspace` (SQLite only); `pnpm -C admin lint typecheck test build`; admin E2E for the editor specs on Chromium (`pnpm -C admin e2e -- editor` or the project's equivalent) — Expected: all pass; paste the summary lines into the exit doc.
- [ ] **Step 3: Commit** `docs: M3-1 exit`.
