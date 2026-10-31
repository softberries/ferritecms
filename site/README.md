# The FerriteCMS documentation site

The documentation is written as Markdown in [`content/`](content/) and published with
FerriteCMS itself: `ferrite import markdown` imports the folder as a tree of pages and the
built-in `docs` theme renders it.

## Build and run it locally

From the repository root:

```sh
site/build.sh /tmp/ferrite-docs http://localhost:8099
```

The script:

1. creates the data folder with `ferrite init --title FerriteCMS` (the folder must not
   hold a site yet);
2. creates an admin account `docs` without showing a password; to sign in to the admin,
   set one with `ferrite user reset-password --data /tmp/ferrite-docs --login docs` (with
   `serve` stopped);
3. runs `ferrite import markdown site/content --front-page --source ferrite-docs`;
4. activates the `docs` theme;
5. prints the command that serves the site; for a loopback URL (`localhost`,
   `127.0.0.1` or `[::1]`) it starts with the `FERRITE_SERVER__LISTEN` the URL's port
   needs (none for the default `127.0.0.1:8080`).

It stops with a non-zero exit code when any step fails. An import that exits 2 (a file
skipped for bad front matter, a missing image or a broken link) fails the build too. A
failed build leaves the folder as far as it got, and `init` refuses an existing site: fix
the files, then re-run the import into that folder (below) and activate the theme
(`ferrite theme activate --data /tmp/ferrite-docs docs`), or build into a new folder.

`FERRITE` names the command that runs ferrite (default `cargo run -q -p ferrite --`), for
example `FERRITE=target/release/ferrite site/build.sh …`. Build the admin first
(`pnpm -C admin install && pnpm -C admin build`) if you want to sign in to it.

To pick up edits, re-run the import into the same folder, with `serve` stopped:

```sh
cargo run -q -p ferrite -- import markdown site/content --data /tmp/ferrite-docs --front-page --source ferrite-docs
```

Pages changed in the files are updated; pages edited in the admin since the last import are
kept.

## Writing pages

- One file per page. Start with a `# Title` (or a `title` in the front matter) and use `##`
  and `###` for sections: the theme's "On this page" list shows those two levels.
- `NN-` prefixes on files and folders set the order in the sidebar; a folder's `index.md`
  is the folder's page.
- Optional front matter is TOML between `+++` lines (`title`, `slug`, `order`, `draft`,
  `date`, `description`).
- Link to other pages with relative `.md` links; the import turns them into the pages'
  addresses. A link to a `.md` file that is not in `content/` is a broken link and fails
  the build, so refer to repository files by path in code font instead.
- Commands must be copy-pasteable, and every claim must be checked: add a row for it to
  [`VERIFIED.md`](VERIFIED.md).

## Hosting

The site is not hosted anywhere yet. Where it is published, and under which domain, is
still to be decided.
