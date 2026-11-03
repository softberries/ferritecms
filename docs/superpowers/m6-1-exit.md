# M6-1 exit: multilingual content

**Criterion** ([ROADMAP](../ROADMAP.md), M6-1; design [13](../design/13-multilingual.md);
plan [`plans/2026-11-02-m6-1-multilingual.md`](plans/2026-11-02-m6-1-multilingual.md);
ruling P6 in [`rulings/m6.md`](rulings/m6.md)): a site can hold its content in several
locales, with translations linked, locale-prefixed URLs, `hreflang`, sitemaps and feeds per
locale, theme helpers and a switcher, the API, MCP and admin working with locales, and the
imports keeping languages, while a site with one locale behaves exactly as before.

**Result: done, with the reductions listed below** (2026-11-03, branch
`m6-1-multilingual`, cut from `m6` at 1cbef9c; 15 commits; migration `0014_locales`). Plan
Task 16 (the admin's taxonomy and menu screens per locale) was not built, and Task 17 (the
WordPress import) was built and tested against hand-written files only.

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
| MCP | `content_translate` (57 tools), the `translate` prompt (5 prompts), `locale` on `content_list`/`content_create`/`content_update`, locales in `site_overview`; reference page and design 12 updated |
| Admin | Settings → Languages (add by tag in core's spelling, labels, make default with a confirmation, remove; the server's `409` shown); content list language filter and Language column; editor Translations section (links, "Translate into …"); hidden on a one-locale site |
| Imports | WordPress: Polylang `language` terms and `post_translations` groups, WPML Export and Import meta; language taxonomies not registered; unlisted languages skipped and reported; groups joined in the fix-up. Markdown: `locale`, `translation_of` front matter, `--locale` |
| Docs | guide `02-guides/09-multilingual.md` (checked against the demo, rows in `site/VERIFIED.md`), links from Themes, Import from WordPress, Import Markdown, AI assistants, REST API, MCP tools; design 00, 01, 02, 07, 08, 12, 13 (§14 "As built"); the rollback runbook; README; CLAUDE.md |

## Verified

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`: clean.
- `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace
  --no-fail-fast` (Postgres 17): 4 401 passed, 1 failed, 0 SKIP. The failure is
  `import_cli::import_site_a_end_to_end_sqlite`, which binds `127.0.0.1:8080` and found it
  taken by a FerriteCMS the user runs on this machine (`cannot listen on 127.0.0.1:8080`);
  it failed the same way before any M6-1 change touched the importer. The cluster
  integration test ran in that run: 13 passed.
- The golden one-locale test (`serve::single_locale`) passed after every task.
- OpenAPI snapshot current (`openapi_snapshot_is_current`); `pnpm -C admin api:check`
  current.
- Admin: `typecheck`, `lint` (eslint + prettier), Vitest 137 files / 836 tests,
  `bundle:check` ok, `build` ok.
- E2E (`pnpm -C admin e2e`, Chromium on SQLite and Postgres, release build of this
  branch): 402 passed, 0 failed, `multilingual.spec.ts` included (it is a site-wide spec).
  `multilingual.spec.ts` also passed in Firefox and WebKit (SQLite).
  `pnpm -C admin e2e --project public-themes-chromium`: 18 passed (one-locale demo sites).
- `cargo test -p ferrite --test docs_site`: 4 passed (the docs build with the new guide).
- The multilingual demo, run by hand (release build, `seed demo --profile multilingual`,
  `serve`, `curl`): `/de/` 200 with `<html lang="de">` and the switcher, `/de/about/` with
  `hreflang` en/de/x-default, `/de/feed/` with `<language>de</language>`, sitemaps with
  `xhtml:link` alternates, `/de/robots.txt` 404, an untranslated post under `/de/` 404,
  `/api/v1/meta` listing both locales (rows in `site/VERIFIED.md`).

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
- **E2E in Firefox and WebKit** for every spec: only `multilingual.spec.ts` was run there.
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
- **R-1-16** `PATCH` with a new `locale` or `translation_of` saves the edit first, then
  relocates in a second write (two versions).
- **R-1-17** The WordPress import never adds languages to the site: items in an unlisted
  language are skipped and reported (`locale_not_on_site`); design 13 §8.1 had them added.
  `--default-locale`, the report's `languages` section and term locales are not built.
- **R-1-18** Imports join translation groups after writing the items
  (`ImportBatch::join_group`, which refuses an item edited since the import wrote it), not
  by writing in dependency order; Markdown reports what cannot join as `translation-of`.
- **R-1-19** Plan Task 16 (the admin's taxonomy and menu screens) and `locale` on the MCP
  term and menu tools are not built; the REST API covers them. Roadmap follow-ups.
- **R-1-20** `POST /content/{type}/{id}/translations` answers `{item, notes}`
  (`TranslationCreated`); an MCP `content_update` with `translation_of: null` leaves the
  group as it is.

## For the merge into `m6`

`m6` gained M6-2 (`0015_plugin_publishers`) and M6-6 (`0016_webhooks`) after this branch
was cut. Expect conflicts in: tests that name the schema version (this branch moved them
from 13 to 14), the export catalogue (`TABLES`), `openapi.json` (regenerate), the admin's
`schema.d.ts` (regenerate), `CLAUDE.md`, `docs/ROADMAP.md`, `site/VERIFIED.md`,
`rulings/m6.md`. M6-6's R-M6-6-8 asks every new write path to emit webhook events: this
branch's `translate`, `relocate`, `translate_term` and `join_group` emit none.
