+++
description = "Turn a folder of Markdown files into a tree of pages, and keep it in sync."
+++

# Import Markdown

`ferrite import markdown` turns a folder of Markdown files into pages. Folders become
parent pages, so the folder tree becomes the page tree. These docs are written this way
and imported with it.

```sh
ferrite import markdown ./docs --data ./mysite
```

With SQLite, stop `serve` first. Add `--dry-run` to run the whole import and then roll
it back, so you can read the report without changing the site.

## How files become pages

| In the folder | On the site |
|---|---|
| `guide.md` | a page with the slug `guide` |
| `02-guide.md` | a page with the slug `guide` and the order 2 |
| `setup/index.md` (or `README.md`) | the page for the folder `setup`; the other files in `setup/` are its children |
| a folder without an index | a page with an empty body, titled from the folder name (`getting-started` becomes "Getting started") |
| `index.md` at the top | a top-level page with the slug `home` |

A file or folder name that starts with digits and a dash (`01-`, `02-`) sets the order of
the page among its siblings; the digits are not part of the slug. Hidden files and
folders (names starting with `.`) are skipped, and symbolic links are never followed.

The page title is, in order: the `title` in the front matter, else the first `# heading`
(which is then taken out of the body), else the file name. Start each file with one
`# Title` and use `##` and `###` for its sections.

## Front matter

A file can start with TOML between two `+++` lines:

```toml
+++
title = "Install"
slug = "install"
order = 1
draft = false
date = "2026-10-30T12:00:00Z"
description = "How to install the binary."
+++
```

| Key | Effect |
|---|---|
| `title` | the page title |
| `slug` | the slug, instead of the one from the file name |
| `order` | the order among siblings, instead of the `NN-` prefix |
| `draft` | `true` imports the page as a draft |
| `date` | the publish date (RFC 3339 with an offset); a date in the future schedules the page |
| `description` | the excerpt and the SEO description |

Any other key, invalid TOML, or a YAML block between `---` lines makes the file skipped
and reported. Convert YAML front matter to TOML first.

## Markdown, links and images

The Markdown is CommonMark plus tables and strikethrough.

- A relative link to another `.md` file or folder of the import, such as
  `[Install](../01-getting-started/01-install.md)`, points at that page on the site.
  Fragments (`#section`) are kept.
- A link to a `.md` file that is not part of the import is reported as broken.
- A relative image inside the folder (`![Diagram](images/flow.png)`) is uploaded as a
  media item. An image outside the folder is never read and is reported.
- `http://` and `https://` links and images are kept as they are.

## The front page

`--front-page` makes the top-level `index.md` the site's front page, as long as it is
published:

```sh
ferrite import markdown ./docs --data ./mysite --front-page
```

## Run it again

The import remembers which file made which page, under a source name (the folder's name,
or `--source NAME`). Running it again with the same source:

- leaves unchanged files alone;
- updates the pages of changed files;
- **keeps** a page that was edited in the admin since the import wrote it;
- leaves the page of a renamed or deleted file on the site, and reports it.

## Read the result

The command prints the counts, for example
`created 12, updated 0, unchanged 0, kept (edited here) 0, media 1`, then one line per
problem with the file it concerns. The exit code is 0 when all is well, 2 when files were
skipped or images or links were missing or broken, 3 when another import is running, and
1 on an error.

## Limits

One import reads at most 5000 Markdown files and 5000 folders; a larger folder is an
error, and nothing is written. A file over 1 MiB, and anything in a folder nested deeper
than 64 levels, is skipped and reported.

Other options: `--type` imports into another content type (default `page`; it must be
hierarchical when the folder has subfolders, and `--front-page` works only with `page`),
and `--as LOGIN` names the admin the import runs as (default: the first active admin).
