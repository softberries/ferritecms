# M1.5 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M1.5): *two nodes behind a proxy; a save on node A is
reflected on node B within ~1 s; killing a node mid-job doesn't lose the job.*

**Result (2026-10-23, branch `m15-cluster` at `7efa3d3` plus the Task 12 commit):** met for
two nodes **without** a proxy, with the substitutions and gaps below. Nothing here was run
behind a reverse proxy or in Docker.

| Part of the criterion | Verified by | Result |
|---|---|---|
| Save on node A reflected on node B within ~1 s | in-process nodes (`invalidation_latency_under_1s`, `poll_only_node_converges_within_1s`), two `ferrite serve` children (`two_processes_share_invalidation`), manual two-process run | yes: p50 8.0–14.4 ms with `LISTEN`, max 258 ms poll-only, all under 1 s |
| Killing a node mid-job doesn't lose the job | `lease_expiry_after_crashed_node` (in-process crash, P25) | yes: finished by node 1 2.52 s after the crash (2 s test lease) |
| Behind a proxy | `examples/cluster/Caddyfile` validated only | **not run** |

## What was not verified (limits)

1. **No reverse proxy run.** The Caddy configuration (`examples/cluster/Caddyfile`:
   `reverse_proxy node1:8080 node2:8080`, `lb_policy round_robin`, `health_uri /readyz`) was
   written against the Caddy docs and is validated only as part of `docker compose config`;
   no request went through Caddy. Nodes were reached directly on their own ports.
2. **No `docker compose up`.** Docker does not respond on the development machine
   (`docker info` hangs), so neither the image nor the compose stack was run here (P24).
   Locally verified: `docker compose -f examples/cluster/docker-compose.yml config --quiet`
   exits 0 (client-side, no daemon needed) and `crates/ferrite/tests/examples.rs` loads the
   example's config and env and runs `ferrite config check` on it. CI adds `docker compose
   config --quiet`, `docker build -t ferrite:ci .` and `config check` inside the built image;
   the first CI run of that job is its first real test. MinIO bucket init, Caddy active
   health checks, the container health check (`/ferrite healthcheck`) and volume
   permissions are untested at runtime.
3. **The crash is simulated in process** (P25): `RunningNode::abort()` drops every task
   without releasing or deregistering, and `pg_terminate_backend` kills that node's
   connections, which leaves the database as a killed process would. No OS process was
   SIGKILLed mid-job; socket teardown timing is not exercised.
4. **A silently dead `LISTEN` connection** (network partition without RST) is not tested
   with a real network cut; the poll-only path is covered by a node started with
   listening turned off (`poll_only_node_converges_within_1s`).
5. **Manual run used local shared media** (`local_shared = true`, one folder) rather than
   S3; the two-process automated test uses an in-process `s3s` S3 server. MinIO itself was
   not used anywhere.
6. Latencies below are from a debug build on one idle laptop (Darwin 25.6.0 arm64,
   rustc 1.98.1, PostgreSQL 17.11 at `localhost:54329`; CI uses `postgres:18`).

## Automated tests

All commands from the repository root at the Task 12 working tree. `$PG` =
`postgres://ferrite@localhost:54329/ferrite_test`.

### Invalidation latency (in-process nodes, one Postgres schema)

```
$ CI=true FERRITE_TEST_PG_URL=$PG cargo test -p ferrite --test cluster -- --nocapture
running 9 tests
test rolling::concurrent_first_start_migrates_once ... ok
invalidation latency, poll only (250 ms): n=10 min=51.0ms p50=171.0ms max=258.2ms all=[51, 85, 87, 115, 165, 171, 173, 186, 252, 258]
test invalidation::poll_only_node_converges_within_1s ... ok
job exclusivity: 300 jobs in 154.660792ms, runs per node {0: 97, 1: 104, 2: 99}
test jobs::job_exclusivity_three_nodes ... ok
auto-disable: 24 shared failures, disabled 4.272326125s after the last request (auto: 24 failures in 10m)
test reload::auto_disable_is_cluster_wide ... ok
crashed node's job finished by node 1 2.523637333s after the crash (lease 2s)
test jobs::lease_expiry_after_crashed_node ... ok
periodic: 5 rows for period starts [1792747046, 1792747047, 1792747048, 1792747049, 1792747050], runs by nodes {0}
test jobs::periodic_job_runs_once_per_period ... ok
reload: plugin disable on 3 nodes 24.17425ms, theme activation 23.222208ms
test reload::theme_and_plugin_reload_reach_every_node ... ok
invalidation latency, LISTEN, 5000 ms poll: n=10 min=7.0ms p50=8.0ms max=31.3ms all=[7, 7, 7, 8, 8, 8, 14, 14, 20, 31]
invalidation latency, listener cut, 5000 ms poll (20 connections cut over 10 saves): n=10 min=22.3ms p50=23.7ms max=26.4ms all=[22, 23, 23, 24, 24, 24, 24, 24, 25, 26]
test invalidation::invalidation_latency_under_1s ... ok
rolling: 30 jobs in 10.083912334s (10.011530875s after the new node joined); old node ran 10
test rolling::rolling_version_coexistence ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 32.40s
```

A separate run of the two named tests gave LISTEN p50 14.4 ms / max 25.6 ms, listener-cut
p50 24.5 ms / max 26.6 ms, and the crashed node's job finished 2.522 s after the crash.

What the numbers mean: latency is timed from the start of `ContentService::update` on node 0
until node 1 serves the new title on both the post page and `/`. In the LISTEN and
listener-cut phases both nodes poll only every 5 s, so only `NOTIFY` (or the `Reconnected`
catch-up after the test kills the `:listen` connections before each save) can meet the 1 s
bound. The poll-only phase runs node 1 with listening off and a 250 ms poll.
`lease_expiry_after_crashed_node`: node 0 claims a job and its handler starts; node 0 is
crashed (P25); node 1 reclaims the job after the 2 s test lease and completes it; node 0's
row stays (stale, not deregistered) and a third node can still join.

Without Postgres the same binary skips every case:

```
$ env -u CI -u FERRITE_TEST_PG_URL cargo test -p ferrite --test cluster -- --nocapture
SKIP: Postgres contract test skipped (FERRITE_TEST_PG_URL is not set)   (×9)
test result: ok. 9 passed; 0 failed; …
```

### Two `ferrite serve` processes

```
$ CI=true FERRITE_TEST_PG_URL=$PG cargo test -p ferrite --test serve -- --nocapture two_processes_share_invalidation
two processes: edit served by both after [35.722834ms, 56.878584ms]
test two_processes_share_invalidation ... ok
test result: ok. 1 passed; 0 failed; … finished in 2.97s
```

Two child processes with their own data folders on one Postgres schema and an in-process S3
(`s3s`) server; a third process edits the seeded post through `ContentService`.

### Compose example config

```
$ cargo test -p ferrite --test examples -- --nocapture
test compose_ferrite_variables_are_known_settings ... ok
test cluster_example_config_is_valid ... ok
test result: ok. 2 passed; 0 failed; …
$ docker compose -f examples/cluster/docker-compose.yml config --quiet; echo $?
0
```

`cluster_example_config_is_valid` copies `examples/cluster/ferrite.toml` into a temp data
folder, parses `ferrite.env`, loads both through `ferrite_config::load` and asserts cluster
mode, S3 endpoint `http://minio:9000`, `allow_http`, bucket `ferrite`, region `us-east-1`,
Postgres host `postgres`, `trusted_proxies = ["172.28.0.0/16"]` (and that the compose
subnet is the same), then runs `ferrite config check` on the folder (exit 0, `ok:
mode=cluster database=postgres media=s3 workers=…`). The second test checks that every
`FERRITE_*` name in `docker-compose.yml` is a known setting (a misspelt name fails it).

### Full workspace, both backends

```
$ CI=true FERRITE_TEST_PG_URL=$PG cargo test --workspace     → exit 0; 62 test-result lines, 1266 passed, 0 failed
$ env -u CI -u FERRITE_TEST_PG_URL cargo test --workspace     → exit 0; 62 test-result lines, 1266 passed, 0 failed (Postgres cases SKIP-pass)
$ cargo fmt --all --check                                     → ok
$ cargo clippy --all-targets -- -D warnings                   → ok
```

## Manual two-node run

Two real `ferrite serve` processes (debug build) on the local Postgres, dedicated schema
`m15_exit_manual_1`, shared local media folder. `$S` is
`<scratchpad>/m15exit-1`.

```
$ ferrite init --data $S/n1 --url http://127.0.0.1:18081 --title "M1.5 exit" \
    --db postgres://ferrite@localhost:54329/ferrite_test --schema m15_exit_manual_1
```

Then both data folders got this `ferrite.toml` (`n` = 1 or 2):

```toml
[site]
url = "http://127.0.0.1:18081"
title = "M1.5 exit"
[server]
listen = "127.0.0.1:1808<n>"
[database]
url = "postgres://ferrite@localhost:54329/ferrite_test"
schema = "m15_exit_manual_1"
[media]
backend = "local"
root = "$S/media"          # absolute path, the same folder for both nodes
local_shared = true
[cluster]
mode = "cluster"
node_name = "manual-node<n>"
```

```
$ ferrite config check --data $S/n1
ok: mode=cluster database=postgres media=local workers=14
$ ferrite config check --data $S/n2
ok: mode=cluster database=postgres media=local workers=14
$ ferrite migrate status --data $S/n1
database version: 4
binary version: 4 (ferrite 0.1.0)
pending migrations: none
verdict: up to date
$ ferrite seed demo --data $S/n1
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
$ ferrite serve --data $S/n1 &   ferrite serve --data $S/n2 &
node1 healthz=200 readyz=200
node2 healthz=200 readyz=200
$ psql … -c "select name, version, heartbeat_at > now() - interval '30 seconds' from nodes"
manual-node1|0.1.0|t
manual-node2|0.1.0|t
```

**How the edits were made.** There is no content-edit CLI and no write API on `serve` yet
(M2), so "save via node A" cannot be done literally. Two writers were used, both outside the
serving processes:

- **A) direct SQL plus outbox write** (one `psql` transaction): `UPDATE content_items SET
  title = …, version = version + 1, updated_at = now()` for the post
  `/welcome-to-ferritecms/`, and the same statement shape `EventRepo::append` uses:
  `WITH ins AS (INSERT INTO events (kind, keys, origin_node, created_at) VALUES
  ('invalidate', '["post:<id>","archive:post"]', NULL, now()) RETURNING seq) SELECT seq,
  pg_notify('ferrite_events_m15_exit_manual_1', seq::text) FROM ins`;
- **B) the CLI through the core services:** `ferrite seed demo --force` (adds a second batch
  of posts through `ContentService`, which writes the outbox events itself).

Before each edit both nodes had served the page twice (page cache warm). A Python script
(`urllib`, 10 ms between attempts) then fetched from node 2 first, then node 1, timing from
the moment the writer returned:

```
warm: both nodes served /welcome-to-ferritecms/ twice with the old title
A) SQL+outbox commit returned: ['SET', 'BEGIN', 'UPDATE 1', '54|', 'COMMIT']
A) node2 served the new title after 22.8 ms (1 polls)
A) node1 served the new title after 52.7 ms (1 polls)
B) CLI exit=0: Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
B) node2 home page listed the new batch 9.8 ms after the CLI exited (1 polls)
B) node1 home page listed the new batch 22.7 ms after the CLI exited (1 polls)
```

"1 polls" means the **first** request after the writer returned already showed the change, so
these times are request round trips (node 1's include node 2's request before it), an upper
bound on propagation, not a measurement of it. To show that the cache, not a cache miss,
served the old content, a control run updated the row **without** an event:

```
C) row updated, no event, after 1.5 s: node1 serves new title = False
C) row updated, no event, after 1.5 s: node2 serves new title = False
C) event committed: node1 serves new title 23.5 ms later
C) event committed: node2 serves new title 45.6 ms later
```

So both nodes kept serving the cached old title until the outbox event arrived, and then
both served the new one within one request.

Shutdown and job state afterwards:

```
$ kill -TERM <node1> <node2>
both exited 0.13 s after SIGTERM
$ psql … -c "select count(*) from nodes; select status, count(*) from jobs group by 1"
nodes left|0
done|10
node1 log: shutdown signal received; draining connections (up to 10s) signal="SIGTERM"
           left the node registry … / node stopped / shut down
```

The ten `done` jobs were the periodic jobs (`events.prune`, `jobs.prune`, `nodes.prune`,
`plugin_failures.prune` once each, `content.publish_due` six times over the 2 min 40 s the nodes ran), one
row per period although two nodes scheduled them.
