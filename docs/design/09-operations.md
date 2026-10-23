# 09 — Operations: Upgrade, Rollback, Backup

## 1. Upgrade (solo)

```
ferrite self-update [--version X] [--channel stable]
```

1. Download release for this target + signature; verify (minisign/ed25519 public key
   embedded in the binary).
2. Run the **new** binary's `ferrite migrate status --data …` to show what will happen.
3. Copy current binary to `ferrite.prev` (alongside, same dir).
4. Atomically replace the binary.
5. Restart (systemd: `Restart=always` + exit with a dedicated code, or print the
   command when not under a supervisor).
6. New binary starts → pre-migration snapshot (`VACUUM INTO`) → applies migrations →
   writes `data/upgrade.json` (`{from, to, schema_from, schema_to, snapshot, at}`).

### Version skipping

- **Forward**: any jump is allowed (0.1 → 0.5). Migrations are cumulative and run in
  order; data backfills needed by a contract step live in the migrations themselves,
  not in "release N+1 writes both" application code alone.
- **Backward**: binary rollback is supported only to the **binary that was running
  immediately before** the upgrade (`ferrite.prev`, recorded in `upgrade.json`). If the
  jump crossed a contract migration, `min_binary_version` will make that old binary
  refuse to start and the operator is told to use `rollback --schema`.

## 2. Rollback (priority #1 per D3)

There are two cases, both supported:

### a) Expand-only upgrade (the normal case)

Because of expand/contract ([02 §8](02-storage.md#8-upgrade-compatibility-expandcontract--d8)),
the previous binary runs against the new schema. Rolling back the binary is enough:

```
ferrite rollback            # run with either binary
```

- Swaps `ferrite.prev` ↔ `ferrite` and restarts. No schema change. Instant.

### b) Schema must go back too

Needed when the upgrade included a contract migration, or the operator wants the exact
old schema:

```
ferrite rollback --schema   # must be run with the NEWER binary (it holds the down migrations)
```

1. Read `data/upgrade.json`.
2. Run `down` migrations to `schema_from` (refuses lossy steps unless `--allow-lossy`;
   instead offers `--restore-snapshot`).
3. `--restore-snapshot`: stop-the-world restore of the pre-upgrade SQLite snapshot
   (current DB moved to `backups/rolled-back-<ts>.db`, never deleted automatically).
   Content written since the upgrade is lost in this mode — the tool says so and asks
   for confirmation.
4. Swap binaries, restart.

The important ordering point: **down migrations ship with the newer binary**, so schema
rollback is always driven by the newer binary before swapping back.

## 3. Upgrade & rollback (cluster / Postgres)

- Deploy is the operator's (container image tag, systemd, k8s). Recommended runbook,
  shipped in docs:
  1. Take a provider snapshot (or rely on PITR; note the timestamp).
  2. `ferrite migrate status` with the new image → review.
  3. Roll nodes ([04 §9](04-cluster.md#9-rolling-deploys)); first new node migrates.
- **Rollback**: redeploy the previous image. Expand-only → done.
  If a contract migration ran: either `ferrite migrate down --to N` with the new image,
  then redeploy old; or provider PITR to the noted timestamp.
- `ferrite migrate status` prints the compatibility verdict for every version that the
  DB has seen (`schema_meta.min_binary_version`), so operators know whether the old
  image can run.

## 4. Backup & restore

| | Solo (SQLite + local media) | Cluster (Postgres + S3) |
|---|---|---|
| Primary | `ferrite backup --to <dir|file.tar.zst|s3://…>`: `VACUUM INTO` snapshot + `ferrite.toml` + media artifacts (incremental by hash) | Provider automated backups + PITR; S3 bucket versioning/replication |
| Portable | `ferrite export` ([02 §10](02-storage.md#10-portable-export-d14)) | same — independent of the provider |
| Schedule | `[backup] schedule = "daily"`, `keep = 7`, `to = "s3://…"` → periodic job | external (provider) + optional scheduled `export` to S3 |
| Restore | `ferrite restore <backup> --data ./site` (into empty folder or with `--force` moving the old one aside) | provider restore / `ferrite import-site` |

Content-level "undo" (point-in-time for a single item) is revisions (02). Site-wide
point-in-time is backup restore (solo) or PITR (cluster).

Backups are verified: after writing, the snapshot is opened read-only and
`PRAGMA integrity_check` runs; S3 uploads compare sha256.

## 5. TLS

- Default: run behind a reverse proxy (Caddy/nginx/Traefik/LB). As built (M1c-2, ruling
  R31): only `X-Forwarded-For` is honoured, and only when the TCP peer is inside
  `server.trusted_proxies` (CIDRs or bare IPs; default none). The client address is the
  right-most entry that is not itself a trusted proxy and is used for logging (`client_ip`
  on the request span). `X-Forwarded-Proto` and `X-Forwarded-Host` are ignored from every
  peer: absolute URLs come from `site.url`, and the page-cache host is pinned to it (R25),
  so a TLS-terminating proxy cannot change the scheme (not needed while `site.url` is set to
  the public `https://` URL). `crates/ferrite-http/src/app/client.rs`.
- Built-in ACME (solo only in v1): rustls + an ACME client crate (`instant-acme` or
  `rustls-acme` — choose in M4), HTTP-01 on :80 or TLS-ALPN-01. Certificates stored in
  `data/tls/`. In cluster mode ACME would need shared challenge state — out of scope;
  use the LB.

## 6. Metrics & logs

See [01 §7](01-architecture.md#7-observability). Admin dashboard charts read
`/api/v1/system/metrics/summary`, which aggregates the node-local registry; in cluster
mode each node writes a 1-minute summary row (`node_metrics`, retained 7 days) so the
dashboard shows cluster-wide figures. (Prometheus remains the source of truth for real
monitoring.)

## 7. Packaging

- Release targets: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`,
  `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`,
  `x86_64-apple-darwin` (wasmtime/cranelift support for all to verify in CI).
- Docker image: distroless/static or `scratch` + CA certs; `VOLUME /data`; non-root.
- systemd unit with hardening (`ProtectSystem=strict`, `ReadWritePaths=/var/lib/ferrite`,
  `NoNewPrivileges`, `MemoryMax`).
- Example `docker-compose.yml` for cluster: 2× ferrite + postgres + minio + caddy.

## 8. CLI summary

### Implemented (M1c-1 and M1c-2: `crates/ferrite`)

```
ferrite init --data DIR --url URL [--title T] [--db URL] [--schema NAME] [--media s3://bucket/prefix]
ferrite migrate status --data DIR
ferrite migrate up     --data DIR [--to VERSION]
ferrite migrate down   --data DIR --to VERSION [--allow-lossy] [--yes]
ferrite migrate verify --data DIR
ferrite serve          --data DIR                      # M1c-2
ferrite seed demo      --data DIR [--force]            # M1c-2
```

**Logging.** Logs go to stderr. `FERRITE_LOG` is an `EnvFilter` expression (for example
`FERRITE_LOG=debug`); unset, the filter is `info`; an invalid value prints a warning and falls
back to `info` (`crates/ferrite/src/main.rs`). `FERRITE_LOG` is not a config key. The binary
also installs the `ring` rustls default crypto provider at start.

**`ferrite init`** (`crates/ferrite/src/init.rs`):

- Flags: `--data` and `--url` are required. `--title` defaults to `My Site`. `--db` is a
  database URL (default: SQLite `ferrite.db` in the data folder). `--schema` is a Postgres
  schema, created if missing. `--media s3://bucket[/prefix]` selects S3 media (default: local
  `media/` in the data folder). A bad `--media` URL is an error.
- Writes `<data>/ferrite.toml` from the template in `ferrite-config`, created exclusively
  with mode `0600` on Unix because it can hold the database URL. For S3 it writes the
  bucket and prefix with region `us-east-1` and tells you to set the real region, endpoint
  and credentials before serving.
- **Refuses to overwrite** an existing `ferrite.toml`, and **refuses an existing database**:
  an existing SQLite file, or a Postgres schema that already has applied migrations. Use
  `ferrite migrate` for those.
- Creates the data folder, `media/` (local media only) and `backups/`, validates the written
  config, then creates the database and applies all migrations. If any step fails, it removes
  what this run created (the config file, new folders, and with SQLite the new database and
  lock files) so a retry starts clean.
- **Postgres is not cleaned up.** Each migration commits on its own, so an init that fails
  partway through migrating leaves the schema (created by `--schema` if it was missing) with
  the migrations applied so far recorded in `_sqlx_migrations`. A retry then refuses it as an
  existing database. To recover, drop the schema (`DROP SCHEMA <name> CASCADE`) and run
  `ferrite init` again; without `--schema` the tables are in the connection's current schema
  (normally `public`), so drop that database instead or remove its ferrite tables and
  `_sqlx_migrations` by hand.
- The validation step applies `FERRITE_*` environment overrides, so it checks the effective config, which can differ from the file that was written.
- It does **not** create an admin user yet (`ferrite user create` is a later milestone).

**`ferrite migrate`** (`crates/ferrite/src/migrate.rs`, `open.rs`):

- `status` prints database and binary schema versions, the minimum binary version the
  schema requires (if any), pending migrations with their directives (`phase`,
  `min-binary`, `lossy-down`), unknown applied migrations and a verdict. **Exit codes: 0**
  up to date or newer-but-compatible schema, **2** migrations pending, **3** schema
  incompatible with this binary, **1** any error (bad config, missing database, connection
  failure, and so on).
- `up [--to N]` applies pending migrations (all, or up to version `N`).
- `down --to N` reverts everything above version `N`. Migrations whose down loses data need
  `--allow-lossy`. It asks for typed confirmation (`yes`) on a terminal and refuses to run
  without `--yes` when stdin is not a terminal.
- `verify` checks the checksums of applied migrations against this binary.
- **`status`, `down` and `verify` never create a database**: with SQLite they fail with
  "database not found at <path>; run `ferrite init` first" when the file is missing, without
  creating the folder, lock file or database. Only `init` and `up` may create one.
- SQLite caveat: opening the database takes the exclusive data-folder lock
  (`ferrite.lock`), so **every `migrate` subcommand, `status` included, and `seed demo` fail
  while another process holds the lock, and a running `ferrite serve` holds it**: stop the
  server first. There is no read-only open. Postgres has no folder lock, so
  these commands work next to a running server.
- Postgres caveat: `status`, `down` and `verify` do not create tables, but connecting with
  a configured `database.schema` that does not exist creates that empty schema.

**`ferrite serve`** (`crates/ferrite/src/serve.rs`):

- **Startup**, in order; any failure exits 1 (rulings P15, P22): load the config; open the
  database (created if missing); the migration check under the migrate lock (pending
  migrations are applied when `database.auto_migrate` is on; on Postgres a pending contract
  migration also needs `database.allow_contract`; an incompatible schema is refused; 02 §8);
  join the node registry (refused when a live node has another `site_id` or `site.url`; in
  solo mode another live node is waited for up to 35 s; 04 §3); open the media store and run
  a write/read/delete probe; create the page cache and set the invalidation cursor
  (**before** any node state is read, so a change committed during boot is applied by the
  first poll); build the permalink rules from the `permalink.post`/`permalink.page` options
  and check them against the registered types and taxonomies; load the active theme (at
  startup a broken one falls back to the embedded `blog`, 06 §7); install the enabled plugins
  (a broken row is skipped, 05 §7a); build the route table; bind `server.listen` (default
  `127.0.0.1:8080`; override with `FERRITE_SERVER__LISTEN`, port `0` picks a free one); start
  the background tasks: the invalidation loop (with `LISTEN` wake-ups in cluster mode) and the
  hot-reload task, the plugin-failure flusher (every 5 s), the job runner (`cluster.workers`
  handlers; prune jobs, scheduled publishing), the periodic scheduler (every 10 s: the prune
  jobs hourly and `content.publish_due` every 30 s) and the heartbeat (every 10 s); print
  `Listening on http://<addr>` on stdout. A failure after joining the registry removes the
  node's row again.
- **Hot swap** (ruling P18, 04 §8): `theme.changed`, `plugins.changed`, `plugin.disabled`
  (the plugin stops on this node at once) and `options.changed` for `permalink.post` /
  `permalink.page` make one background task re-read the theme, the plugins or the permalink
  rules (and route table) from the database and swap them in; history lost past the cursor
  reloads all three. The page cache is flushed after each swap. A theme, plugin version or
  permalink set that fails to load keeps the current one and is reported in `nodes.meta`
  (`theme.error`, `plugins.<name>.status = "failed"` with `error`, `permalinks_error`); there
  is no runtime fallback to `blog`. A failed reload is retried per component after 5 s,
  doubling up to 5 min, until it succeeds (a newer event retries at once; ruling R14). The
  replaced theme's `/_theme/<hash>/` assets stay served until the next swap.
- **`nodes.meta`** (heartbeat, and at once after every reload): `site_id`, `site_url`
  (normalised, no trailing slash), `mode`, `clock_skew_ms`, `theme` (`name`, `hash`,
  `origin`, `error`), `plugins` (per plugin row: `version`, `hash`, `status` =
  `loaded`/`failed`/`disabled`, `error` up to 1 KiB), `plugins_error` (the plugin rows could
  not be read), `permalinks_error`.
- **Shutdown** on SIGINT or SIGTERM: stop accepting connections and stop the job runner at
  once; open requests get up to 10 s while running jobs get 9 s, after which the runner
  releases the rest without using an attempt (another node takes them at once; the runner
  is abandoned after 12 s, ruling R6); then the background tasks stop (the failure flusher
  writes once more) and the node deregisters, together within 1 s; the database pools close
  (up to 2 s, cut so that shutdown so far ends within 13 s of the signal); `main` then shuts
  the runtime down with a 1 s grace, dropping connection tasks still running. A clean
  shutdown exits 0; the process is gone at most 14 s after the first signal. A second
  SIGINT/SIGTERM during startup (after joining the registry the node first leaves it, within
  1 s) or shutdown exits at once with 130 (SIGINT) or 143 (SIGTERM).
- **Health:** `/healthz` is `200 ok`; `/readyz` is `200` when a store read succeeds within
  2 s and the startup migration check passed, else `503`.
- **Settings it uses** (`[server]`, `[cache]`, `[plugins]`, 01 §4): `server.listen`,
  `server.trusted_proxies` (§5), `cache.max_bytes` (default 256 MB), `cache.poll_interval_ms`
  (default 250; how often the invalidation loop reads the event outbox),
  `plugins.render_deadline_ms` (default 200, ruling R18; the hard timeout is 1.5 × it),
  `plugins.action_deadline_ms`, `plugins.default_memory`, `plugins.default_fuel`,
  `plugins.disable_after_failures`, `plugins.failure_window` (counted cluster-wide, P20),
  `cluster.mode`, `cluster.node_name`, `cluster.workers`. `cache.logged_in` is parsed but
  unused until logins exist (M2). Render concurrency (2 × available parallelism, 5 s wait,
  then 503) and the 60 s short TTL are constants.

**`ferrite seed demo`** (`crates/ferrite/src/seed.rs`):

- Writes demo content through the core services where they exist: an `admin` user (no
  password, `must_reset`; login arrives in M2), categories News and Guides, tags rust and cms,
  12 published posts with block bodies dated over the past months, three pages (About,
  About/Team, Contact), a `primary` and a `footer` menu, one generated sample PNG with its
  variants (the core sizes plus the active theme's `[images]`) used as the featured image of
  some posts, and the `site.title` / `site.description` options.
- Refuses (exit 1) when the site already has content, unless `--force`, which adds another
  batch (user, terms and image file reused; posts and pages get fresh `-2`, `-3`, … slugs;
  both menus and the two options are replaced to point at the new batch).
- **Not atomic:** each step is its own transaction, so a run that fails part-way leaves what
  it wrote; `--force` then adds a whole new batch, it does not resume (ROADMAP M2: seed
  atomicity).
- Requires an initialised site (with SQLite it fails when `ferrite.db` is missing instead of
  creating it) and runs the same migration check as `serve`.

### Planned (later milestones)

```
ferrite backup / restore                                        # M4
ferrite export / import-site / db copy                          # M4
ferrite media sync / media gc                                   # M4
ferrite import wordpress …                                      # M3
ferrite plugin new|check|install|list|enable|disable|remove     # M2
ferrite theme install|activate|list                             # M2
ferrite user create|reset-password|list                         # M2 (init will then create the admin user)
ferrite self-update / rollback [--schema] [--restore-snapshot]  # M4
ferrite doctor        # validates config, connectivity, permissions, clock skew, versions
```
