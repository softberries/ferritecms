+++
description = "Back up and restore a FerriteCMS site, schedule backups, export a portable copy, copy a database and clean up media."
+++

# Backups and moves

| | Solo (SQLite) | Postgres |
|---|---|---|
| Back up | `ferrite backup`, or `[backup]` while `serve` runs | your provider's backups and point-in-time recovery; `[backup]` writes export archives |
| Restore | `ferrite restore` | the provider's restore, or `ferrite import-site` from an export |
| Portable copy | `ferrite export` / `ferrite import-site` | the same |

With SQLite, every command on this page except `media sync` needs `serve` stopped: a
running site holds the data folder's lock. A running SQLite site is backed up by the
schedule instead.

## Back up a solo site

```sh
ferrite backup --data ./mysite --to ./site-backup.tar.zst
```

The database is copied with `VACUUM INTO` and checked with `PRAGMA integrity_check`
before anything is written. The backup holds `ferrite.toml`, `secret.key`, the database
and the media files. `--no-media` leaves the media out; `--no-secret-key` leaves the site
key out (two-factor secrets then cannot be opened after a restore). `--to -` writes the
archive to standard output.

Nothing is encrypted: the backup holds password hashes, sealed secrets and the site key.
To encrypt it, write it to standard output (`--to -`) and pipe it to `age` or `gpg`.

### Into a repository

When `--to` is a folder or `s3://bucket[/prefix]`, it becomes a backup repository: each
run adds a snapshot under `snapshots/<UTC time>/`, and media files are stored once under
`artifacts/` and copied only when the repository lacks them, so later runs are
incremental.

```sh
ferrite backup --data ./mysite --to ../backups --keep 7
```

`--keep N` keeps the newest N snapshots and deletes older ones and the files no kept
snapshot needs. The folder must be empty or already a repository (it holds
`ferrite-backup-repo.json`), and must lie outside the data folder and the media folder.
S3 credentials come from the `AWS_*` environment variables.

Exit codes: 0 done; 2 done, but some media files were missing from the store (listed on
stderr); 3 the repository is of a newer format than this binary; 1 any other error.

`ferrite backup` refuses a Postgres site: use the provider's backups and
`ferrite export`.

## Scheduled backups

In `ferrite.toml`:

```toml
[backup]
schedule = "daily"     # or "weekly"; "off" by default
to = "../backups"      # a folder outside the data folder, or s3://bucket[/prefix]
keep = 7
with_media = true
```

`serve` then runs a `site.backup` job once per period, on one node. A daily backup is due
at 00:00 UTC and a weekly one on Thursdays at 00:00 UTC; the first `serve` in a period
runs it at once. On SQLite each run is a repository snapshot, as above; on Postgres it
writes an export archive into `<to>/exports/`. Failed runs show in the admin under
**System**, Jobs. `config check` and `serve` apply the repository rules above to `to`
(outside the data and media folders; a local folder absent, empty or already a
repository) and refuse to start when it breaks them.

## Restore

```sh
ferrite restore ./site-backup.tar.zst --data ./restored
ferrite restore ../backups --data ./restored                       # the newest snapshot
ferrite restore ../backups --data ./restored --snapshot 20261031T055625Z
```

The data folder must be absent or empty. With `--force` an existing folder is moved
aside to `<data>.pre-restore-<time>` first (never deleted). The media files go to the
store the restored `ferrite.toml` names. The database is not migrated: `restore` prints
the migration status, and `serve` migrates at start (`database.auto_migrate`). A failed
restore leaves the folder as it was.

Exit codes: 0 done; 3 the backup's schema is newer than this binary (nothing written);
1 any other error.

## A portable copy: export and import-site

`export` writes the site's content and settings into one archive that loads into any
fresh site, SQLite or Postgres:

```sh
ferrite export --data ./mysite --out site-export.tar.zst --with-media
ferrite init --data ./newsite --url https://new.example.com
ferrite import-site site-export.tar.zst --data ./newsite --with-media
```

The archive leaves out passing data (sessions, jobs, events, the search index, which is
rebuilt) and never holds `secret.key`. Copy the old `secret.key` (or `auth.secret_key`)
to the new site as well, or two-factor sign-in has to be set up again. On Postgres
`export` can run while the site serves, since it reads one consistent snapshot.
`import-site` needs a fresh database (made by `init`, nothing added yet) and loads
everything in one transaction, so a refused or damaged archive leaves no rows.

## Copy a database

`db copy` moves the database to another backend, for example from SQLite to Postgres:

```sh
ferrite db copy --from sqlite:./mysite/ferrite.db --to postgres://ferrite@db.example.com/ferrite --to-schema mysite
```

The target must be fresh, and a SQLite target's folder must exist. Media is not copied
(use `media sync`), and no `ferrite.toml` is written: the command prints the
`[database]` lines to use.

## Media storage

- `ferrite media sync --data ./mysite --to s3://my-bucket/site` copies every stored file
  to another store (`local:<path>` or `s3://…`); it opens no database, so it works while
  the site runs. It never deletes.
- `ferrite media gc --data ./mysite` deletes files nothing refers to any more, once they
  have been unused for `media.gc_grace_days` (7). `serve` runs the same clean-up daily.
  `--dry-run` only counts.
- `ferrite media regenerate --data ./mysite` makes missing image sizes (after a theme
  adds sizes); `--all` makes every size again.

### Copies and the media clean-up

Each database has an install id, and a copy made by `restore`, `import-site` or
`db copy` gets its own. The clean-up deletes files only from a store whose
`ferrite-owner.json` names this site's database, so a copy that shares the old media
store never deletes the original's files. When the copy really replaces the original
(a restore in place, a moved database), make it the owner:

```sh
ferrite media gc --data ./mysite --claim
```
