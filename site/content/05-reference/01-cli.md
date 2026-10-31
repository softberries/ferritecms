+++
description = "Every ferrite command: synopsis, options and exit codes."
+++

# Command line

`ferrite <command> --help` prints the same details; `ferrite --version` the version.

## Conventions

- `--data DIR` is the site's data folder, the one holding `ferrite.toml`.
- With SQLite, `serve` holds the data folder's lock, so a command that opens the site's
  database (`migrate`, `seed`, `user`, `theme install`, `theme activate`, `theme list`,
  `plugin`, `import`, `media gc`,
  `media regenerate`, `export`, `backup` and the like) fails while it runs: stop `serve`
  first. `config check`, `healthcheck`, `media sync` and `theme pack` open no database,
  and `doctor` reads a running site without the lock. On Postgres there is no folder
  lock.
- Exit codes: 0 means done and 1 an error, unless the command lists others. A usage
  error (an unknown or missing option) exits 2 for every command, so a script that
  checks for a command's own exit 2 should also look at its output.
- Commands that ask for confirmation take `--yes`, which is required when standard input
  is not a terminal.

## Sites and databases

### init

```text
ferrite init --data DIR --url URL [--title TITLE] [--db URL] [--schema NAME] [--media s3://bucket/prefix]
```

Creates the data folder with `ferrite.toml` and a migrated database. `--title` defaults
to `My Site`; `--db` to SQLite in the data folder (`postgres://…` for Postgres, with
`--schema` for the schema to keep the tables in); `--media` to the data folder. A solo
site also gets its site key, `secret.key`. With Postgres and S3, set
`FERRITE_AUTH__SECRET_KEY` first.

### serve

```text
ferrite serve --data DIR
```

Serves the site. Prints `Listening on http://…` and stops cleanly on SIGINT or SIGTERM.

### migrate

```text
ferrite migrate status --data DIR
ferrite migrate up --data DIR [--to VERSION]
ferrite migrate down --data DIR --to VERSION [--allow-lossy] [--yes]
ferrite migrate verify --data DIR
```

`status` shows the database and binary versions, the pending migrations and a verdict:
exit 0 up to date (or a newer schema this binary can run), 2 migrations pending, 3 the
schema is incompatible with this binary, 1 an error. `up` applies pending migrations (up
to `--to`). `down` reverts everything above `--to`; a migration whose down loses data
needs `--allow-lossy`. `verify` checks the applied migrations' checksums.

### config check

```text
ferrite config check --data DIR
```

Loads `ferrite.toml` and the `FERRITE_*` variables as `serve` does, without opening the
database or the media. Prints `ok: mode=… database=… media=… workers=… mail=… metrics=…
tls=… backup=…` and exits 0, or lists the problems and exits 1.

### doctor

```text
ferrite doctor --data DIR [--offline]
```

Runs 25 checks and changes nothing ([Monitoring](../04-operations/05-monitoring.md)).
`--offline` skips the checks that reach other services (S3, SMTP, the ACME directory,
the release lookup); the database is still opened. Exit 3 when the schema is newer than
this binary allows, else 1 when a check is an error, else 2 when one is a warning, else
0.

### healthcheck

```text
ferrite healthcheck [--url URL]
```

Probes a plain `http://` URL (default `http://127.0.0.1:8080/readyz`): exit 0 on a 200,
else 1.

### seed demo

```text
ferrite seed demo --data DIR [--profile blog|business] [--force]
```

Adds demo content: posts, pages, terms, menus and a sample image. `--profile business`
adds a small-business site with a static front page for the `business` theme. `--force`
adds another batch even when the site has content.

## Users

```text
ferrite user create --data DIR --login LOGIN --email EMAIL [--role admin|editor|author|contributor] [--display-name NAME] [--password-stdin]
ferrite user reset-password --data DIR --login LOGIN [--password-stdin]
ferrite user list --data DIR
ferrite user disable-totp --data DIR --login LOGIN
```

- `create` makes an account (role `admin` by default). Without `--password-stdin` it
  prints a generated one-time password, which must be changed at the first sign-in. A
  login is `a-z`, `0-9`, `.`, `_` and `-`, starting with a letter or digit.
- `reset-password` sets a new password and ends every session of the account; its API
  tokens are kept.
- `list` prints every account's login, role, status and e-mail.
- `disable-totp` turns two-factor sign-in off, for a lost authenticator.

## Themes and plugins

```text
ferrite theme install --data DIR --dir THEME_DIR [--activate]
ferrite theme activate --data DIR NAME
ferrite theme list --data DIR
ferrite theme pack --dir THEME_DIR --out FILE
```

- `install` stores a theme folder (holding `theme.toml` and `templates/`); one that does
  not compile is refused. `--activate` makes it the active theme.
- `activate` makes an installed or built-in theme the active one.
- `list` prints `<name>  <version>  <installed|embedded>  [active]`.
- `pack` writes the bundle `POST /api/v1/themes` and the admin take, and needs no data
  folder. It prints `packed <n> files, <bytes> bytes, sha256 <hex>`.

```text
ferrite plugin check --data DIR --dir PLUGIN_DIR
ferrite plugin install --data DIR --dir PLUGIN_DIR [--enable] [--yes]
ferrite plugin list --data DIR
ferrite plugin enable --data DIR NAME
ferrite plugin disable --data DIR NAME
ferrite plugin remove --data DIR NAME [--purge]
```

- A plugin folder holds `ferrite-plugin.toml` and one `*.wasm`.
- `check` prints the permission report and stores nothing; exit 1 when the manifest or
  the component is refused.
- `install` prints the report and asks before installing (or updating); `--yes` approves
  it. A new plugin stays disabled unless `--enable`.
- `list` prints `<name>  <version>  <enabled|disabled>  [<reason>]`.
- `remove` keeps the plugin's stored data and settings for 30 days; `--purge` deletes
  them now.

## Imports

### import wordpress

```text
ferrite import wordpress FILE --data DIR [--uploads DIR_OR_ZIP] [--fetch-media] [--dry-run] [--report DIR] [--no-keep-permalinks] [--no-site-options] [--category-base BASE] [--tag-base BASE] [--as LOGIN] [--takeover]
```

Imports a WordPress export file (WXR). `--uploads` is a copy of `wp-content/uploads` or
a `.zip` of it; `--fetch-media` downloads attachments missing from it from the old site.
`--dry-run` runs everything, then rolls it back. `--report` is where `report.json` and
`report.html` go (default `<data>/import/reports/<run-id>/`). `--category-base` and
`--tag-base` (default `category` and `tag`) are the old site's, for redirects. `--as` is
the admin who imports (default the first active one). `--takeover` takes over an import
whose process stopped (no heartbeat for 120 s).

Exit 0 done; 2 done, with refused redirects, missing media or attachments refused for
their size or type (in the report); 3 another import is running; 1 an error.
See [Import from WordPress](../02-guides/06-import-wordpress.md).

### import markdown

```text
ferrite import markdown DIR --data DATA [--as LOGIN] [--type KIND] [--front-page] [--dry-run] [--source NAME] [--takeover]
```

Imports a folder of Markdown files as pages (`--type`, default `page`). `--front-page`
makes the root `index.md` (or `README.md`) the front page. `--source` names the import
(default the folder's name); a re-run with the same name updates what changed in the
files and keeps pages edited in the admin. `--takeover` takes over an import whose
process stopped (no heartbeat for 120 s).

Exit 0 done; 2 done, with skipped files, missing or refused images or broken links
(each printed); 3 another import is running; 1 an error.
See [Import Markdown](../02-guides/07-import-markdown.md).

## Backups, copies and media

### backup

```text
ferrite backup --data DIR --to FILE.tar.zst|-|FOLDER|s3://bucket[/prefix] [--no-media] [--no-secret-key] [--keep N]
```

SQLite sites only. Exit 0 done; 2 done, but media files were missing from the store; 3
the repository's format is newer than this binary; 1 an error.

### restore

```text
ferrite restore SOURCE --data DIR [--snapshot NAME] [--force]
```

`SOURCE` is a backup `.tar.zst`, `-` for standard input, or a repository. A restored
`ferrite.toml` whose media folder is an absolute path inside a folder `--force` moves
aside (or inside an absent one) is refused. Exit 0 done; 2 restored, but the check of
the restored folder afterwards failed (the site is in place: look at it before serving);
3 the backup's schema is newer than this binary (nothing written); 1 an error (nothing
put in place).

### export

```text
ferrite export --data DIR --out FILE|- [--with-media]
```

Exit 0 done; 2 done, but media files were missing from the store; 3 the database's
schema is newer than this binary; 1 an error (an older schema too: run `migrate up`
first). A failed export removes its file.

### import-site

```text
ferrite import-site ARCHIVE|- --data DIR [--with-media]
```

Loads an export into a fresh site made by `init`. Exit 0 done; 3 the archive's format or
schema is newer than this binary; 1 an error.

### db copy

```text
ferrite db copy --from URL --to URL [--from-schema NAME] [--to-schema NAME]
```

A URL is `postgres://…` or `sqlite:<path to ferrite.db>`. The target must be fresh.
Media is not copied. Exit 0 done; 3 the source or target schema is newer than this
binary; 1 an error.

### media

```text
ferrite media sync --to STORE (--from STORE | --data DIR)
ferrite media gc --data DIR [--dry-run] [--grace-days N] [--claim]
ferrite media regenerate --data DIR [--all] [--item ID]
```

- `sync` copies every stored file from one store to another; a store is
  `local:<path>` or `s3://bucket[/prefix]` (credentials from the `AWS_*` environment).
  It never deletes. Prints `copied N (B bytes), skipped M, failed F`. Exit 0, 2 when some
  copies failed, 1 an error.
- `gc` deletes stored files nothing refers to, after the grace period (`--grace-days`,
  default `media.gc_grace_days`, 7). `--claim` makes this site the owner of its media
  store first. Exit 0 (also for the first run on a store, which only writes
  `ferrite-owner.json`); 2 when the store belongs to another site (or its marker cannot
  be read) or a delete failed; 3 when another clean-up or a backup holds the lock; 1 an
  error.
- `regenerate` makes missing image sizes; `--all` redoes every image, `--item` one.
  Prints `regenerated N, skipped S, failed F`. Exit 0, 2 when some items failed, 1 an
  error.

See [Backups and moves](../04-operations/02-backups.md).

## Updates

### self-update

```text
ferrite self-update [--version X.Y.Z] [--check] [--yes] [--data DIR [--skip-config-check]]
```

Updates this binary to a newer signed release. With `--data` the new binary's `config
check` and `migrate status` run for the site first; a configuration it refuses stops the
update, unless `--skip-config-check`. Exit 0 updated or already up to date; 2 a newer
release exists (with `--check`); 1 an error (an older or equal `--version` too: use
`rollback`). A build from source has no release key and refuses.

### rollback

```text
ferrite rollback [--data DIR] [--schema] [--allow-lossy] [--restore-snapshot] [--yes]
```

Goes back to `ferrite.prev`; with `--schema` the schema too, and with
`--restore-snapshot` (SQLite) the pre-upgrade snapshot instead of down migrations. On
Postgres `--schema` refuses while a node is running; `--restore-snapshot` refuses a
snapshot at another schema than the upgrade started from. Exit 0 rolled back; 3 the
previous binary cannot open this schema; 1 an error.
See [Upgrades and rollback](../04-operations/03-upgrades.md).
