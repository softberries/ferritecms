+++
title = "Multilingual sites"
description = "Publish a site in several languages: add languages, translate posts and pages, per-language menus, addresses and search engines, themes, import and AI assistants."
+++

# Multilingual sites

A FerriteCMS site can be written in up to 20 languages. One language is the **default**:
its content keeps the addresses it has today. Every other language gets an address prefix,
so the German version of `/about/` lives at `/de/about/`. Each translation is an item of its
own (its own title, body, slug, status and revisions), linked to the others in a
**translation group**.

A site with one language works exactly as before, and the admin shows no language
controls until you add a second language.

## Add a language

Open **Settings → Languages**. Type a language tag such as `de`, `fr` or `pt-BR` and the
name visitors see (`Deutsch`), choose **Add language**, then **Save changes**.

- The first language in the list is the default. **Make default** moves another one to
  the top. FerriteCMS asks first, because the old default's content moves under its own
  prefix and the new default's content loses its prefix. Old addresses are not redirected.
- The address prefix is the tag in lower case: `pt-BR` is served under `/pt-br/`.
- A language that still holds content (items, terms or menus) cannot be removed; the
  save is refused with what it still holds. Move or delete that content first.

The same list is the setting `site.locales` in `PATCH /api/v1/settings`, a list of
`{ "tag", "label" }` with the default first.

## Translate a post or page

Open the post in the editor. On a site with more than one language, the settings panel
has a **Translations** section: the item's language, its translations (each a link to it,
with its status) and a **Translate into …** button for every language it has no
translation in yet.

**Translate into Deutsch** makes a draft copy of the item in German, in the same
translation group, and opens it. Translate the title, the body and the SEO fields, then
publish it like any other item. Save your changes first: the copy starts from the saved
version. When the item's parent page or one of its terms has no German translation yet,
the copy goes without it and FerriteCMS tells you.

The content list has a language filter and a **Language** column that shows, for each
row, its own language and which other languages it has, by status.

From the REST API: `POST /api/v1/content/{type}/{id}/translations` with `{"locale": "de"}`
answers `201` with `{ "item", "notes" }`. A second translation into the same language is
refused with `409`. `POST /api/v1/content/{type}` takes `locale` and `translation_of`
(the id of an item whose group the new item joins), and `GET /api/v1/content/{type}`
takes `?locale=de`.

## Terms and menus

Terms (categories, tags and your own taxonomies) and menus have a language too.

- A term is translated with `POST /api/v1/terms/{id}/translations` and `{"locale": "de"}`:
  the German term gets the same name and slug, which you can then rename. `GET
  /api/v1/taxonomies/{taxonomy}/terms?locale=de` lists one language's terms.
- A menu for a theme location in another language is a menu whose `locale` is that
  language and whose `translation_of` names the default-language menu, for example
  `PUT /api/v1/menus/primary-de` with `"locale": "de"` and `"translation_of": "primary"`.
  German pages then show `primary-de` where the theme shows `primary`; a language without
  its own menu shows the default one.

The admin's taxonomy and menu screens do not show languages yet; use the API for
per-language terms and menus.

## Addresses and search engines

- Content in the default language has no prefix; content in another language is under
  `/<prefix>/`, and that language's home page is `/<prefix>/`.
- There is no fallback: an address in a language the item has no translation in answers
  `404`.
- Each page lists its translations as `<link rel="alternate" hreflang="…">` in its head,
  with `x-default` pointing at the default-language version, and `<html lang>` is the
  page's language.
- Each language has its own feed (`/de/feed/`). The sitemap lists every page with its
  alternates. `robots.txt` exists only at the site's root.

## Themes

The three starter themes show a language switcher and translate their own interface text
on a multilingual site. Templates get:

- `site.locales`: the site's languages, each with `tag`, `label`, `url` (its home page)
  and `current`; one entry on a one-language site.
- `translations(post)`: the published translations of an item, each with `locale`,
  `label`, `url`, `title` and `current`.
- `locale_links(post)`: one entry per language for a switcher, linking to the item's
  translation where there is one, else to that language's home page.
- `t("text")`: the theme's own text in the page's language, from a `[strings.<tag>]` table
  in `theme.toml`:

```toml
[strings.de]
"Skip to content" = "Zum Inhalt springen"
```

See [Themes](04-themes.md) for the rest of the template helpers.

## Import

- **WordPress**: the importer reads an item's language from Polylang (its `language`
  taxonomy) or from WPML's "Export and Import" add-on (its `_wpml_import_*` post meta), and
  links translations into groups. Add the site's languages in Settings → Languages
  before the import: an item in a language the site does not list is skipped and named in
  the report, so add the language and run the import again. Terms and menus are imported
  in the default language. A plain WPML export holds no language data; its items import
  in the default language. Both formats were tested with sample files written from the
  plugins' documentation, not with exports of real sites. See
  [Import from WordPress](06-import-wordpress.md).
- **Markdown**: front matter `locale = "de"` and `translation_of = "../en/about.md"` (a
  path relative to the file, inside the same import), and `--locale TAG` for files without
  one. See [Import Markdown](07-import-markdown.md).

## AI assistants

Connected assistants see the site's languages in `site_overview`, can filter
`content_list` by `locale`, and have the tool `content_translate` and the prompt
`translate`. Ask, for example:

> Translate the post "Spring opening hours" into German and leave it as a draft.

The assistant makes the German draft, translates it and saves it for you to review. See
[AI assistants (MCP)](08-ai-assistants.md).

## Try it

`ferrite seed demo --data ./mysite --profile multilingual` writes the demo content with a
German translation of the three newest posts, the About and Contact pages, a German
category and a German main menu. Serve the site and open `/de/`.
