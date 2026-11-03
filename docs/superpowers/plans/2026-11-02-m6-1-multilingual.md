# M6-1 — Multilingual content Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Content items, terms and menus carry a locale and translation groups; the public site serves non-default locales under `/<prefix>/` with `hreflang`, per-locale feeds and sitemap alternates; themes, admin, REST API, MCP and both importers handle locales; a one-locale site stays byte-identical.

**Architecture:** Locales live in the options table (`site.locales`, default first), settled at startup from the old `site.locale` option or the config. Migration `0014_locales` adds `locale` + `translation_group` to `content_items` and `terms`, `locale` + `translation_of` to `menus`, and widens the slug unique indexes by `locale`. `PermalinkRules` carries the locales and prefixes paths; the router strips a prefix before the existing `resolve`; `SiteReader` reads take a `ReadLocale` (`All` on one-locale sites, so no query changes). Locale changes are separate core operations (`translate`, `relocate`, `translation_of`), never `ContentDraft` fields.

**Tech Stack:** Rust 2024, axum 0.8, sqlx (SQLite + Postgres), MiniJinja (themes), `rmcp` 3.8 (MCP), utoipa (OpenAPI), React/TanStack admin, Vitest, Playwright. No new dependency.

**Spec:** `docs/design/13-multilingual.md` (read it first; it wins over this plan on behaviour).

## Global Constraints

- Migration `0014_locales` on **both** backends, `-- ferrite:phase expand` and `-- ferrite:lossy-down`, with `.down.sql`; no other migration number (0015–0019 belong to other M6 parts). Contract tests on both backends (`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:5432/ferrite_test`).
- The golden test of Task 1 stays green after every later task (byte-identical one-locale output, design §9).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`; `thiserror` in libraries, `anyhow` only in `crates/ferrite`; no `allow(dead_code)`.
- Capabilities are checked in core, never in handlers or tools.
- REST API changes: DTOs with `utoipa` annotations; regenerate `crates/ferrite-http/openapi.json` with `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`, then `pnpm -C admin api:gen` for `admin/src/api/schema.d.ts`.
- No WIT change (`wit/ferrite-plugin@0.1.0` untouched; design D14).
- Admin: i18n keys in `admin/src/i18n/en/*.json`; 11-admin-ui rules (tokens only, 44 px touch targets, axe clean); every locale control hidden on one-locale sites.
- Narrow file footprint (wave A runs M6-2 plugin trust and M6-6 webhooks in parallel): do not touch `crates/ferrite-plugin-host`, `crates/ferrite-http/src/extensions.rs`, `crates/ferrite-core/src/extensions/`, `crates/ferrite/src/ext_cmd.rs` beyond the one-line `startup_check` argument of Task 5, and keep event kinds unchanged (webhooks read content events; M6-1 adds dependency keys, not event kinds).
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A one-locale site: every route of the three starter themes, `/feed/`, `/sitemap.xml`, the sitemap files and `robots.txt` byte-identical to M5 (golden test), and no locale predicate in its SQL (a `CountingStore`/query-text assertion).
- `site.locales` gaining a prefix that equals a top-level page slug, a type or taxonomy name, `page`, `author` or `2024` → refused naming the clash.
- Removing a locale that still holds a draft → `409` with the counts; reordering → URLs move on every node after the reload (cluster test).
- `/de/about/` and `/about/` coexist (same slug, same parent level); `/DE/about` → 301 `/de/about/`; `/de/robots.txt`, `/de/_media/…` → 404.
- Translate by a contributor of another user's published post → their own draft; of another user's draft → 403/404 as `can_read` decides; a second Translate into the same locale → 409 naming the existing id.
- Startup on a database migrated by 0014 where `site.locale` is unset and `ferrite.toml` says `de`: rows end up `de` after `settle`, once, under the migrate lock.
- A hostile `post_translations` description (huge, nested, wrong types, a cycle, two items claiming one locale) → report lines, no panic, no partial group.

---

### Task 1: Golden one-locale snapshot

Captures today's public output before anything changes, so every later task proves design §9.

**Files:**
- Create: `crates/ferrite/tests/serve/single_locale.rs` (+ `mod single_locale;` in `crates/ferrite/tests/serve/main.rs`), fixtures in `crates/ferrite/tests/fixtures/golden-single-locale/{blog,business,docs}/*.txt`
- Test only; no production change.

**Interfaces:**
- Produces `fn normalize(body: &str) -> String` (UUIDs → `<uuid>`, `/_media/<hash>/` and `/_theme/<hash>/` → `<hash>`, RFC 3339 / RFC 2822 / ISO dates and `lastmod` values → `<date>`, `?v=` cache busters → `<v>`) and `fn assert_golden(theme, name, body)` that writes the fixture when `FERRITE_UPDATE_GOLDEN=1` and compares otherwise.
- Routes per profile: `/`, a post, a page, a child page, `/page/2/` where it exists, a category and a tag archive, an author archive, a year archive, `/?s=` with a hit, a 404, `/feed/`, a term feed, `/sitemap.xml`, each sitemap file, `/robots.txt`. Profiles: `seed demo` (blog), `seed demo --profile business` + `theme activate business`, the docs site via `site/build.sh` logic (or `import markdown site/content --front-page` + `theme activate docs`).

- [ ] Write the test against the current code; run with `FERRITE_UPDATE_GOLDEN=1` to write the fixtures; run again without it → passes; run twice more to prove the normalisation is stable.
- [ ] Commit `test(serve): golden one-locale snapshot of the starter themes, feeds and sitemaps`.

### Task 2: Locale tags and `SiteLocales` (pure)

**Files:**
- Create: `crates/ferrite-domain/src/locale.rs` (+ `pub mod locale;` and re-exports in `crates/ferrite-domain/src/lib.rs`)
- Modify: `crates/ferrite-http/src/app/state.rs` (`bcp47` → call `ferrite_domain::locale::bcp47_loose`; its test moves to the domain module)

**Interfaces:**
- `pub fn parse_tag(s: &str) -> Result<String, LocaleError>` (strict subset, canonical case, `_` read as `-`, ≤ 35 bytes), `pub fn bcp47_loose(s: &str) -> String`, `pub fn prefix(tag: &str) -> String`, `pub fn language(tag: &str) -> &str`, `pub fn native_name(tag: &str) -> Option<&'static str>`.
- `pub struct LocaleDef { pub tag: String, pub label: String }`, `pub struct SiteLocales { pub default: LocaleDef, pub others: Vec<LocaleDef> }` with `from_option(value: &serde_json::Value) -> Result<SiteLocales, LocaleError>`, `to_option(&self) -> Value`, `single(tag)`, `is_multilingual()`, `all()`, `contains(tag)`, `by_prefix(segment) -> Option<&LocaleDef>`, `prefix_of(tag) -> Option<String>`, `MAX_LOCALES = 20`, `MAX_LABEL = 50`.
- `pub const LOCALES_OPTION: &str = "site.locales"`, `LOCALE_TEXTS_OPTION = "site.locale_texts"`.

- [ ] Unit tests: `parses_canonical_case` (`PT_br` → `pt-BR`, `zh-hant-tw` → `zh-Hant-TW`, `es-419`), `refuses_extensions_variants_private_use_and_junk` (`en-US-x-a`, `de-1996`, `"><x`, `""`, 36 bytes), `loose_matches_old_bcp47_table` (the table from `state.rs`), `prefix_is_lowercase`, `site_locales_roundtrip_and_caps` (21 entries, duplicate prefixes, label with a bidi override, empty label → native name → tag), `by_prefix_is_ascii_case_insensitive`.
- [ ] Implement; `cargo test -p ferrite-domain locale` and `cargo test -p ferrite-http` pass; golden test green.
- [ ] Commit `feat(domain): locale tags and the site locale list`.

### Task 3: Migration 0014 and the widening lint

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0014_locales.{up,down}.sql`
- Modify: `crates/ferrite-store/src/migrate.rs` (`expand_lint` + `widens_unique`, unit tests), `crates/ferrite-store/src/catalogue.rs` (new columns, nullable `translation_group`/`translation_of`), `crates/ferrite-store-tests/src/migrations.rs` (roundtrip + backfill tests), `docs/design/02-storage.md` (§4 rows, §7 layout line, §8 the widening rule).

**Interfaces:**
- Schema per design §3 exactly (column names, index names, `COLLATE "C"` on Postgres text). Domain structs are **not** changed in this task: the stores' row decoders ignore the new columns, so the workspace stays green.
- `widens_unique(stmt, next_stmt, down) -> bool`: SQLite `DROP INDEX x` immediately followed by `CREATE UNIQUE INDEX x ON t (cols…)` / Postgres `ALTER TABLE t DROP CONSTRAINT x, ADD CONSTRAINT x UNIQUE [NULLS NOT DISTINCT] (cols…)`, passing when the new list is a strict superset (order kept, extra columns inserted) of the list the down re-creates under `x`.

- [ ] Tests: `migration_0014_roundtrip` (up, down with `allow_lossy`, up) on both backends; `backfill_uses_site_locale_option` (`"pt_BR.UTF-8"` → `pt-BR`; no option → `en`); `old_style_insert_without_new_columns_works` (an INSERT listing only the 0013 columns succeeds: `''` locale, NULL group); `slug_unique_per_locale` and `translation_unique_per_group` (raw SQL); `down_deletes_non_default_rows_and_restores_indexes`; `catalogue_matches_live_schema` still passes; lint unit tests `widening_unique_passes`, `narrowing_or_same_columns_fails`, `drop_without_recreate_fails`, and the existing migration lint test over every embedded migration passes for 0014.
- [ ] Implement; `cargo test -p ferrite-store -p ferrite-store-sqlite -p ferrite-store-postgres` with `FERRITE_TEST_PG_URL` passes.
- [ ] Commit `feat(store): locales and translation groups schema (0014_locales)`.

### Task 4: Domain fields and store repos

Mechanical but wide: every struct literal of `ContentItem`, `NewContent`, `Term` and `Menu` (about 20 + 13 + 36 + 19 files, tests included) gets the new fields.

**Files:**
- Modify: `crates/ferrite-domain/src/{content.rs, taxonomy.rs, menu.rs, search.rs}`; `crates/ferrite-store/src/repos.rs` (signatures, `LocaleRepo` on `Repos`/`Tx`, `TranslationRef`, `MenuSummary`, `LocaleUsage`); both stores' `repos/{content.rs, taxonomy.rs, menu.rs, search.rs, mod.rs}` and a new `repos/locale.rs` each; `crates/ferrite-store/src/export.rs`/`import_writer.rs` only if they list columns by hand; `crates/ferrite-store-tests/src/{content.rs, taxonomy.rs, menu.rs, search.rs, fixtures.rs}` + new `locale.rs`; every literal across the workspace (tests fill `locale: "en".into()`, `translation_group: id`); create `crates/ferrite-core/src/locales.rs` with only `pub async fn read(repos) -> Result<SiteLocales, CoreError>` (the `site.locales` option, else `SiteLocales::single(bcp47_loose(site.locale option or "en"))`; Task 5 adds the rest), and core's insert paths (`content.rs` `create_in`, `import/{content,terms,menus}.rs`, `seed.rs`, `taxonomy.rs`, `menu.rs`) write `read(tx).default.tag` with `translation_group: None`; updates keep the stored values.

**Interfaces:**
- `ContentItem { …, locale: String, translation_group: ContentId }`, `NewContent { …, locale: String, translation_group: Option<ContentId> /* None = id */ }`, `Term { …, locale: String, translation_group: TermId }`, `Menu { …, locale: String, translation_of: Option<String> }`, `ContentQuery.locale: Option<String>`, `SearchQuery.locale: Option<String>`.
- `ContentRepo::get_by_slug(kind, locale: Option<&str>, parent, slug)`, `group_members(&[ContentId]) -> Vec<TranslationRef>` (≤ 200, `Invalid` beyond), `relocate(id, locale, group, slug, expected_version) -> ContentItem` (version + 1); `TermRepo::get_by_slug(tax, locale: Option<&str>, slug)`, `TermRepo::list(tax, locale: Option<&str>)`, `TermRepo::group_members(&[TermId])`; `MenuRepo::get_translation(name, locale)`, `MenuRepo::list() -> Vec<MenuSummary>`; `LocaleRepo::{usage(locale) -> LocaleUsage, relabel_all(to) -> LocaleUsage, relabel_blank(to) -> LocaleUsage, fill_groups() -> u64}` (media items excluded from `usage`). A read of a NULL group returns the row's id.
- Callers of the changed signatures pass `None` (no filter) so behaviour is unchanged.

- [ ] Contract tests (both backends): `insert_defaults_group_to_id`, `locale_filter_on_list_count_and_file_bounds`, `media_ignores_locale_filter`, `get_by_slug_with_and_without_locale`, `same_slug_two_locales`, `group_members_batches_and_caps`, `relocate_checks_version_and_group_uniqueness`, `term_slug_per_locale`, `menu_translation_lookup`, `locale_usage_counts`, `relabel_all_and_blank`, `fill_groups_sets_null_groups`, `search_locale_filter`.
- [ ] Implement; `cargo test --workspace` (with Postgres) and the golden test pass.
- [ ] Commit `feat(store): locale fields, filters and translation lookups`.

### Task 5: Core locales: settings, settle, reload

**Files:**
- Modify: `crates/ferrite-core/src/locales.rs` (Task 4 created it with `read`)
- Modify: `crates/ferrite-core/src/settings.rs` (`SettingKind::Locales`, `SettingKind::LocaleTexts`, the two keys, module table), `crates/ferrite-core/src/options.rs` (`touches_permalinks` → also `site.locales`; `set_many` in-tx checks call `locales::check_change`; `site.locale` coupling; `invalidate [all]`), `crates/ferrite-core/src/permalink.rs` (`stored_rules` reads `site.locales`; `PermalinkRules { locales }`, `locales()`; no path change yet), `crates/ferrite/src/open.rs` (`startup_check(store, binary, db, data_dir, site_locale: &str)` calls `locales::settle` after migrating), its callers (`serve.rs`, `seed.rs`, `ext_cmd.rs`, `media_cmd.rs`, `import_cmd.rs`, `user_cmd.rs`: one argument each), `crates/ferrite/src/init.rs` (settle after the first migration), `crates/ferrite/src/reload.rs` (`PERMALINK_OPTIONS` gains `site.locales`), `crates/ferrite-http/src/api/dto/site.rs` (the two settings in `SettingsDto`/patch), `openapi.json`.
- Tests: `crates/ferrite-core/tests/locales.rs` (new), `crates/ferrite-core/tests/settings.rs`, `crates/ferrite/tests/startup.rs`, `crates/ferrite/src/reload.rs` unit test, `crates/ferrite-http/tests/api/site.rs`.

**Interfaces:**
- `locales::read(repos: &dyn Repos) -> Result<SiteLocales, CoreError>` (the option, else `single(bcp47_loose(site.locale option or "en"))`), `locales::settle(store: &dyn Store, config_locale: &str) -> Result<SettleReport, CoreError>` (design §2.2 steps 1–4), `locales::check_change(tx, old: &SiteLocales, new: &SiteLocales, rules: &dyn Permalinks) -> Result<Transition, CoreError>` (`Transition::{Unchanged, Rename{from,to}, Reorder, Grow, Shrink(removed)}`), `locales::texts(repos, tag) -> Option<LocaleText>`.

- [ ] Tests: `read_falls_back_to_site_locale_option`, `settle_writes_option_once_and_relabels_single_locale_rows`, `settle_fills_blank_rows_on_multilingual_site`, `settle_is_idempotent_and_emits_all_only_on_change`, `add_locale_with_clashing_prefix_refused` (top-level page `de`, type `book`, taxonomy `genre`, `page`, `2024`), `remove_used_locale_conflicts_with_counts`, `rename_single_locale_relabels`, `reorder_keeps_rows`, `site_locale_alone_on_multilingual_must_match_default`, `locale_texts_keys_must_be_listed`, `locale_write_emits_options_changed_and_all`; startup: `startup_settles_from_config_when_option_unset`; reload: `locales_option_marks_permalinks_dirty`; API: `settings_round_trip_locales`, `openapi_snapshot_is_current`.
- [ ] Implement; tests pass on both backends; golden test green.
- [ ] Commit `feat(core): site locales setting, startup settle and reload`.

### Task 6: Core content: locale on create, translate, relocate, groups

**Files:**
- Modify: `crates/ferrite-core/src/content.rs` (`create_in` takes a `Placement { locale, group }`; `ContentService::create_localized(actor, draft, locale: Option<String>, translation_of: Option<ContentId>)`; `translate`; `relocate(actor, id, expected_version, Relocation { locale: Option<String>, translation_of: Option<Option<ContentId>> })`; slug allocation per locale (`save_with_free_slug`, `root_namespace` and the reserved rule with `Permalinks::locale_prefixes()`); parent and term locale checks; `invalidation_keys` + `group_key`), `crates/ferrite-core/src/content/lifecycle.rs` (transitions add `group:<g>`), `crates/ferrite-core/src/events.rs` (`group_key`, `term_group_key`, grammar table), `crates/ferrite-core/src/permalink.rs` (`Permalinks::locale_prefixes` default empty; `PermalinkRules` returns its prefixes), `crates/ferrite-core/src/content/admin.rs` (`AdminQuery.locale`, translation refs for list rows), `crates/ferrite-core/src/import/content.rs` (`ImportContent.locale`, `translation_group`), `crates/ferrite-core/src/seed.rs` (passes the default).
- Tests: `crates/ferrite-core/tests/content_locales.rs` (new), `tests/events.rs`, `tests/content_admin.rs`.

**Interfaces:**
- `pub struct Translated { pub item: ContentItem, pub notes: Vec<TranslateNote> }`, `TranslateNote::{ParentUntranslated, TermUntranslated(TermId)}`; audit actions `content.translate`, `content.relocate`.
- `AdminQuery.locale: Option<String>`; `ContentAdmin` row gains `translations: Vec<TranslationRef>`.

- [ ] Tests: `create_defaults_to_default_locale`, `create_in_unlisted_locale_refused`, `same_slug_in_two_locales`, `prefix_slug_refused_for_top_level_default_item` (caller slug error, generated `-2`), `translate_copies_fields_seo_and_maps_terms_and_parent`, `translate_notes_untranslated_parent_and_terms`, `translate_twice_conflicts_with_existing_id`, `contributor_translates_published_post_as_own_draft`, `translate_unreadable_source_not_found`, `relocate_published_item_stores_redirect`, `relocate_with_children_refused`, `join_group_refuses_taken_locale`, `leave_group_becomes_own_group`, `terms_of_other_locale_refused_on_multilingual_site`, `writes_emit_group_keys` (old and new group on a group change), `admin_list_filters_by_locale_and_returns_translations`.
- [ ] Implement; tests pass; golden test green.
- [ ] Commit `feat(core): content locales, translate and translation groups`.

### Task 7: Core terms and menus per locale

**Files:**
- Modify: `crates/ferrite-core/src/taxonomy.rs` (`create_term_in`, `translate_term`, `relocate_term`, `list_terms(taxonomy, locale)`, term events + `tgroup:`), `crates/ferrite-core/src/menu.rs` (`put` validates `locale` listed and `translation_of` names an existing default-locale menu and is free for the locale; `for_location(name, locale)`; `list` returns `MenuSummary`; `menu_locations` per locale), `crates/ferrite-core/src/import/{terms.rs, menus.rs}` (locale fields).
- Tests: `crates/ferrite-core/tests/taxonomy_locales.rs` (new), `crates/ferrite-core/tests/menu_locales.rs` (new).

**Interfaces:**
- `TaxonomyService::create_term_in(actor, input, locale: Option<String>, translation_of: Option<TermId>) -> Term`, `translate_term(actor, id, locale) -> Term`; `MenuService::for_location(&self, name, locale: &str) -> Result<Option<Menu>, CoreError>` (translation, else the default menu).

- [ ] Tests: `term_slug_shared_across_locales`, `term_parent_must_share_locale`, `translate_term_maps_parent`, `term_events_carry_group_key`, `menu_translation_must_point_at_default_menu`, `menu_translation_unique_per_locale`, `for_location_falls_back_to_default_menu`, `deleting_default_menu_keeps_translations_listed`.
- [ ] Implement; tests pass; golden test green.
- [ ] Commit `feat(core): terms and menus per locale`.

### Task 8: Permalinks with prefixes and the localized router

**Files:**
- Modify: `crates/ferrite-core/src/permalink.rs` (`path()` prefixes non-default locales; `term_path_in(prefix, taxonomy, slug)`; `check_types` unchanged; doc comment), `crates/ferrite-http/src/route.rs` (`RouteTable` holds prefixes from the rules; `pub struct RouteLocale { pub tag: Option<String> }`; `pub fn resolve_localized(raw_path, raw_query, table) -> (RouteLocale, Route)`; module docs), `crates/ferrite-http/src/route/tests.rs` (new cases; existing ones untouched), `crates/ferrite-core/tests/permalink.rs`.

**Interfaces:**
- `resolve_localized` per design §4: prefix match → inner `resolve` on the rest (`/` when empty) → `Canonical` targets re-prefixed; files, `_`-routes and health under a prefix → `NotFound`; no prefix → `RouteLocale { tag: None }` and `resolve`'s result unchanged.

- [ ] Tests: `path_prefixes_non_default_locale` (post, page, child page, custom type, `%year%` structure), `term_path_in_prefixes`, `default_locale_path_unchanged`; router `prefix_routes_content_listing_feed_search`, `prefix_canonicalises_case_and_slash`, `prefix_with_files_or_internal_routes_is_404`, `unknown_two_letter_segment_is_content`, `no_locales_means_resolve_unchanged` (property: for 200 generated paths `resolve_localized(p) == (None, resolve(p))` with an empty prefix list).
- [ ] Implement; `cargo test -p ferrite-core -p ferrite-http` passes; golden test green.
- [ ] Commit `feat(routing): locale prefixes in permalinks and the router`.

### Task 9: SiteReader per locale

**Files:**
- Modify: `crates/ferrite-core/src/site.rs`, `crates/ferrite-core/src/site/{model.rs, hydrate.rs, tree.rs}` (`ReadLocale`; locale-scoped reads; `translations`, `term_translations`; front page per locale; `PublicItem.locale`; `SitemapEntry.alternates`; deps `group:`/`tgroup:`/`option:site.locales`), every caller in `crates/ferrite-http/src/app/*.rs` and `crates/ferrite-http/src/mcp`/`api` that calls a changed reader method (pass `ReadLocale::All` for now), `crates/ferrite-core/tests/{site_reader.rs, site_tree.rs, site.rs}` (calls updated), new `crates/ferrite-core/tests/site_locales.rs`.

**Interfaces:**
- `pub enum ReadLocale { All, Only(String) }`; `pub struct TranslationLink { pub locale: String, pub label: String, pub url: String, pub title: String, pub current: bool }`; `SiteReader::translations(&self, item: &PageItem) -> Result<SiteRead<Vec<TranslationLink>>, CoreError>`; `SiteReader::front_page(&self, loc: &ReadLocale, page)`; `SiteReader::is_front_page(&self, item)` true for the front page's translations too; the other methods named in design §5 take `loc` first.

- [ ] Tests: `all_issues_no_locale_predicate` (`CountingStore` / captured `ContentQuery` has `locale: None`), `only_filters_singles_listings_search_feeds_archives`, `term_archive_finds_term_by_locale`, `front_page_per_locale_and_fallback_to_listing`, `translations_lists_public_members_in_site_order`, `sitemap_entries_carry_alternates_only_when_multilingual`, `related_and_tree_stay_in_item_locale`, `translation_reads_record_group_key`.
- [ ] Implement; tests pass; golden test green.
- [ ] Commit `feat(core): locale-scoped site reads, translations and sitemap alternates`.

### Task 10: Public site wiring: head, site info, feeds, sitemaps

**Files:**
- Modify: `crates/ferrite-http/src/app/{handler.rs, pipeline.rs, state.rs, feeds.rs, bridge.rs, error_page.rs, comment_form.rs}` (use `resolve_localized`; build `ReadLocale` from `RouteLocale` and the table — `All` when the table has no prefixes; `site_info(theme, locale, deps)` with `locales`, `default_locale`, per-locale texts; `PluginRequest` gets the request locale; feeds' `<language>`, links, channel titles; sitemaps' `xmlns:xhtml` + `xhtml:link` only when multilingual; previews in the item's locale; `BlockingBridge::new(…, loc)`), `crates/ferrite-http/src/head.rs` (`HeadPage.alternates`; hreflang + `x-default`), `crates/ferrite-theme/src/render.rs` (`SiteInfo { locales, default_locale }` fields only; no helper yet).
- Tests: new `crates/ferrite-http/tests/app/locales.rs` (+ `mod` in `tests/app/main.rs`), `crates/ferrite-http/src/head/tests.rs`, `crates/ferrite-http/tests/app/feeds.rs`; cluster: `crates/ferrite/tests/cluster` gains `locales_reload_on_every_node` (behind `FERRITE_TEST_PG_URL`).

**Interfaces:**
- `HeadPage { item, path, alternates: &[Alternate] }`, `pub struct Alternate { pub hreflang: String, pub href: String }`.

- [ ] Tests: `de_post_served_under_prefix_with_html_lang_and_hreflang`, `default_post_has_x_default_and_de_alternate`, `missing_translation_is_404`, `de_front_page_is_translation_of_front_page`, `de_feed_lists_de_items_with_language_de`, `sitemap_alternates_and_namespace_only_when_multilingual`, `locale_texts_title_in_head_and_feed`, `plugin_request_info_carries_page_locale` (a fixture plugin echoing `request-info.locale`), `adding_translation_refreshes_cached_original` (cache flush through `group:`), `head_unchanged_without_alternates`; cluster `locales_reload_on_every_node`; golden test green.
- [ ] Implement; tests pass.
- [ ] Commit `feat(http): localized pages, hreflang, feeds and sitemaps`.

### Task 11: Theme helpers and theme strings

**Files:**
- Modify: `crates/ferrite-theme/src/{render.rs, bridge.rs, manifest.rs, page_tree.rs}` (`translations(post)`, `locale_links(post=none)`, `t(text)`; `HelperBridge::translations` default empty; `ThemeManifest.strings` with caps; the render's locale for `t` and memo keys; module docs and dependency-key list), `crates/ferrite-http/src/app/bridge.rs` (`translations`, menus via `MenuService::for_location`, query/related/tree in the render's locale), `docs/design/06-themes-rendering.md` (§4 helper table, manifest `[strings]`, keys).
- Tests: `crates/ferrite-theme/tests/theme/{render.rs, manifest.rs}`, `crates/ferrite-http/tests/app/locales.rs` (helpers end to end).

**Interfaces:**
- Template values per design §5; `t` lookup exact tag → language subtag → text; manifest caps 32 tables × 500 strings × 1000 characters (`ThemeError::Manifest` beyond).

- [ ] Tests: `t_falls_back_tag_language_text`, `strings_caps_refused`, `t_output_is_escaped`, `translations_helper_memoised_and_counted_against_bridge_budget`, `locale_links_marks_current_and_untranslated`, `menu_in_de_uses_translation_then_default`, `query_and_page_tree_in_render_locale`; golden test green.
- [ ] Implement; tests pass.
- [ ] Commit `feat(theme): translations, locale_links and translatable theme strings`.

### Task 12: Starter themes and the multilingual demo

**Files:**
- Modify: `themes/{blog,docs,business}/templates/base.html` (switcher include inside `{%- if site.locales | length > 1 %}`), new `themes/{blog,docs,business}/templates/partials/languages.html`, user-visible strings wrapped in `t()` across the three themes' templates, `themes/*/theme.toml` (`[strings.de]`, plus `version` bump), `themes/*/assets/*.css` (switcher styles, 44 px targets); `crates/ferrite/src/seed.rs` + `crates/ferrite/src/seed/` (`--profile multilingual`: blog content in `en` and `de`, translated menu `primary-de`, translated front page and categories); `admin/e2e/public-themes.spec.ts` (axe on `/de/` pages).
- Tests: `crates/ferrite/tests/serve/themes.rs` (`multilingual_demo_serves_both_locales`), golden test (must stay green: whitespace control), `pnpm -C admin e2e --project public-themes-chromium`.

- [ ] Write the tests; implement; golden test, serve tests and the axe project pass.
- [ ] Commit `feat(themes): language switcher and translated strings in the starter themes; seed demo --profile multilingual`.

### Task 13: REST API

**Files:**
- Modify: `crates/ferrite-http/src/api/{content.rs, taxonomies.rs, menus.rs, meta.rs, mod.rs}`, `crates/ferrite-http/src/api/dto/{content.rs, site.rs}` (+ term and menu DTOs where they live), `openapi.json`, `admin/src/api/schema.d.ts` (`pnpm -C admin api:gen`), `docs/design/07-admin-api.md` (§1 endpoint rows).
- Tests: new `crates/ferrite-http/tests/api/locales.rs` (+ `mod` in `tests/api/main.rs`).

**Interfaces:**
- Design §7: DTO fields, `?locale=` filters (unknown → `400` problem `invalid-locale`), `POST /content/{type}/{id}/translations` (`201`, `Location`, `notes`), `POST /taxonomies/{tax}/terms/{id}/translations`, `PATCH` `locale` / `translation_of`, `MenuDto` fields, `/meta` `default_locale` + `locales`.

- [ ] Tests: `create_with_locale_and_translation_of`, `list_and_counts_filter_by_locale`, `translate_endpoint_201_location_notes`, `translate_conflict_names_existing`, `patch_locale_moves_permalink`, `patch_translation_of_null_leaves_group`, `terms_and_menus_carry_locale`, `meta_lists_locales_without_auth`, `contributor_can_translate_but_not_publish`, `openapi_snapshot_is_current`.
- [ ] Implement; `cargo test -p ferrite-http --test api` passes; `pnpm -C admin typecheck` passes with the new schema.
- [ ] Commit `feat(api): locales on content, terms and menus; translate endpoints`.

### Task 14: MCP tools

**Files:**
- Modify: `crates/ferrite-http/src/mcp/tools/{content.rs, terms.rs, menus.rs, site.rs, settings.rs}`, `crates/ferrite-http/src/mcp/{prompts.rs, server.rs}` (`translate` prompt; instructions name the locales when several), `site/content/05-reference/02-mcp-tools.md` (the new tool row), `docs/design/12-mcp.md` (§5.2 list, counts).
- Tests: `crates/ferrite-http/tests/api/{mcp_content.rs, mcp_structure.rs, mcp_reference.rs}`.

**Interfaces:**
- `content_translate { type, id, locale }` → `{ item (with body_markdown), notes }`; `locale` / `translation_of` on create/update/list/get; terms and menu tools take `locale`.

- [ ] Tests: `translate_then_update_markdown`, `list_filters_by_locale`, `get_reports_translations`, `term_create_in_locale`, `menu_set_translation`, `site_overview_lists_locales`, `prompts_list_has_translate`, `reference_page_lists_every_tool`.
- [ ] Implement; tests pass.
- [ ] Commit `feat(mcp): locale-aware content tools and content_translate`.

### Task 15: Admin — Languages settings, content list, editor

**Files:**
- Create: `admin/src/settings/languages.tsx` (+ test), `admin/src/content/locale-filter.tsx`, `admin/src/content/locale-chips.tsx`, `admin/src/editor/panel/translations-section.tsx` (+ tests), `admin/src/lib/locales.ts` (the `/meta` list as a query hook, tag validation mirroring `parse_tag`), `admin/e2e/multilingual.spec.ts`
- Modify: `admin/src/settings/groups.ts` (group `languages`, icon), `admin/src/settings/fields.ts`, `admin/src/content/{filters.tsx, list-columns.tsx, list-cells.tsx, status-tabs.tsx}`, `admin/src/routes/_app/content.$type.index.tsx` (search param `locale`), `admin/src/editor/panel/side-panel.tsx`, `admin/src/editor/editor-state.ts` (locale in state), `admin/src/i18n/en/{content.json, editor.json, admin.json}`.

**Interfaces:**
- Design §6. Locale UI hidden when `/meta` lists one locale.

- [ ] Vitest: `languages_validates_tags_and_blocks_removing_used_locale`, `default_change_confirms`, `locale_filter_persists_per_type`, `chips_show_status_and_translate_missing`, `translations_section_translate_opens_new_draft_with_notes`, `link_and_unlink`, `single_locale_renders_no_locale_controls`.
- [ ] E2E `multilingual.spec.ts` (Chromium, both backends): add `de` in Settings → Languages, translate a post from the list chip, edit and publish it, see `/de/<slug>/` with the switcher, filter the list by `de`, try removing `de` (blocked with counts); axe clean on the new screens.
- [ ] `pnpm -C admin typecheck && pnpm -C admin lint && pnpm -C admin test` and the spec pass.
- [ ] Commit `feat(admin): languages settings, locale filter and translations in the editor`.

### Task 16: Admin — taxonomies and menus per locale

**Files:**
- Modify: `admin/src/structure/taxonomies/{taxonomy-screen.tsx, term-table.tsx, term-tree.tsx, term-form.tsx, terms-model.ts}`, `admin/src/structure/menus/{menus-list.tsx, menu-editor.tsx, menu-model.ts}`, `admin/src/i18n/en/structure.json`, tests next to them; `admin/e2e/{taxonomies.spec.ts, menus.spec.ts}` (new cases).

- [ ] Vitest: `term_tree_filters_by_locale`, `translate_term_prefills_form`, `location_lists_menu_per_locale`, `create_locale_menu_copies_items`.
- [ ] E2E: translate a category, create `primary-de`, see it on `/de/`.
- [ ] Gates as in Task 15; commit `feat(admin): terms and menus per locale`.

### Task 17: WordPress import keeps languages

**Files:**
- Create: `crates/ferrite-import-wp/src/lang.rs` (Polylang and WPML readers), `crates/ferrite-import-wp/tests/corpus/site-c.xml` + `site-c.php` (a scratch WordPress with Polylang (free) and two languages, built and exported like sites A and B; `README.md` row), `crates/ferrite-import-wp/tests/corpus/wpml-shape.xml` (hand-written from WPML's documented `_wpml_import_*` meta; marked synthetic in the README), `crates/ferrite-import-wp/tests/languages.rs`
- Modify: `crates/ferrite-import-wp/src/pass/{types.rs (NOT_TAXONOMIES), content.rs, terms.rs, options.rs}`, `src/report.rs` (`languages` section, `locale_not_on_site`, `term_locale_mismatch`, `invalid_locale`), `src/run.rs` (`default_locale` option), `crates/ferrite/src/import_cmd.rs` (`--default-locale`), `crates/ferrite-http/src/api/import.rs` + DTO (`options.default_locale`), `openapi.json`, `docs/design/08-wp-import.md`.

**Interfaces:**
- `lang::item_language(item) -> Option<Result<String, String>>`, `lang::groups(terms, items) -> Vec<WpGroup { members: Vec<(wp_id, locale)> }>` (Polylang `post_translations` via `php.rs`; WPML `_wpml_import_translation_group`); groups resolved after pass 3 in the fix-up (ids known), writing `translation_group` through `ImportBatch`.

- [ ] Before coding: build `site-c` with Polylang, export, and record in the README what the file actually holds for items, `post_translations` terms and term languages (design §8.1 marks these unconfirmed); adjust `lang.rs` to the real shape.
- [ ] Tests: `polylang_languages_and_groups_imported`, `polylang_taxonomies_not_registered`, `wpml_meta_languages_and_groups`, `plain_wpml_export_imports_in_default_locale`, `unknown_language_added_to_site_locales`, `no_site_options_skips_unlisted_locales`, `terms_take_items_locale_or_default_with_report`, `rerun_keeps_groups_and_edited_items`, hostile: `post_translations_hostile_descriptions` (oversize, nested, non-int ids, cycle, duplicate locale).
- [ ] Implement; `cargo test -p ferrite-import-wp` and `cargo test -p ferrite --test import_cli` pass.
- [ ] Commit `feat(import-wp): keep Polylang and WPML languages and translation groups`.

### Task 18: Markdown import locales

**Files:**
- Modify: `crates/ferrite-import-md/src/{front.rs, scan.rs, run.rs}` (`locale`, `translation_of`, `--locale`; dependency order; problem kind `locale`), `crates/ferrite/src/import_cmd.rs` (`--locale`), `crates/ferrite-import-md/tests/` (new cases), `site/content/02-guides/07-import-markdown.md` (front matter keys).

- [ ] Tests: `front_matter_locale_and_translation_of`, `cli_locale_default`, `unknown_locale_is_problem_exit_2`, `translation_of_outside_import_is_problem`, `rerun_keeps_groups`.
- [ ] Implement; `cargo test -p ferrite-import-md` and `cargo test -p ferrite --test import_md` pass.
- [ ] Commit `feat(import-md): locale and translation_of front matter`.

### Task 19: Docs

**Files:**
- Create: `site/content/02-guides/09-multilingual.md` (settings, translating, themes' `translations`/`locale_links`/`t`, URLs and SEO, import, MCP "translate this post"), screenshots in `site/content/images/` (Languages settings, editor Translations section; WebP 2×, from the multilingual demo)
- Modify: `site/content/02-guides/index.md`, `site/content/02-guides/04-themes.md` (helpers), `site/content/02-guides/06-import-wordpress.md`, `site/content/02-guides/08-ai-assistants.md` (translate prompt), `site/content/03-developers/03-rest-api.md`, `site/VERIFIED.md` (one row per claim checked), `docs/design/00-overview.md` (row 13), `docs/design/01-architecture.md` (crate map: `ferrite_core::locales`, `ferrite_domain::locale`), `docs/operations/` rollback runbook (a multilingual database needs `rollback --schema`; design §3), `README.md` (one line in the feature list).

- [ ] Every command in the guide run against the multilingual demo; `cargo test -p ferrite --test docs_site` passes.
- [ ] Commit `docs: multilingual sites guide and references`.

### Task 20: Exit

- [ ] Full gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` with `FERRITE_TEST_PG_URL`, `FERRITE_TEST_PG_URL=… cargo test -p ferrite --test cluster`, admin typecheck/lint/test, E2E (Chromium on both backends; `multilingual.spec.ts` also in Firefox and WebKit), `public-themes-chromium`, the docs build.
- [ ] Manual checks, recorded with their output: a v0.2.0 binary against a one-locale database at schema 14 (`migrate status` exit 0, `serve`, `/` 200; `.github/scripts/compat.sh` locally), then the new binary again (settle heals the old binary's rows); the multilingual demo validated with an external hreflang/sitemap validator if one is reachable (else say so); a real Polylang export (site-c) imported end to end; WPML: state that only the documented shape was tested.
- [ ] Write `docs/superpowers/m6-1-exit.md` (what was built, what was verified, what was not), rulings in `docs/superpowers/rulings/m6.md` (section M6-1), tick the M6-1 items in `docs/ROADMAP.md`, add the follow-ups (design §13, `post.locale` to the plugin world gaps list), update `CLAUDE.md` (status paragraph: M6-1, migration `0014_locales`, 57 tools / 5 prompts; how to add a locale locally).
- [ ] Commit `docs: M6-1 exit`.
