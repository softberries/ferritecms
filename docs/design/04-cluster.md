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

`ferrite-cluster` exposes one API with two implementations chosen at startup:

| Concern | Solo (SQLite or single node) | Cluster (Postgres) |
|---|---|---|
| Event bus | in-process `tokio::broadcast` + outbox rows (for uniformity) | outbox rows + `NOTIFY ferrite_events` + catch-up polling |
| Job queue | `jobs` table, claimed by in-process workers | `jobs` table, `FOR UPDATE SKIP LOCKED` |
| Locks | in-process `Mutex` keyed by name | `pg_try_advisory_lock(hash(name))` via sqlx `PgAdvisoryLock` |
| Node registry | single row, informational | heartbeat rows, used for metrics and admin "Nodes" page |

`cluster.mode = "auto"`: cluster when the DB is Postgres, else solo. Postgres + solo is
allowed (explicit `mode = "solo"`), e.g. a single VPS with a managed DB.

## 3. Startup validation

Refuse to start (with a clear message) on:

- `mode = cluster` with SQLite.
- `mode = cluster` with `media.backend = local` — **unless** `media.local_shared = true`
  (operator asserts the folder is a shared volume, e.g. NFS/EFS).
- Another node registered with a **different `site_id`** in the same DB/schema (wrong
  `search_path`).
- Data folder lock already held (solo + SQLite).

Warn on: node clock skew > 2 s vs. DB `now()` (leases and scheduled publish depend on
time; the DB clock is the source of truth for all lease and schedule comparisons).

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

**As built in M1c-2** (`crates/ferrite-cache/src/invalidation.rs`, both backends, used by
`ferrite serve` in every mode): polling. M1.5 adds wake-ups and the history check (below).

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

**Store side, as built (M1.5 Task 2, P1/P2/P4):**

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

**Loop side, as built (M1.5 Task 6, P3):**

- `InvalidationLoop::spawn(poll_interval, listen, shutdown)`. With `listen` (cluster mode;
  wired by `serve` in Task 10) a subtask owns the events subscription and drains it
  continuously. `Notified` wakes the loop for an immediate poll through a
  `tokio::sync::Notify`; permits coalesce, so a burst of notices costs one extra poll.
  `Reconnected` also sets a "check history" flag. A subscribe or `recv` error drops the
  subscription and resubscribes after 1 s, doubling to 30 s; once that succeeds the flag is
  set too. SQLite (`subscribe` → `None`) ends the subtask and the loop only polls. The
  ticker keeps polling every `cache.poll_interval_ms` regardless: notify is an
  optimisation only. The subtask ends on shutdown and drops the subscription before the
  loop's task completes.
- **History check** runs before the poll when the flag is set, and on the first poll after
  one or more failed polls: `min = events.min_seq()`; if `min > last_seq + 1`, events were
  pruned past the cursor, so the loop warns, **flushes the whole page cache**, calls
  `EventHook::on_history_lost` (the node reloads its state), sets `last_seq = min - 1` and
  drops open gaps, the applied-past-gap set and late seqs below `min`. Otherwise it only
  catches up from `last_seq`. If the outbox was pruned to empty the check cannot detect
  the loss (`min_seq` is 0); that needs a node to have missed 24 h of polls with no event
  since.
- `EventHook { on_event, on_history_lost }` (synchronous, must not block) is attached with
  `InvalidationLoop::with_hook`. It sees every event applied after `new()` returned, late
  arrivals included, after the event's cache effect; events applied inside `new()` (the
  boot window) are not passed on, because the caller reads node state after creating the
  loop (P22). `node.drain` stays a no-op (P26).
- **Late-probe bound.** The probe pages through committed rows from the smallest late seq
  at 1000 rows per batch, resuming where the previous tick stopped, so a late arrival is
  found within ⌈span / (POLL_BATCH × probe budget)⌉ ticks (span = largest − smallest late
  seq; probe budget = the batches a tick has left after the main read, 1 to 10). The bound
  counts from the probe's position; an arrival it already passed is found after it wraps,
  at most one tick more. The algorithm is unchanged from M1c-2.
- On Postgres, `idle_in_transaction_session_timeout` (60 s, set since Task 2) is shorter
  than the 10-minute late horizon, so no idle writer holds a seq uncommitted past it (a
  `Tx` busy with long statements is not bounded, P4).

**Planned (M1.5):** contract tests with concurrent writers across nodes.

### Event kinds

| Kind | Keys | Effect |
|---|---|---|
| `invalidate` | dependency keys | page cache drops matching entries |
| `options.changed` | option keys | reload autoloaded options |
| `theme.changed` | theme name | reload theme from artifact, flush cache |
| `plugins.changed` | plugin names | reload plugin registry (install/enable/disable/update) |
| `plugin.disabled` | plugin name, reason | stop calling it immediately |
| `node.drain` | node id | admin-requested drain (optional) |

Events older than 24 h are pruned by a periodic job (planned, M1.5; nothing prunes the
outbox yet).

As built in M1c-2, the invalidation loop only touches the page cache: `invalidate` and
`options.changed` drop the pages under their keys (`all` flushes); `theme.changed`,
`plugins.changed` and `plugin.disabled` flush everything; `node.drain` does nothing; an
unknown kind is logged and its keys are dropped. Nothing reloads the theme, the plugin
registry or the permalink rules on an event yet (they are read at startup; hot swap is
M1.5), and no core write path emits `theme.changed` or `plugins.changed` yet.

## 5. Sessions

DB-backed (`sessions` table, [02](02-storage.md)). Session token = 32 random bytes,
cookie holds the token, DB stores SHA-256. `last_seen_at` is updated at most once per
5 minutes per session to avoid a write per request. Logout/role change/password change
delete sessions → effective on all nodes immediately (no session cache), or with a
≤ 30 s per-node cache if profiling demands it (then invalidated via event).

## 6. Job queue

One table, `jobs` ([02](02-storage.md#cluster)). Used for:

| Kind | Trigger | Notes |
|---|---|---|
| `content.publish_scheduled` | `scheduled_at` set | `unique_key = "publish:<id>"`, `run_at = scheduled_at` |
| `plugin.action` | action hooks (`post.saved`, …) | one job per (plugin, event); off request path |
| `media.variants` | image upload | |
| `import.wordpress` | admin-triggered import | long-running, heartbeats its lease |
| `artifacts.gc`, `events.prune`, `revisions.prune`, `sessions.prune` | periodic | scheduler inserts with `unique_key = "<kind>:<period>"` so only one per period exists cluster-wide |

### Claim (Postgres)

```sql
UPDATE jobs SET status='running', locked_by=$node, locked_until=now()+$lease,
                attempts=attempts+1
WHERE id IN (
  SELECT id FROM jobs
  WHERE status='queued' AND run_at <= now()
     OR (status='running' AND locked_until < now())        -- expired lease: crashed node
  ORDER BY priority DESC, run_at
  LIMIT $n
  FOR UPDATE SKIP LOCKED)
RETURNING *;
```

### Claim (SQLite)

Same statement without `FOR UPDATE SKIP LOCKED` — a single writer connection makes it
atomic; there is only one node.

### Semantics

- At-least-once execution. Handlers must be idempotent (publish: "set published if still
  scheduled for that time"; variants: content-addressed, naturally idempotent).
- Lease default 60 s; long jobs extend (`locked_until`) every 20 s.
- Retry with exponential backoff (`run_at = now() + 2^attempts * 10s`, capped at 1 h);
  after `max_attempts` (default 5) → `dead`, visible in admin with error.
- Wake-up: workers poll every 1 s when idle; cluster mode also `NOTIFY ferrite_jobs` on
  insert for low latency.
- Worker concurrency per node configurable (`cluster.workers`, default = CPU count,
  min 2).

### Periodic scheduler

Each node runs a lightweight ticker; for each periodic task it attempts
`INSERT … ON CONFLICT (unique_key) DO NOTHING` with `unique_key = "<kind>:<period-start>"`.
Whichever node inserts first wins; no leader election needed.

## 7. Distributed locks

`cluster.lock(name)` for rare, coarse operations:

- theme/plugin install & activation (validate + write artifact + rows),
- WordPress import (one at a time per site),
- `ferrite migrate` (sqlx already locks; our wrapper also takes ours so `serve` nodes
  don't start mid-migration).

Postgres: session-level advisory lock on a dedicated connection (released on drop /
disconnect — so a crashed holder releases automatically). Solo: in-process mutex.

## 8. Plugins & themes across nodes (D11)

- Installing uploads the `.wasm` / theme bundle as an **artifact** (content-addressed)
  and writes `plugins`/`themes` rows + a `plugins.changed`/`theme.changed` event, all in
  one transaction (artifact upload happens before; orphan artifacts are GC'd).
- Every node keeps a local cache directory (`data/cache/artifacts/<hash>`, or a temp dir
  when no data folder) and loads components from there; on event it fetches missing
  hashes from the MediaStore, compiles (wasmtime `Component::new`; precompiled cache
  keyed by hash + wasmtime version + CPU features), then atomically swaps its registry.
- Nodes that fail to load a plugin version report it in the `nodes.meta` and log; they
  keep the previous version until resolved. Admin shows per-node plugin state.
- Solo mode uses exactly the same path (artifacts in `data/media/artifacts/`), so there
  is one code path.

## 9. Rolling deploys

With expand-only migrations ([02 §8](02-storage.md#8-upgrade-compatibility-expandcontract--d8)):

1. First node of version N+1 starts → takes migration lock → applies expand migrations.
2. Old nodes (N) keep serving: they tolerate new columns/tables.
3. Remaining nodes replaced one by one.
4. Event and job payloads are versioned (`{"v":1,…}`); a node ignores (re-queues with
   backoff) job kinds/versions it doesn't know, so N nodes don't fail N+1 jobs.

## 10. What's deliberately out of scope

- Cross-node page cache sharing (each node warms its own cache; a CDN in front is the
  recommended way to scale reads further).
- Read replicas for the app. (Possible later: `database.read_url` for the reader pool;
  the outbox and sessions must stay on the primary.)
- Multi-region active/active.
