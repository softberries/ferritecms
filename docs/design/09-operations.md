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

**Rolling deploy, as built (M1.5;
[04 §9](04-cluster.md#9-rolling-deploys)):**

- Start the first new node. It takes the `migrate` advisory lock and applies the pending
  expand migrations (`database.auto_migrate`, default on); other nodes starting at the
  same moment wait for the lock (up to 5 min, logged every 10 s), then find nothing to do.
  Two nodes started together on an empty schema (what `docker compose up` does) migrate
  exactly once.
- Old nodes keep serving next to new ones: codecs ignore unknown columns, old runners
  claim only job kinds they know and hand back unknown payload versions without using an
  attempt, and an unknown event kind invalidates its keys.
- Replace the remaining nodes one at a time. SIGTERM releases the node's running jobs
  (another node claims them at once) and deregisters it; give the process manager at
  least 15 s before SIGKILL (shutdown is bounded at 14 s; the compose example uses
  `stop_grace_period: 20s`).
- `site.url` must not change during a rolling deploy: a node whose `site.url` differs from
  a live node's refuses to start (P15). Change it with all nodes stopped (or 30 s after the
  last one stopped).
- Watch the `nodes` table: `version`, `schema_version`, `heartbeat_at` and `meta` (theme
  and plugin state per node) for every live node.

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

**The site key is not in the database** (M2a-1, P4). A solo site keeps it in
`<data>/secret.key` (mode `0600`); a cluster gets it from `auth.secret_key[_file]`. Back it
up separately from database dumps and keep it with the same care as the database
credentials: without it, a restored database still has its users, sessions and API tokens,
but stored TOTP secrets cannot be opened (those users need `ferrite user disable-totp` and a
new enrollment). Until `ferrite backup` exists (M4), copying the data folder by hand
includes `secret.key`.

## 5. TLS

- Default: run behind a reverse proxy (Caddy/nginx/Traefik/LB). As built (M1c-2, ruling
  R31): only `X-Forwarded-For` is honoured, and only when the TCP peer is inside
  `server.trusted_proxies` (CIDRs or bare IPs; default none). The client address is the
  right-most entry that is not itself a trusted proxy. It is logged (`client_ip` on the
  request span) **and keys the per-IP rate limits** (M2a-1: 10 login attempts and 10
  unknown or expired bearer tokens a minute per address, [07 §2](07-admin-api.md#2-auth)).
  **Behind a proxy, list it in `server.trusted_proxies`**: otherwise every request has the
  proxy's address, the per-IP limits become one bucket for the whole site, and 10 login
  attempts a minute from anyone stop everyone from logging in. A request with
  `X-Forwarded-For` from a peer that is not trusted logs a warning naming the peer, once
  per peer address (for the first 64 such peers of the process; R26). `X-Forwarded-Proto`
  and `X-Forwarded-Host` are ignored from every peer: absolute URLs come from `site.url`,
  and the page-cache host is pinned to it (R25), so a TLS-terminating proxy cannot change
  the scheme (not needed while `site.url` is set to the public `https://` URL).
  `crates/ferrite-http/src/app/client.rs`.
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
- **Docker image (as built, M1.5):** `Dockerfile` at the repository root, multi-stage:
  `rust:1-bookworm` (current stable, matching `rust-toolchain.toml`'s `channel = "stable"`)
  builds `cargo build --release --locked -p ferrite`; the runtime is
  `gcr.io/distroless/cc-debian12:nonroot` (glibc, no shell, uid 65532) with the binary at
  `/ferrite`, an empty `/data` owned by the non-root user, `VOLUME /data`, `EXPOSE 8080`,
  `ENTRYPOINT ["/ferrite"]`, `CMD ["serve", "--data", "/data"]`. CA certificates come with
  the distroless base. `.dockerignore` keeps `target/`, `.git`, docs and examples out of
  the build context. Health checks use `ferrite healthcheck` (§8), because the image has
  no shell, curl or wget. CI builds the image (`docker build -t ferrite:ci .`) and runs
  `config check` in it; publishing images is M4.
- systemd unit with hardening (`ProtectSystem=strict`, `ReadWritePaths=/var/lib/ferrite`,
  `NoNewPrivileges`, `MemoryMax`) (M4).
- **Cluster example (as built, M1.5):** `examples/cluster/` — `docker-compose.yml`,
  `ferrite.toml`, `ferrite.env`, `Caddyfile`. Usage (also in the compose file's header):

  ```
  docker compose -f examples/cluster/docker-compose.yml up --build
  open http://localhost:8080
  docker compose -f examples/cluster/docker-compose.yml exec node1 /ferrite seed demo --data /data
  ```

  Services: `postgres` (`postgres:18`, `pg_isready` health check, data volume at
  `/var/lib/postgresql`), `minio` (`minio/minio`, `server /data`), `minio-init`
  (`minio/mc`: creates bucket `ferrite`, then exits), `node1`/`node2` (built from the
  repository, `serve --data /data`, `env_file: ferrite.env`, `ferrite.toml` mounted
  read-only at `/data/ferrite.toml`, `FERRITE_CLUSTER__NODE_NAME` per node, health check
  `/ferrite healthcheck --url http://127.0.0.1:8080/readyz`, start after Postgres is
  healthy and `minio-init` completed, `stop_grace_period: 20s`) and `caddy` (`caddy:2`,
  port `8080:8080`, `reverse_proxy node1:8080 node2:8080` with `lb_policy round_robin`
  and `health_uri /readyz`). The shared `ferrite.toml` sets `site.url =
  "http://localhost:8080"`, `server.listen = "0.0.0.0:8080"`, `server.trusted_proxies =
  ["172.28.0.0/16"]` (the compose network's subnet, so Caddy's `X-Forwarded-For` is
  honoured), `cluster.mode = "cluster"` and S3 media on MinIO (`bucket = "ferrite"`,
  `region = "us-east-1"`, `endpoint = "http://minio:9000"`, `allow_http = true`).
  `ferrite.env` holds `FERRITE_DATABASE__URL` and the S3 key pair: **demo values**,
  shared with the `postgres` and `minio` service definitions; replace them together.
  **Verification status (P24):** `crates/ferrite/tests/examples.rs` loads the example's
  `ferrite.toml` + `ferrite.env` through `ferrite_config::load` and `ferrite config check`,
  and checks that every `FERRITE_*` name in the compose file is a known setting; CI runs
  `docker compose … config --quiet`, `caddy validate` on the Caddyfile and `docker build`. `docker compose up` is not run
  anywhere yet, so problems in the runtime wiring (Caddy health checks, the MinIO bucket
  init) would only show when someone runs it.

## 8. CLI summary

### Implemented (M1c-1, M1c-2, M1.5 and M2a: `crates/ferrite`)

```
ferrite init --data DIR --url URL [--title T] [--db URL] [--schema NAME] [--media s3://bucket/prefix]
ferrite migrate status --data DIR
ferrite migrate up     --data DIR [--to VERSION]
ferrite migrate down   --data DIR --to VERSION [--allow-lossy] [--yes]
ferrite migrate verify --data DIR
ferrite serve          --data DIR                      # M1c-2; cluster wiring M1.5
ferrite seed demo      --data DIR [--force]            # M1c-2
ferrite config check   --data DIR                      # M1.5
ferrite healthcheck    [--url URL]                     # M1.5, for container health checks
ferrite user create    --data DIR --login L --email E [--role admin|editor|author|contributor]
                       [--display-name N] [--password-stdin]                     # M2a
ferrite user reset-password --data DIR --login L [--password-stdin]              # M2a
ferrite user list      --data DIR                                                # M2a
ferrite user disable-totp --data DIR --login L                                   # M2a
ferrite theme install  --data DIR --dir PATH [--activate]                        # M2a-2
ferrite theme activate --data DIR NAME                                           # M2a-2
ferrite theme list     --data DIR                                                # M2a-2
ferrite plugin check   --data DIR --dir PATH                                     # M2a-2
ferrite plugin install --data DIR --dir PATH [--enable] [--yes]                  # M2a-2
ferrite plugin list|enable|disable|remove --data DIR [NAME] [--purge]            # M2a-2
```

**`ferrite config check`** (`crates/ferrite/src/config_cmd.rs`): loads `<data>/ferrite.toml`
plus the `FERRITE_*` environment exactly as `serve` does (`ferrite_config::load`) and never
opens the database or the media store. Valid: prints one line,
`ok: mode=<solo|cluster> database=<sqlite|postgres> media=<local|s3> workers=<n> mail=<configured|off>` (`mode`
is the effective mode, `auto` resolved; `workers` the effective `cluster.workers`; `mail` whether `[mail]` is set), exit
0. Invalid: prints `error: invalid configuration (<n> problem(s))` and one
`  <path>: <message>` line per problem to stderr (secret values are never printed), exit 1.
Checks that need the database (site id, live nodes, migrations) are not part of it.

**`ferrite healthcheck`** (`crates/ferrite/src/healthcheck.rs`): `GET`s `--url` (default
`http://127.0.0.1:8080/readyz`; plain `http://` only) and exits 0 on a 200 response, 1 on
any other status, a connection error or no response within 5 s. It exists because the
distroless image has no HTTP client for a container `HEALTHCHECK`.

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
- **Site key** (P4, F30): when the written config resolves to solo mode and configures no
  `auth.secret_key`, `init` creates `<data>/secret.key` (64 hex characters, mode `0600`,
  never printed) and removes it again if `init` fails. Otherwise it writes no key file; a
  solo site without one gets it at its first `serve`.
- **Cluster configs need the key up front** (R1, R9): a config that resolves to cluster
  mode (explicit `cluster.mode = "cluster"`, or `auto` with Postgres and S3 or
  `local_shared` media) fails validation without `auth.secret_key[_file]`, so `ferrite init
  --db postgres://… --media s3://…` needs the key in the environment, e.g.
  `FERRITE_AUTH__SECRET_KEY=$(openssl rand -hex 32) ferrite init …`, and every node must
  then use the same key. The error is `auth.secret_key: required in cluster mode (every node
  must use the same key)`, and `init` removes what it created.
- It does **not** create an admin user: run `ferrite user create --login … --email …`
  next (it prints a one-time password).

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
  (`ferrite.lock`), so **every `migrate` subcommand, `status` included, `seed demo` and every
  `user` subcommand fail while another process holds the lock, and a running `ferrite
  serve` holds it**: stop the server first. There is no read-only open. Postgres has no
  folder lock, so these commands work next to a running server.
- Postgres caveat: `status`, `down` and `verify` do not create tables, but connecting with
  a configured `database.schema` that does not exist creates that empty schema.

**`ferrite serve`** (`crates/ferrite/src/serve.rs`):

- **Startup**, in order; any failure exits 1 (rulings P15, P22): load the config; open the
  database (created if missing); the migration check under the migrate lock (pending
  migrations are applied when `database.auto_migrate` is on; on Postgres a pending contract
  migration also needs `database.allow_contract`; an incompatible schema is refused; 02 §8);
  join the node registry (refused when a live node has another `site_id` or `site.url`; in
  solo mode another live node is waited for up to 35 s; on SQLite rows left by a crashed
  process are deleted instead, so a restart is immediate; 04 §3); open the media store and run
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
  doubling up to 5 min, until it succeeds (a newer event retries at once; ruling R14); a
  theme that fell back or a plugin row skipped at startup is retried the same way (R17). The
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
  shutdown exits 0; the process is gone at most 14 s after the first signal. A signal
  during startup (after joining the registry the node first leaves it, within 1 s), or a
  second signal during shutdown, exits at once with 130 (SIGINT) or 143 (SIGTERM).
- **Health:** `/healthz` is `200 ok`; `/readyz` is `200` when a store read succeeds within
  2 s and the startup migration check passed, else `503`.
- **Settings it uses** (`[server]`, `[cache]`, `[plugins]`, 01 §4): `server.listen`,
  `server.trusted_proxies` (§5), `cache.max_bytes` (default 256 MB), `cache.poll_interval_ms`
  (default 250; how often the invalidation loop reads the event outbox),
  `plugins.render_deadline_ms` (default 200, ruling R18; the hard timeout is 1.5 × it),
  `plugins.action_deadline_ms`, `plugins.default_memory`, `plugins.default_fuel`,
  `plugins.disable_after_failures`, `plugins.failure_window` (counted cluster-wide, P20),
  `cluster.mode`, `cluster.node_name`, `cluster.workers`, `cache.logged_in` (`bypass` or
  `per-user`, 06 §3), `auth.secret_key[_file]` (else `<data>/secret.key` in solo mode, created
  on first start if missing). Render concurrency (2 × available parallelism, 5 s wait,
  then 503) and the 60 s short TTL are constants.

**`ferrite user`** (`crates/ferrite/src/user_cmd.rs`, ruling P17): the operator path for
accounts. It acts without a login or capability checks (`UserService::operator()`): it is
how the first admin is created and how a locked-out admin gets back in, so whoever can run
it on the data folder (or with the database URL) controls the site. Every subcommand loads
the config, opens the database and runs the same migration check as `serve`; with SQLite it
takes the data-folder lock, so it fails while `serve` runs (stop the server first; Postgres
has no folder lock). The same rules as the API apply (login format, unique login and
e-mail, password policy of 12 to 1024 bytes); any error exits 1 with the message, e.g.
`Error: invalid login: …`, `Error: invalid login: login taken`, `Error: user not found:
"bob"`.

- **Passwords never come from the command line.** With `--password-stdin` one line is read
  from stdin (a trailing `\n` or `\r\n` is dropped) and `must_reset` is cleared, e.g.
  `printf '%s\n' "$PW" | ferrite user create … --password-stdin`. Without it a random
  password (base64url of 15 random bytes, 20 characters) is generated and printed once as
  `password: <p>`, and the account must change it at its first login (`must_reset`).
- `create`: `--role` defaults to `admin`, `--display-name` to the login. Prints
  `created user <login> (<role>)`.
- `reset-password`: sets the password (rules above) and deletes every session of the user,
  on every node at once; API tokens stay valid (revoke them over the API).
- `list`: one line per account, by login: `<login>  <role>  <status>  <email>` (two spaces
  between columns, no header).
- `disable-totp`: turns TOTP off (clears the secret and the replay step); the recovery for
  a lost authenticator, as there are no recovery codes.

**`ferrite theme` and `ferrite plugin`** (`crates/ferrite/src/ext_cmd.rs`, ruling P26): the
operator path for extensions, over `ExtensionService` and the install pipeline, with no
login, no capability checks and no audit rows. Like `ferrite user`, every subcommand loads
the config, opens the database and media and runs the migration check; with SQLite it takes
the data-folder lock, so it fails while `serve` runs. Errors exit 1 with the message.

- `theme install --dir PATH [--activate]`: packs the folder into a bundle; a theme that
  does not compile is refused before anything is stored. `theme activate NAME` loads the
  stored bundle first (hash-checked) and refuses a theme that does not load, leaving the
  active theme unchanged, as the API does; an embedded theme without a row is installed from
  the embedded files. `theme list` prints `<name>  <version>  <installed|embedded>` plus
  `  active` for the active one (the embedded `blog` when no row is active).
- `plugin check --dir PATH`: `PATH` holds `ferrite-plugin.toml` and exactly one `*.wasm`.
  Loads both on a scratch host, prints the permission report and stores nothing; exit 1 when
  the manifest or component is refused. `plugin install --dir PATH [--enable] [--yes]` prints
  the same report (one line per capability, `!` in front of high-risk ones; an update also
  lists what it adds to the installed grant under `this update WIDENS the installed grant`,
  and the limits and `fingerprint:`), then asks `install? [y/N]` on a terminal. Without a
  terminal it exits 1 unless `--yes` is given, installing nothing. What is approved is
  exactly the report printed: the install passes that report's fingerprint to the pipeline
  (R1), so a widening update is installed only after that approval. A new plugin is
  installed disabled; `--enable` enables it.
- `plugin list` prints `<name>  <version>  <enabled|disabled>`, plus `  <reason>` after an
  automatic disable. `plugin enable NAME` / `disable NAME`; an unknown name exits 1
  (`plugin not found`).
- `plugin remove NAME [--purge]`: removes the plugin. Its KV and settings stay for 30 days
  (a reinstall within that time finds them; a `plugin.purge` job deletes them after);
  `--purge` deletes them at once (the delete's own job sweeps again shortly after).
- Setting values (including secrets) are never printed by any of these commands.

**`ferrite seed demo`** (`crates/ferrite/src/seed.rs`):

- Writes demo content through the core services where they exist: an `admin` user (no
  password, `must_reset`, so it cannot log in until `ferrite user reset-password --login
  admin` sets one), categories News and Guides, tags rust and cms,
  12 published posts with block bodies dated over the past months, three pages (About,
  About/Team, Contact), a `primary` and a `footer` menu, one generated sample PNG with its
  variants (the core sizes plus the active theme's `[images]`) used as the featured image of
  some posts, and the `site.title` / `site.description` options.
- Refuses (exit 1) when the site already has content, unless `--force`, which adds another
  batch (user, terms and image file reused; posts and pages get fresh `-2`, `-3`, … slugs;
  both menus and the two options are replaced to point at the new batch).
- **Not atomic:** each step is its own transaction, so a run that fails part-way leaves what
  it wrote; `--force` then adds a whole new batch, it does not resume (ROADMAP M2a-2: seed
  atomicity).
- Requires an initialised site (with SQLite it fails when `ferrite.db` is missing instead of
  creating it) and runs the same migration check as `serve`.

### Planned (later milestones)

```
ferrite backup / restore                                        # M4
ferrite export / import-site / db copy                          # M4
ferrite media sync / media gc                                   # M4
ferrite import wordpress …                                      # M3
ferrite plugin new|check|install|list|enable|disable|remove     # M2a-2 (new: later)
ferrite theme install|activate|list                             # M2a-2
ferrite self-update / rollback [--schema] [--restore-snapshot]  # M4
ferrite doctor        # validates connectivity, permissions, clock skew, versions (config: `config check`)
```
