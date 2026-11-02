# Upgrading and rolling back a cluster

A cluster is several `ferrite serve` nodes on one Postgres database (and schema) with S3
media, usually as containers (`examples/cluster/`). Nodes are replaced by deploying a new
image; `ferrite self-update` and `ferrite rollback` are for solo sites
([solo-upgrade.md](solo-upgrade.md)) and do not apply in a container (the binary is not
writable there). The design is in [09 §3](../design/09-operations.md#3-upgrade--rollback-cluster--postgres)
and [04 §9](../design/04-cluster.md#9-rolling-deploys).

Images: `ghcr.io/softberries/ferritecms:<X.Y.Z>` (also `:<X.Y>` and `:latest` for the
newest stable release; 09 §7). Nothing is published yet: the first tag is the release gate
in `docs/superpowers/m4-2-exit.md`. Pin a version tag (or a digest) in your deployment,
never `latest`, so a rollback can name the previous one.

In the commands below, `ferrite <args>` means running the given image once with **the
same configuration and environment as a node** (the same `ferrite.toml`, the
`FERRITE_DATABASE__URL`, `FERRITE_AUTH__SECRET_KEY` and S3 settings, on a network that
reaches the database), for example with the compose example:

```sh
docker compose -f examples/cluster/docker-compose.yml run --rm --no-deps node1 <args>
```

after setting that service's image to the version you mean. Postgres has no folder lock,
so these commands run next to live nodes.

## 1. Before the deploy

1. **Write down the current state:** the running image tag, and

   ```sh
   ferrite migrate status --data /data      # with the CURRENT image
   ```

   Note `database version: N`. That N is where a schema rollback goes back to.
2. **Take a database snapshot**, or note the current time (UTC) if you rely on your
   provider's point-in-time recovery (PITR). Postgres takes no snapshot of its own (no
   `pre-*.db` as on SQLite).
3. **Check the new version against the database:**

   ```sh
   ferrite migrate status --data /data      # with the NEW image
   ```

   - exit 2 "needs migrate up": the pending migrations are listed with their directives
     (`phase=expand|contract`, `min-binary=…`, `lossy-down=yes|no`);
   - exit 0: nothing to apply;
   - exit 3: this image cannot run on this schema (stop: wrong image);
   - exit 1: an error (config, connection).

   A pending **contract** migration (`phase=contract`) is applied only with
   `database.allow_contract = true`, and its `min-binary` will make every older image
   refuse to start: plan the rollback as in §4 b before going on.
4. Release notes: new config keys. An older image refuses keys it does not know, so add
   new keys only once no older node needs to start again (or keep them out of the shared
   `ferrite.toml` until the deploy is final).
5. A migration that rewrites a large table locks it while it runs (09 §3 names
   `0006_media_comments`'s `jobs` indexes). With a large `jobs` table, apply it at a quiet
   moment first: `ferrite migrate up --data /data` with the new image.

## 2. Rolling deploy

1. **Start one node on the new image.** With `database.auto_migrate` (default on) it takes
   the `migrate` advisory lock and applies the pending migrations; nodes starting at the
   same moment wait for the lock and then find nothing to do. Wait until its `/readyz`
   answers 200: with the compose example `docker compose -f
   examples/cluster/docker-compose.yml ps` shows the node `(healthy)` (its health check
   runs that probe), or ask it directly inside the running container (the binary is
   `/ferrite`; the distroless image has no `PATH` lookup for it):

   ```sh
   docker compose -f examples/cluster/docker-compose.yml exec node1 /ferrite healthcheck --url http://127.0.0.1:8080/readyz
   ```

   (Not with the one-off `run` above: that is a new container, whose 127.0.0.1 is not
   the node.)
2. **Old nodes keep serving** next to it: they ignore new columns, claim only job kinds
   they know (a new kind such as `node_metrics.prune` waits for a new node) and hand back
   job payload versions they do not support without using an attempt.
3. **Replace the remaining nodes one at a time.** SIGTERM drains a node (requests up to
   10 s, jobs released for another node) and deregisters it; give it at least 15 s before
   SIGKILL (the compose example uses `stop_grace_period: 20s`). Do not change `site.url`
   during the deploy: a node whose `site.url` differs from a live node's refuses to start.
4. **Watch the nodes:** the admin's System screen (or `GET /api/v1/system/nodes`) and the
   `nodes` table show every live node's `version`, `schema_version` and last heartbeat;
   with `server.metrics_listen` set, `ferrite_build_info{version=…}` on each node's
   `/metrics`.

`upgrade.json`: the first node of a new version writes one into **its own** data folder;
on a cluster it is informational only (no snapshot on Postgres, and other nodes write
none). Do not roll a cluster back from it.


Webhooks (M6-6, design 18 §10.4): a node older than the webhooks release emits no
webhook events for the writes it serves and does not claim `webhook.deliver` jobs (it has
no handler). The writes served by old nodes during the deploy record no event, so there is
nothing to redeliver for them later: keep the mixed window short.

## 3. After the deploy

- `ferrite migrate status --data /data` with the new image: exit 0, "up to date".
- The dashboard's Site traffic (requests, 5xx errors, page p95) covers every node; compare
  it with the hours before the deploy.

## 4. Roll back

### a) Expand-only migrations (the normal case)

Redeploy the previous image tag, node by node as in §2. Migrations are expand-only, so the
previous version runs on the newer schema; its `migrate status` reports "newer schema,
compatible" (exit 0). That is the whole rollback.

Optionally, once **every** node runs the previous image, take the schema back to N to drop
what the newer version added. The down migrations ship with the newer image:

```sh
ferrite migrate down --data /data --to N --yes            # with the NEWER image
```

A step whose down loses data refuses unless `--allow-lossy` is given; read the pending
list from §1 first (`lossy-down=yes`). The old nodes do not use what is dropped.

### b) A contract migration ran

The previous image refuses to start (`migrate status` exit 3: the schema requires a newer
binary). Either:

1. **Down migrations:** stop every node of the new version (the site is down meanwhile),
   then

   ```sh
   ferrite migrate down --data /data --to N --yes [--allow-lossy]   # with the NEWER image
   ferrite migrate status --data /data                              # with the PREVIOUS image: exit 0
   ```

   and start the previous image on every node. Or
2. **PITR:** restore the database to the time noted in §1 (everything written since is
   lost), then start the previous image. A restored or cloned database needs care with
   the media GC's install id (09 §4).

### c) What not to use on a cluster

- `ferrite rollback --schema`: it works from one node's `upgrade.json`. On Postgres it
  refuses while any node's heartbeat is fresher than 30 s (it names them), but `migrate
  down` has no such check. Use `migrate down --to N` as above, with every node stopped.
- `ferrite rollback --restore-snapshot`: SQLite only.
- `ferrite self-update`: deploy images instead.

## 5. Checklist

| Step | Command (image) | Expect |
|---|---|---|
| Record N | `migrate status` (current) | `database version: N`, exit 0 |
| Snapshot or PITR time | provider | noted |
| Check the new version | `migrate status` (new) | exit 2 (or 0); no `phase=contract` unless planned |
| First new node | deploy one node | `/readyz` 200, migrations applied once |
| Other nodes | one at a time, ≥ 15 s stop grace | `nodes` shows the new version |
| Verify | `migrate status` (new) | exit 0, up to date |
| Roll back, expand-only | redeploy previous tag | `migrate status` (previous): "newer schema, compatible" |
| Roll back, contract | stop nodes; `migrate down --to N` (new); previous image | `migrate status` (previous): exit 0 |
