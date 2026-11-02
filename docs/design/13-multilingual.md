# 13 — Multilingual content

Status: **design for M6-1** · Date: 2026-11-02

A FerriteCMS site can hold its content in several languages. Every content item, term and
menu has a locale; translations of one item form a translation group; the public site
serves the default locale at the root and every other locale under a prefix (`/de/…`),
with `hreflang` alternates, per-locale feeds and sitemap alternates. Editors translate
from the admin ("Translate" copies an item as a draft in the target locale), the REST API
and the MCP tools. The WordPress importer keeps the language that a Polylang export (and a
WPML export made with WPML Export and Import) carries.

A site with one locale stays exactly as it is: same URLs, same HTML, same feeds and
sitemaps (§9). That is a hard requirement, enforced by a golden test.

## 1. Goals and non-goals

Goals:

1. **Locales in settings**: a default locale and a list of further locales (BCP 47 tags).
2. **A locale on every content item, term and menu**, and a translation group linking an
   item's translations (one group per original, at most one member per locale).
3. **Public URLs**: the default locale unprefixed, every other locale under `/<prefix>/`;
   `hreflang` links, `<html lang>`, sitemap alternates, RSS per locale.
4. **Themes**: the current locale, `translations()` of an item, a language switcher and
   translatable theme strings; the three starter themes use them.
5. **Admin, API, MCP**: a locale filter on lists, "Translate", translation status per
   item; `locale` on the API and the MCP content tools.
6. **Import**: WordPress keeps an item's language where the WXR carries one; the Markdown
   import takes a locale from front matter.
7. **Both profiles**, solo and cluster, with the existing invalidation and reload paths.

Non-goals (M6-1): machine translation inside FerriteCMS (assistants do it over MCP, §7),
per-locale media (alt text, captions), translated slugs of content type and taxonomy
prefixes (`/de/category/…` keeps `category`), locale-specific date names in the `date`
filter, `Accept-Language` negotiation or automatic redirects by browser language, a
domain per locale, content fallback (showing the default-locale item under `/de/`), a
`locale` field in the plugin WIT `post` record (§8.3), side-by-side editing of two
locales in one editor.

## 2. Model

### 2.1 Locale tags

A locale is a BCP 47 tag of the subset `language[-Script][-REGION]`: a language of 2–3
ASCII letters, an optional script of 4 letters, an optional region of 2 letters or 3
digits (`en`, `pt-BR`, `zh-Hant-TW`, `sr-Latn`, `es-419`). Input is case-insensitive and
`_` is read as `-`; the stored form is canonical case (`pt-BR`, `zh-Hant`). Extensions,
variants and private use are refused. The URL prefix of a locale is its tag lowercased
(`/pt-br/`). Module `ferrite_domain::locale`: `parse_tag(&str) -> Result<String, LocaleError>`
(strict), `bcp47_loose(&str) -> String` (moved from `ferrite-http`'s `app::state::bcp47`,
unchanged behaviour: `en_GB.UTF-8` → `en-GB`, empty → `en`), `prefix(&str) -> String`,
`native_name(&str) -> Option<&'static str>` (a built-in table of about 60 languages:
`de` → `Deutsch`, `pt` → `Português`), `SiteLocales { default: LocaleDef, others:
Vec<LocaleDef> }` with `LocaleDef { tag, label }`, `is_multilingual()`, `contains(tag)`,
`by_prefix(segment)`, `prefix_of(tag) -> Option<String>` (`None` for the default).

### 2.2 Settings

| key | value |
|---|---|
| `site.locales` (new) | a JSON array, default first, of `{ "tag": "<BCP 47>", "label": "<text>" }`; 1–20 entries, tags strict (§2.1) and distinct by prefix; `label` optional (1–50 characters, no control or bidi characters), defaulting to `native_name`, then the tag |
| `site.locale_texts` (new) | a JSON object keyed by a non-default tag of `site.locales`: `{ "title": …, "description": … }`, each optional, validated like `site.title` / `site.description`; the site title and tagline in that locale |
| `site.locale` (existing) | unchanged meaning for a one-locale site: the text of `<html lang>` and the feeds' `<language>` (through `bcp47_loose`); for a multilingual site kept equal to the default tag |

**Settling (D3).** The database holds the locales; core never needs `ferrite.toml`. The
binary's `startup_check` (every command that opens the store for writing calls it) runs
`ferrite_core::locales::settle(store, &config.site.locale)` under the migrate lock, after
migrating:

1. If `site.locales` is absent, write `[{ "tag": bcp47_loose(site.locale option, else
   config) }]` (the label left to the default).
2. Fill `translation_group = id` where it is NULL (content items and terms written by a
   pre-M6 binary).
3. One locale: relabel every content item, term and menu whose locale is not the default
   to the default. Several: relabel only `''` (rows written by a pre-M6 binary) to the
   default.
4. If anything changed, append `invalidate [all]` in the same transaction.

So the migration's backfill guess (§3) and rows written by an older binary during a
rollback heal at the next start, and nothing reads a blank locale.

**Writing `site.locales`** goes through `OptionService::set_many` (P19) with a new
`SettingKind::Locales`, under `LOCK_TYPES` like `permalink.*` (it changes every URL), in
one transaction:

- every tag strict; the default's tag may stay loose only while the list has one entry;
- no prefix may equal a content type or taxonomy name, a `slug::RESERVED` or
  `slug::ROUTE_WORDS` word, four digits, or the slug of a top-level default-locale item of
  a root-namespace type (`Permalinks::root_namespace_peers`; the refusal names the item);
- a tag that disappears must hold no content item, term or menu (`Conflict` naming the
  counts, `LocaleRepo::usage`); media items do not count (§2.3);
- one entry → one entry with another tag is a **rename**: every row is relabelled;
- the order may change (a new default): no row changes, URLs move (the admin warns, §6);
- `site.locale` is written to the new default tag in the same write; a `site.locale`
  write alone on a multilingual site must equal the default (`InvalidField`), on a
  one-locale site it renames like the above;
- events: `options.changed [option:site.locales, option:site.locale]` and
  `invalidate [all]`; the reloader rebuilds the permalink rules and route table on
  `option:site.locales` (`crates/ferrite/src/reload.rs` `PERMALINK_OPTIONS` gains it).

`site.locale_texts` keys must be in `site.locales` (checked against the list in the same
write); a locale's removal drops its entry in the same transaction.

### 2.3 Content items

`content_items` gains `locale` and `translation_group` (§3). Domain:
`ContentItem.locale: String`, `ContentItem.translation_group: ContentId`, the same on
`NewContent` (the store fills `translation_group` with `id` when the caller passes the
new id). Rules, enforced in `ContentService`:

- A new item's locale is the default unless the caller names a listed locale.
- **Translation group**: the id of the group's original. An item created alone is its
  own group. A group holds at most one item per locale (unique index). Joining another
  group (`translation_of`) takes that item's group; leaving makes the item its own group
  again. Groups have no foreign key: deleting the original leaves the id as a label.
- **Slugs are unique per (type, locale, parent)**: `/about/` and `/de/about/` coexist.
  The root-namespace peer check (posts and top-level pages sharing `/<slug>/`) is per
  locale. A top-level default-locale item of a root-namespace type may not take a locale
  prefix as slug (a caller slug is `InvalidField { field: "slug" }`, a generated one gets
  `-N`).
- **Parents** must have the item's locale. Changing the locale of an item with children
  is `Invalid` ("change the children first").
- **Terms** on an item must have the item's locale (multilingual sites only;
  `InvalidField { field: "terms" }`).
- **Locale change** of an existing item (`relocate`): the target locale must be listed
  and free in its group; slug re-allocated in the new locale; a published item gets the
  usual permalink-change redirect (301 from the old path, the descendant walk is empty
  by the parent rule).
- **Media items** keep the default locale and are never filtered by locale (they are
  shared); their `translation_group` is their id.

`Revision` and `Autosave` carry no locale (an item's locale is not versioned).

### 2.4 Translate

`ContentService::translate(actor, id, locale) -> Translated { item, notes }`:
`EditOwn`; the source must be readable by the actor (`can_read`); the locale listed,
not the source's, and free in the group (else `Conflict` with the existing member's id).
The draft copies title, excerpt, body, fields, `menu_order` and the SEO meta; slug = the
source slug (allocated in the target locale); parent = the parent's translation in the
target locale, else top level (note `parent_untranslated`); terms = each term's
translation in the target locale, else dropped (note `term_untranslated:<id>`); author =
actor; status draft; group = the source's group. It runs `create_in` (revision,
search index, `post.saved` action jobs, events) and audits `content.translate`
(`{source, locale}`).

### 2.5 Terms

`terms` gains `locale` and `translation_group`, same rules: slug unique per (taxonomy,
locale); a term's parent has its locale; `TaxonomyService::create_term_in(actor,
NewTermInput, locale, translation_of)`, `translate_term(actor, id, locale)` (copies name,
slug, description; parent = the parent's translation or none), `relocate_term`. Domain
`Term.locale`, `Term.translation_group`. Term archives exist per locale:
`/de/category/nachrichten/` (§4).

### 2.6 Menus

`menus` gains `locale` and `translation_of` (the name of the default-locale menu this one
translates; no foreign key; unique `(translation_of, locale)`). Menu names stay globally
unique; the admin names a translation `<name>-<prefix>` (`primary-de`). A theme location
is a default-locale menu name; `menu("primary")` in locale `de` shows the menu whose
`translation_of = 'primary'` and `locale = 'de'`, else `primary` itself (D11). Menu items
may target anything (no locale check). `MenuService::for_location(name, locale)`;
`MenuRepo::get_translation(name, locale)`. Domain `Menu.locale: String`,
`Menu.translation_of: Option<String>`.

## 3. Storage (migration `0014_locales`)

Both backends, `-- ferrite:phase expand`, `-- ferrite:lossy-down`.

SQLite:

```sql
ALTER TABLE content_items ADD COLUMN locale TEXT NOT NULL DEFAULT '';
ALTER TABLE content_items ADD COLUMN translation_group BLOB;
UPDATE content_items SET locale = <L0>, translation_group = id;
ALTER TABLE terms ADD COLUMN locale TEXT NOT NULL DEFAULT '';
ALTER TABLE terms ADD COLUMN translation_group BLOB;
UPDATE terms SET locale = <L0>, translation_group = id;
ALTER TABLE menus ADD COLUMN locale TEXT NOT NULL DEFAULT '';
ALTER TABLE menus ADD COLUMN translation_of TEXT;
UPDATE menus SET locale = <L0>;
DROP INDEX content_items_slug_uq;
CREATE UNIQUE INDEX content_items_slug_uq
    ON content_items (type, locale, coalesce(parent_id, x''), slug);
DROP INDEX terms_taxonomy_slug_uq;
CREATE UNIQUE INDEX terms_taxonomy_slug_uq ON terms (taxonomy, locale, slug);
CREATE UNIQUE INDEX content_items_translation_uq ON content_items (translation_group, locale);
CREATE INDEX content_items_locale_list_idx
    ON content_items (type, locale, status, published_at DESC, id DESC);
CREATE UNIQUE INDEX terms_translation_uq ON terms (translation_group, locale);
CREATE UNIQUE INDEX menus_translation_uq ON menus (translation_of, locale);
```

`<L0>` is computed in SQL from the `site.locale` option when it is a JSON string
(`_`→`-`, cut at `.` or `@`; SQLite `replace`/`substr`/`instr`, Postgres
`split_part`/`replace`), else `'en'`. It is only a guess: `settle` (§2.2) corrects it from
the config at the first start. Postgres: `locale text COLLATE "C" NOT NULL DEFAULT ''`,
`translation_group uuid`, `translation_of text COLLATE "C"`, the slug constraint replaced
in one statement (`ALTER TABLE content_items DROP CONSTRAINT content_items_slug_uq, ADD
CONSTRAINT content_items_slug_uq UNIQUE NULLS NOT DISTINCT (type, locale, parent_id,
slug)`), `terms_taxonomy_slug_uq` the same way, the other indexes as above.

- **Expand rules (02 §8).** `translation_group` stays nullable on both backends, so a
  v0.2.0 binary can insert rows (its `NULL` groups and `''` locales are settled at the
  next new start; NULLs are distinct in both unique indexes). The two unique replacements
  are widenings (a strict superset of the columns under the same name, so every row
  pair the old index allowed is still allowed and the old binary's retry on
  `content_items_slug_uq` still fires). `ferrite_store::migrate::expand_lint` gains
  `widens_unique`: a `DROP INDEX x` followed by `CREATE UNIQUE INDEX x` (SQLite), or a
  `DROP CONSTRAINT x, ADD CONSTRAINT x UNIQUE …` (Postgres), passes when the new column
  list contains the one the down restores plus more, as `widens_checks` does for CHECKs
  (precedent: ruling R56). An old binary on a **multilingual** database is not supported
  (its slug lookups ignore the locale); `rollback` docs say so.
- **Down (lossy).** Deletes the content items, terms and menus whose locale is not the
  first tag of `site.locales` (an item cascade covers meta, revisions, term links,
  comments, autosaves, search docs, media rows), deletes the options `site.locales` and
  `site.locale_texts`, restores the two narrower unique indexes, drops the new indexes
  and columns (SQLite `ALTER TABLE … DROP COLUMN`, allowed since no column carries a
  foreign key or index after the index drops).
- **Catalogue** (`crates/ferrite-store/src/catalogue.rs`): the new columns, nullable
  `translation_group` / `translation_of`. Archives of an older schema load at their schema
  and migrate up (`site_copy`), so the backfill and `settle` cover them.
- **Store API** (`crates/ferrite-store/src/repos.rs`): `ContentQuery.locale:
  Option<String>` (`None`: any; media queries ignore it), `SearchQuery.locale`,
  `ContentRepo::get_by_slug(kind, locale: Option<&str>, parent, slug)`,
  `ContentRepo::group_members(groups: &[ContentId]) -> Vec<TranslationRef>` (≤ 200 groups;
  `TranslationRef { id, group, kind, locale, status, slug, parent_id, published_at }`),
  `ContentRepo::relocate(id, locale, group, slug, expected_version)`;
  `TermRepo::get_by_slug(tax, locale: Option<&str>, slug)`, `TermRepo::list(tax, locale:
  Option<&str>)`, `TermRepo::group_members(groups)`; `MenuRepo::get_translation(name,
  locale)`, `MenuRepo::list` returns `MenuSummary { name, label, locale, translation_of }`;
  a new `LocaleRepo` on `Repos`: `usage(locale) -> LocaleUsage { items, terms, menus }`,
  `relabel_all(to)`, `relabel_blank(to)`, `fill_groups()`.

Contract tests on both backends: the migration roundtrip, the backfill from the option,
slug uniqueness per locale, group uniqueness, `NULL` groups from an old-style insert,
the filters, `group_members`, the lossy down deleting the non-default rows.

## 4. URLs and routing

- **Permalinks.** `PermalinkRules` gains `locales: SiteLocales` (read with the structures
  by `stored_rules`: one more option, `site.locales`). `path()` prepends the item's locale
  prefix for a non-default locale: `/de/` + the structure's expansion. Items whose locale
  is not listed (a race with a removal) get no prefix and are not routable (§2.2 refuses
  removal while rows exist). `term_path(taxonomy, slug)` stays; `term_path_in(prefix:
  Option<&str>, taxonomy, slug)` is the localized form used by readers and feeds.
  `Permalinks::locale_prefixes()` (default empty) feeds core's reserved-slug rule.
- **Router.** `RouteTable::new` takes the prefixes from the rules. A new
  `resolve_localized(raw_path, raw_query, table) -> (RouteLocale, Route)` (`RouteLocale
  { tag: Option<String> }`, `None` = default): when the first segment matches a prefix
  (ASCII case-insensitive), the rest is resolved by the existing `resolve` and every
  `Canonical` target is re-prefixed (`/DE/about` → 301 `/de/about/`; `/de` → 301
  `/de/`). Under a prefix only content, listings and feeds exist: files (`robots.txt`,
  sitemaps), `/_media`, `/_theme`, `/_preview`, `/_comment`, `/_p`, health routes are
  404 (one robots file, one sitemap index for the site). The default locale's own tag is
  no prefix: `/en/…` on an English-default site means what it always meant. `resolve`
  itself and its tests are unchanged.
- **Front page per locale (D9).** `/de/` shows the translation in `de` of the
  `reading.front_page` item when it is public, else the `de` post listing. A front page
  translation's own permalink answers 301 to its locale root (as M4-4 P67 for `/`).
- **Listings** under a prefix (front, search `?s=`, type, term, author, date archives)
  list only that locale; feeds `/de/feed/` and `/de/category/x/feed/` likewise.
  Without a prefix on a multilingual site, the default locale only.
- **No fallback (D8).** A missing translation is a 404 under its prefix; listings never
  mix locales; the switcher links to the locale's front page instead (§5).
- **Redirects** stay one global table of source paths (a source may carry a prefix).
- **Previews** (`/_preview/<id>`) render with the item's locale.

## 5. Public output

- **Reader.** `SiteReader` methods take `loc: &ReadLocale` (`ReadLocale::All` |
  `ReadLocale::Only(tag)`): `single`, `front_page`, `is_front_page`, `archive`,
  `term_archive(_page)` (the term looked up by locale), `author_archive_page`,
  `date_archive`, `search`, `feed`, `related` (the item's own locale), `tree`, `latest`,
  `term_items`. The HTTP layer passes `All` on a one-locale site (no locale predicate in
  any query: D4) and `Only(request locale)` otherwise. New: `translations(item: &PageItem)
  -> SiteRead<Vec<TranslationLink>>` (`{ locale, label, url, title, current }` of the
  group's public members in `site.locales` order; one `group_members` read), and
  `term_translations(term)`. `PublicItem` gains `locale`; `PageItem` is unchanged.
  `SitemapEntry` gains `alternates: Vec<(String, String)>` (tag, path), filled per
  sitemap page with batched `group_members` reads, only on multilingual sites.
- **Head** (`crates/ferrite-http/src/head.rs`). `HeadPage` gains `alternates:
  &[Alternate]`; `core_head` adds `<link rel="alternate" hreflang="<tag>" href="<abs>">`
  for each public translation (itself included) and `hreflang="x-default"` for the
  default-locale member when it exists, on items, the front page (each locale's root)
  and term archives with translated terms. Nothing on one-locale sites. Plugins see the
  links in the `render.head` filter (`link-tag.hreflang` exists in the WIT already).
- **Site info.** `SiteInfo` (theme `site`) gains `locales: Vec<LocaleLink>` (`{ tag,
  label, url (that locale's root), current }`), `default_locale`; `site.locale` is the
  request's locale on a multilingual site (so `<html lang="{{ site.locale }}">` in every
  starter theme is right without a change); `site.title` / `site.description` come from
  `site.locale_texts[locale]` when set. On a one-locale site `site.locales` has one entry
  and every value is as today.
- **Feeds.** `<language>` is the feed's locale; links carry the prefix; channel title
  from `site.locale_texts`.
- **Sitemaps.** Files stay per type and list every locale's URLs. On a multilingual site
  the `<urlset>` declares `xmlns:xhtml="http://www.w3.org/1999/xhtml"` and each `<url>`
  carries `<xhtml:link rel="alternate" hreflang=… href=…/>` for every public member of its
  group plus `x-default`; front page translations are listed as their locale root. One
  locale: byte-identical to today.
- **Theme helpers** (`crates/ferrite-theme/src/render.rs`, 06 §4): `translations(post)`
  (the `TranslationLink` list; a bridge call, memoised per item), `locale_links(post=none)`
  (one entry per site locale: `{ tag, label, url, current, translated }`, the url being
  the translation's when it exists, else the locale's root; the switcher's data),
  `t(text)` (the active theme's string for the current locale, §5.1, else `text`). `menu`,
  `query`, `related`, `page_tree` work in the current locale (the bridge holds the
  render's `ReadLocale`). Themes still run no code: these return data.
- **Theme strings (5.1).** `theme.toml` may hold `[strings.<tag>]` tables mapping a
  source text to its translation (`[strings.de] "Read more" = "Weiterlesen"`). Lookup:
  the exact tag, then its language subtag (`pt-BR` → `pt`), then the text itself. Caps:
  32 tables, 500 strings each, 1000 characters per value; the manifest refuses more.
  Output is a plain string (auto-escaped). On a one-locale site `t("x")` is `x` unless
  the theme translates the default locale.
- **Dependency keys** (`ferrite_core::events`): new `group:<translation group>` and
  `tgroup:<term group>`. Every content write and lifecycle transition that emits
  `post:<id>` also emits `group:<item's group>` (both groups on a group change);
  term writes emit `tgroup:<group>`. Pages that read translations (head alternates,
  `translations()`, `locale_links()`, a locale's front page reading the front page's
  translation) record the group key. Pages record `option:site.locales` and
  `option:site.locale_texts`. Cache keys need nothing new: the prefix is part of the
  path.
- **Plugins.** `request-info.locale` (already in the world) carries the request's locale
  instead of `site.locale`; `post` gets no locale field (§8.3).

## 6. Admin (11-admin-ui)

Every locale control is hidden while the site has one locale, so the admin of a
one-locale site looks as it does today, except for the new settings group.

- **Settings → Languages** (`/admin/settings/languages`, new group): the locale list
  with drag order (the first is the default, marked), add (tag with live validation and
  the native-name suggestion), label edit, remove (disabled with the counts while the
  locale holds content), per-locale site title and tagline (`site.locale_texts`). A
  change of the default shows a confirm dialog stating that URLs of both locales move
  and no redirects are made (D12).
- **Content list**: a locale segmented filter (All, then each locale; remembered per
  type in `localStorage`), a Languages column of locale chips per row (filled = exists,
  with its status as the chip's title; outline = missing, a click translates), and the
  counts per status respect the filter.
- **Editor**: the side panel gets a Translations section: the item's locale (a select
  for a draft; for a published item a confirm about the URL change), one row per other
  site locale with the translation's status and an Open link, or a Translate button;
  "Link to existing…" (a picker of same-type items in that locale without a member here)
  and "Unlink". Translate opens the new draft with a notice listing the notes
  (untranslated parent or terms).
- **Taxonomies**: a locale filter on the term tree, Translate per term, terms' locale in
  the form. **Menus**: each location shows its menu per locale; "Create the Deutsch
  menu" makes `primary-de` (`translation_of = primary`), optionally copying the items.
- Strings in `admin/src/i18n/en/*.json`; Vitest for the models; Playwright specs
  `admin/e2e/multilingual.spec.ts` on both backends.

## 7. API and MCP

REST (`/api/v1`, DTOs in `crates/ferrite-http/src/api/dto/`, OpenAPI regenerated):

- `ContentDto` + `locale`, `translation_group`, `translations: [TranslationRefDto { id,
  locale, status, title, permalink }]` (the other members the caller may read);
  `ContentSummaryDto` + `locale`, `translations` (locale and status only; one
  `group_members` read per page).
- `GET /content/{type}?locale=de`, `GET /content/{type}/counts?locale=de`, `GET
  /content/refs?locale=` filter (unknown locale: `400`).
- `POST /content/{type}`: `locale` (default the site default), `translation_of` (an item
  id whose group to join).
- `PATCH /content/{type}/{id}`: `locale` and `translation_of` (`null` leaves the group);
  run as `relocate` after the field update in the same request's transaction.
- `POST /content/{type}/{id}/translations` `{ "locale": "de" }` → `201` `ContentDto` with
  `Location`, plus `notes: [string]`; `409` when the locale has a member (the problem
  names its id).
- Terms: `TermDto` + `locale`, `translation_group`; list `?locale=`; create `locale`,
  `translation_of`; `POST /taxonomies/{tax}/terms/{id}/translations`.
- Menus: `MenuDto` + `locale`, `translation_of`; `GET /menus` lists them; `PUT
  /menus/{name}` takes both.
- Settings: `site.locales`, `site.locale_texts` in `SettingsDto` / `SettingsPatch`.
- `GET /meta` (public) + `default_locale`, `locales: [{ tag, label, prefix }]` (public
  information anyway: the hreflang links show it), so every admin user has the list
  without the settings scope.

MCP (`crates/ferrite-http/src/mcp/tools/`): `content_list` (`locale` filter),
`content_get` (`locale`, `translations`), `content_create` (`locale`,
`translation_of`), `content_update` (`locale`, `translation_of`), new
`content_translate { type, id, locale }` (returns the draft as Markdown plus the notes;
annotated not read-only, not destructive, idempotent false) — the assistant translates
the Markdown and calls `content_update`; `terms_list`/`term_create` (`locale`),
`menu_get`/`menu_set` (`locale`, `translation_of`), `site_overview` reports the locales,
`settings_update` takes the two new keys. New prompt `translate` (arguments `type`, `id`,
`locale`). 57 tools, 5 prompts. The server `instructions` name the site's locales when
there are several.

## 8. Import, export, plugins

### 8.1 WordPress (08-wp-import)

What a WXR file holds, checked on 2026-11-02 against public sources (WordPress core trac
ticket 34414; WPML's "multilingual content import" documentation; Polylang's REST API
documentation and community code reading `post_translations`), not against a real
export; the plan's Task 17 builds one for Polylang first:

- **Polylang** stores languages as real taxonomies, which the WordPress exporter writes
  like any other: `language` (term slug = the language code used in Polylang's URLs,
  e.g. `de`; Polylang keeps its locale such as `de_DE` in the term description,
  serialized) on every item as `<category domain="language" nicename="de">`, and
  `post_translations` terms whose description is a PHP-serialized map `language code →
  WP post id`, attached the same way. *Confirmed from Polylang's storage model in public
  code and documentation; not yet from a real export.* Term languages
  (`term_language`, `term_translations`) are relations between terms, which WXR does not
  export: expect them to be lost (*not confirmed*).
- **WPML** keeps languages in its own table `icl_translations`, which the plain
  WordPress export does not include: a plain WXR of a WPML site carries **no** language
  data (WPML's own statement on core trac ticket 34414). With the add-on "WPML Export and
  Import" the export carries post meta `_wpml_import_language_code`,
  `_wpml_import_translation_group` and (for translations)
  `_wpml_import_source_language_code` (*names confirmed from WPML's documentation, the
  WXR shape not confirmed; WPML is commercial and was not run*).
- Today the importer would register `language` and `post_translations` as custom
  taxonomies (`pass/types.rs` imports every taxonomy outside `NOT_TAXONOMIES`); M6-1 adds
  `language`, `post_translations`, `term_language`, `term_translations` to
  `NOT_TAXONOMIES` and reads them as language data.

Behaviour: an item's language = its Polylang `language` term slug, or
`_wpml_import_language_code`, parsed with `parse_tag` (an invalid one is reported and the
item gets the default). Groups from `post_translations` maps or
`_wpml_import_translation_group` values; the group id is the imported id of the member in
the site's default locale, else the first member. Languages found that the site does not
list are added after the existing ones in the same batch as the options pass (with
`--no-site-options` they are reported and those items skipped as `locale_not_on_site`);
`--default-locale TAG` (API option `default_locale`) sets the default when the site is
still empty. Terms get the locale of the items that use them when all those items share
one locale, else the default (a mismatched link is dropped and reported:
`term_locale_mismatch`). Menus: the default locale (Polylang's per-language menu
locations live in options, not in WXR). Re-runs map through `import_map` as today; a
locale change of an already-imported item applies when the item was not edited here.
The report gains a `languages` section (code, items, groups).

### 8.2 Markdown (`ferrite-import-md`)

Front matter `locale = "de"` and `translation_of = "<path relative to this file>"`
(the page this one translates; must be in the same import), and `--locale TAG` (the
default for files without one). Files in the same import and group are created in
dependency order. Unknown locales are skipped with a `locale` problem line (exit 2).

### 8.3 Plugins

No WIT change in M6-1 (D14): `request-info.locale` already exists and now carries the
page's locale, so a plugin can localise its output. A `locale` field on the `post`
record needs a new world version (05 §11) and joins the "plugin world gaps" list in the
roadmap's Later section.

### 8.4 Export, backup, search

Export, import-site, `db copy`, backup and restore carry the new columns through the
catalogue. Search keeps one index for all locales (tokenisation is language-neutral,
02 §4) with a locale filter on the query: public search is per request locale, the admin
filters by the list's locale filter.

## 9. One locale: byte-identical

A site whose `site.locales` has one entry serves byte for byte what M5 served, for
every route of the three starter themes, feeds, sitemaps and `robots.txt`:

1. queries carry no locale predicate (`ReadLocale::All`);
2. no prefix exists, `path()` is unchanged;
3. no `hreflang` links, no `xmlns:xhtml`;
4. `site.locale`, `site.title`, `site.description` are as before;
5. the starter themes print the switcher only inside `{%- if site.locales | length > 1 %}`
   with whitespace control, and `t("x")` returns `x`;
6. the API and the admin only add fields and a settings group.

The exception: a template that prints a whole item (`post | tojson`) shows the new
`locale` field. A golden test (plan Task 1) captures the HTML of the seeded sites before
M6-1 and every later task keeps it passing.

## 10. Cluster

Nothing node-local beyond what exists: `site.locales` lives in the options table and
travels like `permalink.*` (option write → `options.changed` + `invalidate [all]` through
the outbox; every node's reloader rebuilds `PermalinkRules` and the `RouteTable` and
flushes; a request pins one rule set, R10). `settle` runs under the migrate lock, so
concurrent node starts settle once. Content, term and menu writes carry their group
keys through the same event bus. Both profiles behave the same.

## 11. Security

- Locale tags are strict ASCII (§2.1) before they reach a URL, an attribute or a SQL
  bind; labels and `site.locale_texts` are length-capped text without control or bidi
  characters; theme strings are auto-escaped output.
- Translate and linking check rights on both ends: `can_read` on the source or the
  group's item, edit rights on the item that moves; a contributor's translation is their
  own draft. Removing a locale needs `manage_options`.
- A group holds at most 20 members (one per locale), so `translations()` and the
  alternates are bounded; `group_members` takes at most 200 groups per call.
- Hostile WXR: a `post_translations` description is untrusted PHP serialisation, read
  with the existing `php.rs` parser (size-capped, no object instantiation); unknown
  members, cycles or two items claiming one locale in a group are reported, the later
  ones left alone.

## 12. Decisions

- **D1** Tags are a strict `language[-Script][-REGION]` subset with canonical case: enough
  for real sites, and safe in URLs and attributes without escaping rules of their own.
- **D2** The URL prefix is the lowercased tag, not configurable: one rule, no second
  name to keep unique.
- **D3** The database is the authority for locales; `settle` at startup writes
  `site.locales` from the old option or the config once, and heals rows written by older
  binaries: core never needs `ferrite.toml`, and every node and CLI agrees.
- **D4** One-locale sites run without any locale predicate: byte-identity and query
  plans stay exactly as they were.
- **D5** `translation_group` is the original's id, nullable in the schema but always set
  by core (`settle` fills old rows): expand rules forbid `NOT NULL` without a default on
  a column that must hold another column's value.
- **D6** The slug and term-slug unique indexes are widened under the same names, and
  `expand_lint` learns to accept a widening of a unique index (as R56 did for CHECKs):
  otherwise translations could not share slugs, or the migration would have to be a
  contract one that v0.2.0 cannot run on.
- **D7** Locale and group changes are their own operations (`relocate`, `translate`,
  `translation_of`), not `ContentDraft` fields: an item's language is not edited like its
  title, and the 80-odd `ContentDraft` literals stay untouched.
- **D8** No content fallback: a page in `de` that does not exist is a 404, and listings
  never mix locales; mixing languages on one page confuses readers and search engines.
- **D9** The front page per locale is the translation of `reading.front_page`: no new
  setting, and translating the front page is all an editor does.
- **D10** Terms get translation groups too (beyond the brief's `locale`): Translate maps
  an item's terms through them, and term archives get alternates.
- **D11** A menu translation points at the default-locale menu by name, with a fallback
  to it: theme locations stay the menu names they are today.
- **D12** Changing the default locale moves URLs and makes no redirects: it is a rare,
  deliberate change, and redirect rules for both locales' whole URL space would be a
  guess; the admin says so before it saves.
- **D13** Theme UI strings are data in `theme.toml` (`[strings.<tag>]`, keyed by the
  source text, gettext style): themes still execute nothing, and an untranslated string
  prints as written.
- **D14** No WIT change: `request-info.locale` already exists; `post.locale` waits for
  the next world version with the other plugin world gaps.
- **D15** Media is shared across locales (default locale, never filtered): alt text per
  locale is a later feature.
- **D16** One search index with a locale filter: tokenisation is already
  language-neutral, so per-locale indexes would only duplicate it.
- **D17** Items' terms and parents must share the item's locale (API, admin, MCP); the
  importer drops mismatched term links with a report line rather than failing a run.
- **D18** At most 20 locales: bounds groups, alternates and the switcher.
- **D19** `/meta` publishes the locale list: it is public through the hreflang links, and
  every signed-in role needs it in the admin.

## 13. Later

Per-locale media text; translated type and taxonomy prefixes; locale-aware `date` names;
a domain per locale; optional fallback listings; `post.locale` in a new plugin world;
term language import from Polylang (needs a WordPress plugin or the database); a
translation memory or glossary for assistants; side-by-side editing.
