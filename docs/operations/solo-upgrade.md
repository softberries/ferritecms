# Upgrading and rolling back a solo site

A solo site is one `ferrite serve` with SQLite (or one Postgres database) and local media,
usually under systemd as in `packaging/systemd/README.md`. This runbook assumes that layout:
the binary at `/usr/local/bin/ferrite` owned by root, the data folder `/var/lib/ferrite`
owned by the `ferrite` user, and the commands below run as root. The design is in
[09 §1, §2 and §5](../design/09-operations.md); a cluster follows
[cluster-runbook.md](cluster-runbook.md) instead.

Nothing has been released yet: until the first tag (the release gate in
`docs/superpowers/m4-2-exit.md`) there is no release to update to, and a binary built from
the repository has no release key, so `self-update` refuses with "this build has no release
key".

## 1. Before an update

- **Have a backup you can restore.** On SQLite every start that migrates takes a
  pre-migration snapshot (`/var/lib/ferrite/backups/pre-<from>-to-<to>-<ts>.db`, the newest
  five kept, plus the one `upgrade.json` names), which is what `rollback
  --restore-snapshot` uses. It is no substitute for a backup elsewhere: `[backup]` in
  `ferrite.toml`, or `ferrite backup --data /var/lib/ferrite --to …` with the service
  stopped (09 §4).
- **See what is new:**

  ```sh
  ferrite self-update --check      # exit 2: a newer release exists; 0: up to date; 1: error
  ```

  `--check` needs no release key and changes nothing. A typo in the flags also exits 2
  (clap), so a script should look for the line `a newer release is available`.

## 2. Update

```sh
systemctl stop ferrite                         # optional: lets the check below read the database
ferrite self-update --data /var/lib/ferrite    # asks before it replaces the binary
systemctl start ferrite                        # or `systemctl restart ferrite` if you did not stop it
```

What happens (09 §1):

1. The newest stable release for this binary's target is looked up on GitHub; a
   prerelease is installed only when named exactly with `--version X.Y.Z-rc.N`.
2. `SHA256SUMS` and its minisign signature are checked against the key built into the
   running binary, then the asset's sha256. Any mismatch stops here with nothing changed.
3. The new binary must report the release's version. With `--data` it also runs its
   `migrate status` for the site and shows it: "needs migrate up" is normal; exit 3 (the
   new binary cannot run this schema) stops here. With the service running on SQLite the
   status cannot be read (the folder lock): you get a warning and the update goes on.
4. It asks `Replace … ? [y/N]` (`--yes` skips this; it is required without a terminal),
   copies the running binary to `/usr/local/bin/ferrite.prev`, and renames the new one
   into place. It restarts nothing.

At the next start the new binary takes the snapshot, applies the migrations and writes
`/var/lib/ferrite/upgrade.json`. Check it:

```sh
journalctl -u ferrite -n 50        # "applied migrations", "wrote upgrade.json"
cat /var/lib/ferrite/upgrade.json  # from, to, schema_from, schema_to, snapshot, at
ferrite --version
ferrite healthcheck                # /readyz on 127.0.0.1:8080
```

**Restart after every update.** `ferrite.prev` is the binary installed before the last
swap. Two updates without a restart in between leave a `ferrite.prev` the site never ran,
and `rollback --schema` then refuses (the record names the binary that did run). Rollback
always goes back one step.

**File ownership.** `self-update --data` and every `rollback` with `--data` open the
database as root. On SQLite that can leave root-owned files in the data folder (the
database's `-wal`/`-shm`, a snapshot, a database moved under `backups/`). Before starting
the service again:

```sh
ls -la /var/lib/ferrite /var/lib/ferrite/backups
chown -R ferrite:ferrite /var/lib/ferrite      # only if something is owned by root
```

## 3. Roll back

First decide what has to go back. `upgrade.json` names the move (`from` → `to`, schema
`schema_from` → `schema_to`).

### a) The binary only (the normal case)

Migrations are expand-only, so the previous binary runs on the newer schema:

```sh
systemctl stop ferrite
ferrite rollback --data /var/lib/ferrite
systemctl start ferrite
```

It swaps `ferrite` and `ferrite.prev` (running it again re-upgrades). With `--data` it
first runs the previous binary's `migrate status`: exit 3 means that binary cannot open
this schema (a contract migration ran), and you need (b).

**Config keys of the newer version.** An older binary refuses keys it does not know. A
binary from before M4-2 stops with `server.tls: unknown key` or
`env:FERRITE_SERVER__METRICS_LISTEN: unknown key` (checked in the M4-2 exit). Remove
`[server.tls]`, `server.metrics_listen` and their `FERRITE_SERVER__*` overrides (systemd
drop-ins too) before starting it; `rollback --data` says so when its check fails, and
`rollback` without `--data` prints the same note. The same holds for every key a future
release adds: compare `ferrite.toml` with what the older version documents.

### b) The schema too: down migrations

```sh
systemctl stop ferrite
ferrite rollback --schema --data /var/lib/ferrite     # run as the newer binary
systemctl start ferrite
```

- Run it as the binary `upgrade.json` names as `to`: `/usr/local/bin/ferrite` before a
  binary rollback, `/usr/local/bin/ferrite.prev` after one (the down migrations ship
  with the newer binary). Any other binary is refused.
- The binary that runs afterwards must report the record's `from` (checked before
  anything changes).
- It runs the down migrations to `schema_from`. A step that loses data refuses unless
  `--allow-lossy` is given; the message offers `--restore-snapshot` instead.
- Afterwards `upgrade.json` is renamed to `upgrade-rolled-back-<ts>.json`, the binaries
  are swapped (when run as `ferrite`), and `schema_meta.last_binary_version` names the
  older binary, so a later re-upgrade records a move that can itself be rolled back.
- On SQLite the folder lock refuses while the service runs. **On Postgres nothing
  detects a running site: stop the service first.**

### c) The schema too: the pre-upgrade snapshot (SQLite)

```sh
systemctl stop ferrite
ferrite rollback --schema --restore-snapshot --data /var/lib/ferrite   # asks; --yes without a terminal
systemctl start ferrite
```

- **Everything written since the upgrade (`at` in `upgrade.json`) is lost** from the live
  site. The tool shows `at` and the snapshot's time first and asks you to type `yes`.
- The current database (and its `-wal`/`-shm`) is moved, never deleted, to
  `/var/lib/ferrite/backups/rolled-back-<ts>.db`; the snapshot is copied into place with
  the replaced database's owner, group and mode.
- **Media uploaded since the upgrade** is referenced only by the moved-aside database. The
  daily media GC deletes those files after `media.gc_grace_days` (default 7). If you may
  want that content back, copy the media folder aside first or raise the grace period.
- Postgres has no snapshot: use your provider's point-in-time recovery to the time before
  the upgrade, then roll the binary back with (a).
- It refuses (with nothing changed) when the database is on another filesystem than the
  data folder, on a filesystem without hard links, while `ferrite.db-journal` exists, and
  when the record names no snapshot.

### After any rollback

`chown` as in §2, start the service, and check `journalctl -u ferrite` and `ferrite
healthcheck`. A rollback goes back one step only; for anything older restore a backup
(`ferrite restore`, 09 §4).

## 4. Built-in HTTPS (ACME), first setup

Instead of a reverse proxy, a solo site can get its own certificate from Let's Encrypt
(09 §5). Requirements: a DNS name pointing at the host, port 443 reachable from the
internet (TLS-ALPN-01 runs on the TLS listener itself), solo mode.

1. Point the name's A/AAAA records at the host and open 443 (and 80 for the redirect).
2. In `/var/lib/ferrite/ferrite.toml`, set `site.url` to the `https://` URL, keep
   `server.listen` on loopback, and start with the **staging** directory:

   ```toml
   [site]
   url = "https://example.com"

   [server]
   listen = "127.0.0.1:8080"          # plain HTTP for `ferrite healthcheck`

   [server.tls]
   mode = "acme"
   contact = "mailto:admin@example.com"
   directory = "staging"              # untrusted certificates, much higher rate limits
   redirect_http = "0.0.0.0:80"       # optional: 301 to the https site
   # domains = ["example.com"]        # default: the host of site.url
   # listen = "0.0.0.0:443"
   ```

3. Check and restart:

   ```sh
   sudo -u ferrite ferrite config check --data /var/lib/ferrite   # ... tls=acme ...
   systemctl restart ferrite
   journalctl -u ferrite -f            # the order, then the certificate being deployed
   curl -sk -o /dev/null -w '%{http_code}\n' https://example.com/   # -k: staging is untrusted
   ```

   Until the first certificate is issued, TLS handshakes fail; the plain listener still
   answers. Failures are logged as warnings and retried with growing waits.
4. When staging works, set `directory = "production"` (or remove the line) and restart.
   Production orders afresh (the cache is per directory).

Notes:

- The unit grants `CAP_NET_BIND_SERVICE`, so the `ferrite` user can bind 443 and 80. In a
  container use a port mapping instead (`-p 443:8443` with `listen = "0.0.0.0:8443"`).
- Certificates and the account key live in `/var/lib/ferrite/tls/` (0700, files 0600).
  They are **not** in backups: a restored or moved site orders a new certificate, and
  Let's Encrypt allows 5 for the same names per 7 days. Copy `tls/` along when you move a
  site. A changed `contact` only registers a new account; a changed `domains` orders a
  new certificate.
- Responses over TLS carry `Strict-Transport-Security: max-age=31536000`; the plain
  listener's never do.
- Cluster mode refuses `mode = "acme"`: terminate TLS at the load balancer.
- An older binary refuses `[server.tls]` (§3 a).

## 5. systemd notes

- **Start limit.** A start that keeps failing (a broken upgrade, a bad config) is retried
  every 5 s, at most five times in ten minutes; then the unit stays failed instead of
  taking a new snapshot every few seconds. Fix the cause, then:

  ```sh
  systemctl reset-failed ferrite && systemctl start ferrite
  ```

- **Paths outside the data folder.** `ProtectSystem=strict` leaves `/var/lib/ferrite` as
  the only writable path. A local `[backup] to` or `[media] root` elsewhere needs a
  drop-in (`systemctl edit ferrite`), created after the folder exists:

  ```ini
  [Service]
  ReadWritePaths=/var/backups/ferrite
  ```

  `ReadWritePaths=` in a drop-in adds to the unit's list.
- **Other commands** that open the database (`migrate`, `user`, `backup`, `import
  wordpress`, …) need the service stopped on SQLite and should run as the `ferrite` user
  (`sudo -u ferrite ferrite …`) so the files stay its own.
- **Environment overrides** (`Environment=FERRITE_…`) belong in a drop-in too; remember
  them when rolling back to an older binary (§3 a).
