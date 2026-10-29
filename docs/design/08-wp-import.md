# 08 — WordPress Import

## 1. Entry points

```
ferrite import wordpress export.xml --uploads ./wp-content/uploads --data ./site [--dry-run]
POST /api/v1/import/wordpress            # admin; runs as an import.wordpress job
```

Both use `ferrite-import-wp`, which talks to core services (not to SQL), so imports work
on either backend and media goes to whichever MediaStore is configured.

## 2. Pipeline

```
WXR (quick-xml, streaming) ─► parse items into an intermediate model (IR)
   ─► pass 1: authors, terms (all taxonomies, parents resolved after), content types seen
   ─► pass 2: attachments → media (files from --uploads or download from <wp:attachment_url> if --fetch-media)
   ─► pass 3: posts/pages/custom types → content (blocks conversion, URL rewrite, meta mapping)
   ─► pass 4: comments, menus (nav_menu_item), redirects, options (title, tagline, permalink structure)
   ─► report (import-report.html + JSON)
```

- Streaming parse: WXR files can be hundreds of MB; never load the whole DOM.
- The IR is written to a temp SQLite file (even when target is Postgres) so passes can
  be re-run and the import is **resumable**; the job heartbeats its lease.
- Idempotent: an `import_map` (WP id → ferrite id, kept in the temp file and summarised
  into `content_meta` `import.wp.id`) lets a re-run update instead of duplicate.
- Transactions in batches of 500 items; each batch emits invalidation events once.

## 3. Mapping

| WordPress | FerriteCMS |
|---|---|
| `post`, `page` | built-in types; hierarchy and `menu_order` preserved |
| custom post types | auto-created content types (non-built-in), fields from ACF if present |
| `attachment` | `media` items; `_wp_attachment_image_alt` → `alt` |
| `category`, `post_tag`, custom taxonomies | taxonomies + terms with parents |
| `wp:status` publish/draft/pending/private/future/trash | same statuses (`future` → `scheduled` + job) |
| authors | users, role from WXR if present else `author`; WXR has no password hashes → `must_reset` |
| comments | comments with status mapping (approved/hold/spam) |
| `nav_menu_item` | menus |
| `_yoast_wpseo_title`, `_yoast_wpseo_metadesc`, `_yoast_wpseo_canonical`, `_yoast_wpseo_meta-robots-*`, Rank Math `rank_math_title`/`rank_math_description`/`rank_math_robots` | `seo.title`, `seo.description`, `seo.canonical`, `seo.robots` meta |
| `_thumbnail_id` | featured image field |
| ACF fields (`field_*` keys + `_` reference meta) | custom fields where the ACF field type maps; others kept as raw meta and reported |
| other post meta | `content_meta` under `wp.*` namespace (kept, not used) |

Exact meta key names for Yoast/Rank Math/ACF must be verified against real exports in
M3 before relying on this table.

## 4. Content conversion

- **Gutenberg** (`<!-- wp:paragraph -->…<!-- /wp:paragraph -->`): parse block comments
  into a tree; map core blocks (paragraph, heading, list, quote, image, gallery, code,
  preformatted, table, separator, embed, buttons, columns → flattened) to TipTap JSON
  nodes. Unknown blocks → `html` block with the inner HTML (sanitised) + report entry.
- **Classic HTML**: run `wpautop`-equivalent paragraph logic, then HTML → TipTap JSON
  via an HTML parser (`html5ever`/`scraper`).
- **Shortcodes** (`[gallery]`, `[caption]`, `[embed]`, `[video]`): core ones converted;
  others left as text inside an `html` block and listed in the report with counts and
  example posts.
- **URL rewrite**: media URLs (`/wp-content/uploads/…`, including size-suffixed
  `-300x200.jpg` variants) → new media URLs; internal links to old permalinks → new
  URLs.

## 5. Redirects

For every published item: old permalink (from `<link>` and computed from the WP
permalink structure if available) → new URL, `301`, `origin = import`. Also attachment
pages and category/tag base paths (`/category/x/` → new term URL). Skip when old == new.

## 6. Report

`import-report.html` (self-contained) + `import-report.json`:

- counts per type, created/updated/unchanged/kept/skipped/failed ("kept": changed in
  WordPress but edited here since the import wrote it, so left alone; an item the
  import scheduled that has since been published by its schedule counts as edited
  here too, because publishing bumps its version; M3-1 final review A2),
- unknown blocks and shortcodes with counts and links to affected items,
- missing media files,
- meta keys kept but unmapped (top 50 by frequency),
- users requiring password reset,
- redirects created.

Stored as an artifact and linked from the admin import screen.

## 7. Test corpus

- Small synthetic WXR fixtures per feature (checked in).
- 2–3 real exports of different sizes/plugins (Yoast vs Rank Math, Gutenberg vs classic)
  — sourced in M3; must be anonymised or properly licensed before checking in; otherwise
  kept out of the repo and run via an env-var path.
