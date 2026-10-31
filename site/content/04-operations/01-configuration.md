+++
description = "Every ferrite.toml section and key with its default, the FERRITE_* environment overrides and secrets from files."
+++

# Configuration

A site is configured by `ferrite.toml` in its data folder. `ferrite init` writes one with
the common keys and comments; every key not written has the default below. Unknown keys
are an error, so a typo stops the site from starting rather than being ignored.

Check the file, with the environment applied exactly as `serve` applies it, without
touching the database or the media:

```sh
ferrite config check --data ./mysite
```

It prints `ok: mode=… database=… media=… workers=… mail=… metrics=… tls=… backup=…` and
exits 0, or lists every problem and exits 1.

## Values

- **Sizes** are a number of bytes or a string such as `"256MB"`, `"32MiB"`, `"512KB"` or
  `"1GB"`. `KB`, `MB` and `GB` mean the same as `KiB`, `MiB` and `GiB` (powers of 1024).
- **Durations** are a number of seconds or a string such as `"90s"`, `"10m"` or `"2h"`.
- **Secrets** (`database.url`, `auth.secret_key`, `mail.smtp_url`,
  `media.s3.access_key_id` and `media.s3.secret_access_key`) can also be read from a file:
  write `url_file = "/run/secrets/db_url"` instead of `url = "…"`. One trailing newline is
  removed. Setting both forms is an error.
- Relative paths are relative to the data folder.

## Environment overrides

Every key can be set with a `FERRITE_` variable: the section and key in upper case,
joined by a double underscore.

```sh
FERRITE_SERVER__LISTEN=127.0.0.1:0 ferrite serve --data ./mysite
FERRITE_DATABASE__MAX_CONNECTIONS=32 ferrite config check --data ./mysite
```

An unknown key is an error from the environment too: `FERRITE_SITE__NOPE=1` gives
`env:FERRITE_SITE__NOPE: unknown key`.

A variable wins over the file. Its value is read as a TOML value when it is one
(`true`, `16`, `["a", "b"]`), else as text; for keys that take text, the raw text is used.
`server.trusted_proxies` and `server.tls.domains` also take a comma-separated list.
`FERRITE_LOG` (the log level, for example `debug`; default `info`) and variables
starting with `FERRITE_TEST_` are not configuration and are never read as keys.

## [site]

| Key | Default | |
|---|---|---|
| `url` | required | the public URL, `http://` or `https://`. Links, feeds and the sitemap are built from it. |
| `title` | `""` (`init` writes `"My Site"`) | used until a title is set in the admin's **Settings**, which wins |
| `locale` | `"en"` | the same: a fallback for the **Settings** value |
| `timezone` | `"UTC"` | an IANA name such as `"Europe/Warsaw"`; dates in themes are shown in it. Read at start. |

## [server]

| Key | Default | |
|---|---|---|
| `listen` | `"127.0.0.1:8080"` | the plain HTTP address |
| `trusted_proxies` | `[]` | CIDRs or addresses of reverse proxies whose `X-Forwarded-For` is believed |
| `metrics_listen` | not set | an address for `GET /metrics`; must not use `listen`'s port on the same address, or on `0.0.0.0`/`::` on either side ([Monitoring](05-monitoring.md)) |

## [server.tls]

Built-in HTTPS for solo sites ([HTTPS](04-https.md)).

| Key | Default | |
|---|---|---|
| `mode` | `"off"` | `"off"` or `"acme"` |
| `contact` | required with `acme` | a `mailto:` address for the ACME account |
| `domains` | the host of `site.url` | DNS names for the certificate; must include that host |
| `directory` | `"production"` | `"production"` (Let's Encrypt), `"staging"` or an `https://` ACME directory URL |
| `listen` | `"0.0.0.0:443"` | the HTTPS address |
| `redirect_http` | not set | an address that answers every request with a `301` to the HTTPS site |

With `mode = "off"` the other keys are accepted but have no effect; `config check` warns
about them.

## [database]

| Key | Default | |
|---|---|---|
| `url` | `"sqlite://ferrite.db"` | `sqlite:…` (relative to the data folder) or `postgres://…`; also `url_file` |
| `schema` | not set | Postgres only: keep every table in this schema (letters, digits and `_`, at most 48 characters) |
| `max_connections` | `16` | |
| `auto_migrate` | `true` | apply pending migrations when `serve` starts |
| `allow_contract` | `false` | Postgres: also apply migrations that remove things, at start |

SQLite URLs take no `?` options.

## [media]

| Key | Default | |
|---|---|---|
| `backend` | `"local"` | `"local"` or `"s3"` |
| `root` | `"media"` | local only: the folder for the files |
| `local_shared` | `false` | local only: the folder is a volume shared by every node of a cluster |
| `max_upload` | `"256MB"` | the largest upload, at most `"1GiB"` |
| `image_workers` | `1` | image size jobs at once per node, 1 to 16 |
| `upload_idle_timeout` | `"30s"` | an upload that sends nothing for this long is cut off |
| `strip_metadata` | `false` | remove EXIF and XMP (GPS included) from JPEG, PNG and WebP uploads |
| `gc_grace_days` | `7` | days an unused file is kept before the media clean-up deletes it, 1 to 365 |

## [media.s3]

With `backend = "s3"`:

| Key | Default | |
|---|---|---|
| `bucket` | required | |
| `region` | required | |
| `endpoint` | not set | for S3-compatible stores, an `http(s)://` URL |
| `access_key_id`, `secret_access_key` | not set | both or neither; neither uses the `AWS_*` environment's credentials. Also `*_file`. |
| `prefix` | `""` | a key prefix inside the bucket |
| `allow_http` | `false` | allow a plain `http://` endpoint |
| `virtual_hosted_style` | `false` | `bucket.host` addressing instead of `host/bucket` |
| `public_base_url` | not set | link media to this URL (the bucket or a CDN, which must allow public reads of `artifacts/`) instead of serving it through the site |
| `copy_if_not_exists` | not set | how the store refuses to overwrite on copy, in object_store's form (`header:<name>:<value>`, `header-with-status:…`, `multipart`); not set, a check-then-copy is used |
| `conditional_put` | not set (`etag`) | `"etag"` or `"disabled"`, for stores that reject `If-None-Match` on create |

## [cluster]

| Key | Default | |
|---|---|---|
| `mode` | `"auto"` | `"solo"`, `"cluster"` or `"auto"`. `auto` is cluster when the database is Postgres and the media is shared (S3, or `local_shared = true`), else solo. |
| `node_name` | `$HOSTNAME-<pid>`, else `node-<pid>` | this node's name, at most 64 bytes |
| `workers` | the larger of 2 and the CPU count | background jobs at once on this node, 1 to 256 |

## [cache]

| Key | Default | |
|---|---|---|
| `max_bytes` | `"256MB"` | the page cache's size |
| `logged_in` | `"bypass"` | pages for signed-in visitors: `"bypass"` renders them without the cache, `"per-user"` caches them per user |
| `poll_interval_ms` | `250` | how often a node reads the changes that invalidate cached pages |
| `search_ttl` | `"60s"` | how long search results stay cached, up to `"1h"`; `0` caches none |

## [plugins]

| Key | Default | |
|---|---|---|
| `default_memory` | `"32MB"` | memory per plugin call |
| `default_fuel` | `50000000` | instructions per plugin call |
| `render_deadline_ms` | `200` | time per filter call while a page renders |
| `action_deadline_ms` | `5000` | time per action call |
| `render_budget_ms` | `1000` | time all filters of one page share; between `render_deadline_ms` and 60000 |
| `max_concurrent_calls` | the larger of 8 and 4 × the CPU count | plugin calls at once per node |
| `disable_after_failures` | `20` | failures within `failure_window` that disable a plugin |
| `failure_window` | `"10m"` | |
| `volatile_ttl` | `"60s"` | how long a page whose plugin calls failed or were marked volatile stays cached, 1 s to 1 h |

## [auth]

| Key | Default | |
|---|---|---|
| `secret_key` | not set | the site key, 64 hex characters (`openssl rand -hex 32`); also `secret_key_file` |

The site key seals stored secrets (two-factor secrets, sealed plugin settings, reset
links). A solo site without one uses `<data>/secret.key`, which `init` creates. Cluster
mode requires the key here, the same on every node.

## [mail]

Without this section the site sends no mail (no password reset by e-mail, no plugin
notices).

| Key | Default | |
|---|---|---|
| `smtp_url` | not set | `smtps://user:pass@host` (TLS), `smtp://host?tls=required` (STARTTLS) or plain `smtp://host`; credentials only over TLS or to a relay on the same machine. Also `smtp_url_file`. |
| `from` | required with `smtp_url` | `noreply@example.com` or `Site <noreply@example.com>` |

## [backup]

Scheduled backups while `serve` runs ([Backups and moves](02-backups.md)).

| Key | Default | |
|---|---|---|
| `schedule` | `"off"` | `"off"`, `"daily"` or `"weekly"` |
| `to` | required unless off | a folder outside the data folder (`"../backups"`) or `s3://bucket[/prefix]` |
| `keep` | `7` | snapshots (or, on Postgres, export archives) kept, 1 to 1000 |
| `with_media` | `true` | include the media files |
