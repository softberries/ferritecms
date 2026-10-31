+++
description = "Write a FerriteCMS theme: theme.toml, templates, the template hierarchy, the context and every helper."
+++

# Theme authoring

A theme is a folder of [MiniJinja](https://docs.rs/minijinja) templates and static
assets. Themes do not run code on the server: templates get data from core and a fixed
set of helpers, and anything that needs logic belongs in a plugin. Three themes are built
into the binary (`blog`, `docs`, `business`); their folders under `themes/` in the
repository are good starting points.

## The folder

```text
my-theme/
  theme.toml
  templates/
    index.html        required: the last fallback for every page
    single.html       optional, see the hierarchy below
    partials/…        anything you include or extend
  assets/
    style.css         served at asset('style.css')
```

- Every file under `templates/` must be UTF-8 and compile when the theme is loaded; a
  theme that does not compile is refused.
- At most 2000 files and 20 MiB in all, folders at most 16 deep. Symbolic links are
  refused and hidden files (names starting with `.`) are skipped.
- Templates are found by their path under `templates/` only: `include`, `extends` and
  `import` cannot reach other files.

## theme.toml

```toml
[theme]
name        = "my-theme"          # required: 1 to 64 characters of a-z, 0-9 and -
version     = "0.1.0"             # required: a semver version
author      = "You"
license     = "MIT"
homepage    = "https://example.com"   # http:// or https:// only
description = "One line for the admin"

[menus]                           # menu locations: name = "label"
primary = "Primary navigation"
footer  = "Footer"

[images]                          # extra image sizes, made for every upload
card = { width = 640, height = 360, crop = true }

[settings]                        # shown in the admin's Customize panel
accent_color = { type = "color", default = "#2563eb", label = "Accent colour" }
show_toc     = { type = "bool", default = true }
```

Unknown keys are refused.

- **Menus.** Location names are `a-z`, `0-9` and `-`. A template reads the menu stored
  under that name with `menu("primary")`.
- **Images.** Size names are `a-z`, `0-9`, `_` and `-`. They come on top of the core
  sizes `thumbnail`, `medium` and `large`; a size with one of those names, a size with a
  `height` but no `crop`, and a size with a zero dimension are skipped. After adding
  sizes, run `ferrite media regenerate` to make them for older uploads.
- **Settings.** Keys are `a-z`, `0-9`, `_` and `-`. The types are `text`, `textarea`,
  `bool`, `number`, `select` (needs `options = [...]`), `color` (`#rgb` or `#rrggbb`) and
  `media` (a media item; it takes no default). `label` defaults to the key. There is no
  secret type: every setting is visible to templates. Templates read
  `site.settings.<key>`: the stored value when it is valid, else the default, else the
  type's empty value.

## The template hierarchy

For each page FerriteCMS picks the first template that exists in this list, then falls
back to `index.html`:

| Page | Templates tried, in order |
|---|---|
| A single item (not a page) | `single-{type}-{slug}.html`, `single-{type}.html`, `single.html` |
| A page | `page-{slug}.html`, `page-{id}.html`, `page.html`, `single.html` |
| A type archive | `archive-{type}.html`, `archive.html` |
| A category or tag | `category-{slug}.html` (or `tag-…`), `taxonomy-{tax}-{slug}.html`, `category.html` (or `tag.html`), `taxonomy-{tax}.html`, `taxonomy.html`, `archive.html` |
| Another taxonomy's term | `taxonomy-{tax}-{slug}.html`, `taxonomy-{tax}.html`, `taxonomy.html`, `archive.html` |
| An author | `author-{login}.html`, `author.html`, `archive.html` |
| A date archive | `date.html`, `archive.html` |
| Search results | `search.html` |
| A static front page | `front-page.html`, then the page's own list |
| The front page as a list of posts | `front-page.html`, `home.html` |
| Not found (404) and gone (410) | `404.html` |

A server error uses `500.html` only, with no fallback to `index.html`; without it the
built-in error page is shown. It renders with very little context, so keep it standalone.

## What templates see

| Name | Set on | What it holds |
|---|---|---|
| `site` | every page | `title`, `url`, `description`, `locale`, `timezone`, `settings` |
| `post` | a single item or page | the item (below) |
| `page` | a page | the same object as `post` |
| `posts` | listings and search | `items`, `page`, `per_page`, `total`, `pages`, `prev_page`, `next_page` |
| `archive` | listings and search | `kind` (`front`, `type`, `term`, `author`, `date` or `search`), `content_type`, `term`, `author`, `year`, `month` |
| `pagination` | listings | `page`, `pages`, `prev_url`, `next_url` (ready-made links, `none` at either end) |
| `request` | every page | `path`, `s` (the search query), `page`, `status`, `preview` |

On the 404 and 410 page `request.path` is empty, since one body is shared by every
missing address.

**An item** (`post`, each of `posts.items`, and what `related()` and `query()` return)
has `id`, `kind`, `slug`, `title`, `excerpt`, `url`, `body` (the rendered HTML, already
marked safe), `toc`, `date` (published, RFC 3339, or `none`), `updated`, `menu_order`,
`parent_id`, `fields` (custom fields), `author` (`id`, `login`, `display_name`), `terms`
(each with `taxonomy`, `slug`, `name`, `url`, `id`) and `featured` (the featured image or
`none`).

**`post.toc`** lists every heading of the body in order, as `{level, id, text}`. Each
heading in the body has an `id` made the way GitHub makes them: lowercase, letters,
digits, `-`, `_` and spaces kept, spaces turned into `-`; a repeated heading gets `-1`,
`-2` and so on. An "On this page" list is a loop:

```jinja
{% for e in post.toc %}{% if e.level == 2 or e.level == 3 %}
  <a href="#{{ e.id }}">{{ e.text }}</a>
{% endif %}{% endfor %}
```

The ids `content`, `comments`, `respond`, `search`, `menu`, `top`, `main`, `toc`, `nav`
and `footer` are never given to a heading, so a theme can use them for its own landmarks.

Output is escaped automatically, except in templates whose names end in `.txt`, `.css`
or `.js`.

## Helpers

| Helper | Returns |
|---|---|
| `head()` | the page's `<head>` tags: title, canonical link, SEO tags and what plugins add. Put it in `<head>`. |
| `footer()` | the HTML plugins add before `</body>`; empty without such plugins. |
| `asset(path)` | the URL of a file under `assets/`, `/_theme/<hash>/<path>`. The hash changes with the theme, so assets can be cached for good. |
| `menu(name)` | `{items: [{label, url, children}]}`, or `none` when no menu has that name. Links to items that are not public are left out. |
| `related(post, n=3)` | up to `n` (at most 10) public items of the same type that share a term with `post`, newest first. |
| `query(type=…, term="tax/slug", limit=10, order="date")` | public items of a type, at most 50. With `term` only those with that term. `order="date"` is newest first; `order="menu"` sorts by the order field, then the title, then the id. |
| `page_tree(post=none, kind=none)` | the public items of one type as a tree: for a sidebar, breadcrumbs and previous/next links. See below. |
| `image(media, size)` | `{url, width, height, srcset}` for a public media item (an item, an object with an `id`, or an id) in that size; the original for an unknown size. |
| `comments(post)` | one page of the approved comments of a public post, with the form's details; `none` for other items. |
| `plugin_route(name, path)` | the URL of a plugin's route, `/_p/<name><path>`. |

Filters added to MiniJinja's own:

| Filter | Does |
|---|---|
| `date(fmt="%Y-%m-%d", tz=…)` | formats an RFC 3339 time with strftime codes, in `site.timezone` unless `tz="Europe/Warsaw"` is given. `none` gives an empty string. |
| `excerpt(n=55)` | plain text of HTML, at most `n` words, with `…` when cut. |
| `slugify` | the slug core would make of a text. |
| `json` | JSON that is safe inside `<script>` and in single-quoted attributes. |

MiniJinja's `range`, `dict` and `namespace`, its standard filters and tests, and
`urlencode` are there too. `debug()` is removed, and there is no `truncate`: use
`excerpt`.

### page_tree()

`page_tree(post)` reads the tree of `post`'s type and marks `post` as the current page;
`page_tree(kind="page")` reads a type's tree without a current page. It returns:

- `items`: the top-level nodes, each with all its descendants in `children`;
- `current`, `ancestors` (top level first, without `current`), `children` (the current
  page's children);
- `prev` and `next`: the pages before and after the current one, reading the whole tree
  from top to bottom;
- `truncated`: true when the type has more than 1000 public items; only the first 1000
  are in the tree.

Every node is `{id, title, url, depth, current, open, excerpt, featured, children}`, with
`depth` from 0 and `open` true on the current page and its ancestors. Siblings are sorted
by their order field, then by title. A page whose parent is not public is left out of
the tree with everything below it; the page itself still renders. With no type, or a
type with no public items, `page_tree` returns `none`.

```jinja
{% set tree = page_tree(post) %}
{% if tree and tree.next %}<a href="{{ tree.next.url }}" rel="next">{{ tree.next.title }}</a>{% endif %}
```

### Limits

A render may execute 5,000,000 template instructions and write at most 8 MiB. The data
helpers (`menu`, `related`, `query`, `page_tree`, `image`, `comments`) may be called 20
times per render in all; after that they return nothing. Repeated `menu()` calls with
the same name, and `page_tree()` calls for the same type, count once. A render that hits
the instruction or size limit, like any template error, is logged and answered with a
500 page (the theme's `500.html`, else the built-in one); the next render starts afresh.

## A minimal theme

`templates/index.html`:

```jinja
<!doctype html>
<html lang="{{ site.locale or 'en' }}">
<head>{{ head() }}<link rel="stylesheet" href="{{ asset('style.css') }}"></head>
<body style="--accent: {{ site.settings.accent }}">
<main>
{% if post %}
  <h1>{{ post.title }}</h1>
  {{ post.body }}
{% elif posts %}
  <h1>{{ site.title }}</h1>
  {% for p in posts.items %}<p><a href="{{ p.url }}">{{ p.title }}</a> {{ p.date | date("%-d %B %Y") }}</p>{% endfor %}
{% else %}
  <h1>Not found</h1>
{% endif %}
</main>
{{ footer() }}
</body>
</html>
```

with this `theme.toml`:

```toml
[theme]
name = "my-theme"
version = "0.1.0"

[menus]
primary = "Primary navigation"

[settings]
accent = { type = "color", default = "#2563eb", label = "Accent colour" }
```

## Install and activate

With SQLite, stop `serve` first. Install the folder and make it the active theme:

```sh
ferrite theme install --data ./mysite --dir ./my-theme --activate
ferrite theme list --data ./mysite
```

In the admin, **Appearance → Upload theme** takes the folder and packs it in the browser.
The API (`POST /api/v1/themes`) takes a bundle, which `theme pack` makes without a data
folder:

```sh
ferrite theme pack --dir ./my-theme --out my-theme.tar
```

`pack`, `install` and `activate` all refuse a theme that does not load. See
[Themes](../02-guides/04-themes.md) for activating and customising themes.
