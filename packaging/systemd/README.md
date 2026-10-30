# FerriteCMS under systemd

`ferrite.service` runs one solo site (SQLite and local media) as the `ferrite` user, with
the data folder `/var/lib/ferrite` as the only writable path. The commands below run as
root on a systemd Linux host (Debian, Ubuntu, Fedora and the like).

## Install

1. Install the binary. Download the release asset for the host, check it and put the binary
   in place, owned by root and not writable by the service:

   ```sh
   VERSION=X.Y.Z
   TARGET=x86_64-unknown-linux-gnu   # or aarch64-unknown-linux-gnu, *-linux-musl
   curl -LO https://github.com/softberries/ferritecms/releases/download/v$VERSION/ferrite-$VERSION-$TARGET.tar.gz
   curl -LO https://github.com/softberries/ferritecms/releases/download/v$VERSION/SHA256SUMS
   curl -LO https://github.com/softberries/ferritecms/releases/download/v$VERSION/SHA256SUMS.minisig
   minisign -Vm SHA256SUMS -P <the release public key, from crates/ferrite/release.pub>
   sha256sum --check --ignore-missing SHA256SUMS
   tar -xzf ferrite-$VERSION-$TARGET.tar.gz
   install -o root -g root -m 0755 ferrite-$VERSION-$TARGET/ferrite /usr/local/bin/ferrite
   ```

   The `gnu` builds are made on Ubuntu 22.04 and need glibc 2.35 or newer. The `musl`
   builds are static and run on any distribution, but need the system's CA certificates
   for outgoing HTTPS (S3 media, plugin `http`, `self-update`): install the
   `ca-certificates` package (on Alpine: `apk add ca-certificates`). The certificates are
   read from the usual places (`/etc/ssl/certs` and the like); `SSL_CERT_FILE` or
   `SSL_CERT_DIR` point elsewhere.

2. Create the user and the data folder:

   ```sh
   useradd --system --home-dir /var/lib/ferrite --shell /usr/sbin/nologin ferrite
   install -d -o ferrite -g ferrite -m 0750 /var/lib/ferrite
   ```

3. Create the site as that user (`init` writes `ferrite.toml`, the site key `secret.key`
   and the database):

   ```sh
   sudo -u ferrite /usr/local/bin/ferrite init --data /var/lib/ferrite --url https://example.com
   sudo -u ferrite /usr/local/bin/ferrite user create --data /var/lib/ferrite --login me --email me@example.com
   ```

   `server.listen` in `/var/lib/ferrite/ferrite.toml` sets the address (behind a reverse
   proxy, also list it in `server.trusted_proxies`; design 09 §5).

4. Install and start the unit:

   ```sh
   install -o root -g root -m 0644 ferrite.service /etc/systemd/system/ferrite.service
   systemctl daemon-reload
   systemctl enable --now ferrite
   systemctl status ferrite
   journalctl -u ferrite -f
   ```

   Local changes (a different `MemoryMax`, environment overrides such as
   `Environment=FERRITE_LOG=debug`) belong in a drop-in, `systemctl edit ferrite`, so a
   newer copy of the unit can replace the file.

## Folders outside the data folder

`ProtectSystem=strict` leaves `/var/lib/ferrite` as the only path the service can write.
Two settings can point elsewhere:

- `[backup] to`, which must be outside the data folder (a relative path is relative to it,
  so `"../backups"` is `/var/lib/backups`);
- `[media] root`, when the media live in a local folder outside the data folder.

With a local folder there, the scheduled backups (or the uploads) fail with a read-only
file system error until the unit may write it. Either use S3 (`to = "s3://…"`, or
`[media.s3]`), or create the folder for the `ferrite` user and add it in a drop-in:

```sh
install -d -o ferrite -g ferrite -m 0750 /var/backups/ferrite
systemctl edit ferrite
```

```ini
[Service]
ReadWritePaths=/var/backups/ferrite
```

`ReadWritePaths=` in a drop-in adds to the unit's list, so the data folder stays
writable. `systemctl restart ferrite` applies it. A folder that does not exist when the
service starts makes it fail to start, so create the folder first.

## While it runs

With SQLite, `serve` holds the data-folder lock: `seed`, `user`, `migrate`, `backup`,
`import wordpress` and the other commands that open the database need the service
stopped (`systemctl stop ferrite`), and run as the `ferrite` user (`sudo -u ferrite …`) so
the files stay its own. A running site is backed up by `[backup]` in `ferrite.toml` (see
above for a local `to`).

## Update and roll back

The binary is root-owned and `ProtectSystem=strict` makes `/usr/local/bin` read-only to the
service, so a site never replaces its own binary. Update as root, outside the unit, then
restart:

```sh
ferrite self-update --data /var/lib/ferrite   # verifies the signed release, keeps ferrite.prev
systemctl restart ferrite
```

With `--data`, `self-update` also runs the new binary's `migrate status` for the site
first. With SQLite that check cannot open the database while the service runs:
`self-update` prints a warning and goes on, so stop the service first to see it.

`--check` only reports whether a newer release exists. `self-update` installs GitHub's
latest release, which is always the newest stable version (a later backport of an older
line does not become the latest).

To go back to the previous binary (`/usr/local/bin/ferrite.prev`):

```sh
systemctl stop ferrite
ferrite rollback --data /var/lib/ferrite
systemctl start ferrite
```

`ferrite rollback --schema` also takes the schema back (09 §2).

Every root command given `--data` opens the site's database: `self-update --data`,
`rollback --data` and `rollback --schema`. On SQLite it can leave files owned by root in
the data folder (the database's `-wal` / `-shm` files, a snapshot or a moved database
under `backups/`). Before starting the service again, check with
`ls -la /var/lib/ferrite /var/lib/ferrite/backups`, and if anything is owned by root, run
`chown -R ferrite:ferrite /var/lib/ferrite`. Running those commands as
`sudo -u ferrite` avoids this when the binary's folder is writable to that user (it is
not with the root-owned install above).

## TLS

By default run it behind a reverse proxy that terminates TLS. With the built-in ACME (09
§5) the site listens on 443 (and 80) itself: the unit grants `CAP_NET_BIND_SERVICE` for
that, so it does not need root.

## Hardening notes

The unit has no `MemoryDenyWriteExecute`: the plugin runtime compiles WebAssembly to
machine code at run time. `systemd-analyze security ferrite` lists what else is
restricted.
