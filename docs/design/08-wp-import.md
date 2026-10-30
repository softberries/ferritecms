# 08 — WordPress Import

## 1. Entry points

```
ferrite import wordpress export.xml --data ./site [--uploads ./wp-content/uploads|uploads.zip]
    [--fetch-media] [--dry-run] [--report DIR] [--no-keep-permalinks] [--no-site-options]
    [--category-base category] [--tag-base tag] [--as LOGIN] [--takeover]     # M3-1
POST /api/v1/import/wordpress            # multipart wxr, uploads?, options? → 202 + job (M3-2)
GET  /api/v1/import/jobs/{job_id}        # the job and its run (what the admin polls)
GET  /api/v1/import/runs[?limit&cursor], /import/runs/{id}, …/report.json, …/report.html
/admin/import                            # the admin screen (`g i`, 11 §7)
```

All use `ferrite-import-wp`, which writes through core's import API
(`ferrite_core::import`: `ImportService` starts a run, `ImportBatch` writes one
transaction; not SQL), so imports work on either backend and media goes to whichever
MediaStore is configured. Only an active admin may import (`Capability::Import`, checked
once when the run starts). The CLI exits 0 when done, 2 when done with refused redirects
or missing or refused media, 3 when another import is running, 1 on any error; a run that
failed after it started still writes its partial report (M3-2 P29). `--uploads` takes the
uploads folder or a `.zip` of it, read in place (`ZipSource`, P24).

**Through the API (as built, M3-2; 07 §1).** The POST streams `wxr` (at most 512 MiB) and
the optional `uploads` zip (at most 4 GiB) into the media store as `import` artifacts and
queues an `import.wordpress` job as the caller, answering `202` with the job and
`Location: /api/v1/import/jobs/{job_id}`; `options` (JSON, at most 4 KiB) carries
`dry_run`, `keep_permalinks`, `site_options`, `fetch_media`, `category_base` and
`tag_base` with the CLI's defaults; `takeover` is never the caller's. While an import job
is queued or running, or a run is `running` with a fresh heartbeat, it is `409`. The job
(04 §6; 6 h timeout, 3 attempts) runs on whichever node claims it: it checks the user is
still an active admin, fetches the inputs to `<data>/import/job-<id>/`, runs the import,
stores the report (a failed run's partial one too) and deletes the inputs once the run is
`done` or the job fails for good. A retry on the same node resumes the run's IR; one on
another node parses again (`import_map` keeps that idempotent). Rulings: P1–P34 and
R1–R83 in `superpowers/rulings/m3.md`.

## 2. Pipeline

```
WXR (quick-xml, streaming) ─► parse: records into the IR (temp SQLite)
   ─► pass 1: authors → users, custom types and taxonomies (with ACF fields), terms
   ─► pass 2: attachments → media (files from --uploads, or <wp:attachment_url> with --fetch-media)
   ─► pass 3: permalink.post inferred, then posts/pages/custom types → content
              (block conversion, URL rewrite, SEO and ACF mapping, meta)
   ─► pass 4: comments, menus (nav_menu_item), redirects, options (title, tagline)
   ─► fixup: items whose links named items written after them are rewritten
   ─► report (report.json + report.html)
```

- Streaming parse: the parser runs on a blocking thread and hands records through a
  queue of 8, so memory stays near the largest item; a DOCTYPE with an internal subset,
  invalid UTF-8 and a text node over 64 MiB are refused (P22).
- The IR is a temp SQLite file at `<data>/import/<run-id>/ir.sqlite` (even when the site
  is on Postgres): tables `channel`, `authors`, `terms`, `items(seq, kind, wp_id,
  record_json)`, `passes(name, done_at)`, `meta` (the WXR's SHA-256), and the passes'
  indexes `media_urls`, `content_paths` and `fixups` (the fix-up queue, written before
  the batch that needs it commits). A run of the same file after a failed one resumes it
  (the folder is found by the digest and renamed to the new run's): finished steps are
  skipped. The folder is deleted when the run is done; a dry run's is deleted on every
  exit and is never resumed (R49).
- Idempotent: `import_map(source, kind, wp_id → ferrite_id, hash, version)` in the site
  database (migration `0009_import`, both backends; kinds `user`, `term`, `media`,
  `content`, `comment`, `menu`, `option`, `redirect`) lets a re-run update instead of
  duplicate. `source` is the normalised `<wp:base_blog_url>`. The hash covers what is
  written (the converted item, the resolved menu, the option value). A re-run leaves an
  unchanged hash alone, updates an item still at the version the import left, and keeps
  (and reports) one edited here since (P4). Terms and comments are matched, not updated
  (R46, R15). Items also get `import.wp.id` meta, never read back.
- Runs: `import_runs(id, source, token, state, started_by, started_at, heartbeat_at,
  finished_at, stats, dry_run, job_id, step, report_hash)` (at most one `running`, by a
  partial unique index; the last three since `0010`). Every batch transaction starts
  with an `UPDATE` of the run's heartbeat matched on its token, so a taken-over run cannot
  write (P5); the step (`parse`, `pass1`…`pass4`, `fixup`, `report`) is written the same
  fenced way as each step begins, between batches (P28). A run is stale after 120 s
  without a heartbeat; the CLI takes one over only with `--takeover`, a job when the run
  has no job, is its own, or its job is not `running` (R69), and an admin's POST abandons
  it (R65). The `import` lock is best-effort.
- Transactions in batches of at most 500 items; each batch emits one `invalidate` event
  (`[all]` past 1 000 keys). Imported items get a revision and their search entry, no
  plugin actions (`post.saved`, `post.published`), and scheduled ones their
  `content.publish_scheduled` job (P6).
- `--dry-run` runs everything and rolls every batch back; media is staged into a
  throwaway store under the run's folder (R29). The report says which counts are not
  meaningful (later batches cannot see earlier ones' rows).

## 3. Mapping

| WordPress | FerriteCMS |
|---|---|
| `post`, `page` | built-in types; hierarchy and `menu_order` preserved |
| custom post types | auto-created content types (non-built-in), fields from ACF if present |
| `attachment` | `media` items; `_wp_attachment_image_alt` → `alt`; parents not imported (R28) |
| `category`, `post_tag`, custom taxonomies | taxonomies + terms with parents |
| `wp:status` publish/draft/pending/private/future/trash | same statuses (`future` → `scheduled` + job; a past `future` date → published, reported); auto-draft, `inherit` outside attachments and `request-*` skipped (P7) |
| dates | `published_at`/`created_at` = `post_date_gmt` (`post_date` read as UTC when the GMT date is zero), `updated_at` = `post_modified_gmt` (P7) |
| `post_name` | the slug where core allows it, else `-N`, reported (P8) |
| authors | users matched by e-mail, then login; else created as `author` (WXR has no roles), no password, `must_reset`; a missing or invalid e-mail becomes `wp<id>.<login>@users.invalid` (P9, R26); content of an author not in the WXR → the importing admin |
| comments | plain-text comments; `1` → approved, `0` → pending, `spam`, `trash`; threading kept; pingbacks and trackbacks skipped; author fields cleaned to `submit`'s rules and reported (P18, R18) |
| `nav_menu_item` | one menu per `nav_menu` term (name = slug; `<name>-wp` beside a native menu of that name); post type → item, taxonomy → term, custom → url; no theme locations (not in WXR) (P19, R39) |
| channel `<title>`, `<description>` | `site.title`, `site.description` unless `--no-site-options` (P20) |
| Yoast `_yoast_wpseo_title`, `_yoast_wpseo_metadesc`, `_yoast_wpseo_canonical`, `_yoast_wpseo_meta-robots-noindex` (`1` noindex), `_yoast_wpseo_meta-robots-nofollow` (`1` nofollow), `_yoast_wpseo_meta-robots-adv`; Rank Math `rank_math_title`, `rank_math_description`, `rank_math_canonical_url`, `rank_math_robots` (PHP-serialised) | `seo.title`, `seo.description`, `seo.canonical`, `seo.robots` (a sorted subset of `noindex`, `nofollow`, `noarchive`, `nosnippet`, `noimageindex`); `%%title%%`/`%title%`, sitename and `sep` (`-`) substituted, other variables removed and counted; a canonical equal to the old `<link>` dropped (P10; keys verified in the plugin source and exports A and B, 2026-10-28) |
| `_thumbnail_id` | the `featured_image` field |
| ACF `acf-field-group`/`acf-field` items in the export | fields of an auto-created custom type: text, textarea, email, url, password → `text`; wysiwyg → `rich_text`; number, range → `number`; true_false → `bool`; date pickers → `date`; single select, radio, button_group → `select`; image, file → `media`; single post_object, page_link → `relation`; relationship, gallery, checkbox → `repeater` of one field; ACF Pro repeater → `repeater`; others stay `wp.<name>` meta, reported; on `post`/`page` all ACF values stay `wp.<name>` meta (P16) |
| other post meta | `content_meta` under `wp.*` (kept, not used; a repeated key becomes a JSON array, R35); the top 50 keys are reported |

## 4. Content conversion

- **Gutenberg** (`<!-- wp:paragraph -->…<!-- /wp:paragraph -->`): block comments are
  parsed into a tree (WordPress's grammar; attributes with a literal `-->` stay text,
  R25) and mapped to block-tree nodes: paragraph, heading, list, list-item, table,
  pullquote, code, preformatted, verse, quote, separator, image, gallery (a run of
  images), embed, video/audio/file (a link), html, shortcode and freeform (as classic
  HTML); buttons, columns, group, row, stack, details, cover and media-text are
  flattened; spacer and more are dropped; nextpage is dropped and counted. An unknown
  block becomes an `html` node with its inner HTML (dropped when that is empty, such as
  `core/latest-posts`), counted with up to 5 example items (P11, P12).
- **Classic HTML**: `wpautop`-equivalent paragraph logic and WordPress's auto-embed of a
  bare YouTube or Vimeo URL line, then HTML → nodes with `scraper` on `html5ever =0.40.1`
  (R24).
- **Shortcodes**: `[caption]`/`[wp_caption]`, `[gallery]`, `[embed]`, `[video]` and
  `[audio]` are converted; any other stays as text in its paragraph, counted with up to
  5 example items; none expands inside `<pre>` (R30).
- **Nodes added for the import** (P11): `embed` (`attrs.url`, `attrs.provider` =
  `youtube` or `vimeo`; rendered as a privacy-mode `<iframe>`:
  `https://www.youtube-nocookie.com/embed/<id>`, `https://player.vimeo.com/video/<id>`;
  other providers become a link) and an optional `caption` on `image` (rendered as
  `<figure>…<figcaption>`); both editable in the admin. Underline, sub, sup, colour and
  alignment are dropped with the text kept, and counted; `h1` is demoted to `h2`.
- **Limits** for untrusted markup: html5ever is not run on markup nested past 512 levels
  or with more than 1 024 `object`/`applet`/`marquee` elements (kept as plain text,
  counted as `too deep`; R31–R34); blocks nested past 48 levels are flattened to text so
  the stored JSON stays readable (R48); NUL is stripped from every string.
- **URL rewrite** (P14): every URL form of an attachment (its URL, each size file of
  `_wp_attachment_metadata`, the `-scaled` original; host-insensitive, under the
  source's `/wp-content/uploads/`) maps to its media item: an `img` src becomes
  `ferrite:media/<uuid>`, a link the media's public URL. Links to an old item URL (its
  `<link>`, `?p=N`, `?page_id=N`) become the new item's path, in the fix-up step for
  items written later; links to drafts and trash resolve too (R36). A file that is not
  found keeps its old URL and is reported missing.

## 5. Redirects

WXR carries no options, so the old URL of an item is its `<link>`. With the default
`--keep-permalinks`, pass 3 first infers a post structure when every published post's
`<link>` path fits one pattern of `%year%`, `%month%`, `%slug%`, `%category%` and
lower-case literals, and sets `permalink.post` to it, on the first import only while the
site still has core's default (R37). Then every published item whose new path differs
from its old one gets a `301` (`origin = import`), and so do attachment pages (to the
media URL), tags (`/tag/<slug>/`) and categories (`/category/<slug>/`) whose term URL
differs; `--category-base` and `--tag-base` name the old bases. Old == new is skipped;
rows the redirect rules refuse (2-cycles included, R17) are reported, not fatal. A stored
redirect the import did not write, or that was changed since, is kept and noted (R41).
`--no-keep-permalinks` keeps the site's structure and only redirects. When the import
sets `permalink.*`, the write enqueues a `permalink.redirects` job like any other change
of those options (M3-2 P30, 06 §3), which may add redirects from paths the old
structure gave the imported posts here, even though they never served (R63).

## 6. Report

`report.json` + `report.html` (one self-contained page), written by the CLI to
`<data>/import/reports/<run-id>/` or `--report DIR`, and by the job as `import`
artifacts (as built, M3-2 P29): the two files plus a manifest
`{"v":1,"json":"<sha256>","html":"<sha256>"}` that `import_runs.report_hash` names,
served by `GET /import/runs/{id}/report.json` and `…/report.html` (the HTML under
`Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox`) and
linked from the admin's runs table. A run that failed after it started has a report too:
`failed: {step, error}` and a first note saying so; its counts cover only what that
attempt did (P29). Contents:

- counts per type, created/updated/unchanged/kept/skipped/failed ("kept": changed in
  WordPress but edited here since the import wrote it, so left alone; an item the
  import scheduled that has since been published by its schedule counts as edited
  here too, because publishing bumps its version; M3-1 final review A2),
- unknown blocks and shortcodes with counts and links to affected items,
- missing media files,
- meta keys kept but unmapped (top 50 by frequency), and ACF fields with no mapping,
- users requiring password reset,
- redirects made and refused, slugs core changed, `future` items already past,
- what the conversion dropped (`dropped_formatting`: marks, tags, `h1 demoted`, `too
  deep`, page breaks; the top 100 kinds plus `other`),
- the inferred permalink structure, and notes grouped by kind.

Lists keep at most 1 000 entries with `totals` counting every one seen, notes at most 500
kinds, the HTML page 200 rows per table (R42). An attempt that replaces an earlier one's
report leaves the old files to the artifacts GC, which deletes them once they have been
unreferenced for the grace period (03 §6).

## 7. Test corpus

- Checked in (`crates/ferrite-import-wp/tests/corpus/`): exports A (Gutenberg, Yoast,
  ACF) and B (classic, Rank Math), written by us and exported from a scratch
  WordPress 7.1.3, their uploads (128 408 bytes) and the builder scripts.
- WordPress's theme-test files (`themeunittestdata.wordpress.xml`,
  `64-block-test-data.xml`; GPL-2.0, not checked in): `FERRITE_WP_CORPUS=<dir> cargo test
  -p ferrite-import-wp --test corpus_real` imports both into a fresh site; without the
  variable the test prints `SKIP`.
- No anonymised production export was used in M3-1.

## 8. Markdown import

`ferrite import markdown DIR` (M4-3b P62, rulings R-b4 to R-b6 and R-b9; CLI in 09 §8; crate
`ferrite-import-md`) imports a folder of Markdown files as pages. It does not use the WXR
pipeline above, but it shares the run and map rules:

- **Run.** An import run of source `markdown:<NAME>` (`--source`, default the folder's
  name), started with `ImportService::start` under the same lock and fence as a WordPress
  run, so exit 3 means another import is running (any source). It records no steps and has
  no work file or report file: the report is printed.
- **Map key.** `import_map.wp_id` is `map_id(s)`: the first 8 bytes of SHA-256 of a string
  `s` as a big-endian `i64`, masked to be non-negative. Kind `content` for a page: `s` is
  the file's path relative to `DIR` (`/` separators), except for a folder's page (R-b9),
  whose `s` is the folder's path plus `/` (`/` for the root): one key whether an
  `index.md`/`README.md` fills the page or it is synthesized, so adding or removing the
  index later updates that page in place (same id, slug and URL; its children stay
  under it), and links to the folder or to its index reach the same page. Kind `media`
  for an image: `s` is the image's path. Pages and media also get the meta `import.md.id`
  with that key (`ImportContent.id_meta`; WordPress items keep `import.wp.id`).
- **Map hash.** A page: SHA-256 over the file's bytes and the format version `md1` (a
  synthesized folder page: `md1` and the folder's path), then the parent page's key string
  (R-b6), so a switch between an index and a synthesized page is an `updated`, and a page
  whose parent page changed is updated too. An image: SHA-256 over its bytes and `md1`.
- **Re-runs**, as for WordPress items (§2, P4):

  | The file since the last run | The page since the import wrote it | Outcome |
  |---|---|---|
  | unchanged (same hash) | any | `unchanged`: nothing written |
  | changed | not edited (same version) | `updated` in place |
  | changed | edited here | `kept (edited here)`: nothing written |
  | renamed or deleted | any | stays on the site, reported `not-in-source` |
  | skipped (bad front matter, unreadable, too large, not UTF-8; or under a folder that could not be read or is too deep) | any | nothing written, reported (`bad-front-matter`, `unreadable`, …), never `not-in-source` |
  | new | none | `created` |

  A skipped `index.md`/`README.md` leaves its folder's page as an earlier run left it; on
  a first run the folder gets the empty stand-in page so its files still have a parent
  (`MdDoc.index_skipped`).

  A mapped image is read only to compare its hash (a new one is hashed while it is
  staged, so each file is read once). One whose bytes changed is never replaced by a
  re-run: the media item keeps the file it was first imported with (core does not
  replace a media item's file) and the run reports `image-changed`; replace it in the
  media library. A mapped image that is now gone, unreadable or refused keeps its media
  item in the pages that name it (reported `missing-image`, `unreadable` or
  `refused-image`), so the link pass, which converts a page's whole body again, does not
  drop it from a page whose file did not change.
- **Passes.** 1 media, 2 content (parents first; relative links kept as written; then the
  front page), 3 links: every page created, updated or unchanged (not one kept, nor one
  edited here since) is converted again with its links pointing at the target pages'
  paths, and written with `ImportBatch::set_body_and_fields` only when that body differs
  from the stored one. That write moves the map row's version with the item's, so it does
  not count as an edit here on the next run; a page whose own file did not change still
  gets the new path of a target whose slug changed. Batches commit one by one (at most 500
  items); a dry run uses one transaction for the whole run and rolls it back.
- **Slugs.** A sibling's taken slug gets core's `-N` (the later file in name order;
  `01-install.md` before `install.md`, folder `guides/` before `guides.md`), reported
  `slug-changed`; links resolve to each page's real path.
- **Front page.** `--front-page` writes `reading.front_page` (map kind `option`, id 1, the
  value's hash) on the first run, and on a re-run only while the site still holds what
  the import last set; else the site's choice is kept, reported `front-page-kept`.
- **Never read:** a path above `DIR`, through a symlink (files are opened with
  `O_NOFOLLOW` on unix), rooted or absolute, or hidden. Such images are reported `outside`
  or `missing-image`, a symlinked `.md` file or folder `outside`, and the rest of the
  folder still imports (exit 2). A rooted image source (`/x.png`) is a URL on the site:
  it is reported `outside`, never read from disk, and kept in the body as written; a
  relative one that is not imported is left out of the body.
- **Markdown.** GFM table column alignment (`|:--|--:|`) is dropped without a note: the
  editor's table cells have no alignment.
