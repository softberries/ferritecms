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

### `upgrade.json` (as built, M4-2 P30)

```json
{"v": 1, "from": "0.1.0", "to": "0.2.0", "schema_from": 10, "schema_to": 12,
 "snapshot": "backups/pre-10-to-12-20261030T120000Z.db", "at": "2026-10-30T12:00:00.123456Z"}
```

- **Fields:**
  - `from`: `schema_meta.last_binary_version` before this start, `null` when none was
    stored (a database from before M4-2). `to`: this binary.
  - `schema_from`/`schema_to`: the schema version before and after (equal when only the
    binary changed).
  - `snapshot`: the SQLite pre-migration snapshot, relative to the data folder; `null`
    when nothing was migrated, and always on Postgres.
  - `at`: RFC 3339, UTC.
- **Who writes it:** the startup check and `ferrite migrate up`, always under the migrate
  lock, so concurrent cluster starts write it once. The startup check runs in `serve` and
  in every other command that migrates when it starts: `seed`, `user`, the
  `theme`/`plugin` commands, `import wordpress`, `media gc`/`regenerate`.
- **In two steps around a migration:**
  1. Before migrating, a provisional record: `schema_to` is the target, `snapshot` is
     `null`.
  2. After migrating, the final record with the snapshot.

  A start that dies in between (after the migrations committed) leaves the provisional
  record. The next start has nothing left to apply and sees the record of this move
  (same `from` and `to`). It finalises the record with the newest
  `backups/pre-<schema_from>-to-<schema_to>-*.db`, found by name.

  An unfinished earlier move is carried forward: the next move keeps its `schema_from`
  and snapshot. That covers two cases:
  - an attempt at the same move that stopped partway (the schema between its
    `schema_from` and `schema_to`);
  - a move from the stored binary to another one that died after migrating (A→B
    crashed, then C starts: C's record is `from: A` with A→B's start and snapshot).

  Only an unfinished record is carried: one whose `schema_to` lies beyond the current
  schema, or a provisional one (`snapshot: null` with `schema_from < schema_to`). A
  finished A→B that a binary rollback to A kept is not carried. A later A→C records its
  own start and snapshot (`pre-<B's schema>-to-<C's>`), so `--restore-snapshot` loses no
  more than needed. On Postgres a finished record also has `snapshot: null`, so
  A→B→(rollback)A→C there does carry A→B's `schema_from`. There is no snapshot to
  restore, and `migrate down` reaches the older start.

  A `migrate_up` that fails without moving the schema leaves the file as it was: the
  record that was there is written back, or the provisional one is removed when there
  was none.
- **When it changes:** the stored `last_binary_version` differs from this binary, or
  migrations were applied. Then `last_binary_version` is set to this binary. A start that
  changed nothing leaves an existing file alone.
- **A binary rollback keeps it.** When nothing was applied and this binary is **older**
  (semver) than the stored `last_binary_version`, the file is kept and only the key is
  updated. This is `ferrite rollback` swapping back to `ferrite.prev`.
  - A schema rollback afterwards still finds the move.
  - Re-upgrading (B again) also keeps the file: it already names A → B.
  - So A → B → A → B ends with the original A → B record and its snapshot.
- **Not written:**
  - For a database that had no schema before (`ferrite init`, a first `serve` or
    `migrate up` on an empty database): there is nothing to roll back to. The version
    is still stored, so the next start writes nothing either.
  - On a retry whose file already names this move.
- **One step only.** Each new move replaces the file, so rollback goes back one step:
  - an upgrade B → C, even a patch release without migrations (that record has
    `snapshot: null`), replaces the record of A → B;
  - the B snapshot then falls back into the normal "newest five" pruning.
- **`migrate up --to N`, then `serve`.** This writes two records. The second
  (`schema_from: N`) replaces the first, so a schema rollback from the file reaches only
  N. Go further down with `ferrite migrate down --to <M>` explicitly.
- **Atomic:** a temp file in the data folder, synced, renamed over the old file, then the
  folder is synced. Write errors name the path. Temp files a crash left behind
  (`.upgrade.json.*.tmp`) older than a day are removed by the next write.
- **Snapshots:** the pruning of pre-migration snapshots (newest five) never deletes the
  one this file names (02 §7); opening a SQLite site reads the file to know which.
- **An unreadable file** (not JSON, or another `v`, e.g. a newer binary's format after a
  binary rollback):
  - The site still opens, with a warning. Pruning keeps **every** snapshot, since it
    cannot know which one the file names.
  - The next record moves the bad file aside to `upgrade.json.invalid-<UTC timestamp>`
    (never deleted) before writing.
  - Only `ferrite rollback` refuses to work from an unreadable file.
- **Clusters:** each node writes its own data folder's file. The key is shared, so only
  the first node to start on a new version (or a CLI run from another machine) writes
  one. On a cluster the file is informational, and Postgres takes no snapshot. A
  cluster's schema rollback is an explicit `ferrite migrate down --to <N>` (or provider
  PITR), per the runbook in §3.
- **Other commands:** `import-site` and `db copy` migrate their target without writing
  the file. `restore` does not migrate; the restored database (with the
  `last_binary_version` it was backed up with) is recorded by its next start like any
  other.

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
- `data/upgrade.json` (§1) is informational on a cluster: only the first node of a new
  version writes one, into its own folder, and Postgres takes no snapshot. Roll the schema
  back with an explicit `ferrite migrate down --to <N>` (N from `migrate status` before the
  deploy), not from that file.

**Rolling deploy, as built (M1.5;
[04 §9](04-cluster.md#9-rolling-deploys)):**

- Start the first new node. It takes the `migrate` advisory lock and applies the pending
  expand migrations (`database.auto_migrate`, default on); other nodes starting at the
  same moment wait for the lock (up to 5 min, logged every 10 s), then find nothing to do.
  Two nodes started together on an empty schema (what `docker compose up` does) migrate
  exactly once.
- A migration's DDL locks what it touches until it commits; the old nodes' statements
  wait, up to the 30 s `statement_timeout` of application connections. `0006_media_comments` (M2a-2b) is the one to know about: it builds three
  `jobs` indexes without `CONCURRENTLY` (sqlx cannot run that, [02 §7](02-storage.md#7-migrations)),
  so job writes on every node (enqueue, claim, lease extensions, finishing, prune) wait for the build, and
  `media`/`comments` are briefly locked by its `ADD COLUMN`s. `jobs` is pruned, so this
  is usually well under a second; with a large `jobs` table, run `ferrite migrate up`
  at a quiet moment before rolling the nodes.
- Old nodes keep serving next to new ones: codecs ignore unknown columns, old runners
  claim only job kinds they know and hand back unknown payload versions without using an
  attempt, and an unknown event kind invalidates its keys.
- Replace the remaining nodes one at a time. SIGTERM releases the node's running jobs
  (another node claims them at once) and deregisters it; give the process manager at
  least 15 s before SIGKILL (shutdown is bounded at 14 s; the compose example uses
  `stop_grace_period: 20s`).
- `site.url` must not change during a rolling deploy: a node whose `site.url` differs from
  a live node's refuses to start (P15). Change it with all nodes stopped (or 30 s after the
  last one stopped).
- Watch the `nodes` table: `version`, `schema_version`, `heartbeat_at` and `meta` (theme
  and plugin state per node) for every live node.

## 4. Backup & restore

| | Solo (SQLite + local media) | Cluster (Postgres + S3) |
|---|---|---|
| Primary | `ferrite backup --to <dir|file.tar.zst|s3://…>`: `VACUUM INTO` snapshot + `ferrite.toml` + media artifacts (incremental by hash) | Provider automated backups + PITR; S3 bucket versioning/replication |
| Portable | `ferrite export` ([02 §10](02-storage.md#10-portable-export-d14)) | same — independent of the provider |
| Schedule | `[backup] schedule = "daily"\|"weekly"`, `to = "<dir>\|s3://…"`, `keep = 7` → the `site.backup` job inside `serve`: a repository snapshot, `keep` snapshots kept | external (provider), plus the same `[backup]`: an export archive to `<to>/exports/`, `keep` archives kept |
| Restore | `ferrite restore <backup> --data ./site` (into empty folder or with `--force` moving the old one aside) | provider restore / `ferrite import-site` |

Content-level "undo" (point-in-time for a single item) is revisions (02). Site-wide
point-in-time is backup restore (solo) or PITR (cluster).

Backups are verified: the snapshot is opened read-only and `PRAGMA integrity_check` must
return `ok` before anything is written, and a repository's `ferrite.db` is read back after
the upload and its sha256 compared.

**As built (M4-1, `crates/ferrite/src/backup/`).** `ferrite backup` is SQLite only (a
Postgres site exits 1 with a pointer to the provider's backups and `ferrite export`) and,
like every SQLite CLI command, needs `serve` stopped; a running site is backed up by the
`[backup]` schedule, a job inside `serve` (the CLI cannot open the database while `serve`
holds `ferrite.lock`). It refuses (exit 1) a site whose `database.url` is not
`sqlite://ferrite.db` (relative to the data folder): `restore` only restores a database at
`DIR/ferrite.db`, and an absolute URL, even to the site's own `<data>/ferrite.db`, would point
the restored configuration back at the original folder, so such a backup could never be
restored (B-I2). With media, a run holds the `artifacts.gc` lock from before the snapshot
until the blobs are written (R17, [03 §6](03-media.md#6-garbage-collection)), waiting up to
5 minutes for a running GC pass, then failing with exit 1. The sequence:

1. `VACUUM INTO <data>/backups/backup-<UTC yyyymmddThhmmssZ>.db` (mode 0600; never
   `pre-*.db`, which the pre-migration snapshot pruning deletes); files older than a day
   with exactly the names a run writes (`backup-<yyyymmddThhmmssZ>.` + `db`, `db-wal`,
   `db-shm`, `db-journal`, `toml`, `key` or `artifacts.txt`), left by killed runs, are removed
   first, and nothing else in the folder is touched; an archive `--to` inside `backups/` is
   refused;
2. open it read-only, `PRAGMA integrity_check` must say `ok`, read its `artifacts` hashes,
   schema version and site id;
3. write the target (below), copying the blobs from the site's media store (`ferrite.toml`
   and `secret.key` are copied once into the backups folder and hashed as they are copied);
4. delete the local files (also when a step failed).

`--to` decides the shape:

- **a `.tar.zst` file, or `-` (stdout):** one archive in the [02 §10](02-storage.md#10-portable-export-d14)
  format with `kind: "backup"`: `manifest.json`, then `ferrite.toml`, `secret.key` (unless
  `--no-secret-key`), `ferrite.db` and, with media, `artifacts.txt` (the archived blobs, one
  hash per line), each listed in the manifest's `files` with its size and sha256, then
  `artifacts/<ab>/<hash>` (unless `--no-media`);
- **a folder or `s3://bucket[/prefix]`: a repository.** It is marked by
  `<repo>/ferrite-backup-repo.json` (`{"format": 1, "created_at": …}`), written by the first
  backup into an empty or new folder or prefix (a folder holding only a filesystem's
  `lost+found` counts as empty); a non-empty location without the marker is refused (exit 1)
  and nothing in it is touched, and so is a folder inside the data folder or the local media
  root, or an S3 prefix overlapping the media store's. A marker of a newer format than the
  binary is exit 3, as for `restore`.
  `<repo>/artifacts/<ab>/<hash>` is shared by all snapshots and a blob is copied only when
  the repository lacks it (incremental by hash); each run adds
  `<repo>/snapshots/<UTC yyyymmddThhmmssZ>/` with `ferrite.db`, `ferrite.toml`,
  `secret.key`, `artifacts.txt` (the blobs the snapshot needs, one hash per line; its size,
  sha256 and line count are in the manifest, so the list has no size limit) and, written
  last, `manifest.json`. A snapshot without a manifest is incomplete and ignored. Two runs in
  the same second get consecutive names. `--keep N` (1–1000; default keep all) then deletes
  the snapshot folders older than the newest N complete ones (only the file names a snapshot
  holds), then every repository blob no kept snapshot lists that is older than 24 hours, and
  staging objects under `tmp/` (media-store names only) older than 24 hours: one site per
  repository. A younger unlisted blob is kept (R14): it may belong to a snapshot another
  process is still writing into the same repository (its manifest, which lists it, comes
  last); a run marks the local blobs it finds already present as used now for the same
  reason (S3 objects are not touched, so on S3 a reused blob is protected only by being
  listed in a kept snapshot). When pruning
  fails after the snapshot was written, the command exits 1 and the new snapshot stays. S3
  credentials and endpoint come from the `AWS_*` environment, as for `media sync`.

**Scheduled backups (as built, M4-1 P15, `crates/ferrite/src/jobs/backup.rs`).** With
`[backup] schedule = "daily"` or `"weekly"` ([01 §4](01-architecture.md#4-configuration)),
`serve` registers the `site.backup` job and a periodic every 86 400 s or 604 800 s; with
`"off"` (the default) it registers neither. Like every periodic, each period's job has the
unique key `site.backup:<period start>`, so one node runs it per period; one run at a time
holds the store lock `site.backup` (a second is `Busy` and tried again a few seconds
later). Its timeout is 6 hours, and a failure is retried up to 3 attempts in all, then the
job is `dead` (System → Jobs; the next period starts again). Periods are aligned to the Unix
epoch on the database clock: a daily backup is due at 00:00 UTC, a weekly one on Thursday
00:00 UTC (1970-01-01 was a Thursday). The first `serve` in a period inserts that period's
job due at the period's start, which has passed, so it runs right away; a node restarted
later in the same period does not run it again. `to` is a folder or
`s3://bucket[/prefix]` (`AWS_*` credentials, as above), never an archive path; a relative
path is relative to the data folder (made absolute first, so `--data .` works) and must lie
outside it (`"../backups"`). `config check` and `serve` run the same location check (the
repository rules of `ferrite backup --to <folder>`: not inside the data folder or the local
media root, symlinks resolved where the paths exist, no S3 prefix overlapping the media
store's; a local folder must be absent, empty but for `lost+found`, or already hold the
repository marker (B-I3); an S3 prefix's content is checked only when the job runs, since
the check opens no network connection), and `serve` refuses to start when it fails. On a
SQLite site both also refuse a `database.url` other than `sqlite://ferrite.db` (see
`ferrite backup` above). A run holds the `artifacts.gc` lock while it copies blobs (R17);
a GC pass holding it longer than 5 minutes makes the run `Busy` (retried a few seconds
later, the attempt refunded).

- **SQLite:** the repository backup above, in process (the snapshot is safe next to the
  live writers), with `secret.key`, the blobs unless `with_media = false`, and
  `--keep <keep>`. Restore with `ferrite restore <to>`. A restored folder keeps the same
  `[backup]`, and a relative `to` such as `../backups` then names the same repository for
  a sibling copy: `ferrite restore` prints a warning naming the repository when the
  restored schedule is not off. Set `FERRITE_BACKUP__SCHEDULE=off` (or edit `[backup]`)
  before serving a copy that must not write into it; otherwise both sites' snapshots count
  towards `keep` and `ferrite restore <to>` takes the newest of either.
- **Postgres:** a [02 §10](02-storage.md#10-portable-export-d14) export archive (blobs
  unless `with_media = false`; never `secret.key`) written to
  `<data>/backups/export-<UTC yyyymmddThhmmssZ>.tmp` (mode 0600), uploaded to
  `<to>/exports/<UTC yyyymmddThhmmssZ>.ferrite.tar.zst` (a local folder gets it as
  `….part` first, then renamed; S3 sees it only when the upload completes), read back and its
  sha256 compared, then the local file is deleted (on every path; leftovers of killed runs
  older than a day are removed first). The tables are spooled as NDJSON in a
  `<data>/backups/ferrite-export-*` folder while the archive is built (not the system
  temporary folder, which the CLI `ferrite export` uses; removed afterwards, a killed run's
  after a day). An S3 upload cut off by the timeout is aborted in the background; one left
  by a killed process stays an incomplete multipart upload, so give the bucket a lifecycle
  rule that removes those. `<to>` is a repository with its marker like any other; the newest `keep` archives are kept and only names of that exact form are deleted
  (plus `.part` leftovers older than a day). Load one with `ferrite import-site`. In
  cluster mode a local `to` is on whichever node ran the job (`serve` logs a warning):
  use `s3://` or a shared folder.

A scheduled run whose site lacks some blobs (below) succeeds, with a warning in the log that
gives the count and the first hashes; it is not retried.

An artifact whose blob is missing from the media store is left out and named in the
manifest's `missing` list (exit 2); a blob whose content does not hash to its name fails the
backup (exit 1). Nothing is encrypted: archives, repository files and restored files that
hold secrets are written with mode `0600`, and `--to -` can be piped to `age` or `gpg`.

`ferrite restore SOURCE --data DIR` takes a backup archive, `-` (stdin) or a repository (the newest
complete snapshot, or `--snapshot <name>`; a repository holding only a Postgres site's `exports/`
archives is refused with a pointer to `ferrite import-site`). `DIR` must be absent or empty (a
filesystem's `lost+found` does not count, so a fresh volume works); `--force` moves a non-empty
folder aside to `DIR.pre-restore-<ts>` (a sibling, never deleted), refused while another process
holds its `ferrite.lock`. That lock is the only sign of a running site it checks: a folder whose
`ferrite.toml` keeps the database (and so the lock) elsewhere, or a Postgres folder, has none, so
stop `serve` yourself first. The manifest is checked first: an export archive, a non-SQLite backup,
an unexpected file name or a schema newer than the binary (exit 3) are refused before anything is
written. The files go to a staging folder, `.<name>.restoring-<ts>-<pid>` next to an absent or
occupied `DIR` (for an absent one inside `DIR`, then created, when its parent refuses the sibling),
or `DIR/.restoring-<ts>-<pid>` inside an empty `DIR`; the blobs go through the media store the
**restored** `ferrite.toml` names (local or S3; blobs already there are skipped, trusted by their
content hash); the restored configuration must keep the database at `DIR/ferrite.db`. Only then is
the sibling staging folder renamed to the absent `DIR`, or the staged entries moved into the empty
`DIR` with `ferrite.db` last (a folder holding the database is complete; a name that appeared there
meanwhile is refused, never replaced), or the old folder moved aside and the staging folder renamed
to `DIR`. A failed restore leaves `DIR` as it was; blobs it already stored in an absolute local or
an S3 media root stay (content-addressed; the store's owner GC collects them after the grace period
if nothing references them). Staging folders older than a day with exactly those names, left by
killed runs, are removed at the start. A bind mount on the same device as its parent is not detected
up front: `--force` then fails at the final rename and changes nothing. It then prints `migrate
status`; it does not migrate (`serve` does, under `database.auto_migrate`). A backup without
`secret.key` restores with a note on TOTP (below).

**A restored site is a new install (R16, R19).** A backup snapshot carries no
`schema_meta.install_id`; before the restored site is put in place it gets a fresh one and
every `artifacts.orphaned_at` mark is cleared. Its media GC deletes only on a pass that reads
an owner marker (`ferrite-owner.json`) naming its install
([03 §6](03-media.md#6-garbage-collection)), so a copy restored next to the live site onto
the same S3 bucket or absolute media root never deletes from it. The live site, which owns
the store, still deletes what *its* database does not reference after the grace period,
including the copy's own new uploads: a copy that keeps running and uploading needs its own
`[media]` store. `restore` warns when the restored media store is outside `DIR`. For a
restore **in place** after a disaster (the original site is gone and the restored one takes
over its store), run `ferrite media gc --data DIR --claim` once: until then the restored
site's GC marks but deletes nothing there, and logs an error each day. One media store
belongs to one site: never claim a store another running site uses.

**Clones made outside Ferrite keep the install id.** A Postgres PITR restore or `pg_dump`
copy, or a copied `ferrite.db`, has the original's `install_id`, so served next to the
original on the same media store both would delete as its owner. Before serving such a
clone, give it a new install: `DELETE FROM schema_meta WHERE key = 'install_id';` on the
clone (the next migration or GC pass writes a fresh one; for Postgres, in the site's schema).
A backup archive or repository snapshot unpacked by hand needs nothing: its snapshot holds
no install id.

A mount point cannot be renamed, so `--force` refuses one (and any folder on another device
than its parent) before doing any work. In the container image `/data` is a volume: restore
into it while it is empty (`docker run --rm -v site:/data -v "$PWD":/in <image> restore
/in/site.tar.zst --data /data`, the image's entrypoint being `ferrite`, with the serving
container stopped), or empty it first, keeping a copy of what it held.

**The site key is not in the database** (M2a-1, P4). A solo site keeps it in
`<data>/secret.key` (mode `0600`); a cluster gets it from `auth.secret_key[_file]`. Back it
up separately from database dumps and keep it with the same care as the database
credentials: without it, a restored database still has its users, sessions and API tokens,
but stored TOTP secrets cannot be opened (those users need `ferrite user disable-totp` and a
new enrollment). A solo `ferrite backup` includes `secret.key` unless `--no-secret-key`; an
`export` never does.

## 4a. S3 media buckets

Applies to `[media] backend = "s3"` (see [03 §3](03-media.md#s3-backend)). Checked on
2026-10-29; every provider statement below is backed by the table at the end of this
section. What is not in that table as "yes" is not stated as fact here.

### Lifecycle rules

Ferrite stages large uploads under `<prefix>tmp/<uuid>` as S3 multipart uploads and cleans up
after itself on errors and cancellation, best effort ([03 §3](03-media.md#staging-cleanup)).
A crash can still leave an incomplete multipart upload or a `tmp/` object. Add two bucket
lifecycle rules as the backstop:

1. abort incomplete multipart uploads after 1 day (the whole bucket);
2. expire objects under `<prefix>tmp/` after 1 day. With no `[media] prefix`, that is `tmp/`.

An **example, field names not verified** (see the claims table: the JSON field names were not
checked against the AWS API reference; check them there before use) for `aws s3api
put-bucket-lifecycle-configuration --bucket BUCKET --lifecycle-configuration
file://lifecycle.json` (with `prefix = "site1/"` use `site1/tmp/`):

```json
{
  "Rules": [
    {
      "ID": "ferrite-abort-incomplete-multipart",
      "Status": "Enabled",
      "Filter": { "Prefix": "" },
      "AbortIncompleteMultipartUpload": { "DaysAfterInitiation": 1 }
    },
    {
      "ID": "ferrite-expire-tmp",
      "Status": "Enabled",
      "Filter": { "Prefix": "tmp/" },
      "Expiration": { "Days": 1 }
    }
  ]
}
```

On a versioned bucket `Expiration` only adds a delete marker; also add a
`NoncurrentVersionExpiration` action (not tested here) to free the space.

Never put a lifecycle expiry on `artifacts/`: those are the site's media, plugins and themes.
Other providers have their own lifecycle features and syntax; none is described here because
none was checked (see the table).

### `copy_if_not_exists` and `conditional_put` per provider

Ferrite publishes an upload with a conditional put (`If-None-Match: *`) and a server-side
copy. `copy_if_not_exists` tells the client how the store makes that copy fail when the
destination exists; without it the client falls back to a `head` then `copy`, which is not
atomic but is safe here because keys are content hashes
([03 §3](03-media.md#write-paths-and-the-8-mib-threshold)). The value uses `object_store`'s
encoding: `header:<name>:<value>`, `header-with-status:<name>:<value>:<status>` or
`multipart`. `ferrite config check` cannot tell whether a value is right for your store:
after changing it, upload a file of 8 MiB or more and check the logs: smaller uploads are
never placed with a copy, so they do not exercise it.

| Provider | `copy_if_not_exists` | `conditional_put` |
|---|---|---|
| AWS S3 | `multipart` (the mode `object_store` documents for S3; it does not copy tags or attributes) | `etag` (default) |
| Cloudflare R2 | not verified (`object_store`'s docs only): `header:cf-copy-destination-if-none-match:*` is the value they give; Cloudflare's docs do not list the header, so leave it unset unless you have tested it | `etag`: R2 documents `If-None-Match` on PutObject |
| RustFS (cluster example) | leave unset (fallback) | `etag`: tested, a second `If-None-Match: *` put gets `412 PreconditionFailed` |
| Backblaze B2, Hetzner, Scaleway | leave unset (fallback) | not verified: if uploads fail with a precondition error, set `conditional_put = "disabled"` |

AWS also documents `If-None-Match` on CopyObject; `header:If-None-Match:*` is therefore a
candidate for AWS S3, but it has not been run against Ferrite, so the table above keeps
`multipart`. Neither the `ferrite init` template nor 01 §4 suggests a concrete header (R18).
`multipart` does not copy object attributes; Ferrite sets none on stored artifacts today, so
the `config check` note about it costs nothing yet. Whatever the value, a placement the
store refuses as "exists" is checked with a `head` of the final key and fails the upload
when no object is there (R18), so a wrong value shows up as failed uploads of 8 MiB and
more, not as lost ones.

### Endpoints and addressing

- **Cloudflare R2:** endpoint `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`, region
  `auto` (an empty region or `us-east-1` is an alias for it). Cloudflare's docs do not say which
  addressing styles work, so leave `virtual_hosted_style` at its default (off, the bucket goes in the path); whether R2 needs that is not verified.
- **Backblaze B2:** endpoint `https://s3.<region>.backblazeb2.com`, HTTPS only, no bucket in
  the host. Backblaze's page shows `us-east-005` as a region name; take the region from the
  endpoint your B2 console shows for the bucket (the naming rule is not verified). Both path and virtual-hosted style are supported.
- **Hetzner Object Storage:** endpoint `https://<location>.your-objectstorage.com`, where
  `<location>` is the bucket's location (`fsn1` and `nbg1` are shown in Hetzner's guides), and the same
  location code is the `region`. Hetzner's guides use virtual-hosted style, so set
  `virtual_hosted_style = true` and give the endpoint without the bucket.
- **Scaleway:** not verified; see the table. Check the endpoint for your region in
  Scaleway's console before use.

### Claims and their sources

| Claim | Source | Verified (yes/no, date) |
|---|---|---|
| `AbortIncompleteMultipartUpload` is a lifecycle action, set in days; `Expiration` applies per prefix `Filter` | AWS S3 user guide, lifecycle configuration elements | yes, 2026-10-29 |
| The JSON field names `DaysAfterInitiation` and `Filter.Prefix` in the example above | AWS S3 API reference for lifecycle (not fetched; the user guide above names the actions only) | no |
| Expiry of `tmp/` is the only safe expiry (`artifacts/` holds referenced media) | Ferrite's own key layout, 03 §3 | yes (code and design, 2026-10-29) |
| AWS S3 supports `If-None-Match` on PutObject, CopyObject and CompleteMultipartUpload; losers get 412 | docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html | yes, 2026-10-29 |
| `object_store` encodes S3 copy-if-not-exists as `header:`, `header-with-status:` or `multipart`; `multipart` is the native S3 mode and does not copy attributes | docs.rs/object_store/0.14.2 `S3CopyIfNotExists` | yes, 2026-10-29 |
| R2 copy-if-not-exists works with header `cf-copy-destination-if-none-match: *` | docs.rs `S3CopyIfNotExists` (R2 example). Cloudflare's own S3 compatibility page does not list the header; its release notes only say CopyObject accepts Cloudflare headers conditional on the destination | no (header name not found in Cloudflare docs) |
| R2 supports `If-None-Match` on PutObject, atomically (one of concurrent writers wins, others get 412) | developers.cloudflare.com/r2/api/s3/api/ | yes, 2026-10-29 |
| R2 endpoint is `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`; region `auto` (`us-east-1` and empty are aliases) | developers.cloudflare.com/r2/api/s3/api/ | yes, 2026-10-29 |
| R2 needs path-style addressing | only third-party guides (Botble, RcloneView) found; not on Cloudflare's pages | no |
| B2 endpoint is `https://s3.<region>.backblazeb2.com`, HTTPS only | backblaze.com/docs/cloud-storage-call-the-s3-compatible-api | yes, 2026-10-29 |
| B2 supports path-style and virtual-hosted style; `us-east-005` is a region name | same page | yes, 2026-10-29 |
| B2 region naming rule (the region is the segment in the bucket's endpoint) and B2 conditional put or copy behaviour | not found on the pages read | no |
| Hetzner endpoint is `https://<location>.your-objectstorage.com`; region is the location code (`fsn1`, `nbg1` shown); guides use virtual-hosted style | docs.hetzner.com/storage/object-storage/getting-started/using-s3-api-tools | yes, 2026-10-29 |
| Hetzner conditional put or copy support, and the `hel1` endpoint | Hetzner's list of supported actions was not read; `hel1` only from third-party pages | no |
| Scaleway endpoints `s3.<region>.scw.cloud` for `fr-par`, `nl-ams`, `pl-waw` | seen in search results quoting Scaleway's docs; the pages returned 403 when fetched | no |
| `docker pull minio/minio` and `minio/mc` fail | observed locally with Docker 29.8.1 | yes, 2026-10-29 (the reason, MinIO no longer publishing them, is not verified) |
| RustFS is Apache-2.0 licensed | github.com/rustfs/rustfs (License section) | yes, 2026-10-29 |
| RustFS 1.0.4 was released on 2026-10-25 | github.com/rustfs/rustfs/releases (listing shows 25 Oct; the year is taken from the asset timestamps) | yes, 2026-10-29 |
| RustFS image runs as UID 10001; ports 9000/9001; `RUSTFS_ACCESS_KEY`/`RUSTFS_SECRET_KEY` | docs.rustfs.com/installation/docker | yes, 2026-10-29 |
| RustFS 1.0.4 accepts a first `If-None-Match: *` PutObject and rejects the second with `PreconditionFailed` | run against the cluster example, 2026-10-29 | yes, 2026-10-29 |

## 5. TLS

- Default: run behind a reverse proxy (Caddy/nginx/Traefik/LB). As built (M1c-2, ruling
  R31): only `X-Forwarded-For` is honoured, and only when the TCP peer is inside
  `server.trusted_proxies` (CIDRs or bare IPs; default none). The client address is the
  right-most entry that is not itself a trusted proxy. It is logged (`client_ip` on the
  request span) **and keys the per-IP rate limits** (M2a-1: 10 login attempts and 10
  unknown or expired bearer tokens a minute per address, [07 §2](07-admin-api.md#2-auth)).
  **Behind a proxy, list it in `server.trusted_proxies`**: otherwise every request has the
  proxy's address, the per-IP limits become one bucket for the whole site, and 10 login
  attempts a minute from anyone stop everyone from logging in. A request with
  `X-Forwarded-For` from a peer that is not trusted logs a warning naming the peer, once
  per peer address (for the first 64 such peers of the process; R26). `X-Forwarded-Proto`
  and `X-Forwarded-Host` are ignored from every peer: absolute URLs come from `site.url`,
  and the page-cache host is pinned to it (R25), so a TLS-terminating proxy cannot change
  the scheme (not needed while `site.url` is set to the public `https://` URL).
  `crates/ferrite-http/src/app/client.rs`.
- Built-in ACME (solo only in v1): rustls + an ACME client crate (`instant-acme` or
  `rustls-acme` — choose in M4), HTTP-01 on :80 or TLS-ALPN-01. Certificates stored in
  `data/tls/`. In cluster mode ACME would need shared challenge state — out of scope;
  use the LB.

## 6. Metrics & logs

See [01 §7](01-architecture.md#7-observability).

**As built (M2a-2b P19, P20).** Admins (`manage_system`; tokens `system:read`, the
retry `system:write`) have:

- `GET /api/v1/system/metrics/summary`: a snapshot of the node that answered: `node`
  (id, `started_at`), `renders` (template renders since it started), `page_cache`
  (`entries`, `bytes`, `hits`, `misses` of its page cache), `plugins` (`calls` that ran,
  `failures`, every plugin), and the site's `jobs` per status (from the database). Each
  node answers for itself; there is no aggregation yet.
- `GET /api/v1/plugins/{name}/metrics`: 24 hours of the plugin's failures per hook and
  hour (every node's) and the answering node's per-hook call statistics (05 §8).
- `GET /api/v1/system/jobs?status&kind&limit&cursor`: the jobs, newest first (limit
  default 50, at most 200), **without payloads**; `last_error` scrubbed of URLs, paths,
  addresses and credentials (the node log has the full text); `handled` says whether
  some live node lists the kind in its `nodes.meta.job_kinds` (each node publishes the
  kinds its runner handles). A queued job with `handled = false` waits for a node that
  knows the kind (a newer version in a rolling deploy, say). Each filter combination reads one index
  (`jobs_status_created_idx`, `jobs_kind_created_idx`, `jobs_created_idx`; 0006).
- `POST /api/v1/system/jobs/{id}/retry`: a `dead` job back to `queued`, due now with
  `attempts = 0`, `last_error` and `unique_key` kept, in one conditional statement;
  audited `job.retry` (`{id, kind}`). Any other status is `422` (field `status`); the
  retry does not check `handled` (the answer says it). A done job is never re-run; run
  it again by its own trigger. The hourly job prune re-checks status and age in its outer
  `WHERE`, so a job retried while a prune batch waits on its row lock is kept. If the
  node list cannot be read after a committed retry, the answer is still `200` with
  `handled: null`.
- Both summary endpoints count jobs with a `GROUP BY status` over the table and run the
  page cache's pending maintenance on each call: fine for an admin page, not for a
  tight polling loop.

**Planned (M4).** A node-local metrics registry and `/metrics` (Prometheus); in cluster
mode each node writes a 1-minute summary row (`node_metrics`, retained 7 days) so the
dashboard can show cluster-wide figures and time series. (Prometheus remains the source
of truth for real monitoring.)

## 7. Packaging

- Release targets: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`,
  `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`,
  `x86_64-apple-darwin` (wasmtime/cranelift support for all to verify in CI).
- **Docker image (as built, M1.5; admin stage M2b-1):** `Dockerfile` at the repository
  root, multi-stage: `node:24-bookworm-slim` (stage `admin`: `npm install -g pnpm@10.28.2`,
  `pnpm install --frozen-lockfile`, `pnpm build`) builds the admin SPA; `rust:1-bookworm`
  (current stable, matching `rust-toolchain.toml`'s `channel = "stable"`) copies its
  `/src/admin/dist` in and builds `cargo build --release --locked -p ferrite`, which embeds
  it; the runtime is
  `gcr.io/distroless/cc-debian12:nonroot` (glibc, no shell, uid 65532) with the binary at
  `/ferrite`, an empty `/data` owned by the non-root user, `VOLUME /data`, `EXPOSE 8080`,
  `ENTRYPOINT ["/ferrite"]`, `CMD ["serve", "--data", "/data"]`. CA certificates come with
  the distroless base. `.dockerignore` keeps `target/`, `.git`, docs, examples and the admin's local
  `node_modules`, `dist`, `test-results`, `e2e/.auth` and `.tanstack` out of the build context. Health checks use `ferrite healthcheck` (§8), because the image has
  no shell, curl or wget. CI builds the image (`docker build -t ferrite:ci .`) and runs
  `config check` in it; publishing images is M4.
- systemd unit with hardening (`ProtectSystem=strict`, `ReadWritePaths=/var/lib/ferrite`,
  `NoNewPrivileges`, `MemoryMax`) (M4).
- **Cluster example (as built, M1.5):** `examples/cluster/` — `docker-compose.yml`,
  `ferrite.toml`, `ferrite.env`, `Caddyfile`. Usage (also in the compose file's header):

  ```
  docker compose -f examples/cluster/docker-compose.yml up --build
  open http://localhost:8080
  docker compose -f examples/cluster/docker-compose.yml exec node1 /ferrite seed demo --data /data
  ```

  Services: `postgres` (`postgres:18`, `pg_isready` health check, data volume at
  `/var/lib/postgresql`), `rustfs` (`rustfs/rustfs:1.0.4`, an S3-compatible
  server, `/data`), `bucket-init` (`amazon/aws-cli`: creates bucket `ferrite`, then exits), `node1`/`node2` (built from the
  repository, `serve --data /data`, `env_file: ferrite.env`, `ferrite.toml` mounted
  read-only at `/data/ferrite.toml`, `FERRITE_CLUSTER__NODE_NAME` per node, health check
  `/ferrite healthcheck --url http://127.0.0.1:8080/readyz`, start after Postgres is
  healthy and `bucket-init` completed, `stop_grace_period: 20s`) and `caddy` (`caddy:2`,
  port `8080:8080`, `reverse_proxy node1:8080 node2:8080` with `lb_policy round_robin`
  and `health_uri /readyz`). The shared `ferrite.toml` sets `site.url =
  "http://localhost:8080"`, `server.listen = "0.0.0.0:8080"`, `server.trusted_proxies =
  ["172.28.0.0/16"]` (the compose network's subnet, so Caddy's `X-Forwarded-For` is
  honoured), `cluster.mode = "cluster"` and S3 media on RustFS (`bucket = "ferrite"`,
  `region = "us-east-1"`, `endpoint = "http://rustfs:9000"`, `allow_http = true`).
  `ferrite.env` holds `FERRITE_DATABASE__URL` and the S3 key pair: **demo values**,
  shared with the `postgres` and `rustfs` service definitions; replace them together.
  **Verification status (P24):** `crates/ferrite/tests/examples.rs` loads the example's
  `ferrite.toml` + `ferrite.env` through `ferrite_config::load` and `ferrite config check`,
  and checks that every `FERRITE_*` name in the compose file is a known setting; CI runs
  `docker compose … config --quiet`, `caddy validate` on the Caddyfile and `docker build`. `docker compose up` is not run in CI. It was run by
  hand on 2026-10-29 (M4-1): both nodes healthy, an upload through node1 read back
  byte for byte through node2, the object in the `ferrite` bucket on RustFS. Every image is
  pinned as `name:tag@sha256:<index digest>`; the digests stay valid until the registry
  deletes them, and need a manual bump to follow new releases. On 2026-10-29 `docker pull minio/minio` and `docker pull minio/mc` were denied ("pull access
  denied ... repository does not exist or may require 'docker login'"), so the example moved to RustFS 1.0.4 (§4a table for its licence, release date and UID).

## 8. CLI summary

### Implemented (M1c-1 to M4-1: `crates/ferrite`)

```
ferrite init --data DIR --url URL [--title T] [--db URL] [--schema NAME] [--media s3://bucket/prefix]
ferrite migrate status --data DIR
ferrite migrate up     --data DIR [--to VERSION]
ferrite migrate down   --data DIR --to VERSION [--allow-lossy] [--yes]
ferrite migrate verify --data DIR
ferrite serve          --data DIR                      # M1c-2; cluster wiring M1.5
ferrite seed demo      --data DIR [--force]            # M1c-2
ferrite config check   --data DIR                      # M1.5
ferrite media sync     --to STORE (--from STORE | --data DIR)   # M4-1; STORE = local:PATH | s3://bucket[/prefix]
ferrite media gc       --data DIR [--dry-run] [--grace-days N] [--claim]  # M4-1
ferrite media regenerate --data DIR [--all] [--item ID]        # M4-1
ferrite export         --data DIR --out FILE|- [--with-media]                      # M4-1
ferrite import-site    ARCHIVE|- --data DIR [--with-media]                         # M4-1
ferrite db copy        --from URL --to URL [--from-schema S] [--to-schema S]       # M4-1; URL = postgres://… | sqlite:PATH
ferrite backup         --data DIR --to FILE.tar.zst|-|DIR|s3://… [--no-media] [--no-secret-key] [--keep N]  # M4-1
ferrite restore        SOURCE --data DIR [--snapshot NAME] [--force]               # M4-1; SOURCE = FILE.tar.zst | - | DIR | s3://…
ferrite healthcheck    [--url URL]                     # M1.5, for container health checks
ferrite user create    --data DIR --login L --email E [--role admin|editor|author|contributor]
                       [--display-name N] [--password-stdin]                     # M2a
ferrite user reset-password --data DIR --login L [--password-stdin]              # M2a
ferrite user list      --data DIR                                                # M2a
ferrite user disable-totp --data DIR --login L                                   # M2a
ferrite theme install  --data DIR --dir PATH [--activate]                        # M2a-2a
ferrite theme activate --data DIR NAME                                           # M2a-2a
ferrite theme list     --data DIR                                                # M2a-2a
ferrite theme pack     --dir PATH --out FILE                                     # M2b-3 (no data folder)
ferrite plugin check   --data DIR --dir PATH                                     # M2a-2a
ferrite plugin install --data DIR --dir PATH [--enable] [--yes]                  # M2a-2a
ferrite plugin list|enable|disable|remove --data DIR [NAME] [--purge]            # M2a-2a
ferrite import wordpress FILE --data DIR [--uploads DIR|ZIP] [--fetch-media] [--dry-run]
    [--report DIR] [--no-keep-permalinks] [--no-site-options] [--category-base B]
    [--tag-base B] [--as LOGIN] [--takeover]                                      # M3-1
```

**`ferrite import wordpress`** (`crates/ferrite/src/import_cmd.rs`; design 08): imports a
WXR file as the active admin `--as` names (default: the first active admin). Writes
`report.json` and `report.html` to `--report` or `<data>/import/reports/<run-id>/` and
prints the counts per kind. `--uploads` is the old `wp-content/uploads` folder or a
`.zip` of it, read in place (M3-2). Exit 0 done, 2 done with refused redirects or missing
or refused media, 3 another import running (`--takeover` takes over one with no heartbeat
for 120 s), 1 on any error (a run that failed after it started still writes its partial
report, M3-2). With SQLite `serve` holds the data folder's lock: stop it first. Media
variants are made by the next `serve` (the `media.variants` jobs).

**Imports through the admin and the API** (M3-2; design 08 §1): with `serve` running, an
admin uploads the WXR file and, optionally, a zip of the uploads on the admin's Import
screen (`/admin/import`) or with `POST /api/v1/import/wordpress`; the import runs as an
`import.wordpress` job on whichever node claims it (04 §6), and the report is stored in
the media store and linked from the runs table. The job needs local disk for a copy of
the inputs (`<data>/import/job-<id>/`) and the run's work file (`<data>/import/<run-id>/`).

**`ferrite config check`** (`crates/ferrite/src/config_cmd.rs`): loads `<data>/ferrite.toml`
plus the `FERRITE_*` environment exactly as `serve` does (`ferrite_config::load`) and never
opens the database or the media store. Valid: prints one line,
`ok: mode=<solo|cluster> database=<sqlite|postgres> media=<local|s3> workers=<n> mail=<configured|off> backup=<off|daily|weekly>` (`mode`
is the effective mode, `auto` resolved; `workers` the effective `cluster.workers`; `mail` whether `[mail]` is set, `backup` the `[backup] schedule`), exit
0. Invalid: prints `error: invalid configuration (<n> problem(s))` and one
`  <path>: <message>` line per problem to stderr (secret values are never printed), exit 1.
Checks that need the database (site id, live nodes, migrations) are not part of it.

`config check` also prints `warning: …` lines to stderr (the exit code stays 0) for settings that
work but may cost something: `media.s3.copy_if_not_exists = "multipart"` (it does not copy object
attributes; Ferrite sets none on stored artifacts today, so it costs nothing yet, and it is the
value §4a lists for AWS S3). With `[backup] schedule` on, it fails (exit 1) for a `backup.to` that
`serve` would refuse, a local folder included that is not empty and not a repository, and, on
SQLite, for a `database.url` other than `sqlite://ferrite.db` (§4).

**`ferrite media sync`** (`crates/ferrite/src/media_cmd.rs`): `--to STORE` and one of `--from STORE`
or `--data DIR` (the site's configured store; only `ferrite.toml` is read, so it runs while `serve`
runs). A STORE is `local:<path>` or `s3://bucket[/prefix]`; every `s3://` URL takes its credentials,
endpoint and options from the environment (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
`AWS_REGION`, `AWS_ENDPOINT_URL`, `AWS_ALLOW_HTTP`, `AWS_COPY_IF_NOT_EXISTS`,
`AWS_CONDITIONAL_PUT`), a `--data` S3 store from `[media.s3]`. Copies every object under
`artifacts/` (recorded in the database or not), skips keys already at the destination with the same
size, checks the size after each copy, never deletes, never copies `tmp/`, 8 copies at once (memory
peaks near 192 MiB, design 03 §8); the destination is probed first and an unusable one exits 1; keys
that are not artifacts are ignored with a notice. Prints `copied N (B bytes), skipped M, failed F`
on stdout and one `failed <key>: <reason>` line per failure on stderr. Exit 0 done, 2 when some
copies failed (the rest were copied), 1 on an error (a bad URL, an unreadable store); a usage error
(neither `--from` nor `--data`, or both) exits 2 as clap does.

**`ferrite export`**, **`ferrite import-site`**, **`ferrite db copy`**
(`crates/ferrite/src/export_cmd.rs`, `site_copy.rs`, `archive.rs`; format and rules in [02
§10](02-storage.md#10-portable-export-d14)): `export` writes the portable archive of the site in
`--data` to `--out` (a new file with mode 0600, an existing one is refused and a failed export
removes it; `-` writes stdout, and the summary goes to stderr); `--with-media` adds every stored
artifact. An artifact whose file is missing from the media store is left out: one `missing artifact
<hash>` line each on stderr, the count in the summary, the hashes in the manifest's `missing` list,
and exit 2; a file whose content does not hash to its name fails the export (exit 1, the hash
named); a usage error (a bad or missing flag) is exit 2 too, as clap does, so a wrapper that treats
2 as "written" must check that the archive exists. The database must be at this binary's schema
(older: exit 1, run `ferrite migrate up`; newer: exit 3); with SQLite stop `serve` first, a Postgres
site can be exported live. `import-site ARCHIVE --data DIR` (`-` reads stdin) loads an export into
the fresh database of a folder made by `ferrite init`, in one transaction, then migrates up and
rebuilds search; `--with-media` puts the archive's blobs into the site's media store (present ones
are skipped), without it they are read, checked and reported but not stored. Prints the row count
and, when users have TOTP on, how to keep their sign-in working (copy `secret.key`, set
`auth.secret_key` or `ferrite user disable-totp`), and a warning when the site's media store is
outside its data folder (it may be another site's: the target keeps its own install id, so its media
GC deletes nothing there until `ferrite media gc --claim`, [03
§6](03-media.md#6-garbage-collection)). Imported `artifacts` rows start without a GC mark. Exit 0
done; 3 when the archive's format or schema (or the target's schema) is newer than the binary; 1 on
any other error, among them a target that is not fresh (the message names the first table with
rows), a tampered table (sha256), a hostile path, an over-long row line (64 MiB), more artifact
bytes than the manifest declares, a truncated stream, an archive that is a backup, database settings
that would refuse the migration to the binary's schema after the load (`database.auto_migrate` off,
or a Postgres contract migration without `database.allow_contract`). No refusal leaves rows behind,
and a schema step taken for the archive is undone (best effort; a warning says when that failed, and
it is skipped when the target holds rows after a commit that reported an error), so the import can
be retried. `db copy --from URL --to URL` streams one database into another (export + import with no
file): `postgres://…` (with `--from-schema`/`--to-schema`) or `sqlite:<path to ferrite.db>` (the
folder lock is taken in its folder; an absent target file is created, and removed again when the
copy fails, with the `ferrite.lock` the run created). URLs only, no stdin. Same source and target
rules and exit codes as `export` and `import-site` (no exit 2: no media); it copies no media (`media
sync`) and writes no `ferrite.toml`, but prints the `[database]` block to put there: a SQLite target
named `ferrite.db` as `url = "sqlite://ferrite.db"` (relative to its folder, which is then the data
folder: the form `backup` and `restore` need), another SQLite file by its absolute path with a note
that `backup` refuses such a site; a Postgres password, in the user info or a `password` query
parameter, is shown as `PASSWORD`, with a note after the block. A last note says the copy has its
own install id: once the site runs on it, run `ferrite media gc --claim` after the old database has
stopped serving the media store (R16).

**`ferrite backup`**, **`ferrite restore`** (`crates/ferrite/src/backup/`; rules in §4):
`backup --data DIR --to TARGET` backs up a solo site to a `.tar.zst` file (a new file with mode
0600, an existing one is refused, a failed backup removes it), `-` (stdout; the summary goes to
stderr) or a repository (a folder, created if missing, or `s3://bucket[/prefix]`).
`--no-media` leaves the artifacts out, `--no-secret-key` leaves `secret.key` out, `--keep N`
(repositories only) prunes to the newest N snapshots and the blobs they list. Prints the files,
the schema, the artifact counts (for a repository: copied and already present) and what was
pruned. Exit 0 done; 2 done but artifacts were missing from the media store (one `missing
artifact <hash>` line each on stderr, listed in the manifest), and also a usage error (clap,
e.g. `--keep 0`): check that the archive or snapshot exists before treating 2 as written; 3 a
repository of a newer format than the binary; 1 on any other error: a Postgres site, a
`database.url` other than `sqlite://ferrite.db`, `serve` running (the folder lock), an
artifacts GC pass holding its lock for more than 5 minutes, a failed integrity check, a damaged
blob, `--keep` with an archive, an archive inside `<data>/backups`, a repository location that
is not empty (but for `lost+found`) and has no marker or lies inside the data folder or media
root; a failed backup leaves no archive, no repository snapshot and no local files, except that
a `--keep` prune failing after the snapshot was written keeps the snapshot.
`restore SOURCE --data DIR [--snapshot NAME] [--force]` prints what it restored, the artifact
counts, notes on missing artifacts and a missing `secret.key`, the folder moved aside, a warning
when the restored media store is outside `DIR` (the restored database is a new install, R16:
`ferrite media gc --claim` only for a restore in place), then the `migrate status` report.
Exit 0 done; 3 when the backup's schema or the repository's format is newer than the binary
(nothing written); 1 on any other error: a non-empty folder without `--force` (`lost+found`
alone does not count), a folder in use, an export archive, a repository with only export
archives (use `import-site`), an unknown snapshot, a damaged archive or repository (sha256,
size), a restored configuration whose database is not `DIR/ferrite.db`.

**`ferrite media gc`** (`crates/ferrite/src/media_cmd.rs`, `crates/ferrite/src/gc.rs`; design 03
§6): one pass of the artifacts GC in process, the same `gc::run` the daily `artifacts.gc` job runs.
It opens the database (with SQLite, stop `serve` first: it holds the data folder's lock) and the
configured media store, then marks unreferenced artifact rows, deletes rows marked longer than the
grace period and then their files, files under `artifacts/` with no row that are older than the
grace period, and `tmp/` objects older than 24 h. Files named by a queued or running job or by an
import report are kept. `--grace-days N` (1..=365) overrides `media.gc_grace_days` (default 7) for
this run; `--dry-run` marks and deletes nothing and reports what a run would do (it writes neither
the owner marker nor a missing install id). Deletes happen only on a pass that reads the media
store's owner marker `ferrite-owner.json` naming this database's install id (R16, R19, 03 §6): a
pass that finds no marker writes it and deletes nothing yet; on a store another site owns, or whose
marker cannot be read, the pass marks and cleans `tmp/` only and logs an error. Either way it
appends `; deletes skipped: <reason>` to its line (exit 0). `--claim` rewrites the marker for this
site first, for a site that replaced the store's owner (a restore in place, a moved database), and
appends `; claimed the media store from install <id> (site …, claimed …)`; with `--dry-run` it names
that owner and writes nothing. Prints `marked N, unmarked M, deleted rows R (B bytes), deleted
unrecorded blobs U (B bytes), deleted tmp T` on stdout (after `dry run: ` for a dry run; `, failed
F` is appended when some blob or `tmp/` deletes failed, which are logged and retried by the next
pass, the exit code staying 0). Exit 0 done, 3 the `artifacts.gc` lock is held (a cluster node's
job, another `media gc`, or a backup copying blobs, R17), 1 on an error; a bad flag (`--grace-days
0`) exits 2 as clap does.

**`ferrite media regenerate`** (`crates/ferrite/src/media_cmd.rs`): makes image size variants again
in process, like `seed demo` (no jobs, so no unique-key clash). The sizes are the core sizes plus
the `[images]` of the theme active in the database (`variant_specs`). By default an item is
processed when it is raster and its recorded `variants` lack a size that applies to its width: a
width-bounded size wider than the original is never made, so it never counts as lacking and a second
run changes nothing. Only size names are compared, so a theme that changes the width of an existing
size needs `--all`, which redoes every raster item. `--item ID` (a content id; unknown is exit 1)
limits the run to one item; a complete item is skipped unless `--all` is given too. Items in the
trash are included. Non-raster items and originals over 64 MiB are skipped (the latter with a line
on stderr). Per item it decodes with the job's code (one image at a time), stores each variant and
records them only while the row still holds that original; the replaced variant files stay for
`media gc`. With SQLite it opens the database like `seed`, so `serve` must be stopped; with Postgres
it may run beside the nodes (each item appends its `post:<id>` invalidation event; tested on
Postgres). Progress goes to stderr every 50 items; stdout is `regenerated N, skipped S, failed F`
and each failure is a `failed <id>: <reason>` line on stderr. Exit 0 done, 2 when some items failed
(the rest were done), 1 on an error.

**`ferrite healthcheck`** (`crates/ferrite/src/healthcheck.rs`): `GET`s `--url` (default
`http://127.0.0.1:8080/readyz`; plain `http://` only) and exits 0 on a 200 response, 1 on
any other status, a connection error or no response within 5 s. It exists because the
distroless image has no HTTP client for a container `HEALTHCHECK`.

**Logging.** Logs go to stderr. `FERRITE_LOG` is an `EnvFilter` expression (for example
`FERRITE_LOG=debug`); unset, the filter is `info`; an invalid value prints a warning and falls
back to `info` (`crates/ferrite/src/main.rs`). `FERRITE_LOG` is not a config key. The binary
also installs the `ring` rustls default crypto provider at start.

**`ferrite init`** (`crates/ferrite/src/init.rs`):

- Flags: `--data` and `--url` are required. `--title` defaults to `My Site`. `--db` is a
  database URL (default: SQLite `ferrite.db` in the data folder). `--schema` is a Postgres
  schema, created if missing. `--media s3://bucket[/prefix]` selects S3 media (default: local
  `media/` in the data folder). A bad `--media` URL is an error.
- Writes `<data>/ferrite.toml` from the template in `ferrite-config`, created exclusively
  with mode `0600` on Unix because it can hold the database URL. For S3 it writes the
  bucket and prefix with region `us-east-1` and tells you to set the real region, endpoint
  and credentials before serving.
- **Refuses to overwrite** an existing `ferrite.toml`, and **refuses an existing database**:
  an existing SQLite file, or a Postgres schema that already has applied migrations. Use
  `ferrite migrate` for those.
- Creates the data folder, `media/` (local media only) and `backups/`, validates the written
  config, then creates the database and applies all migrations. If any step fails, it removes
  what this run created (the config file, new folders, and with SQLite the new database and
  lock files) so a retry starts clean.
- **Postgres is not cleaned up.** Each migration commits on its own, so an init that fails
  partway through migrating leaves the schema (created by `--schema` if it was missing) with
  the migrations applied so far recorded in `_sqlx_migrations`. A retry then refuses it as an
  existing database. To recover, drop the schema (`DROP SCHEMA <name> CASCADE`) and run
  `ferrite init` again; without `--schema` the tables are in the connection's current schema
  (normally `public`), so drop that database instead or remove its ferrite tables and
  `_sqlx_migrations` by hand.
- The validation step applies `FERRITE_*` environment overrides, so it checks the effective config, which can differ from the file that was written.
- **Site key** (P4, F30): when the written config resolves to solo mode and configures no
  `auth.secret_key`, `init` creates `<data>/secret.key` (64 hex characters, mode `0600`,
  never printed) and removes it again if `init` fails. Otherwise it writes no key file; a
  solo site without one gets it at its first `serve`.
- **Cluster configs need the key up front** (R1, R9): a config that resolves to cluster
  mode (explicit `cluster.mode = "cluster"`, or `auto` with Postgres and S3 or
  `local_shared` media) fails validation without `auth.secret_key[_file]`, so `ferrite init
  --db postgres://… --media s3://…` needs the key in the environment, e.g.
  `FERRITE_AUTH__SECRET_KEY=$(openssl rand -hex 32) ferrite init …`, and every node must
  then use the same key. The error is `auth.secret_key: required in cluster mode (every node
  must use the same key)`, and `init` removes what it created.
- It does **not** create an admin user: run `ferrite user create --login … --email …`
  next (it prints a one-time password).

**`ferrite migrate`** (`crates/ferrite/src/migrate.rs`, `open.rs`):

- `status` prints database and binary schema versions, the minimum binary version the
  schema requires (if any), pending migrations with their directives (`phase`,
  `min-binary`, `lossy-down`), unknown applied migrations and a verdict. **Exit codes: 0**
  up to date or newer-but-compatible schema, **2** migrations pending, **3** schema
  incompatible with this binary, **1** any error (bad config, missing database, connection
  failure, and so on).
- `up [--to N]` applies pending migrations (all, or up to version `N`).
- `down --to N` reverts everything above version `N`. Migrations whose down loses data need
  `--allow-lossy`. It asks for typed confirmation (`yes`) on a terminal and refuses to run
  without `--yes` when stdin is not a terminal.
- `verify` checks the checksums of applied migrations against this binary.
- **`status`, `down` and `verify` never create a database**: with SQLite they fail with
  "database not found at <path>; run `ferrite init` first" when the file is missing, without
  creating the folder, lock file or database. Only `init` and `up` may create one.
- SQLite caveat: opening the database takes the exclusive data-folder lock
  (`ferrite.lock`), so **every `migrate` subcommand, `status` included, `seed demo` and every
  `user` subcommand fail while another process holds the lock, and a running `ferrite
  serve` holds it**: stop the server first. There is no read-only open. Postgres has no
  folder lock, so these commands work next to a running server.
- Postgres caveat: `status`, `down` and `verify` do not create tables, but connecting with
  a configured `database.schema` that does not exist creates that empty schema.

**`ferrite serve`** (`crates/ferrite/src/serve.rs`):

- **Startup**, in order; any failure exits 1 (rulings P15, P22): load the config; open the
  database (created if missing); the migration check under the migrate lock (pending
  migrations are applied when `database.auto_migrate` is on; on Postgres a pending contract
  migration also needs `database.allow_contract`; an incompatible schema is refused; 02 §8);
  join the node registry (refused when a live node has another `site_id` or `site.url`; in
  solo mode another live node is waited for up to 35 s; on SQLite rows left by a crashed
  process are deleted instead, so a restart is immediate; 04 §3); open the media store and run
  a write/read/delete probe; create the page cache and set the invalidation cursor
  (**before** any node state is read, so a change committed during boot is applied by the
  first poll); build the permalink rules from the `permalink.post`/`permalink.page` options
  and check them against the registered types and taxonomies; load the active theme (at
  startup a broken one falls back to the embedded `blog`, 06 §7); install the enabled plugins
  (a broken row is skipped, 05 §7a); build the route table; bind `server.listen` (default
  `127.0.0.1:8080`; override with `FERRITE_SERVER__LISTEN`, port `0` picks a free one); start
  the background tasks: the invalidation loop (with `LISTEN` wake-ups in cluster mode) and the
  hot-reload task, the plugin-failure flusher (every 5 s), the `request.served` flusher
  (every 10 s or 1 000 events, M2a-2a), the job runner (`cluster.workers` handlers; prune
  jobs, scheduled publishing, `plugin.action`, `plugin.purge`), the periodic scheduler
  (every 10 s: the prune jobs hourly, `content.publish_due` every 30 s,
  `plugin.purge_sweep` daily) and the heartbeat (every 10 s); print
  `Listening on http://<addr>` on stdout. A failure after joining the registry removes the
  node's row again.
- **Hot swap** (ruling P18, 04 §8): `theme.changed`, `plugins.changed`, `plugin.disabled`
  (the plugin stops on this node at once) and `options.changed` for `permalink.post` /
  `permalink.page` make one background task re-read the theme, the plugins or the permalink
  rules (and route table) from the database and swap them in; history lost past the cursor
  reloads all three. The page cache is flushed after each swap. A theme, plugin version or
  permalink set that fails to load keeps the current one and is reported in `nodes.meta`
  (`theme.error`, `plugins.<name>.status = "failed"` with `error`, `permalinks_error`); there
  is no runtime fallback to `blog`, but such a node reports `theme.fallback = true`. A failed reload is retried per component after 5 s,
  doubling up to 5 min, until it succeeds (a newer event retries at once; ruling R14); a
  theme that fell back or a plugin row skipped at startup is retried the same way (R17). The
  replaced theme's `/_theme/<hash>/` assets stay served until the next swap.
- **`nodes.meta`** (heartbeat, and at once after every reload): `site_id`, `site_url`
  (normalised, no trailing slash), `mode`, `clock_skew_ms`, `theme` (`name`, `hash`,
  `origin`, `error`, `fallback`: the node does not serve the active row's artifact, M2a-2a),
  `plugins` (per plugin row: `version`, `hash`, `status` =
  `loaded`/`failed`/`disabled`, `error` up to 1 KiB), `plugins_error` (the plugin rows could
  not be read), `permalinks_error`.
- **Shutdown** on SIGINT or SIGTERM: stop accepting connections and stop the job runner at
  once; open requests get up to 10 s while running jobs get 9 s, after which the runner
  releases the rest without using an attempt (another node takes them at once; the runner
  is abandoned after 12 s, ruling R6); then the background tasks stop (the failure flusher
  writes once more, the `request.served` buffer is flushed into jobs once more, mail sends
  still running are cancelled) and the node deregisters, together within 1 s; the database pools close
  (up to 2 s, cut so that shutdown so far ends within 13 s of the signal); `main` then shuts
  the runtime down with a 1 s grace, dropping connection tasks still running. A clean
  shutdown exits 0; the process is gone at most 14 s after the first signal. A signal
  during startup (after joining the registry the node first leaves it, within 1 s), or a
  second signal during shutdown, exits at once with 130 (SIGINT) or 143 (SIGTERM).
- **Health:** `/healthz` is `200 ok`; `/readyz` is `200` when a store read succeeds within
  2 s and the startup migration check passed, else `503`.
- **Settings it uses** (`[server]`, `[cache]`, `[plugins]`, 01 §4): `server.listen`,
  `server.trusted_proxies` (§5), `cache.max_bytes` (default 256 MB), `cache.poll_interval_ms`
  (default 250; how often the invalidation loop reads the event outbox),
  `plugins.render_deadline_ms` (default 200, ruling R18; the hard timeout is 1.5 × it),
  `plugins.action_deadline_ms`, `plugins.default_memory`, `plugins.default_fuel`,
  `plugins.disable_after_failures`, `plugins.failure_window` (counted cluster-wide, P20),
  `plugins.max_concurrent_calls` and `plugins.render_budget_ms` (M2a-2a P22, 05 §6),
  `[mail]` (`smtp_url[_file]`, `from`; password reset, plugin mail and the admin's test
  mail, 01 §4; config-only, M2b-3 P49: `GET /system/status` shows `mail.configured` and
  `mail.from`, never the SMTP URL, and `POST /system/mail/test` sends one mail to the
  calling admin to check it; mails name the site by the `site.title` option, else the
  configured title (a blank option counts as unset), read when each mail is made, P51),
  `cluster.mode`, `cluster.node_name`, `cluster.workers`, `cache.logged_in` (`bypass` or
  `per-user`, 06 §3), `auth.secret_key[_file]` (else `<data>/secret.key` in solo mode, created
  on first start if missing). Render concurrency (2 × available parallelism, 5 s wait,
  then 503) and the 60 s short TTL are constants.

**`ferrite user`** (`crates/ferrite/src/user_cmd.rs`, ruling P17): the operator path for
accounts. It acts without a login or capability checks (`UserService::operator()`): it is
how the first admin is created and how a locked-out admin gets back in, so whoever can run
it on the data folder (or with the database URL) controls the site. Every subcommand loads
the config, opens the database and runs the same migration check as `serve`; with SQLite it
takes the data-folder lock, so it fails while `serve` runs (stop the server first; Postgres
has no folder lock). The same rules as the API apply (login format, unique login and
e-mail, password policy of 12 to 1024 bytes); any error exits 1 with the message, e.g.
`Error: invalid login: …`, `Error: invalid login: login taken`, `Error: user not found:
"bob"`.

- **Passwords never come from the command line.** With `--password-stdin` one line is read
  from stdin (a trailing `\n` or `\r\n` is dropped) and `must_reset` is cleared, e.g.
  `printf '%s\n' "$PW" | ferrite user create … --password-stdin`. Without it a random
  password (base64url of 15 random bytes, 20 characters) is generated and printed once as
  `password: <p>`, and the account must change it at its first login (`must_reset`).
- `create`: `--role` defaults to `admin`, `--display-name` to the login. Prints
  `created user <login> (<role>)`.
- `reset-password`: sets the password (rules above) and deletes every session of the user,
  on every node at once; API tokens stay valid (revoke them over the API).
- `list`: one line per account, by login: `<login>  <role>  <status>  <email>` (two spaces
  between columns, no header).
- `disable-totp`: turns TOTP off (clears the secret and the replay step); the recovery for
  a lost authenticator, as there are no recovery codes.

**`ferrite theme` and `ferrite plugin`** (`crates/ferrite/src/ext_cmd.rs`, ruling P26): the
operator path for extensions, over `ExtensionService` and the install pipeline, with no
login, no capability checks and no audit rows. Like `ferrite user`, every subcommand loads
the config, opens the database and media and runs the migration check; with SQLite it takes
the data-folder lock, so it fails while `serve` runs. Errors exit 1 with the message.
`theme pack` is the exception: it reads only the folder and writes only `--out`.

- `theme install --dir PATH [--activate]`: packs the folder into a bundle; a theme that
  does not compile is refused before anything is stored. `theme activate NAME` loads the
  stored bundle first (hash-checked) and refuses a theme that does not load, leaving the
  active theme unchanged, as the API does; an embedded theme without a row is installed from
  the embedded files. `theme list` prints `<name>  <version>  <installed|embedded>` plus
  `  active` for the active one (the embedded `blog` when no row is active).
- `theme pack --dir PATH --out FILE` (M2b-3 P42): writes the canonical bundle of the
  folder (hidden files left out; 06 §1) after loading it as an upload would (a theme that
  does not load exits 1 and writes nothing), and prints `packed <n> files, <bytes> bytes,
  sha256 <hex>`. The file is what `POST /api/v1/themes` takes.
- `plugin check --dir PATH`: `PATH` holds `ferrite-plugin.toml` and exactly one `*.wasm`.
  Loads both on a scratch host, prints the permission report and stores nothing; exit 1 when
  the manifest or component is refused. `plugin install --dir PATH [--enable] [--yes]` prints
  the same report (one line per capability or action hook, `!` in front of high-risk ones and
  `~` in front of medium-risk ones (`request.served`); an update also
  lists what it adds to the installed grant under `this update WIDENS the installed grant`,
  and the limits and `fingerprint:`), then asks `install? [y/N]` on a terminal. Without a
  terminal it exits 1 unless `--yes` is given, installing nothing. What is approved is
  exactly the report printed: the install passes that report's fingerprint to the pipeline
  (R1), so a widening update is installed only after that approval. A new plugin is
  installed disabled; `--enable` enables it.
- `plugin list` prints `<name>  <version>  <enabled|disabled>`, plus `  <reason>` after an
  automatic disable. `plugin enable NAME` / `disable NAME`; an unknown name exits 1
  (`plugin not found`).
- `plugin remove NAME [--purge]`: removes the plugin. Its KV and settings stay for 30 days
  (a reinstall within that time finds them; a `plugin.purge` job deletes them after);
  `--purge` deletes them at once (the delete's own job sweeps again shortly after).
- Setting values (including secrets) are never printed by any of these commands.

**`ferrite seed demo`** (`crates/ferrite/src/seed.rs`):

- Writes demo content through the core services where they exist: an `admin` user (no
  password, `must_reset`, so it cannot log in until `ferrite user reset-password --login
  admin` sets one), categories News and Guides, tags rust and cms,
  12 published posts with block bodies dated over the past months, three pages (About,
  About/Team, Contact), a `primary` and a `footer` menu, one generated sample PNG with its
  variants (the core sizes plus the active theme's `[images]`) used as the featured image of
  some posts, and the `site.title` / `site.description` options.
- Refuses (exit 1) when the site already has content, unless `--force`, which adds another
  batch (user, terms and image file reused; posts and pages get fresh `-2`, `-3`, … slugs;
  both menus and the two options are replaced to point at the new batch).
- **Atomic (M4-1 P20):** the image files are stored first (content-addressed), then every row
  (user, terms, the image's media item, artifacts and `media` row, posts, pages, menus,
  options, with the events the services emit) is written in one transaction
  (`ferrite_core::seed::DemoBatch`). A run that fails part-way writes no row, so a plain
  re-run without `--force` succeeds. The image files of a failed run stay in the media store
  until the artifacts GC removes them.
- Requires an initialised site (with SQLite it fails when `ferrite.db` is missing instead of
  creating it) and runs the same migration check as `serve`.

### Planned (later milestones)

```
ferrite plugin new <name>                                       # later (scaffold a plugin crate)
ferrite self-update / rollback [--schema] [--restore-snapshot]  # M4
ferrite doctor        # validates connectivity, permissions, clock skew, versions (config: `config check`)
```
