# Import test corpus

Two small WordPress sites, written for FerriteCMS tests and exported on 2026-10-28 from a
scratch WordPress 7.1.3. All content (posts, users, comments, images) is synthetic.

| File | What it is |
|---|---|
| `site-a.xml` | WXR export of site A: Gutenberg blocks, Yoast SEO 27.x, ACF 6.8.10 (free), the custom type `book` with the taxonomy `genre`, menus, comments and every post status. 35 items, 4 authors. |
| `site-b.xml` | WXR export of site B: classic editor HTML, shortcodes, Rank Math 1.0.280. 7 items. |
| `site-a-uploads/`, `site-b-uploads/` | The `wp-content/uploads` folders of both sites (flat-colour PNG and JPEG files, their generated sizes, one PDF). |
| `site-a.php`, `site-b.php` | The builder scripts that created the content. |
| `corpus-types.php` | The mu-plugin that registers `book` and `genre` for site A. |

## How they were made

1. A scratch WordPress 7.1.3 with the plugins above activated (Yoast and ACF for site A,
   Rank Math for site B). For site A, `corpus-types.php` is loaded as a mu-plugin.
2. The builder runs with `wp eval-file site-a.php` (site B: `wp eval-file site-b.php` on an
   emptied site).
3. The site is exported as WXR (all content), and its `wp-content/uploads/` folder is copied
   next to the export.

The WordPress theme-test files (`themeunittestdata.wordpress.xml`, `64-block-test-data.xml`
from github.com/WordPress/theme-test-data) are GPL-2.0 and are not checked in; tests that use
them read them from the directory named by `FERRITE_WP_CORPUS` and print `SKIP` without it.
