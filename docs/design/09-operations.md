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

- Default: run behind a reverse proxy (Caddy/nginx/Traefik/LB). Honour
  `X-Forwarded-*` only from `trusted_proxies`.
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

```
ferrite serve --data DIR
ferrite init --data DIR [--db postgres://…] [--media s3://…]   # creates ferrite.toml, admin user
ferrite migrate status|up|down|verify
ferrite backup / restore
ferrite export / import-site / db copy
ferrite media sync / media gc
ferrite import wordpress …
ferrite plugin new|check|install|list|enable|disable|remove
ferrite theme install|activate|list
ferrite user create|reset-password|list
ferrite self-update / rollback [--schema] [--restore-snapshot]
ferrite doctor        # validates config, connectivity, permissions, clock skew, versions
```
