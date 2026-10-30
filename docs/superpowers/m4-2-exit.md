# M4-2 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M4; plan
[`2026-10-30-m4-2-release.md`](plans/2026-10-30-m4-2-release.md), Task 11, ruling P40):
*FerriteCMS can be released and operated: signed release builds for six targets and a
multi-arch image on GHCR, a binary that updates and rolls itself back, built-in HTTPS for
solo sites, Prometheus metrics and dashboard time series, a systemd unit, a cluster runbook
and a backward-compatibility CI job. The exit records self-update and rollback end to end
against a local signed release, `rollback --schema` and `--restore-snapshot` on SQLite,
ACME against Pebble, a `/metrics` scrape, `node_metrics` rows and the admin series (E2E),
an `aarch64-apple-darwin` release build, `systemd-analyze`, a "Not verified" table and the
release gate.*

**Result (2026-10-30, branch `m4-2-exit` from `m4-2` at `a43a58e`):** every exit check
passed on macOS (arm64) with local PostgreSQL 17 (port 54329) and Docker 29.8.1. The
two-binary lifecycle found one bug, fixed with two tests (§3, X1, `fb31c97`). Nothing was
pushed, tagged or published: the release gate (§6) is the user's.

| Exit check | Verified by | Result |
|---|---|---|
| (a) Real two-binary lifecycle: M4-1 site (schema 11) → `self-update` to 0.1.0 from a local signed release → `serve` migrates to 12 → content → `rollback --schema` → re-upgrade → `rollback --schema --restore-snapshot --yes` → a second `self-update`; the site serves at every step | §1 (a) | yes (after fix X1) |
| (b) ACME against Pebble with real TLS-ALPN-01 validation | §1 (b) | yes: 6/6 `tests/tls.rs`, certificate issued |
| (c) `/metrics` on `server.metrics_listen`; `node_metrics` rows after a minute; `GET /api/v1/system/metrics/series` | §1 (c) | yes |
| (d) Dashboard E2E specs on `chromium-sqlite` against a release binary with the admin | §1 (d) | yes: 7/7 |
| (e) Release steps by hand for `aarch64-apple-darwin` (`package.sh`, `sums.sh`, minisign, `verify-sig.sh`), `docker build` and the image, `systemd-analyze verify`, `compat.sh` | §1 (e) | yes |
| (f) Gates: fmt, clippy ×2, rustdoc, the workspace tests with test-seams and Postgres, the admin gates | §2 | yes: 3391 passed, 0 failed |

## What shipped

Per task (merges into `m4-2`; rulings in [`rulings/m4.md`](rulings/m4.md), "M4-2 plan
rulings" P25–P40 and "M4-2 execution rulings" R-pf1–R12 and X1):

1. **Build groundwork** (`271a346`, P27, P28, R1): ring-only crypto (aws-lc gone from
   `Cargo.lock`), `[profile.release]` `lto = "thin"`, `codegen-units = 1`,
   `strip = "symbols"`, the target triple embedded by `crates/ferrite/build.rs`
   (`ferrite::binary_target()`).
2. **Time zones** (`040f3c8`, P29): jiff with the bundled tzdb; `site.timezone` validated as
   an IANA zone; `date(fmt)` in the site zone, `date(fmt, tz=…)`; permalinks stay UTC.
3. **`upgrade.json`** (`6784e7e`, P30, R2): `Store::migrate_up` returns `MigrateReport`,
   `schema_meta.last_binary_version`, the record written by `serve` and `migrate up`
   (provisional, then final), snapshot pruning that keeps the recorded snapshot.
4. **`ferrite self-update`** (`38f9476`, P25, P26, P31): GitHub release lookup, minisign over
   `SHA256SUMS` then the asset's sha256, the new binary's `--version` and `migrate status`,
   an atomic swap keeping `ferrite.prev`; `crates/ferrite/release.pub` is a placeholder.
5. **`ferrite rollback`** (`286d04c`, P32): the binary swap, `--schema` (down migrations to
   `schema_from`, as the record's `to`), `--restore-snapshot` (SQLite; the current database
   moved to `backups/rolled-back-<ts>.db`, never deleted).
6. **ACME TLS** (`1a24a27`, P33): `[server.tls]` (`mode`, `domains`, `contact`,
   `directory`, `listen`, `redirect_http`), rustls-acme 0.15 on ring with TLS-ALPN-01,
   `<data>/tls/` 0700/0600, HSTS on TLS only, the redirect listener, cluster mode refused.
7. **Metrics registry** (`5cc94fe`, P34, R3): hand-rolled Prometheus text 0.0.4 on
   `[server] metrics_listen` only; requests and latency by class, page views, page cache,
   plugin hooks, jobs, pool, nodes, build info.
8. **`node_metrics` and the series** (`1940c98`, P35, R7): migration `0012_node_metrics`
   (both backends, expand, `.down.sql`), the per-minute writer, `node_metrics.prune` (7 days),
   `GET /api/v1/system/metrics/series` (`system:read`), the OpenAPI and admin client.
9. **Dashboard charts** (`7ff5169`, P36, R6): the Site traffic group (Traffic and
   Performance cards, 24 h / 7 d, empty, loading, error and reduced-motion states).
10. **Release CI and packaging** (`688ac59`, `9193152`, P37, P38, R4, R5): `release.yml`
    (six native targets, minisign, a draft-until-complete release, GHCR per arch with
    `imagetools`), `ci.yml`'s `packaging` and `compat` jobs, the Dockerfile's digest pins,
    `packaging/systemd/{ferrite.service,README.md}`, `.github/scripts/`.
11. **Final review fixes** (`a43a58e`, R7–R11) and this exit (`m4-2-exit`): the runbooks
    `docs/operations/cluster-runbook.md` and `docs/operations/solo-upgrade.md`, the design
    docs (01 §1/§2, 02 §4/§7/§8, 04 §6/§9, 06, 07 §3, 09 §1/§2/§3/§5/§7/§8), ROADMAP,
    CLAUDE.md, the rulings, two stale comments, and fix X1.

**Commands:** `ferrite self-update [--version X] [--check] [--yes] [--data DIR]` (0, 2 newer
release with `--check`, 1); `ferrite rollback [--data DIR] [--schema] [--allow-lossy]
[--restore-snapshot] [--yes]` (0, 3, 1); clap usage errors exit 2 for both. **Config:**
`[server] metrics_listen`; `[server.tls]`; `site.timezone` validated. `config check` prints
`metrics=… tls=…` and warns about `[server.tls]` keys set while `mode = "off"`.
**Migration:** `0012_node_metrics` (expand, not lossy). **Files:** `<data>/upgrade.json`,
`upgrade-rolled-back-<ts>.json`, `upgrade.json.invalid-<ts>`, `<data>/tls/`,
`backups/rolled-back-<ts>.db`, `ferrite.prev` beside the binary.

## 1. End-to-end checks

Scratch folder: `/private/tmp/claude-501/-Users-krzysztofgrajek-Projects-rust-ferritecms/a70e2d1a-a8e1-480a-80ba-c15c966313df/scratchpad/t11`
(`$T` below; scripts in `$T/scripts/`, full logs `$T/*.log`). Outputs are trimmed; log
lines with timestamps are cut unless they matter.

### (a) Two-binary upgrade lifecycle

Binaries:

- **A**: `develop` at `f7c2e22` (M4-1, schema 11), with the workspace version set to 0.0.9
  in a scratch worktree (not committed), debug build.
- **B**: this branch, `cargo build --release --locked -p ferrite` with the admin built,
  0.1.0, packed and signed as the release asset (§1 e). No test seams.
- **U** (the updater): this branch with the version set to 0.0.9, built with
  `cargo build -p ferrite --features test-seams`. M4-1 has no `self-update`, so U runs
  the update on the installed A file through the hidden `--test-current-exe`, against the
  local API (`--base-url`, `$T/scripts/relserver.py`, shaped like
  `api.github.com/repos/softberries/ferritecms/releases/{latest,tags/vX}`), trusting the
  throwaway key (`FERRITE_TEST_RELEASE_PUBKEY`). The files swapped on disk are the real
  A and B (sha256 below).

`$T/scripts/lifecycle.sh $T/run2` (log `$T/lifecycle.log`; `site.py` starts `serve`,
GETs the paths, optionally publishes a post over the API, sends SIGTERM):

```
=== 1. the M4-1 site (0.0.9)
$ bin/ferrite --version                     ferrite 0.0.9
$ bin/ferrite init --data data --url http://localhost:18080
  applied migrations [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] from=0 to=11
$ bin/ferrite seed demo --data data         Seeded demo content: 12 posts, 3 pages, ...
$ bin/ferrite migrate status --data data    database version: 11 ... verdict: up to date [exit 0]
$ site.py bin/ferrite data 18080 / /healthz
GET / -> 200 FerriteCMS Demo
GET /healthz -> 200 ok
serve exit: 0
=== 2. self-update 0.0.9 -> 0.1.0
$ U self-update --check --base-url http://127.0.0.1:18765 --test-current-exe bin/ferrite
a newer release is available: 0.1.0 (this binary: 0.0.9)                          [exit 2]
$ U self-update --base-url ... --test-current-exe bin/ferrite --data data --yes
new binary: ferrite 0.1.0
migrate status of the new binary for data:
database version: 11
binary version: 12 (ferrite 0.1.0)
pending migrations:
  0012 node metrics (phase=expand, min-binary=-, lossy-down=no)
verdict: needs migrate up
updated $T/run2/bin/ferrite from 0.0.9 to 0.1.0; the previous binary is kept at $T/run2/bin/ferrite.prev
restart to run it: `systemctl restart ferrite` under systemd, otherwise stop and start `ferrite serve`
                                                                                  [exit 0]
$ shasum -a 256 bin/ferrite $B bin/ferrite.prev $A
d49f3ad8…7665d  bin/ferrite          = d49f3ad8…7665d  target/release/ferrite
f82565f0…e450   bin/ferrite.prev     = f82565f0…e450   wt-a/target/debug/ferrite
=== 3. 0.1.0 serves: migrates 11 -> 12, writes upgrade.json; a post is published
GET / -> 200, GET /healthz -> 200, GET /readyz -> 200 ready
login -> 200; create post -> 201; publish -> 200 published /published-on-0-1-0/
GET /published-on-0-1-0/ -> 200 Published on 0.1.0 – FerriteCMS Demo
$ cat data/upgrade.json
{"v": 1, "from": null, "to": "0.1.0", "schema_from": 11, "schema_to": 12,
 "snapshot": "backups/pre-11-to-12-20261030T101907Z.db", "at": "2026-10-30T10:19:07.348292Z"}
$ bin/ferrite migrate status --data data    database version: 12 ... up to date   [exit 0]
schema_meta: last_binary_version=0.1.0
=== 4. rollback --schema, run as the record's to (0.1.0)
$ bin/ferrite rollback --schema --data data
note: upgrade.json does not record the previous version, so $T/run2/bin/ferrite.prev is not checked
reverted migrations 12 (schema 12 -> 11)
upgrade.json renamed to data/upgrade-rolled-back-20261030T101907Z.json
swapped .../bin/ferrite and .../bin/ferrite.prev: ferrite 0.1.0 is now the installed binary's predecessor
                                                                                  [exit 0]
bin/ferrite --version: ferrite 0.0.9; bin/ferrite.prev --version: ferrite 0.1.0
migrate status (0.0.9): database version: 11 ... up to date                       [exit 0]
schema_meta: last_binary_version=0.0.9          (fix X1; before it: 0.1.0)
site.py (0.0.9): GET / -> 200; GET /published-on-0-1-0/ -> 200 Published on 0.1.0
=== 5. re-upgrade: self-update again, 0.1.0 serves and migrates again
$ U self-update ... --data data --yes       updated ... from 0.0.9 to 0.1.0      [exit 0]
site.py (0.1.0): applied migrations [12]; post "Written after the re-upgrade" published, 200
$ cat data/upgrade.json
{"v": 1, "from": "0.0.9", "to": "0.1.0", "schema_from": 11, "schema_to": 12,
 "snapshot": "backups/pre-11-to-12-20261030T101913Z.db", "at": "2026-10-30T10:19:13.158197Z"}
=== 6. rollback --schema --restore-snapshot (current db moved aside)
$ bin/ferrite rollback --schema --restore-snapshot --data data       (stdin not a terminal)
restore the pre-upgrade snapshot data/backups/pre-11-to-12-20261030T101913Z.db (upgrade at
  2026-10-30T10:19:13.158197+00:00 per upgrade.json; file modified 2026-10-30T10:19:13.153898684+00:00) in place of data/ferrite.db
everything written to the site since 2026-10-30T10:19:13.158197+00:00 is lost; the current
  database is moved to data/backups/rolled-back-20261030T101915Z.db (never deleted)
Error: --restore-snapshot needs --yes when stdin is not a terminal; nothing was changed  [exit 1]
$ bin/ferrite rollback --schema --restore-snapshot --data data --yes
restored data/ferrite.db (schema 11); the database that was current is at data/backups/rolled-back-20261030T101915Z.db
upgrade.json renamed to data/upgrade-rolled-back-20261030T101915Z.json
swapped ...: ferrite 0.1.0 is now the installed binary's predecessor              [exit 0]
data/backups: pre-0-to-11-…db, pre-11-to-12-…101907Z.db, pre-11-to-12-…101913Z.db,
              rolled-back-20261030T101915Z.db (643072 bytes)
ferrite.db -rw-r--r-- (the replaced database's mode)
$ sqlite3 data/backups/rolled-back-*.db "select slug from content_items where title like 'Written after%'"
written-after-the-re-upgrade                      (kept in the moved-aside database)
$ sqlite3 data/ferrite.db "select count(*) ... 'Written after%'"
0                                                 (written after the snapshot: gone from the site)
site.py (0.0.9): GET / -> 200; GET /published-on-0-1-0/ -> 200; GET /written-after-the-re-upgrade/ -> 404
=== 7. a second self-update; 0.1.0 serves and migrates once more
$ U self-update ... --data data --yes       updated ... from 0.0.9 to 0.1.0      [exit 0]
site.py (0.1.0): GET / -> 200, /healthz -> 200, /published-on-0-1-0/ -> 200
$ cat data/upgrade.json
{"v": 1, "from": "0.0.9", "to": "0.1.0", "schema_from": 11, "schema_to": 12,
 "snapshot": "backups/pre-11-to-12-20261030T101922Z.db", ...}
$ bin/ferrite migrate status --data data    database version: 12 ... up to date   [exit 0]
```

The install id stayed `01a18c51-e2e8-72c2-ba4f-25d624adacfb` throughout. Not run here:
the systemd user layout (a root-run rollback with a service user); ownership is covered by
the R11 unit and integration tests, not by this run.

The first run (`$T/life`, log `$T/life*.log`, before X1) stopped at step 6:

```
$ cat data/upgrade.json            (after rollback --schema to 0.0.9 and the re-upgrade)
{"v": 1, "from": "0.1.0", "to": "0.1.0", "schema_from": 11, "schema_to": 12, ...}
$ bin/ferrite rollback --schema --restore-snapshot --data data --yes
Error: .../bin/ferrite.prev reports "ferrite 0.0.9" (exit Some(0)), but upgrade.json records the
upgrade from 0.1.0: the schema goes back for ferrite 0.1.0, so put that binary at .../bin/ferrite.prev
and retry; nothing was changed                                                    [exit 1]
```

### (b) ACME against Pebble

`FERRITE_TEST_PEBBLE=1 cargo test -p ferrite --features test-seams --test tls` (log
`$T/pebble.log`; `ghcr.io/letsencrypt/pebble` pinned by digest, real TLS-ALPN-01
validation back to the node as `host.docker.internal`):

```
test tls_files_are_0600 ... ok
test redirect_listener_only_redirects ... ok
test tls_request_counted_once ... ok
test hsts_only_on_tls ... ok
test busy_port_fails_start_cleanly ... ok
test acme_against_pebble_serves_https ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.11s
```

From a second run with `--nocapture` (Pebble's log):

```
Pulled a task from the Tasks queue: &va.vaTask{Identifier:acme.Identifier{Type:"dns", Value:"host.docker.internal"}, ...
Starting 3 validations.
authz 27d9CNWLrnwa2g5Awn9H4rVNq0ElOlCle3Wu1JrMuwM set VALID by completed challenge IPm7ZE8wXqsVmxNIuWRmTiC5xilYI2JBWiMfoDNok3Y
Order O-ZaEbGnIx1Ya7JKYX0Os53I0aj0Y3D08mkzNVfAsRo is fully authorized. Processing finalization
Issued certificate serial 4d104772bd4fe3e2 for order O-ZaEbGnIx1Ya7JKYX0Os53I0aj0Y3D08mkzNVfAsRo
test acme_against_pebble_serves_https ... ok
```

No Pebble container was left behind (`docker ps -a`).

### (c) `/metrics`, `node_metrics` rows, the series API

`$T/scripts/metrics.py` on the lifecycle site with 0.1.0, `FERRITE_SERVER__LISTEN=127.0.0.1:18080`
and `FERRITE_SERVER__METRICS_LISTEN=127.0.0.1:19464` (log `$T/metrics.log`):

```
$ ferrite config check (with FERRITE_SERVER__METRICS_LISTEN)
ok: mode=solo database=sqlite media=local workers=14 mail=off metrics=127.0.0.1:19464 tls=off backup=off
GET / -> 200, GET / -> 200, GET /published-on-0-1-0/ -> 200, GET /no-such-page/ -> 404,
GET /healthz -> 200, GET /readyz -> 200
GET http://127.0.0.1:19464/metrics -> 200
   ferrite_build_info{version="0.1.0",target="aarch64-apple-darwin"} 1
   ferrite_http_requests_total{class="page",status="2xx"} 3
   ferrite_http_requests_total{class="page",status="4xx"} 1
   ferrite_http_requests_total{class="other",status="2xx"} 2
   ferrite_http_request_duration_seconds_count{class="page"} 4
   ferrite_page_cache_hits_total 1
   ferrite_page_cache_misses_total 4
   ferrite_jobs{status="queued"} 1 / {status="done"} 9
   ferrite_db_pool_connections{state="idle"} 2 / {state="used"} 1
   ferrite_nodes 1
   ferrite_page_views_total 3
  families: build_info, db_pool_connections, http_request_duration_seconds, http_requests_total,
  jobs, nodes, page_cache_bytes, page_cache_entries, page_cache_hits_total,
  page_cache_misses_total, page_views_total, plugin_hook_calls_total,
  plugin_hook_duration_seconds, plugin_hook_failures_total
GET /metrics on the public listener -> 404   (301 to /metrics/, then the 404 page)
GET /other on the metrics listener -> 404
waiting 59s for the next whole minute
node_metrics rows while serving:  (node_id, minute µs, requests, 5xx, views, hits, misses, jobs_done, page ≤100 ms)
   (…, 1793355540000000, 2, 0, 2, 0, 2, 0, 2)    an earlier lifecycle start's flushed minute
   (…, 1793357100000000, 6, 0, 3, 2, 5, 2, 6)    this run's first minute
login -> 200
GET /api/v1/system/metrics/series?from=2026-10-30T09:00:00Z&to=2026-10-30T10:47:04.646687Z&step=hour -> 200
  step hour from 2026-10-30T09:00:00Z to 2026-10-30T11:00:00Z buckets 2
   {'start': '2026-10-30T09:00:00Z', 'requests': 0, ..., 'p95_ms': 0} page_latency [0, 0, …]
   {'start': '2026-10-30T10:00:00Z', 'requests': 8, 'errors_5xx': 0, 'page_views': 5,
    'cache_hits': 2, 'cache_misses': 7, 'p95_ms': 5} page_latency [8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
step=minute -> 200 buckets 108
step=minute over 2 days -> 422 {"type":"/api/v1/problems/validation",…,"detail":"a minute series spans at most 24 hours","errors":[{"field":"step",…
without a session -> 401
serve exit: 0
node_metrics after shutdown (count, requests, page_views): [(3, 14, 5)]
```

The health checks are counted under `other` on `/metrics` and left out of the rows
(M4-2 R3); the six requests of the second row are the four page GETs and the public
`/metrics` 301 and 404.

### (d) Dashboard E2E

The admin built (`pnpm -C admin build`) and `cargo build --release --locked -p ferrite`
(with fix X1), then `pnpm e2e --project chromium-sqlite e2e/dashboard-metrics.spec.ts
e2e/dashboard.spec.ts` (log `$T/e2e-dashboard.log`):

```
Running 7 tests using 2 workers
  ✓  2 [chromium-sqlite] › e2e/dashboard.spec.ts:81:1 › admin_sees_health_and_attention (660ms)
  ✓  3 [chromium-sqlite] › e2e/dashboard.spec.ts:122:1 › author_sees_no_admin_cards (778ms)
  ✓  4 [chromium-sqlite] › e2e/dashboard.spec.ts:148:1 › tiles_match_counts (516ms)
  ✓  5 [chromium-sqlite] › e2e/dashboard.spec.ts:197:1 › settings_failure_keeps_the_dashboard (337ms)
  ✓  6 [chromium-sqlite] › e2e/dashboard.spec.ts:213:1 › screens (14.4s)
  ✓  1 [chromium-sqlite] › e2e/dashboard-metrics.spec.ts:47:1 › traffic_card_shows_page_hits (39.6s)
  ✓  7 [chromium-sqlite] › e2e/dashboard-metrics.spec.ts:155:1 › screens (6.6s)
  7 passed (47.7s)
```

### (e) Release steps, image, systemd, compat

**Release steps for `aarch64-apple-darwin`** as `release.yml` runs them (`$T/scripts/release.sh
$T/rel2`, log `$T/release.log`; the build: `Finished release profile in 3m 32s` from a cold
target folder with a warm registry, 1m 49s for the rebuild with X1):

```
$ file target/release/ferrite                Mach-O 64-bit executable arm64   (53 540 832 bytes)
$ target/release/ferrite --version           ferrite 0.1.0
$ .github/scripts/package.sh 0.1.0 aarch64-apple-darwin target/release/ferrite $T/rel2/v0.1.0
drwxr-xr-x  0 … ferrite-0.1.0-aarch64-apple-darwin/
-rw-r--r--  0 …     1504 ferrite-0.1.0-aarch64-apple-darwin/README.md
-rwxr-xr-x  0 … 53540832 ferrite-0.1.0-aarch64-apple-darwin/ferrite
$ .github/scripts/sums.sh $T/rel2/v0.1.0 1
398c7e5dc4e993991d3ef0440263d60872b0f0bbfe5121b13e70e2e6654f676b  ferrite-0.1.0-aarch64-apple-darwin.tar.gz
$ minisign -S -s $T/keys/test.key -m …/SHA256SUMS -x …/SHA256SUMS.minisig      [exit 0]
$ .github/scripts/verify-sig.sh $T/keys/test.pub $T/rel2/v0.1.0
Signature and comment signature verified
Trusted comment: timestamp:1793355496	file:SHA256SUMS	hashed
signature ok (prehashed)                                                          [exit 0]
$ .github/scripts/verify-sig.sh $T/keys/other.pub …          (another key)
Signature key id in …/SHA256SUMS.minisig is 909BFF9BB605467F
but the key id in the public key is 879D067E6E4DCEA9                              [exit 1]
$ .github/scripts/verify-sig.sh crates/ferrite/release.pub …  (the placeholder)
::error::crates/ferrite/release.pub holds no minisign public key                  [exit 1]
$ .github/scripts/sums.sh $T/rel2/v0.1.0                      (default: six assets)
::error::expected 6 assets, found 1                                               [exit 1]
$ minisign -Vm SHA256SUMS -p test.pub && shasum -a 256 --check --ignore-missing SHA256SUMS
Signature and comment signature verified ... ferrite-0.1.0-aarch64-apple-darwin.tar.gz: OK
```

The release workflow pipes the password to `minisign -S` on stdin; checked with minisign
0.12 (Homebrew) and a password-protected key: the right password signs (exit 0), a wrong
one fails (`Wrong password for that key`, exit 2). Ubuntu's packaged minisign was not run.

**Docker** (`docker build -t ferrite:m4-2-exit .`, 224 s, log `$T/docker-build.log`):

```
$ docker run --rm ferrite:m4-2-exit --version           ferrite 0.1.0
$ docker image inspect …                                arm64 89807785 65532 [/ferrite] [serve --data /data]
$ docker run --rm -v t11-ferrite-data:/data ferrite:m4-2-exit init --data /data --url http://localhost:8080
  database migrated applied=[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
$ docker run --rm -v t11-ferrite-data:/data ferrite:m4-2-exit config check --data /data
ok: mode=solo database=sqlite media=local workers=14 mail=off metrics=off tls=off backup=off
$ docker run -d --name t11-ferrite -e FERRITE_SERVER__LISTEN=0.0.0.0:8080 -p 18090:8080 -v t11-ferrite-data:/data ferrite:m4-2-exit
GET / -> 200
$ docker exec t11-ferrite /ferrite healthcheck          [exit 0]
$ docker stop t11-ferrite                               exit 0
```

The container and the volume were removed afterwards; the image `ferrite:m4-2-exit` was
kept. The three base digests in the Dockerfile matched their tags on 2026-10-30
(`docker buildx imagetools inspect node:24-bookworm-slim` → `sha256:c86ecead…920c`,
`rust:1-bookworm` → `sha256:9181b56c…c8b`, `gcr.io/distroless/cc-debian12:nonroot` →
`sha256:9dac0a79…182f`, each equal to the `FROM` line).

**systemd** in an `ubuntu:24.04` container (a stand-in `/usr/local/bin/ferrite`, the
`ferrite` user, `/var/lib/ferrite`, as CI does; the pulled image was removed afterwards):

```
systemd 255 (255.4-1ubuntu8.18)
$ systemd-analyze verify /unit/ferrite.service                                    [exit 0]
$ systemd-analyze security --offline=true /unit/ferrite.service
✗ UMask=          Files created by service are group-readable by default          0.1
→ Overall exposure level for ferrite.service: 3.2 OK :-)
```

**Backward compatibility** (`.github/scripts/compat.sh` with A, the M4-1 binary, as the old
one and B as the new, on SQLite and Postgres; the schema `ferrite_compat` was dropped
afterwards; log `$T/compat.log`):

```
old: ferrite 0.0.9
new: ferrite 0.1.0
== sqlite: the old binary reads the newer schema
database version: 12
binary version: 11 (ferrite 0.0.9)
pending migrations: none
unknown applied migrations: 12
verdict: newer schema, compatible
== sqlite: the old binary serves it
GET / -> 200
GET /healthz -> 200
== sqlite: ok
== postgres: (the same: newer schema, compatible; GET / -> 200; GET /healthz -> 200)
== postgres: ok
compat: ok
```

The same run showed that an older binary refuses the new config keys
(`server.tls: unknown key`; `env:FERRITE_SERVER__METRICS_LISTEN: unknown key`), as the
rollback docs say.

## 2. Gates

At `fb31c97` plus the docs of this exit (`$T/scripts/gates.sh`, log `$T/gates.log`):

| Gate | Result |
|---|---|
| `cargo fmt --check` | ok (after `cargo fmt` of the new test) |
| `cargo clippy --workspace --all-targets -- -D warnings` | ok |
| the same with `--features ferrite/test-seams,ferrite-http/test-seams,ferrite-import-wp/test-seams` | ok |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | ok |
| `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace --features …test-seams` | exit 0: 110 test binaries, 3391 passed, 0 failed, 1 ignored |
| `pnpm -C admin lint`, `typecheck`, `build`, `bundle:check`, `api:check` | ok |
| `pnpm -C admin test` | 124 files, 743 tests passed |
| E2E `chromium-sqlite`, the two dashboard specs | 7 passed (§1 d) |

One observation: `cargo build -p ferrite --features ferrite/test-seams` builds a binary
**without** the seams (with `-p ferrite`, `ferrite/test-seams` names the crate's own
dev-dependency on itself, which a plain build does not use). A test-seams binary is
`cargo build -p ferrite --features test-seams`. The workspace gates are unaffected: tests
and `--all-targets` enable the seams through that dev-dependency anyway.

## 3. Found during the exit

- **X1 (fixed, `fb31c97`): a schema rollback left `last_binary_version` naming the newer
  binary.** After `rollback --schema` to a binary that does not write the key (any build
  before M4-2; and equally a re-upgrade before the older binary ever started), the next
  upgrade recorded `from: <newer>`, and its own `rollback --schema` refused `ferrite.prev`
  (§1 a, first run). `rollback --schema` now sets the key to the binary that runs next:
  the record's `from`, or for a record without one what that binary's `--version` reports.
  `--restore-snapshot` needed nothing: the snapshot predates the start that wrote the newer
  version. Red run: `rollback_schema_stores_the_older_version` (`left: Some("0.1.0")`,
  `right: Some("0.0.1")`) and `rollback_schema_without_from_stores_the_reported_version`
  (`left: Some("0.1.0")`, `right: Some("0.0.7")`); green: `tests/rollback.rs` 14/14, and
  the second lifecycle run (§1 a) end to end.
- Two stale comments about the `other` class (`MetricsSnapshot::requests`,
  `admin/src/dashboard/traffic-card.tsx`) were brought in line with M4-2 R3 as amended.

## 4. Accepted limits

- Rollback goes back one step (P32); two updates without a restart leave a `ferrite.prev`
  the site never ran (documented, no warning).
- On a cluster `upgrade.json` is informational; the schema goes back with `migrate down
  --to N` (the cluster runbook).
- `--restore-snapshot` loses what was written since the upgrade (it says so and asks), and
  the media GC later deletes uploads only the moved-aside database refers to (documented).
- The page p95 includes form posts and public 405s (M4-2 R3 as amended).

## 5. Not verified

| Item | Why | Covered instead by |
|---|---|---|
| A GitHub run of `release.yml` (tag push and `workflow_dispatch` dry run) | never pushed or tagged (rule) | the steps by hand for one target (§1 e); task 10's `actionlint`/YAML checks |
| Builds of `x86_64`/`aarch64-unknown-linux-gnu`, both `*-linux-musl`, `x86_64-apple-darwin` | no such host here | `aarch64-apple-darwin` built, packed, signed, installed by `self-update` |
| The musl static-binary check and the musl CA-bundle behaviour | no Linux runner here | `release.yml` step; rustls-native-certs source (final review C) |
| GHCR publishing, `imagetools create`, the package's visibility and Actions access | nothing published | local `docker build` and run (arm64 only) |
| The `linux/amd64` image | arm64 host | — |
| The `compat` CI job itself (on GitHub, with a previous tag) | no tag exists yet | `compat.sh` run by hand with the M4-1 binary (§1 e) |
| Real Let's Encrypt (staging or production), renewal at a third of the validity left, the backoff timing ("within a few minutes") | no public host | Pebble (§1 b); rustls-acme 0.15.4 source |
| The systemd unit on a booted systemd host (ports 80/443 through `CAP_NET_BIND_SERVICE`, `ReadWritePaths` drop-ins, `StartLimit*`, a root-run rollback with the service user) | no systemd host | `systemd-analyze verify` and `security` in a container (§1 e); R11 tests |
| Ubuntu's packaged minisign reading the password on stdin (the `sign` job) | not run | minisign 0.12 (Homebrew) does (§1 e) |
| GitHub plan details: rulesets and a "restrict creations" rule for `v*` tags on a private personal repository | the ruleset docs page did not state it for private repositories | — |
| The bundled tzdb's size (P29 said about 0.5 MB) | not measured | — |
| Self-update of a dry-run artifact | dry-run binaries embed the placeholder key | a test-seams updater (§1 a) |

## 6. Release gate (the user's actions, in order)

None of these was done here; each needs the repository owner.

1. **License.** There is no `LICENSE*` file and no `license` field in `Cargo.toml`
   (BRIEF:268 leaves it open). `package.sh` includes `LICENSE*` in every asset once present.
2. **The release key.** Create the minisign key pair and commit the public key (09 §1):

   ```sh
   minisign -G -p release.pub -s release.key      # asks for a password
   cp release.pub crates/ferrite/release.pub       # two lines; commit it
   ```

   Keep `release.key` and its password offline. The workflow refuses to publish while
   `release.pub` is the placeholder, and checks its own signature against the committed
   key. `cargo test` passes with a real key (`embedded_key_file_parses`).
3. **The GitHub environment `release`** (Settings → Environments):
   - deployment branches and tags: only tags matching `v*`;
   - required reviewers if the plan allows: GitHub's docs (read 2026-10-30) say required
     reviewers are not available for private repositories on Free, Pro or Team;
   - environment secrets `MINISIGN_SECRET_KEY` (the contents of `release.key`) and
     `MINISIGN_PASSWORD`, as **environment** secrets, not repository secrets.
   - In a private repository, environments and environment secrets need GitHub Pro (a
     personal account) or Team (verified in GitHub's docs, 2026-10-30).
4. **A tag ruleset** restricting who may create, update or delete `v*` tags (with the
   environment's tag policy, this is what stops anyone with write access from publishing
   a signed release). Plan availability for a private repository: not verified (§5).
5. **First run:** a `workflow_dispatch` run of `Release` on `develop` or `main` (dry run:
   throwaway key, artifacts only) to prove the six builds and the image jobs' inputs.
6. **The first tag `v0.1.0`.** It freezes migrations `0001`–`0012` (m1a R10): from then on
   applied migrations are immutable (sqlx checksums) and the `compat` job runs against it
   from the next tag on.
7. **GHCR after the first publish:** `ghcr.io/softberries/ferritecms` is created private;
   make it public (package settings) and check that it is linked to the repository with
   Actions access (the image's `org.opencontainers.image.source` label links it).
8. **Later:** image signing or provenance (cosign, or GitHub artifact attestations).

## 7. Open follow-ups

In [ROADMAP](../ROADMAP.md), M4, "Deferred from M4-2" (parked ledger items, final-review
minors, the metrics follow-ups, rollback without hard links, the release gate).

## 8. Verified claims

External facts stated in the docs of this exit, and how each was checked (the user's rule).
Claims about the code were checked against the code or a run; the main ones are listed too.

| Claim (where) | Source | Result |
|---|---|---|
| Let's Encrypt: 5 authorization failures per identifier per account per hour; 5 certificates per exact set every 7 days; 50 per registered domain every 7 days; 300 new orders per account every 3 hours; 10 new accounts per IP every 3 hours (09 §5, runbook) | letsencrypt.org/docs/rate-limits (last updated 2026-08-05), read 2026-10-30 | verified |
| Staging limits are "significantly higher" (09 §5) | same | verified |
| ARI renewals are exempt from the limits; rustls-acme 0.15.4 has no ARI, so its renewals are ordinary orders (09 §5) | same page; no `renewalInfo` in the crate's `src/` | verified |
| Let's Encrypt's default lifetime becomes 64 days on 2027-02-10 and 45 days in 2028 (09 §5) | letsencrypt.org/2026/10/07/64-day-certs (the 2028 switch is given as a year only) | verified |
| Renewal at a third of the validity left, computed from each certificate (09 §5) | rustls-acme 0.15.4 `src/state.rs` (`validity[1] - (validity[1] - validity[0]) / 3`) | verified |
| A changed `contact` makes a new account, not a new certificate (09 §5, runbook) | `crates/ferrite/src/tls.rs` `TlsCache::name` (account: contact + directory; certificate: domains + directory) | verified (code) |
| "A broken setup can use up the 5 failures an hour within a few minutes" (09 §5) | the backoff 1, 2, 4, 8 … s plus order polling | derived, not measured |
| Environments and environment secrets in private repositories need GitHub Pro or Team; required reviewers are not available for private repositories below Enterprise (09 §7, §6 here) | docs.github.com "Managing environments for deployment" and "Deployments and environments", read 2026-10-30 | verified |
| Tag rulesets and their plan availability for a private personal repository (§6 here) | docs.github.com "About rulesets" (not explicit for private repositories) | not verified |
| A new GHCR package is private and is linked by the `org.opencontainers.image.source` label (§6 here, 09 §7) | final review C (docs.github.com, package access control) | verified there, not re-read |
| minisign signs prehashed by default, `-Vm FILE -p KEY` verifies with `FILE.minisig`; it reads the password on stdin when piped | minisign 0.12 runs (§1 e) | verified (Homebrew build) |
| The Dockerfile's base digests are the current digests of their tags (09 §7) | `docker buildx imagetools inspect`, 2026-10-30 | verified |
| systemd 255 accepts the unit; exposure 3.2 (09 §7) | `systemd-analyze` in `ubuntu:24.04` (§1 e) | verified |
| An M4-1 binary runs on schema 12 (02 §8) | `compat.sh` (§1 e) | verified (SQLite and Postgres) |
| An older binary refuses `[server.tls]` and `FERRITE_SERVER__METRICS_LISTEN` (09 §2, runbook) | `config check` with the M4-1 binary | verified |
| Shipped binaries enable only jiff's `std` + `tzdb-bundle-always`; test builds get the default features through `s3s-fs` (06) | `cargo tree -p ferrite -e features,normal -i jiff` and `-e features` | verified |
| Clap usage errors exit 2 for `rollback` and `self-update` (09 §2, §8) | runs of the release binary | verified |
| `migrate up` writes `upgrade.json` (09 §8) | `crates/ferrite/src/migrate.rs` → `upgrade::migrate_and_record` | verified (code) |
| `node_metrics.prune` hourly, 7 days, 10 000-row batches, 240 s budget (04 §6) | `crates/ferrite-cluster/src/builtin.rs`, `ferrite_domain::NODE_METRICS_RETENTION`, `MAX_PRUNE_BATCH` | verified (code) |
| No aws-lc in `Cargo.lock` (ROADMAP, 09 §7) | `grep -c '^name = "aws-lc' Cargo.lock` → 0 | verified |
| The bundled tzdb adds about 0.5 MB (P29) | — | not verified |
