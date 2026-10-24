# 04 — Multi-Instance (Cluster) Design

## 1. Requirements

| # | Requirement | Source |
|---|---|---|
| C1 | N identical stateless nodes behind any HTTP load balancer, no sticky sessions | User (D5) |
| C2 | No coordination service besides the database (no Redis, no gossip) | D7 |
| C3 | A content save is visible on all nodes within ~1 s (cache invalidation) | Design target |
| C4 | Background work runs exactly-once-effectively, survives node crashes | Design |
| C5 | All nodes run the same theme and plugin set | D11 |
| C6 | Rolling deploys: nodes of version N and N+1 coexist during a deploy | D8 |
| C7 | Solo mode pays (almost) nothing for this design | Anti-goal |

## 2. Modes

**As built (M1.5).** The coordination mechanisms are chosen by the **database backend**,
not by `cluster.mode` (plan ruling P1): a solo node on Postgres still shares a database
that `ferrite migrate` or a second node can reach.

| Concern | SQLite | Postgres | Code |
|---|---|---|---|
| Event bus | `events` outbox, polled | outbox + `pg_notify` from the inserting statement + polling | `ferrite-store-*` `repos/event.rs`, `ferrite-cache/src/invalidation.rs` |
| Job queue | `jobs` table, claimed on the single writer | `jobs` table, `FOR UPDATE SKIP LOCKED` | `repos/job.rs`, `ferrite-cluster/src/jobs/` |
| Locks | in-process mutex per store | session-level advisory lock on a dedicated connection | `lock.rs` in both store crates |
| Notifications | `Store::subscribe` → `None` (callers poll) | `LISTEN` on a dedicated connection | `ferrite-store-postgres/src/listen.rs` |
| Node registry | `nodes` rows, heartbeat | same | `ferrite-cluster/src/node.rs` |

`cluster.mode` decides only three things (P1): whether the node **listens** for
notifications (the invalidation loop and the job runner; cluster mode only, so a solo
node on Postgres sends `NOTIFY`s that nobody receives), the **solo single-live-node
rule** (§3), and whether `data/themes/` folders are read (solo only, P17,
[06 §1](06-themes-rendering.md#1-theme-package)).

`cluster.mode = "auto"` resolves to `cluster` only for Postgres **and** shareable media
(S3, or local with `local_shared = true`); otherwise `solo` (M1c-1 R5,
`Config::effective_cluster_mode`). Postgres + solo is allowed (explicit `mode = "solo"`
or local media), e.g. a single VPS with a managed DB.

## 3. Startup validation

**As built.** Configuration-only checks run in `ferrite_config::load` (and so in
`ferrite config check`, [09 §8](09-operations.md#8-cli-summary)); refused:

- `mode = "cluster"` with SQLite;
- `mode = "cluster"` with `media.backend = "local"` unless `media.local_shared = true`
  (the operator asserts the folder is a shared volume, e.g. NFS/EFS).

Checks that need the database run when `serve` joins the node registry
(`ferrite_cluster::Registry::join`, P14, P15), after the migration check:

- A **live** node (heartbeat within 30 s by the database clock) with a different
  `site_id` (the `schema_meta` row was replaced under running nodes, e.g. a restore)
  refuses the start.
- A live node with a different `site.url` (normalised, no trailing slash) refuses the
  start: the realistic sign of a wrong `database.schema` / `search_path`, which a
  `site_id` comparison cannot catch because both nodes read the same `schema_meta`.
  Consequence: changing `site.url` needs every old node stopped (or 30 s after the last
  one stopped).
- **Solo mode:** another live node is waited for until it goes stale, up to 35 s
  (a crashed predecessor goes stale after 30 s), then the start is refused. A cluster
  joiner only warns about live nodes registered in solo mode.
- The waiting is unlocked; the final check-and-register pass runs under the
  `cluster.join` lock (10 s wait), so simultaneous starts are checked one at a time
  (ruling R7). A solo node also re-checks on every heartbeat and logs an error when a
  second live node appears (R8: detection only; job leases keep correctness).
- A schema without `site_id` (never migrated) is refused.
- SQLite: the data-folder lock (`ferrite.lock`) refuses a second process on the same folder.
  So on SQLite every other `nodes` row was left by a dead process (a crash, `kill -9`):
  the join deletes those rows without waiting or comparing `site_id`/`site.url`, and the
  heartbeat skips the R8 check (ruling R17). A restart after a crash is immediate, and a
  `site.url` changed in between is accepted. The live-node checks above apply where
  processes share the database (Postgres, either mode).

Clock skew is measured on every heartbeat against `SELECT now()` at the round trip's
midpoint and logged when it goes above 2 s (and when it is back under 1.5 s); it is
reported as `nodes.meta.clock_skew_ms`. It is a warning only: every lease, `run_at` and
heartbeat comparison uses the database clock.

## 4. Event bus & cache invalidation

### Write path (transactional outbox)

Every core write that affects rendered output inserts an `events` row **in the same
transaction**:

```
events(seq, kind = "invalidate", keys = ["post:0192…", "term:0192…", "archive:post"], origin_node, created_at)
```

The insert itself notifies the site's events channel (`ferrite_events[_<schema>]`, payload
`<seq>`), which Postgres delivers when the transaction commits (P2; every mode on Postgres,
P1). Notify is an optimisation only; correctness comes from the table.

### Read path (every node)

**Polling, as built since M1c-2** (`crates/ferrite-cache/src/invalidation.rs`, both backends,
used by `ferrite serve` in every mode); M1.5 added wake-ups and the history check (below).

- Every `cache.poll_interval_ms` (default 250 ms) the loop reads
  `events.since(last_seq, 1000)`, continuing while batches come back full (at most 10
  batches per tick, shared with the late probe below), and applies each event to the page
  cache.
- **Sequence gaps.** `bigserial` values are allocated at insert time but committed out of
  order, so a reader can see seq 11 before 10 commits. Every event read is applied at once
  (invalidation is idempotent), but `last_seq` advances only up to the first gap, so the next
  poll re-reads from there and sees the late commit. Events already applied past a gap are
  remembered and not re-applied. A gap still open after 5 s (`GAP_TIMEOUT`) is skipped,
  because a rolled-back transaction leaves a permanent hole.
- **Skipping a gap (ruling R24, superseding R23)** flushes the whole page cache, and every
  skipped seq goes into a *late set* watched for 10 minutes (`LATE_HORIZON`): each tick probes
  once with `since(min_late - 1, 1000)`, resuming where the last probe stopped, applies any
  late seq that has appeared and drops it from the set. A seq not seen within the horizon is
  dropped with a warning. At most 10 000 seqs are watched (`MAX_LATE`); beyond that a skip
  only flushes (warned). So a writer whose transaction stayed open longer than the gap timeout
  still invalidates when it commits.
- **Boot.** `last_seq` starts at `max(seq)`, except that the last 1000 events are inspected
  first: a hole below the maximum may be a transaction that allocated its seq before boot and
  commits after it, so the cursor starts at the first such hole. The loop is created before
  the server binds, so no page is cached before the cursor is set; the cache starts empty.
- Store errors are logged and retried on the next tick.

**Store side, as built (M1.5, P1/P2/P4):**

- `pg_notify` runs inside the writing statement: `events.append` is
  `WITH ins AS (INSERT … RETURNING seq) SELECT seq, pg_notify(<channel>, seq::text) FROM ins`,
  `jobs.enqueue` notifies with the job kind only when a row was inserted. Postgres delivers
  it at commit, never for a rollback, so no write path can forget it.
- Channels are database-wide, sites are schemas: `ferrite_events` / `ferrite_jobs` without
  a schema, `ferrite_events_<schema>` / `ferrite_jobs_<schema>` (ASCII-lowercased) with one.
- `Store::subscribe(Channel)` returns a `Subscription` on its own one-connection pool
  (`<application_name>:listen`); `recv` gives `Notified`, or `Reconnected` after the
  connection was lost and re-established (notifications in between are lost). SQLite
  returns `None` and keeps polling. `EventRepo::min_seq` is the history check for P3.
  An error from `recv` is followed, once the next `recv` rebuilds the listener, by
  `Reconnected`, so every possible gap shows up as `Reconnected`.
- Consumers must drain a subscription continuously or drop it: a listener that stops
  reading holds back Postgres' NOTIFY queue, and when that queue is full every committing
  transaction that notifies fails.
- App connections run `idle_in_transaction_session_timeout = 60s`.

**Loop side, as built (M1.5, P3):**

- `InvalidationLoop::spawn(poll_interval, listen, shutdown)`. With `listen` (cluster mode,
  set by `serve`) a subtask owns the events subscription and drains it
  continuously. `Notified` wakes the loop for an immediate poll through a
  `tokio::sync::Notify`; permits coalesce, so a burst of notices costs one extra poll.
  `Reconnected` also sets a "check history" flag. A subscribe or `recv` error drops the
  subscription and resubscribes after 1 s, doubling to 30 s; the delay resets only once a
  subscription has delivered a notice or lived 30 s (one that fails at once keeps backing
  off). Once a resubscribe succeeds the flag is set too. SQLite (`subscribe` → `None`)
  ends the subtask and the loop only polls. The ticker keeps polling every `cache.poll_interval_ms` regardless: notify is an
  optimisation only. The subtask ends on shutdown and drops the subscription before the
  loop's task completes.
- **History check** runs before the poll when the flag is set, on the first poll after
  one or more failed polls, and when a poll first sees a gap right at the cursor
  (`last_seq + 1`, one check per gap, so pruned history is caught even without a
  `Reconnected`): `min = events.min_seq()`; if `min > last_seq + 1`, events were
  pruned past the cursor, so the loop warns, **flushes the whole page cache**, calls
  `EventHook::on_history_lost` (the node reloads its state), sets `last_seq = min - 1` and
  drops open gaps, and applied-past-gap and late seqs below `min` (events already applied
  stay remembered, so the hook still sees each seq at most once). Otherwise it only
  catches up from `last_seq`. `events.prune` never deletes the newest row (ruling R9:
  `AND seq < (SELECT MAX(seq) FROM events)` on both backends), so `min_seq` is 0 only for
  an outbox that never had an event and pruned history is always detectable.
- `EventHook { on_event, on_history_lost }` (synchronous, must not block) is attached with
  `InvalidationLoop::with_hook`. It sees every event applied after `new()` returned, late
  arrivals included, after the event's cache effect; events applied inside `new()` (the
  boot window) are not passed on, because the caller reads node state after creating the
  loop (P22). `node.drain` stays a no-op (P26).
- **Late-probe bound.** The probe pages through committed rows from the smallest late seq
  at 1000 rows per batch, resuming where the previous tick stopped, so a late arrival is
  found within ⌈span / (POLL_BATCH × probe budget)⌉ ticks (span = largest − smallest late
  seq; probe budget = the batches a tick has left after the main read, 1 to 10). The bound
  counts from the probe's position; an arrival it already passed is found after it wraps.
  Worst case, at the minimum budget of one batch per tick: ⌈span / POLL_BATCH⌉ + 1
  ticks. The algorithm is unchanged from M1c-2.
- On Postgres, `idle_in_transaction_session_timeout` (60 s, P4) is shorter
  than the 10-minute late horizon, so no idle writer holds a seq uncommitted past it (a
  `Tx` busy with long statements is not bounded, P4).

Cross-node behaviour is tested end to end in `crates/ferrite/tests/cluster/`
(`invalidation_latency_under_1s`, `poll_only_node_converges_within_1s`) and with two
`ferrite serve` processes (`crates/ferrite/tests/serve/site.rs`, `two_processes_share_invalidation`);
see [`m15-exit.md`](../superpowers/m15-exit.md).

### Event kinds

As built. The invalidation loop (`ferrite-cache`) applies the cache effect; the node's
`EventHook` (`ferrite::reload::Reloader`, P18) then reloads node state off the loop.

| Kind | Keys | Emitted by | Cache effect | Node effect |
|---|---|---|---|---|
| `invalidate` | dependency keys | core write paths | drop pages under the keys (`all` flushes) | — |
| `options.changed` | `option:<key>` | `OptionService` | drop pages under the keys | `option:permalink.post` / `option:permalink.page` rebuild the permalink rules and route table (a `permalink.*` write also emits `invalidate [all]`, M1c-2 R10) |
| `theme.changed` | theme name | `ExtensionService` when the active `(name, artifact)` changes | flush | reload the theme from its artifact |
| `plugins.changed` | plugin name | `ExtensionService` on every plugin install / enable / disable | flush | reload the plugin registry |
| `plugin.disabled` | plugin name, reason | the failure flusher on the auto-disable transition (P20) | flush | disable the plugin in this host at once, then reload |
| `types.changed` | `type:<name>` or `taxonomy:<name>` | `ContentTypeService` and `TaxonomyService` on every type or taxonomy create / update / delete (M2a P18), together with `invalidate [all]` in the same transaction | flush | rebuild the permalink rules (re-read from the options, checked against the types and taxonomies) and the route table |
| `node.drain` | node id | nothing yet | none | none (P26) |
| any other kind | any | a newer binary (P28) | drop pages under the keys (logged) | — |

Events are versioned by kind, not by payload (P28): `events` rows have no payload column,
a new event shape gets a new kind, and an older node treats an unknown kind as "invalidate
its keys" (for `types.changed` the accompanying `invalidate [all]` still flushes it, but
it keeps its routes until restart). Lost history (`on_history_lost`) reloads all three components. Events older than
24 h are pruned by the hourly `events.prune` job, except the newest row, which is always
kept (R9) so `min_seq` can reveal pruned history.

## 5. Sessions

**As built (M2a-1, P7):** the `sessions` table ([02 §4](02-storage.md#users--auth)),
`SessionRepo` on both backends and `AuthService` in `ferrite-core` (07 §2). The session
token is 32 random bytes; the cookie holds it as base64url and the database stores its
SHA-256 hex (`id_hash`). `last_seen_at` is written at most once per 5 minutes per session
(`SESSION_TOUCH_EVERY`), so a busy session does not write on every request; idle 7 days,
absolute 30 days (`SESSION_IDLE`, `SESSION_ABSOLUTE` in `ferrite_domain::auth`). There is
**no per-node session cache**: every authenticated request (and every public request whose
cookie has the shape of a session token) reads its session row, so logout, a password change, a role change,
disable and delete, which delete the user's session rows, take effect on the next request
on every node (tested across two nodes on one Postgres schema). Rate-limit counters for
logins and tokens live in the database for the same reason (07 §2). Expired and idle rows
are deleted by `sessions.prune` (§6); an expired row met on a request is deleted then.

## 6. Job queue

**As built (M1.5):** `JobRepo` in both store crates (02 §4 has the table and indexes),
`JobRunner` and `Scheduler` in `ferrite-cluster/src/jobs/`, handlers in
`ferrite-cluster/src/builtin.rs` and `crates/ferrite/src/jobs.rs`.

| Kind | Trigger | Status |
|---|---|---|
| `content.publish_scheduled` | `ContentService` schedule: enqueued in the same `Tx`, `unique_key = "publish:<id>:<scheduled_at unix micros>"`, `run_at = scheduled_at`; publishes only if the item is still scheduled for that time (P12) | built |
| `content.publish_due` | periodic, every 30 s: publishes every item whose `scheduled_at` has passed (safety net for items scheduled before M1.5 or by an older binary; it replaced the M1c-2 per-node ticker) | built |
| `events.prune` (24 h), `jobs.prune` (done 7 d, dead 30 d), `nodes.prune` (24 h after the last heartbeat), `plugin_failures.prune` (`max(24 h, 2 × plugins.failure_window)`) | periodic, hourly; delete in 10 000-row batches for at most 240 s per run (R5) | built |
| `sessions.prune` | periodic, hourly (M2a P23): one reading of the database clock, then sessions past `expires_at` or idle longer than 7 d, API tokens past `expires_at`, rate-limit windows that started more than 1 h ago, and `Idempotency-Key` rows (finished or pending) claimed more than 24 h ago (M2a-2b P10); 10 000-row batches, each table within its own 60 s slice of the 240 s budget so a backlog in one does not starve the others | built (M2a-1; idempotency keys M2a-2b) |
| `plugin.action` | action hooks `post.saved` (content create/update/revision restore), `post.published` (every transition to published, the scheduler's included), `user.created` (API and CLI): core enqueues one job per enabled subscribed plugin row (`PluginRepo::action_subscribers`: name + `enabled` + the manifest's `hooks` keys, filtered in SQL) in the write's own `Tx` (no unique key, priority -1 so core jobs are claimed first, default attempts; payload `{"v":1,"plugin","hook","subject","payload"}` with ids only). The handler drops the action (done) when the row is missing or disabled (P29) or the plugin no longer handles the hook (F35), retries when the plugin is not loaded on the claiming node yet, is loaded there without a hook the stored row lists (the reload has not caught up), or is disabled on that host only, and retries a failed call (counted towards auto-disable). Each call holds one registry snapshot (R13). At most `min(cluster.workers − 1, plugins.max_concurrent_calls)` (at least 1) action jobs call plugins at once per node; one over that, or a call that gets no call permit in time, is released as `JobError::Busy` with the attempt refunded and due again in 1–5 s, never counted as a plugin failure (P22, R20(3); 05 §6). Delivery is at least once, so plugin actions must be idempotent (05 §4) | built (M2a-2a P20); batched `request.served` uses the same kind, one job per subscriber per flush (05 §4a, P21) |
| `plugin.purge` | a plugin delete (API or CLI) enqueues it in the delete's `Tx`: `unique_key = "plugin.purge:<name>:<deleted_at unix micros>"`, due 30 days later, or 2 minutes later with `purge` (the immediate purge's final sweep); payload `{"v":1,"name","deleted_at","settings"}`. Deletes the plugin's KV and `plugin.<name>.settings.*` options and the deletion marker under the `extensions` lock, only if no row of that name exists and the marker still names that deletion (a reinstall keeps the data); a busy lock or store error is retried (05 §2, P9, F13–F16) | built (M2a-2a) |
| `plugin.purge_sweep` | periodic, daily: enqueues a due `plugin.purge` for every deletion marker past the 30-day retention whose plugin is not installed (one per sweep run, keyed `…:sweep:<run micros>`), so a purge job that went dead is retried and settings keys a later version dropped are deleted too | built (M2a-2a) |
| `media.variants` | image upload | M2a-2b |
| `import.wordpress` | admin-triggered import; must override the handler timeout | M3 |
| `artifacts.gc` | periodic | M4 |

`revisions.prune` is not a job: core prunes revisions on save (P26).

### Claim (Postgres, as built)

Two autocommit statements per claim call. The reaper (P7) first moves expired leases that
used their last attempt to `dead`:

```sql
UPDATE jobs SET status = 'dead', last_error = 'lease expired after attempt …', finished_at = now(), …
WHERE status = 'running' AND locked_until < now() AND attempts >= max_attempts AND kind = ANY($kinds);
```

then the claim, a `MATERIALIZED` CTE so the locking subquery runs exactly once (P5):

```sql
WITH next AS MATERIALIZED (
  SELECT id FROM jobs
  WHERE kind = ANY($kinds)
    AND ((status = 'queued' AND run_at <= now())
      OR (status = 'running' AND locked_until < now() AND attempts < max_attempts))
  ORDER BY priority DESC, run_at, id
  LIMIT $n
  FOR UPDATE SKIP LOCKED)
UPDATE jobs SET status = 'running', locked_by = $node,
  locked_until = now() + make_interval(secs => $lease), attempts = attempts + 1
FROM next WHERE jobs.id = next.id
RETURNING …;
```

Only kinds the node has handlers for are claimed (P5), so an old node never touches a new
kind; a kind no running node knows stays `queued` (visible, never burned).

### Claim (SQLite, as built)

A reader-pool `EXISTS` pre-check returns at once when nothing is due (so an idle node
never takes the writer); otherwise the reaper and the same claim without
`FOR UPDATE SKIP LOCKED` run in one transaction on the single writer connection.

### Semantics (as built)

- At-least-once execution; handlers must be idempotent (publish: "set published if still
  scheduled for that time").
- **Lease token (P6):** `(job id, node id, attempts)`. `extend`, `complete`, `fail` and
  `release` act only under exactly that token, so a node whose lease lapsed, even one that
  reclaimed its own job since, cannot overwrite the current attempt.
- **Leases:** 60 s, every in-flight lease renewed every 20 s by the runner (handlers need
  no lease code, P9). A renewal that reports the lease lost cancels the handler. **Local
  fencing (R4):** a job with no successful renewal for `lease − extend_every / 2` (50 s)
  is cancelled whatever the store says, and every store call the runner makes is bounded
  client-side at `extend_every / 2` (10 s), so a partitioned node drops its handler before
  its lease ends. Cancelled jobs write nothing; their lease lapses.
- **Handler timeout:** 300 s by default (`JobHandler::timeout`); a timeout or a panic is a
  retryable failure, `JobError::Permanent` goes straight to `dead`.
- **Retry:** `run_at = now() + min(2^attempts × 10 s, 3600 s)` with `last_error` (at most
  1024 bytes); after `max_attempts` (default 5) the job is `dead`. There is no `failed`
  status (P8). An expired lease at `max_attempts` goes `dead` (P7), so a job that kills its
  node every time stops after 5 crashes.
- **Payload versions (P5, 04 §9):** a payload without `v` is version 1; a `v` the handler
  does not support is released after the backoff with the attempt refunded; a `v` that is
  not an unsigned integer is `dead`.
- **Wake-up:** idle runners poll every 1 s; a freed worker slot claims at once. On Postgres
  `jobs.enqueue` always notifies `ferrite_jobs[_<schema>]` (payload: the kind, every mode,
  P1/P2); only in cluster mode does the runner listen, and then a notice wakes the claim
  loop (resubscribe backoff 1 s doubling to 30 s).
- **Concurrency:** `cluster.workers` handlers per node, default `max(2, available
  parallelism)`, explicit 1..=256 (P16).
- **Shutdown (P10):** the first signal stops claiming; in-flight jobs get 9 s (alongside the
  HTTP drain), then their tasks are aborted and, after up to 0.5 s, released with the
  attempt refunded so another node takes them at once instead of after the lease; the
  final writes get 2 s. `serve` abandons the runner after 12 s (R6).

### Periodic scheduler (as built, P11)

Each node ticks every 10 s (first tick at once), reads `db_now`, and for each periodic job
inserts `unique_key = "<kind>:<floor(db_now / period) × period, unix seconds>"` with
`run_at` = the period start; `ON CONFLICT DO NOTHING` admits one row per period
cluster-wide, so there is no leader election. Periodic rows accumulate until `jobs.prune`.
A node whose ticks all miss a period (stalled) does not create that period's row.

## 7. Distributed locks

**As built (P13):** `Store::try_lock(name)` and `ferrite_store::lock(store, name, wait)`
(retries every 250 ms until `wait`, then `Busy`). Names are `[a-z0-9._-]{1,64}`.

| Name | Held by | Wait |
|---|---|---|
| `migrate` | `startup_check` (`serve`, `seed`, `init`) and `ferrite migrate up/down` (not `status`/`verify`), on both backends | 5 min, logged every 10 s |
| `extensions` | `ExtensionService`: theme/plugin install, activation, enable/disable (artifact row + theme/plugin row + event in one `Tx`) | 30 s |
| `types` | `ContentTypeService`, the taxonomy writes of `TaxonomyService` (M2a P18) and `OptionService` writes that change a `permalink.*` key (R19): the cross-table name and permalink checks, the insert/update/delete and the events in one `Tx`; a lost lock leaves only the type/taxonomy same-name check unprotected (each table's primary key still holds) | 30 s |
| `cluster.join` | the final check-and-register pass of `Registry::join` (R7) | 10 s |
| `import` | reserved for the WordPress import | M3 |

Postgres: a session-level `PgAdvisoryLock` keyed by `"ferrite:<schema>:<name>"` (advisory
locks are database-wide, so the key includes the schema) on a dedicated connection per
held lock (`application_name` `<app>:lock:<name>`, TCP keepalives on), so a crashed holder
releases it when its connection dies. Each attempt opens a new connection (a held
connection with `lock_timeout` is M4, R3). sqlx's own migration lock is taken as well.
SQLite: an in-process mutex per store (one process owns the data folder).

**A lock is not a fence.** The holder is not told when a Postgres advisory lock is lost:
if its connection is terminated (`pg_terminate_backend`, a failover, a proxy or pooler
that closes idle connections even with `idle_session_timeout = 0`), the server frees the
lock and a second node can take it while the first still runs and believes it holds it.
The locks serialise cooperating processes; they do not guarantee exclusion. Today that is
enough: migrations are also protected by sqlx's own advisory lock and a `Tx` per
migration, and `ExtensionService` writes are one short `Tx`. Long-held work (the M3
`import` lock) must carry its own fencing in the data (for example a token checked on
every write, as the job leases do, P6) and must not rely on the lock for exclusion.

## 8. Plugins & themes across nodes (D11)

**As built (M1.5).**

- **Install** writes the artifact first (`MediaStore::put_artifact`, content-addressed),
  then `ExtensionService` records the `artifacts` row, the `themes`/`plugins` row and the
  `theme.changed`/`plugins.changed` event in one transaction under the `extensions` lock
  (P21). Since M2a-2a every path goes through `ferrite_http::extensions::Installer` (05 §2,
  06 §1), which loads the theme or prepares the plugin before uploading, so a broken one is
  refused before anything is written: the admin API (`ExtensionAdmin`, which checks
  `manage_themes`/`manage_plugins` with the `extensions` scope and audits, closing R11) and
  the CLI (`ferrite theme|plugin`, `ExtensionService`, the operator path). Orphan artifacts
  are not collected yet (`artifacts.gc`, M4).
- **Themes** are deterministic ustar bundles (P17, [06 §1](06-themes-rendering.md#1-theme-package)).
  In cluster mode a node reads only the artifact named by the active `themes` row
  (`data/themes/` is ignored, C5); solo keeps the folder as a development fallback.
- **On an event** each node's reloader task fetches the artifact from the MediaStore,
  checks its SHA-256 against the row, loads it (theme: unpack and compile templates; plugin:
  compile and link under `spawn_blocking`) and swaps it in atomically; in-flight requests
  finish on the old version. Reuse is in memory by artifact hash; there is **no on-disk
  artifact or precompiled-component cache** (P19): a restart re-fetches and recompiles.
- **Failure keeps the previous version (P18):** a theme, plugin version or permalink set
  that fails to load leaves the current one running on that node, is reported in
  `nodes.meta` (`theme.error`, `plugins.<name>.status = "failed"`, `permalinks_error`,
  `plugins_error`) and logged, and is retried after 5 s, doubling to 5 min, until it
  succeeds or a newer event arrives (R14). The startup-only fallback to the `blog` theme is
  never used at runtime; a startup that fell back, or that skipped a plugin row, is
  retried the same way from 5 s after boot (R17), so a media blip at boot heals by itself.
- The page cache is flushed twice per change: by the loop on the event, and by the reloader
  after each swap, because a page rendered during the swap may mix versions (R13).
- Solo mode uses the same path (artifacts in the local media store).

## 9. Rolling deploys

**As built (M1.5):** with expand-only migrations
([02 §8](02-storage.md#8-upgrade-compatibility-expandcontract--d8)):

1. The first node of version N+1 starts, takes the `migrate` lock and applies the expand
   migrations (`database.auto_migrate`); nodes starting at the same time wait on the lock
   (tested: `concurrent_first_start_migrates_once`). A contract migration additionally needs
   `database.allow_contract`, and a schema whose `min-binary` is above a binary's version is
   refused by that binary.
2. Old nodes (N) keep serving: store codecs read columns by name and ignore new ones
   (tested: `jobs_tolerate_unknown_columns`, `rolling_version_coexistence`).
3. Remaining nodes are replaced one by one. A stopped node releases its unfinished jobs
   (P10) and deregisters, so its work moves on at once.
4. **Jobs** carry `{"v": n, …}`; a node claims only kinds it knows and hands back (without
   using an attempt) versions it does not support (P5), so N nodes never fail N+1 jobs.
   **Events** are versioned by kind (P28): an unknown kind invalidates its keys.
5. `nodes` lists every node's `version` and `schema_version` during the deploy.

Runbook: [09 §3](09-operations.md#3-upgrade--rollback-cluster--postgres).

## 10. What's deliberately out of scope

- Cross-node page cache sharing (each node warms its own cache; a CDN in front is the
  recommended way to scale reads further).
- Read replicas for the app. (Possible later: `database.read_url` for the reader pool;
  the outbox and sessions must stay on the primary.)
- Multi-region active/active.
