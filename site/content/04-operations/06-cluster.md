+++
description = "Run several FerriteCMS nodes on one Postgres database and shared media storage."
+++

# Cluster

A cluster is several `ferrite serve` nodes behind a load balancer, sharing one Postgres
database and one media store. The database is the only coordination service: nodes find
each other, share jobs and locks, and invalidate each other's page caches through it.

## What every node needs

- **The same Postgres database and schema** (`database.url`, `database.schema`).
- **Shared media:** an S3-compatible bucket (`media.backend = "s3"`), or a local folder
  that is the same shared volume on every node, with `media.local_shared = true`.
- **The same site key** in `auth.secret_key` (or `auth.secret_key_file`, or
  `FERRITE_AUTH__SECRET_KEY`): 64 hex characters, for example from
  `openssl rand -hex 32`. Cluster mode refuses to start without one.
- **The same `site.url`.** A node whose `site.url` differs from a live node's refuses to
  start, which catches a node pointed at the wrong schema.
- `cluster.mode = "cluster"`, or `"auto"`, which becomes cluster with Postgres and shared
  media.
- Its own `cluster.node_name` (default `$HOSTNAME-<pid>`) and, on one host, its own
  `server.listen` port.

## Set up a cluster

Create the database schema and the first configuration with `init`. With Postgres and S3
media, `auto` resolves to cluster mode, so the key is needed up front:

```sh
export FERRITE_AUTH__SECRET_KEY=$(openssl rand -hex 32)
ferrite init --data ./node-a --url https://example.com \
  --db postgres://ferrite@db.example.com/ferrite --schema mysite \
  --media s3://my-bucket/site
```

`init` reminds you to set the S3 region, endpoint and credentials in `ferrite.toml`
([Configuration](01-configuration.md)). Every other node uses the same file and the same
key with its own `node_name`. On one machine, for a test, two folders with a shared
media folder look like this (the second differs only in `listen` and `node_name`):

```toml
[site]
url = "http://localhost:8097"

[server]
listen = "127.0.0.1:8097"

[database]
url = "postgres://ferrite@localhost:5432/ferrite"
schema = "mysite"

[media]
root = "../shared-media"
local_shared = true

[cluster]
mode = "cluster"
node_name = "node-a"
```

Start each node with `serve` and the same key in the environment. With
`server.metrics_listen` set, `ferrite_nodes` on `/metrics` counts the live nodes, and
`ferrite doctor` checks that every live node runs the same version and `site.url` and
that their clocks agree with the database's within 2 seconds.

The repository has a Docker Compose example in `examples/cluster/`: two nodes, Postgres,
an S3-compatible store (RustFS) and Caddy in front.

```sh
docker compose -f examples/cluster/docker-compose.yml up --build
```

## How the nodes work together

- **Page caches:** every change writes an event in the same transaction; each node reads
  the events (Postgres also notifies the nodes at once) and drops the cached pages they
  affect.
- **Jobs** (scheduled publishing, image sizes, imports, backups, plugin actions) are
  claimed by one node at a time.
- **Themes and plugins** are stored in the media store and loaded by every node when one
  is installed or changed. In cluster mode the `themes/` folder of a data folder is not
  read.
- **Sessions and rate limits** are in the database, so a sign-in works on every node and
  a revoked session ends everywhere.
- A plugin that fails too often is disabled for the whole site.

## Commands on a cluster

Postgres has no data-folder lock, so `ferrite user`, `theme`, `plugin`, `import`,
`migrate status` and `export` run next to live nodes, with the same configuration and
environment as a node.

Some things are for solo sites only:

- built-in HTTPS (`[server.tls] mode = "acme"`) is refused: terminate TLS at the load
  balancer, and list it in `server.trusted_proxies`;
- `ferrite backup` refuses Postgres: use the provider's backups and `ferrite export`;
- `self-update` and `rollback --restore-snapshot` do not apply: deploy a new image node
  by node, and roll back by deploying the previous one. The runbook
  `docs/operations/cluster-runbook.md` in the repository has the steps.
