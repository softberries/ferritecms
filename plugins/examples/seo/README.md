# seo

An example FerriteCMS plugin that adds the head tags core does not emit: the Open Graph site
name and locale, Twitter cards, article tags, JSON-LD, search-engine verification tags and
the RSS discovery link. It is one `render.head` filter (priority 20) in a WebAssembly sandbox.

It adds to the head core hands it and never repeats a tag that is already there, whether core
or an earlier plugin put it there.

## What core already does

Do not look to this plugin for these; core emits them without any plugin:

- `<title>`, `<meta name="description">`, `<link rel="canonical">`;
- `<meta name="robots">`, on item pages, from the item's SEO robots field;
- `og:title`, `og:description`, `og:url`, `og:type`, and `og:image` (the item's featured image);
- `/sitemap.xml` and `/robots.txt`; the RSS feed at `/feed/`.

Per-item overrides (`seo.title`, `seo.description`, `seo.canonical`, `seo.robots`) are edited
in the admin and apply to core's tags; this plugin copies the result.

## What the plugin adds

| Tag | Where | Source |
|---|---|---|
| `og:site_name` | every page | the `site.title` option (the stored title, else the configured one) |
| `og:locale` | every page with a regional locale | the request locale, `-` written as `_` (`pl-PL` becomes `pl_PL`); a bare language such as `en` adds nothing, because Open Graph wants `ll_TT` |
| `twitter:card` | every page | `summary_large_image` when core emitted an `og:image`, else `summary` |
| `twitter:title`, `twitter:description` | every page | copied from core's `og:title` and `og:description` |
| `twitter:site` | every page | the `twitter_site` setting |
| `article:published_time`, `article:author` (display name), `article:section` (first category), `article:tag` (one per tag) | single `post` pages | the post |
| JSON-LD `WebSite` with a `SearchAction` (`<site>/?s={search_term_string}`) | `/` | the site URL |
| JSON-LD `BlogPosting`: `headline`, `datePublished`, `url`, `author`, `image`, `publisher` | single `post` pages | the post, core's `og:image`, the organization settings |
| `google-site-verification`, `msvalidate.01` | every page | the verification settings |
| `<link rel="alternate" type="application/rss+xml">` to `<site>/feed/` | every page | on unless `feed_link` is off |

Archives, search and paginated pages get the site-level tags only (no `article:*`, no
`BlogPosting`). Error pages (404, 410, 500) run no plugin filters, so they get none of this.
A `page` item gets the site-level tags only: the plugin reads `post` items.

The JSON-LD is built with `serde_json`, so a title containing quotes or `</script>` stays valid
JSON; the host then re-serialises each block and escapes `<`, `>` and `&`.

## Install

The plugin is a folder holding `ferrite-plugin.toml` and one `.wasm`. From the plugins release
asset (`ferrite-plugins-<version>.tar.gz`, which holds `seo/`), or built from source:

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml -p seo
mkdir seo-plugin
cp plugins/examples/seo/ferrite-plugin.toml seo-plugin/
cp plugins/target/wasm32-wasip2/release/seo.wasm seo-plugin/
```

With the release asset, unpack it (`tar xzf ferrite-plugins-<version>.tar.gz`) and use its `seo/`
folder where the commands below say `seo-plugin`. Then, with `serve` stopped if the site uses SQLite:

```sh
ferrite plugin check   --data /path/to/site --dir seo-plugin   # prints the permission report
ferrite plugin install --data /path/to/site --dir seo-plugin --enable
```

`install` shows the permission report and asks you to approve it (`--yes` approves without
asking; it is required when stdin is not a terminal). Or use the admin's Plugins screen, or the
API: inspect with `POST /api/v1/plugins/inspect`, then `POST /api/v1/plugins` with the report's
`fingerprint` as `approve`. A new plugin is installed disabled unless you pass `--enable`.

## Permissions

The report lists these, and the plugin asks for nothing else:

- `content.read = ["post"]`: read published posts (title, publish time, author, terms, URL).
- `options.read = ["site.title", "site.url"]`: read the site title and the site URL.
- `render.head`: add tags to the head of every page. The report marks this as high risk,
  because a head script can load from any `https://` host. This plugin only adds meta tags,
  one link and JSON-LD, but the grant is what you approve.

It has no network access, no mail, no routes and no storage of its own.

## Settings

Set them on the plugin's page in the admin's Plugins screen, or with
`PATCH /api/v1/plugins/seo/settings`.

| Setting | Type | Default | Effect |
|---|---|---|---|
| `twitter_site` | text | none | `twitter:site`; a leading `@` is added when missing |
| `organization_name` | text | the site title | the JSON-LD `publisher` name |
| `organization_logo` | text | none | the `publisher` logo; used only for an `https://` URL |
| `google_verification` | text | none | the content of `google-site-verification` |
| `bing_verification` | text | none | the content of `msvalidate.01` |
| `feed_link` | bool | on | the RSS discovery link |

A blank text setting adds nothing.

## Limits of this example

The plugin runs in world `ferrite:plugin/plugin@0.1.0`, which constrains it:

- **No modified date.** A `post` carries only its publish time, so there is no
  `article:modified_time` or `dateModified`.
- **No author URL or avatar.** The author is the display name only.
- **Site URL and title come from the host's options.** `site.url` is the configured site URL
  and `site.title` is the stored title, else the one in `ferrite.toml`. If you narrow the
  grant so that one of them cannot be read, the tags that need it are left out: without
  `site.title`, no `og:site_name` and no publisher name; without `site.url`, no `WebSite`
  JSON-LD and no RSS link.
- **Posts only.** The plugin reads `post` items. For other types or custom taxonomies, extend
  `content.read` in the manifest and the code.
- **No IndexNow, no robots or sitemap changes, no content analysis.** Plugins cannot serve
  `/robots.txt` or `/sitemap.xml`, which core owns.
- **No admin page.** Settings use the generic settings form; there is no dedicated SEO screen.

## Uninstall and data

`ferrite plugin remove seo --data /path/to/site` (or the admin) removes the plugin; its settings are kept for 30
days, so a reinstall in that time finds them, and `--purge` deletes them at once. The plugin
stores nothing else. Settings live in the site's options under `plugin.seo.settings.*`, and the
page cache refreshes when you change one.

The plugin data belongs to the name `seo`: install an update or a reinstall only from a source
you trust.
