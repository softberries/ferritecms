# M6-1 exit: multilingual content

**Criterion** ([ROADMAP](../ROADMAP.md), M6-1; design [13](../design/13-multilingual.md);
plan [`plans/2026-11-02-m6-1-multilingual.md`](plans/2026-11-02-m6-1-multilingual.md);
ruling P6 in [`rulings/m6.md`](rulings/m6.md)): a site can hold its content in several
locales, with translations linked, locale-prefixed URLs, `hreflang`, sitemaps and feeds per
locale, theme helpers and a switcher, the API, MCP and admin working with locales, and the
imports keeping languages, while a site with one locale behaves exactly as before.

**Result: done** (2026-11-03, branch `m6-1-multilingual`, cut from `m6` at 1cbef9c,
`m6` merged in at 8d207ad9 with M6-2 and M6-6; migration `0014_locales`). Plan Task 17
(the WordPress import) was built and tested against hand-written files only (below).
A second round after the merge added Task 16, `locale` on the MCP term and menu tools,
webhook payloads and events for the M6-1 write paths, and the fixes of the independent
review (R-1-21 to R-1-28).

## What was built

| Area | Delivered |
|---|---|
| Golden test (Task 1) | `crates/ferrite/tests/serve/single_locale.rs`: the public output of the `blog`, `business` and `docs` starter themes on a one-locale site (home, items, archives, search, feeds, sitemaps, 404s; ids, dates and hashes normalised) against fixtures in `tests/fixtures/golden-single-locale/`, recorded before any locale code; green after every task |
| Domain | `ferrite_domain::locale`: `parse_tag` (BCP 47 subset: language, script, region; canonical spelling), `prefix`, `LocaleDef`, `SiteLocales` (1–20, the default first, labels ≤ 50) |
| Storage | `0014_locales` on both backends: `locale` + `translation_group` on `content_items` and `terms`, `locale` + `translation_of` on `menus`, backfill to the site's locale, slug uniques widened by locale, unique (group, locale); lossy down (deletes non-default rows); the expand lint's `widens_unique` rule; `LocaleRepo` (usage, relabel, group fill), `group_members`, `relocate`, per-locale `get_by_slug`/`slugs_with_prefix`/term lists; contract tests on both |
| Core | `site.locales` and `site.locale_texts` settings (`plan_locales`, `check_change`: a locale that holds content cannot be removed, prefix clashes refused), `locales::settle` at startup under the migrate lock, reload on change; content `create_localized`, `translate` (draft copy with notes), `relocate`; terms `create_localized_term`, `translate_term`; menus per locale (`for_location`); locale-scoped `SiteReader::in_locale`, `translations`, sitemap alternates, page tree per locale |
| Routing and public site | prefixes in permalinks for non-default locales, the router resolving `/<prefix>/…`, per-locale front page, pagination, search, archives, feeds (`/de/feed/`), sitemaps with `xhtml:link` alternates, `hreflang` + `x-default` in the head, `<html lang>`, no fallback (404), `robots.txt` at the root only; cache keys `group:` / `tgroup:` |
| Themes | `site.locales`, `translations()`, `locale_links()`, `t()` with `[strings.<tag>]` (capped); a switcher partial and German strings in `blog`, `business`, `docs`; `seed demo --profile multilingual` |
| API | `/meta` lists locales; content `locale`, `translation_group`, `translations`, create/PATCH `locale` + `translation_of`, `POST /content/{type}/{id}/translations`, `?locale=` on lists and counts (`400 invalid-locale`); terms `locale`, `translation_group`, `?locale=`, `POST /terms/{id}/translations`; menus `locale`, `translation_of`; OpenAPI regenerated |
| MCP | `content_translate` (57 tools), the `translate` prompt (5 prompts), `locale` on `content_list`/`content_create`/`content_update`/`terms_list`/`term_create`/`menu_set` (and `translation_of` on the last three), locale and translation on `menu_get`, locales in `site_overview`; reference page and design 12 updated |
| Webhooks (M6-6) | content payloads carry `locale` and `translation_group`; translate and the localized create emit `content.created`, relocate `content.updated`; design 18 and the webhooks guide updated |
| Admin | Settings → Languages (add by tag in core's spelling, labels, make default with a confirmation, remove; the server's `409` shown); content list language filter and Language column; editor Translations section (links, "Translate into …") and term pickers in the item's locale; the taxonomy screen per locale with term translations; menus per location and locale, "Translate into …" in the menu editor (a copy of the entries as `<name>-<prefix>`), the editor keeping a menu's locale and translation on save; hidden on a one-locale site |
| Imports | WordPress: Polylang `language` terms and `post_translations` groups, WPML Export and Import meta; language taxonomies not registered; unlisted languages skipped and reported; groups joined in the fix-up. Markdown: `locale`, `translation_of` front matter, `--locale` |
| Docs | guide `02-guides/10-multilingual.md` (checked against the demo, rows in `site/VERIFIED.md`), links from Themes, Import from WordPress, Import Markdown, AI assistants, REST API, MCP tools; design 00, 01, 02, 07, 08, 12, 13 (§14 "As built"); the rollback runbook; README; CLAUDE.md |

## Verified

After the merge of `m6` and the second round (head of the branch):

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`: clean.
- `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace
  --no-fail-fast` (Postgres 17): 4 788 passed, 1 failed, 0 SKIP. The failure is
  `import_cli::import_site_a_end_to_end_sqlite`, which binds `127.0.0.1:8080` and found it
  taken by a FerriteCMS the user runs on this machine (`cannot listen on 127.0.0.1:8080`);
  it fails the same way on every run here since before the importer changed. The cluster
  integration test ran in that run: 14 passed.
- The golden one-locale test (`serve::single_locale`) passed after every task and after
  the merge.
- OpenAPI snapshot current (`openapi_snapshot_is_current`, regenerated after the merge);
  `pnpm -C admin api:check` current (`schema.d.ts` regenerated).
- Admin: `typecheck`, `lint` (eslint + prettier), Vitest 143 files / 881 tests, `build` ok.
- E2E (release build of the branch head, Chromium on SQLite and Postgres):
  `multilingual.spec.ts` (both tests), `taxonomies`, `menus`, `content-list`, `editor`,
  `editor-publish`, `settings` and `webhooks`: 100 passed, 0 failed. Before the merge the
  whole Chromium suite passed (402) and `multilingual.spec.ts` (its first test) passed in
  Firefox and WebKit; neither was re-run after the merge.
- `pnpm -C admin e2e --project public-themes-chromium`: 18 passed (before the merge;
  one-locale demo sites).
- `cargo test -p ferrite --test docs_site`: passed (in the workspace run).
- The multilingual demo, run by hand before the merge (release build, `seed demo
  --profile multilingual`, `serve`, `curl`): `/de/` 200 with `<html lang="de">` and the
  switcher, `/de/about/` with `hreflang` en/de/x-default, `/de/feed/` with
  `<language>de</language>`, sitemaps with `xhtml:link` alternates, `/de/robots.txt` 404,
  an untranslated post under `/de/` 404, `/api/v1/meta` listing both locales (rows in
  `site/VERIFIED.md`).

## Not verified

- **A real Polylang or WPML export.** No WordPress with Polylang was run (plan Task 17's
  first step, building `site-c` from a scratch WordPress, was not possible here); the
  importer was written to design 13 §8.1's documented shapes and tested with the
  hand-written `tests/corpus/polylang-shape.xml` and `wpml-shape.xml` (marked synthetic in
  the corpus README). Whether a real Polylang export carries `post_translations`
  descriptions as assumed is unconfirmed.
- **A v0.2.0 binary against a schema-14 database** (plan Task 20's manual check,
  `.github/scripts/compat.sh`): not run. `0014_locales` is expand-only (the lint passes), so
  an older binary should open it, but that was not tried; nor was settle healing the rows
  an old binary writes after the new one runs again (covered only by the store tests of
  blank-locale rows).
- **An external hreflang or sitemap validator**: not run (no network validator used); the
  output was checked by tests and by reading it.
- **axe on a multilingual public page**: `public-themes-chromium` runs on one-locale demo
  sites, so the language switcher's markup was not checked by axe.
- **E2E in Firefox and WebKit** for every spec: only `multilingual.spec.ts`'s first test was
  run there, before the merge; the whole Chromium suite was not re-run after it (the
  touched specs were).
- **Screenshots** for the guide (Settings → Languages, Translations): not made.
- **`rollback --schema` across `0014`**: the runbook's statement is read from the code
  (`-- ferrite:lossy-down`), not run.
- **Intermediate commits** were not each built on their own (Tasks 10–12 and 13–14 were
  split into two commits after both were written).

## Decisions (R-1-n)

- **R-1-1** The `docs` theme's golden output imports a fixture folder
  (`tests/fixtures/golden-single-locale/docs-src/`) instead of the docs site, so the
  fixture does not change when the docs do.
- **R-1-2** The v0.2.0 SQLite index name of the term slug unique is mapped in
  `constraint.rs` so both the old and the widened index give the same `slug` conflict.
- **R-1-3** `TranslationRef` also carries `title` and `author_id` (the admin filters group
  members by `can_read` and shows titles without another read).
- **R-1-4** `ferrite init` does not settle (`import-site` needs the database empty); the
  first `startup_check` does.
- **R-1-5** Plan Tasks 6 and 7 (content and terms/menus) landed as one commit: they share
  the placement and group code.
- **R-1-6** `SiteReader` keeps its signatures; `in_locale(ReadLocale)` gives a scoped
  reader (design 13 §14).
- **R-1-7** Tasks 10–12 (theme helpers, HTTP rendering, starter themes and the demo) are
  one commit, Tasks 13–14 (API, MCP) another.
- **R-1-8** `CoreError::InUse` → `409 conflict`.
- **R-1-9** `Permalinks::locale_prefixes(kind)` reserves prefixes only where top-level
  slugs share the root; type and taxonomy names may not equal a prefix.
- **R-1-10** The starter themes' versions are not bumped (the release bumps them).
- **R-1-11** No `hreflang` on term archives (moved to design 13 §13).
- **R-1-12** `slugs_with_prefix` takes the locale (slug allocation per locale).
- **R-1-13** The event-key test helpers ignore `group:`/`tgroup:` keys, which every content
  write now adds.
- **R-1-14** Deleting `site.locale` on a one-locale site leaves `site.locales` as it is.
- **R-1-15** A menu written with an empty locale keeps its stored locale (a new one takes
  the default): callers that know nothing of locales (the M5 `menu_set` tool) keep working.
- **R-1-16** `PATCH` with a new `locale` or `translation_of` saves the edit, then
  relocates (two versions). Superseded by R-1-26: one transaction.
- **R-1-17** The WordPress import never adds languages to the site: items in an unlisted
  language are skipped and reported (`locale_not_on_site`); design 13 §8.1 had them added.
  `--default-locale`, the report's `languages` section and term locales are not built.
- **R-1-18** Imports join translation groups after writing the items
  (`ImportBatch::join_group`, which refuses an item edited since the import wrote it), not
  by writing in dependency order; Markdown reports what cannot join as `translation-of`.
- **R-1-19** (Superseded.) Plan Task 16 and `locale` on the MCP term and menu tools were
  first left out, then built in the second round: the taxonomy screen shows one locale at
  a time (`LocaleTabs`); a menu translation is made from the editor of the default-locale
  menu as `<name>-<prefix>` with a copy of the entries under fresh ids (entry ids are
  unique across menus); `menu_set` keeps `locale` and `translation_of` when they are left
  out (`null` removes the link).
- **R-1-20** `POST /content/{type}/{id}/translations` answers `{item, notes}`
  (`TranslationCreated`); an MCP `content_update` with `translation_of: null` leaves the
  group as it is.

- **R-1-21** Webhooks (R-M6-6-8): content payloads gain `locale` and `translation_group`
  (additive, payload version 1). `translate` and `create_localized` emit `content.created`,
  `relocate` `content.updated`. `translate_term` emits nothing (the catalogue has no term
  events) and `join_group` nothing (import writes emit only `import.finished`).
- **R-1-22** (Review 1.) A menu translation's put and delete also emit `menu:<of>` for the
  menu it translates now and before, so pages of that locale that showed the default menu
  are flushed. `menu::put_in` returns the keys; the import and the seed use them too.
- **R-1-23** (Review 2, 5.) Whether an item is its locale's front page is decided by the
  front page's translation group whichever member is the original (on a locale-scoped
  reader only, so a one-locale page view reads nothing more); the sitemap lists every
  member of that group at its locale's root.
- **R-1-24** (Review 3.) A change of the default locale checks every prefix (not only new
  ones) against the new default's top-level slugs and refuses with the item named.
- **R-1-25** (Review 4.) Media is shared by every locale: a change of the default locale
  relabels media to it (`LocaleRepo::relabel_media`, contract-tested), and the down of
  `0014_locales` never deletes media rows (`type <> 'media'` on both backends; the file
  changed before any release carries it).
- **R-1-26** (Review 6.) `PATCH` (and MCP `content_update`) with a locale or group move is
  one transaction (`ContentService::update_and_relocate`); a refused move saves nothing.
- **R-1-27** (Review 7.) `mcp`, `oauth`, `.well-known` and `favicon.ico` join
  `slug::RESERVED` (with `admin`, `api`, `feed`, `metrics`, `_p` and the others already
  there); a locale whose prefix is one of them is refused too.
- **R-1-28** The merge of `m6`: migration tests use `latest()` / `up_from` /
  `down_from` (M6-2's folder-driven helpers) and revert the lossy `0015` first where they
  step below it; the user guide became `10-multilingual.md` (M6-6's is `09-webhooks.md`).

## For the merge into `m6`

`m6` at 8d207ad9 (M6-2, M6-6) is merged into this branch, so it merges forward cleanly
unless `m6` moved since. `openapi.json` and `schema.d.ts` are generated: regenerate them
on any later conflict.
