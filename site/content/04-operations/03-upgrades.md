+++
description = "Update a solo FerriteCMS site with self-update, check migrations, and roll the binary or the schema back."
+++

# Upgrades and rollback

This page is about a solo site: one `ferrite serve` with its binary on the host. A
cluster is upgraded by deploying a new image node by node instead ([Cluster](06-cluster.md)).

## Before you update

- Have a backup you can restore ([Backups and moves](02-backups.md)).
- See whether a newer release exists; this changes nothing:

  ```sh
  ferrite self-update --check
  ```

  Exit 2 means a newer release exists, 0 that you are up to date, 1 an error.

## Update the binary

```sh
ferrite self-update --data ./mysite
```

1. The newest stable release for this machine is looked up on GitHub (a release
   candidate is installed only when named with `--version X.Y.Z-rc.N`).
2. `SHA256SUMS` is checked against its minisign signature with the key built into the
   running binary, then the download against `SHA256SUMS`. Any mismatch stops here with
   nothing changed.
3. The new binary must report the release's version. With `--data` it also runs its
   `config check` on the site: a setting the new release refuses stops the update (fix
   it first; `--skip-config-check` goes on when only the service's environment has what
   the check lacks). Then its `migrate status`; exit 3 (it cannot run this schema) stops
   the update.
4. After you confirm (`--yes` skips the question and is required without a terminal),
   the running binary is copied to `ferrite.prev` next to it and the new one takes its
   place.

`self-update` restarts nothing: restart `serve` yourself. Restart after every update, so
that `ferrite.prev` is always the binary the site last ran.

## Migrations

At start, `serve` applies pending migrations (`database.auto_migrate = true`, the
default). On SQLite it first saves the database as
`<data>/backups/pre-<from>-to-<to>-<time>.db`, keeping the newest five such snapshots
(and the one `upgrade.json` names). When the binary or the schema changed, it writes
`<data>/upgrade.json`: the versions before and after (`from`, `to`), the schema versions
(`schema_from`, `schema_to`), the snapshot and the time. A new site has none: there is
nothing to roll back to.

To look before starting:

```sh
ferrite migrate status --data ./mysite
```

| Exit | Meaning |
|---|---|
| 0 | up to date, or the schema is newer but this binary can run it |
| 2 | migrations are pending (`ferrite migrate up --data ./mysite` applies them) |
| 3 | this binary cannot run this schema |
| 1 | an error |

Migrations add things and do not remove them, unless one is marked as a contract
migration, so the previous binary normally runs on the newer schema.

Webhooks during a rolling upgrade of a cluster, the first time onto a release with them: nodes still on the older binary
send no webhook events for the writes they handle (their code has none) and leave the
delivery jobs to the upgraded nodes. Finish the rollout promptly; the writes made through
an old node in the meantime are not delivered.

## Roll back

### The binary only

The normal case:

```sh
ferrite rollback --data ./mysite
```

It swaps `ferrite` and `ferrite.prev` (running it again goes forward again). With
`--data` it first runs the previous binary's `migrate status` and refuses with exit 3
when that binary cannot open the schema; then roll the schema back as well. An older
binary refuses configuration keys it does not know, so remove keys that the newer
version added before starting it.

### The schema too

Run as the newer binary (the `to` of `upgrade.json`; after a binary rollback that is
`ferrite.prev`), with `serve` stopped:

```sh
ferrite rollback --schema --data ./mysite
```

It runs the down migrations to `schema_from`. A step that would lose data needs
`--allow-lossy`. On Postgres it refuses while a node is still running (a heartbeat in the
last 30 seconds), and names it.

On SQLite you can instead put the pre-upgrade snapshot back:

```sh
ferrite rollback --schema --restore-snapshot --data ./mysite
```

**Everything written since the upgrade is lost.** It asks first (`--yes` without a
terminal), and moves the current database to `<data>/backups/rolled-back-<time>.db`
rather than deleting it. The snapshot must be at the schema the upgrade started from;
another one is refused. If the run is interrupted before the copy is in place, `serve`
refuses to start and names the `mv` that finishes it. `--restore-snapshot` works on
SQLite only; on Postgres use the provider's point-in-time recovery, then roll the binary
back.

`rollback` exits 0 when done, 3 when the previous binary cannot open the schema, and 1 on
any other error. It goes back one step only; for anything older, restore a backup.

The full procedure, with systemd and file ownership, is the runbook
`docs/operations/solo-upgrade.md` in the repository.
