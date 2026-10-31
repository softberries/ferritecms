//! `ferrite rollback` (09 §2, M4-2 P32): back to the binary kept as `ferrite.prev`, and with
//! `--schema` also the schema, to the `schema_from` that `<data>/upgrade.json` records:
//! down migrations, or with `--restore-snapshot` the SQLite pre-upgrade snapshot.
//!
//! - A binary rollback swaps `ferrite` and `ferrite.prev` (either may run it; running it
//!   twice re-upgrades). It does not touch `upgrade.json`: the older binary's next start
//!   keeps the record (09 §1), so a schema rollback afterwards still finds the move.
//! - `--schema` runs only as the binary the record names as `to` (it holds the down
//!   migrations). Run as `ferrite` it swaps the binaries afterwards; run as `ferrite.prev`
//!   (after a binary rollback) they are already swapped and stay so.
//! - A rollback that used `upgrade.json` renames it to `upgrade-rolled-back-<ts>.json`.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;
use ferrite_config::{Config, DbKind};
use ferrite_store::{SCHEMA_META_LAST_BINARY, Store};

use super::{probe, swap};
use crate::open::{OpenMode, migrate_lock, open_store, release_migrate_lock};
use crate::upgrade::{self, UPGRADE_FILE, UpgradeRecord};
use crate::{binary_version, load_config};

/// The SQLite data-folder lock `serve` holds (as the store and `restore` take it).
const LOCK_FILE: &str = "ferrite.lock";
/// The first bytes of every SQLite database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Args)]
pub struct RollbackArgs {
    /// The site's data folder. A binary rollback first runs the previous binary's
    /// `migrate status` for it (and refuses when that binary cannot open the schema).
    #[arg(long)]
    pub data: Option<PathBuf>,
    /// Also roll the schema back to the `schema_from` of `<data>/upgrade.json` (run it as
    /// the newer binary, the record's `to`).
    #[arg(long, requires = "data")]
    pub schema: bool,
    /// With `--schema`: allow down migrations that lose data.
    #[arg(long, requires = "schema", conflicts_with = "restore_snapshot")]
    pub allow_lossy: bool,
    /// With `--schema` (SQLite only): put the pre-upgrade snapshot `upgrade.json` names in
    /// place of the database instead of running down migrations. Everything written since
    /// the upgrade is lost; the current database is moved to `backups/rolled-back-<ts>.db`.
    #[arg(long, requires = "schema")]
    pub restore_snapshot: bool,
    /// Do not ask for confirmation of `--restore-snapshot` (required when stdin is not a
    /// terminal).
    #[arg(long)]
    pub yes: bool,
    /// Test seam: the path treated as the running binary.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub test_current_exe: Option<PathBuf>,
}

pub async fn run(args: RollbackArgs) -> Result<ExitCode> {
    let bins = Binaries::locate(&current_exe(&args)?);
    match &args.data {
        Some(data) if args.schema => schema_rollback(&args, data, &bins).await,
        _ => binary_rollback(&args, &bins).await,
    }
}

/// The installed binary and the one kept beside it.
#[derive(Debug, PartialEq, Eq)]
struct Binaries {
    /// The binary's path (`ferrite`).
    installed: PathBuf,
    /// [`swap::prev_path`] of it (`ferrite.prev`).
    prev: PathBuf,
    /// This process runs as `prev` (after a binary rollback the newer binary is there).
    running_prev: bool,
}

impl Binaries {
    /// From the running binary: `X.prev` beside an existing `X` is the kept one, anything
    /// else is the installed one.
    fn locate(exe: &Path) -> Self {
        if let Some(base) = exe
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|n| n.strip_suffix(".prev"))
            .filter(|b| !b.is_empty())
        {
            let installed = exe.with_file_name(base);
            if installed.is_file() {
                return Binaries {
                    installed,
                    prev: exe.to_owned(),
                    running_prev: true,
                };
            }
        }
        Binaries {
            installed: exe.to_owned(),
            prev: swap::prev_path(exe),
            running_prev: false,
        }
    }

    fn require_prev(&self) -> Result<()> {
        if !self.prev.is_file() {
            bail!(
                "there is no previous binary at {} (self-update keeps one there); nothing was \
                 changed",
                self.prev.display()
            );
        }
        Ok(())
    }

    /// Exchanges the two binaries (after sweeping a killed run's leftovers).
    fn swap(&self) -> Result<()> {
        swap::sweep_stale(&self.installed);
        swap::swap_with_prev(&self.installed).with_context(|| {
            "cannot swap the binaries: run rollback as the binary's owner, outside the \
             systemd sandbox"
        })
    }
}

/// The running binary (symlinks resolved), or the test seam's path.
fn current_exe(args: &RollbackArgs) -> Result<PathBuf> {
    #[cfg(feature = "test-seams")]
    if let Some(path) = &args.test_current_exe {
        return std::fs::canonicalize(path)
            .with_context(|| format!("cannot resolve {}", path.display()));
    }
    let _ = args;
    let exe = std::env::current_exe().context("cannot find the running binary")?;
    std::fs::canonicalize(&exe).with_context(|| format!("cannot resolve {}", exe.display()))
}

/// Swaps `ferrite` and `ferrite.prev`; with `--data` only when the binary that becomes
/// `ferrite` can open the site's schema (exit 3 when it cannot).
///
/// An older binary refuses config keys it does not know, so the hints name the keys added
/// since M4-1.
async fn binary_rollback(args: &RollbackArgs, bins: &Binaries) -> Result<ExitCode> {
    bins.require_prev()?;
    if let Some(data) = &args.data {
        let mut probe_args = ["migrate", "status", "--data"].map(OsStr::new).to_vec();
        probe_args.push(data.as_os_str());
        let (code, out) = probe(&bins.prev, &probe_args).await?;
        println!(
            "migrate status of {} for {}:",
            bins.prev.display(),
            data.display()
        );
        print!("{out}");
        match code {
            Some(0) | Some(2) => {}
            Some(3) => {
                eprintln!(
                    "error: {} cannot run this site's database schema (migrate status exit \
                     3): roll the schema back first with `ferrite rollback --schema --data \
                     {}`, run as the newer binary; nothing was changed",
                    bins.prev.display(),
                    data.display()
                );
                return Ok(ExitCode::from(3));
            }
            other => bail!(
                "could not read the migration status with {} (exit {other:?}): see its \
                 output above. With SQLite, stop `serve` first. {OLDER_CONFIG_NOTE} Or run \
                 without --data to skip the check. Nothing was changed",
                bins.prev.display()
            ),
        }
    }
    bins.swap()?;
    println!(
        "swapped {} and {}: the binary that was kept is now the installed one (run \
         `ferrite rollback` again to undo)",
        bins.installed.display(),
        bins.prev.display()
    );
    if args.data.is_none() {
        println!("note: nothing was checked against the site (no --data). {OLDER_CONFIG_NOTE}");
    }
    print_restart();
    Ok(ExitCode::SUCCESS)
}

/// `--schema` (and `--restore-snapshot`).
async fn schema_rollback(args: &RollbackArgs, data: &Path, bins: &Binaries) -> Result<ExitCode> {
    let config = load_config(data)?;
    let data_dir = &config.data_dir;
    let rec = upgrade::load(data_dir)
        .context("rollback works from upgrade.json; nothing was changed")?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "there is no {UPGRADE_FILE} in {}: no recorded upgrade to roll back (roll the \
                 schema back explicitly with `ferrite migrate down --to N`); nothing was \
                 changed",
                data_dir.display()
            )
        })?;
    let me = binary_version()?;
    if rec.to != me.to_string() {
        bail!(
            "{UPGRADE_FILE} records the upgrade from {} to {}, but this binary is {me}: \
             `rollback --schema` must run as ferrite {} (it holds the down migrations); after \
             a binary rollback that is {}; nothing was changed",
            rec.from.as_deref().unwrap_or("an unknown version"),
            rec.to,
            rec.to,
            bins.prev.display()
        );
    }
    // After a binary rollback (running as `.prev`) the binaries are already swapped.
    let swap_after = !bins.running_prev;
    if swap_after {
        bins.require_prev()?;
    }
    // The binary that runs after the rollback must be the one the record names as `from`.
    let older = check_older(
        if swap_after {
            &bins.prev
        } else {
            &bins.installed
        },
        &rec,
    )
    .await?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    if args.restore_snapshot {
        restore_snapshot(args, &config, &rec, &me, &ts).await?;
    } else {
        schema_down(args, &config, &rec, &me, older.as_ref()).await?;
    }

    // The database step used the record: set it aside so the older binary's start sees no
    // stale move.
    let renamed = data_dir.join(format!("upgrade-rolled-back-{ts}.json"));
    match move_no_clobber(&data_dir.join(UPGRADE_FILE), &renamed) {
        Ok(()) => {
            sync_dir(data_dir);
            println!("{UPGRADE_FILE} renamed to {}", renamed.display());
        }
        Err(e) => eprintln!(
            "warning: cannot rename {UPGRADE_FILE} to {} ({e}); rename it by hand",
            renamed.display()
        ),
    }

    if swap_after {
        bins.swap().map_err(|e| {
            anyhow::anyhow!(
                "{e:#}; the schema is rolled back, but the binaries were not swapped: run \
                 `ferrite rollback` (without --schema) to swap them"
            )
        })?;
        println!(
            "swapped {} and {}: ferrite {} is now the installed binary's predecessor",
            bins.installed.display(),
            bins.prev.display(),
            rec.to
        );
    } else {
        println!(
            "the binaries were already swapped (this is {}); {} stays installed",
            bins.prev.display(),
            bins.installed.display()
        );
    }
    print_restart();
    Ok(ExitCode::SUCCESS)
}

/// What an older binary may refuse in `ferrite.toml` or the environment.
const OLDER_CONFIG_NOTE: &str = "An older binary refuses config keys it does not know: \
    remove those before starting it (for example [server.tls], server.metrics_listen and \
    their FERRITE_SERVER__* overrides, which older builds lack).";

/// Refuses, before anything changes, when `older --version` is not the record's `from`. A
/// record without `from` (a database from before M4-2) cannot be checked. Returns the
/// version `older` runs as: the record's `from`, or for a record without one what `older`
/// reports (`None` when it reports nothing usable).
async fn check_older(older: &Path, rec: &UpgradeRecord) -> Result<Option<semver::Version>> {
    let reported = |code: Option<i32>, out: &str| {
        out.split_whitespace()
            .last()
            .and_then(|v| semver::Version::parse(v).ok())
            .filter(|_| code == Some(0))
    };
    let Some(from) = rec.from.as_deref() else {
        println!(
            "note: {UPGRADE_FILE} does not record the previous version, so {} is not checked",
            older.display()
        );
        return Ok(probe(older, &[OsStr::new("--version")])
            .await
            .ok()
            .and_then(|(code, out)| reported(code, &out)));
    };
    let (code, out) = probe(older, &[OsStr::new("--version")]).await?;
    let version = reported(code, &out);
    if version.is_none() || version != semver::Version::parse(from).ok() {
        bail!(
            "{} reports {:?} (exit {code:?}), but {UPGRADE_FILE} records the upgrade from \
             {from}: the schema goes back for ferrite {from}, so put that binary at {} and \
             retry; nothing was changed",
            older.display(),
            out.trim(),
            older.display()
        );
    }
    Ok(version)
}

/// Down migrations to `rec.schema_from` under the migrate lock (SQLite: opening takes the
/// data-folder lock, so `serve` must be stopped). Then `schema_meta.last_binary_version`
/// names `older` (the binary that runs next, when known): the database is that binary's
/// again, so the next upgrade records `from: <older>` even when `older` never starts in
/// between (or is a build from before M4-2, which does not write the key).
async fn schema_down(
    args: &RollbackArgs,
    config: &Config,
    rec: &UpgradeRecord,
    me: &semver::Version,
    older: Option<&semver::Version>,
) -> Result<()> {
    let store = open_store(config, OpenMode::MustExist)
        .await
        .context("with SQLite stop `serve` first; nothing was changed")?;
    if matches!(config.database_kind(), DbKind::Postgres { .. }) {
        refuse_live_nodes(store.as_ref()).await?;
    }
    let before = store.migration_status(me).await?.db_version;
    if before <= rec.schema_from {
        println!(
            "the schema is at {before}, not above {}: nothing to revert",
            rec.schema_from
        );
        store_older_version(store.as_ref(), older).await;
        return Ok(());
    }
    let reverted = down(store.as_ref(), rec.schema_from, args.allow_lossy).await;
    let reverted = match reverted {
        Ok(r) => r,
        Err(e) if !args.allow_lossy && is_lossy_refusal(&e) => {
            let alternative = match config.database_kind() {
                DbKind::Sqlite { .. } => format!(
                    "or --restore-snapshot to put the pre-upgrade snapshot back (everything \
                     written since {} is lost)",
                    rec.at.to_rfc3339()
                ),
                DbKind::Postgres { .. } => "(--restore-snapshot is for SQLite; on Postgres \
                     restore with your provider's PITR instead)"
                    .to_owned(),
            };
            bail!(
                "{e:#}; nothing was changed. Rolling back to schema {} drops data the newer \
                 migrations hold: pass --allow-lossy to accept that, {alternative}",
                rec.schema_from
            );
        }
        Err(e) => return Err(e),
    };
    let shown: Vec<String> = reverted.iter().map(i64::to_string).collect();
    println!(
        "reverted migrations {} (schema {before} -> {})",
        shown.join(", "),
        rec.schema_from
    );
    store_older_version(store.as_ref(), older).await;
    Ok(())
}

/// Sets `schema_meta.last_binary_version` to `older` after a schema rollback. A failure
/// only warns: the schema is already back, and a binary from M4-2 on writes the key at its
/// next start (an older one never does).
async fn store_older_version(store: &dyn Store, older: Option<&semver::Version>) {
    let Some(older) = older else { return };
    if let Err(e) = store
        .read()
        .schema_meta()
        .set(SCHEMA_META_LAST_BINARY, &older.to_string())
        .await
    {
        eprintln!(
            "warning: cannot record {older} as the database's last binary version ({e}); \
             the next upgrade's {UPGRADE_FILE} may name the wrong previous version"
        );
    }
}

async fn down(store: &dyn Store, to: i64, allow_lossy: bool) -> Result<Vec<i64>> {
    let guard = migrate_lock(store).await?;
    let result = store.migrate_down(to, allow_lossy).await;
    release_migrate_lock(guard).await;
    Ok(result?)
}

/// `plan_down`'s refusal of a lossy step (ferrite-store `migrate::plan_down`).
fn is_lossy_refusal(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|c| c.to_string().contains("has a lossy down"))
}

/// `--restore-snapshot`: checks that the recorded snapshot holds the record's
/// `schema_from` (P73), confirms, takes the folder lock, moves the database (and its
/// `-wal`/`-shm`) to `backups/rolled-back-<ts>.db[-wal|-shm]` and copies the snapshot into
/// place. Only the path `upgrade.json` records is used.
async fn restore_snapshot(
    args: &RollbackArgs,
    config: &Config,
    rec: &UpgradeRecord,
    me: &semver::Version,
    ts: &str,
) -> Result<()> {
    let db = match config.database_kind() {
        DbKind::Sqlite { path } => path,
        DbKind::Postgres { .. } => bail!(
            "--restore-snapshot is for SQLite sites (Postgres takes no pre-migration snapshot): \
             restore the database with your provider's point-in-time recovery (PITR) to before \
             {}, or roll the schema back with `rollback --schema` (down migrations); nothing \
             was changed",
            rec.at.to_rfc3339()
        ),
    };
    let data_dir = &config.data_dir;
    let backups = data_dir.join("backups");
    // M4-4 fix round 2 (B1): never start over what an earlier run left.
    refuse_leftovers(&db, &backups)?;
    let Some(snapshot) = &rec.snapshot else {
        bail!(
            "{UPGRADE_FILE} records no snapshot for this upgrade (nothing was migrated, or the \
             upgrade did not finish), so --restore-snapshot is not possible: use \
             `rollback --schema` without it; nothing was changed"
        );
    };
    let snapshot = data_dir.join(snapshot);
    check_snapshot(&snapshot)?;
    check_snapshot_schema(&snapshot, rec, me).await?;
    let taken = std::fs::metadata(&snapshot)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|_| "unknown".to_owned());
    // Before asking: a running site is refused first. Held until the database is replaced.
    let _lock = lock_folder(data_dir)?;
    println!(
        "restore the pre-upgrade snapshot {} (upgrade at {} per {UPGRADE_FILE}; file \
         modified {taken}) in place of {}",
        snapshot.display(),
        rec.at.to_rfc3339(),
        db.display()
    );
    println!(
        "everything written to the site since {} is lost; the current database is moved to \
         {} (never deleted)",
        rec.at.to_rfc3339(),
        backups.join(format!("rolled-back-{ts}.db")).display()
    );
    if !args.yes && !confirm()? {
        bail!("cancelled; nothing was changed");
    }
    replace_db(&db, &snapshot, &backups, ts)?;
    println!(
        "restored {} (schema {}); the database that was current is at {}",
        db.display(),
        rec.schema_from,
        backups.join(format!("rolled-back-{ts}.db")).display()
    );
    Ok(())
}

/// The recorded snapshot exists and is a SQLite database.
fn check_snapshot(snapshot: &Path) -> Result<()> {
    let mut head = [0u8; 16];
    let read = File::open(snapshot).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head));
    match read {
        Ok(()) if &head == SQLITE_HEADER => Ok(()),
        Ok(()) => bail!(
            "the snapshot {} is not a SQLite database; nothing was changed",
            snapshot.display()
        ),
        Err(e) => Err(e).with_context(|| {
            format!(
                "cannot read the snapshot {} that {UPGRADE_FILE} names; nothing was changed",
                snapshot.display()
            )
        }),
    }
}

/// M4-4 fix round 2 (B1): `--restore-snapshot` does not start while an earlier run's
/// leftovers are there, the states the store's open refuses ([`restore_tmps`]) and a
/// `backups/rolled-back-<ts>.db-wal`/`-shm` without its `rolled-back-<ts>.db` (or whose
/// `.db` is a second name of `db`). A new run would move the current database aside and put
/// the snapshot in place, and the old run's moved WAL would then sit beside a database it
/// does not belong to, where the recovery advice would replay it into the snapshot.
fn refuse_leftovers(db: &Path, backups: &Path) -> Result<()> {
    const AGAIN: &str = "`rollback --restore-snapshot` does not start again until that is \
                         done (a new run would put the snapshot beside files of another \
                         database); nothing was changed";
    match restore_tmps(db, backups) {
        RestoreTmps::None => {}
        RestoreTmps::Interrupted(why) => bail!("{why}. {AGAIN}"),
        RestoreTmps::Leftover(found) => {
            let advice: Vec<String> = found.iter().map(Leftover::advice).collect();
            bail!("{}. {AGAIN}", advice.join("; "));
        }
    }
    let mut stray = Vec::new();
    for ts in side_file_runs(backups) {
        let aside_db = backups.join(format!("rolled-back-{ts}.db"));
        if std::fs::symlink_metadata(&aside_db).is_ok() && !same_file(&aside_db, db) {
            // A run that moved the database too (finished, or undone): its files are kept.
            continue;
        }
        for suffix in ["-wal", "-shm"] {
            let aside = with_suffix(&aside_db, suffix);
            if std::fs::symlink_metadata(&aside).is_err() {
                continue;
            }
            let original = with_suffix(db, suffix);
            if same_file(&aside, &original) {
                stray.push(format!(
                    "`rm {}` (a second name of {})",
                    aside.display(),
                    original.display()
                ));
            } else {
                stray.push(format!(
                    "{} has no {} beside it: a run that failed while putting files back left \
                     it (its error named them). If {} is missing and that error said this \
                     file is its WAL, `mv {} {}`; if it belongs to no database in place, \
                     delete it",
                    aside.display(),
                    aside_db.display(),
                    original.display(),
                    aside.display(),
                    original.display()
                ));
            }
        }
        if same_file(&aside_db, db) {
            stray.push(format!(
                "`rm {}` (a second name of {})",
                aside_db.display(),
                db.display()
            ));
        }
    }
    if !stray.is_empty() {
        bail!(
            "an earlier `rollback --restore-snapshot` left files in {}: {}. {AGAIN}",
            backups.display(),
            stray.join("; ")
        );
    }
    Ok(())
}

/// The `<ts>` of every `rolled-back-<ts>.db-wal`/`-shm` in `backups`, sorted, once each.
fn side_file_runs(backups: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(backups) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let rest = name.to_str()?.strip_prefix("rolled-back-")?;
            let ts = rest
                .strip_suffix(".db-wal")
                .or_else(|| rest.strip_suffix(".db-shm"))?;
            (!ts.is_empty()).then(|| ts.to_owned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// P73: the snapshot holds the schema the upgrade started at (`rec.schema_from`). A stale
/// file at another schema (a record finalised late, a carried snapshot) is refused, not
/// restored. Read-only: nothing is created beside the snapshot.
async fn check_snapshot_schema(
    snapshot: &Path,
    rec: &UpgradeRecord,
    me: &semver::Version,
) -> Result<()> {
    let read = async {
        let db = crate::open::SqliteReadOnly::open(snapshot).await?;
        db.migration_status(me).await
    };
    let status = read.await.with_context(|| {
        format!(
            "cannot read the schema of the snapshot {}; nothing was changed",
            snapshot.display()
        )
    })?;
    if status.db_version != rec.schema_from {
        bail!(
            "the snapshot {} is at schema {}, but {UPGRADE_FILE} says the upgrade started at \
             schema {}: it is not this upgrade's snapshot, so it is not restored (roll back \
             with `rollback --schema` without --restore-snapshot, or restore a backup); \
             nothing was changed",
            snapshot.display(),
            status.db_version,
            rec.schema_from
        );
    }
    Ok(())
}

/// P72: refuses while the Postgres node registry shows a node whose heartbeat is fresher
/// than [`ferrite_cluster::STALE_AFTER`] (by the database clock): on Postgres no folder
/// lock keeps a running node out of the down migrations. A cluster whose nodes are all
/// stopped is fine.
async fn refuse_live_nodes(store: &dyn Store) -> Result<()> {
    let nodes = store.read().nodes();
    let read = async { anyhow::Ok((nodes.list().await?, nodes.db_now().await?)) };
    let (all, now) = read
        .await
        .context("cannot read the node registry to check that no node runs; nothing was changed")?;
    let live: Vec<String> = all
        .iter()
        .filter(|n| ferrite_cluster::is_live(now, n.heartbeat_at, ferrite_cluster::STALE_AFTER))
        .map(|n| format!("{} (ferrite {})", n.name, n.version))
        .collect();
    if !live.is_empty() {
        bail!(
            "a node still runs on this database: {}. Stop every node first (a clean stop \
             leaves the registry at once; after a crash wait {} s for its heartbeat to go \
             stale), then retry; nothing was changed",
            live.join(", "),
            ferrite_cluster::STALE_AFTER.as_secs()
        );
    }
    Ok(())
}

/// Takes `<data>/ferrite.lock` (the lock `serve` holds) without waiting.
fn lock_folder(data_dir: &Path) -> Result<File> {
    let path = data_dir.join(LOCK_FILE);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => bail!(
            "data folder is in use: {} is locked by another process (stop `ferrite serve` \
             first); nothing was changed",
            path.display()
        ),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("cannot lock {}", path.display()))
        }
    }
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

/// The prefix and suffix of [`restore_tmp`]'s file name after the database's own name.
const RESTORE_TMP_PREFIX: &str = ".restore-";
const RESTORE_TMP_SUFFIX: &str = ".tmp";

/// `<db>.restore-<ts>.tmp`: the copy of the snapshot [`replace_db`] stages beside `db`
/// before renaming it into place (P70: opening the store looks for it).
pub(crate) fn restore_tmp(db: &Path, ts: &str) -> PathBuf {
    with_suffix(db, &format!("{RESTORE_TMP_PREFIX}{ts}{RESTORE_TMP_SUFFIX}"))
}

/// Every [`restore_tmp`] beside `db` with its `<ts>`, oldest first (an unreadable folder
/// has none).
pub(crate) fn leftover_restore_tmps(db: &Path) -> Vec<(PathBuf, String)> {
    let (Some(dir), Some(name)) = (db.parent(), db.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let prefix = format!("{name}{RESTORE_TMP_PREFIX}");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, String)> = entries
        .flatten()
        .filter_map(|e| {
            let file = e.file_name();
            let ts = file
                .to_str()?
                .strip_prefix(&prefix)?
                .strip_suffix(RESTORE_TMP_SUFFIX)?
                .to_owned();
            (!ts.is_empty()).then(|| (restore_tmp(db, &ts), ts))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// P70: refuses to open a SQLite database that is missing while a [`restore_tmp`] is left
/// beside it (a `rollback --restore-snapshot` killed after moving the database aside and
/// before renaming the copy into place), so no empty database is created in its place.
/// With the database present it also refuses when the run had already moved its `-wal` or
/// `-shm` (M4-4 R-c2, [`moved_side_files`]): opened without its WAL, committed transactions
/// would be lost. A leftover that moved nothing only warns.
pub(crate) fn check_restore_tmps(db: &Path, backups: &Path) -> Result<()> {
    match restore_tmps(db, backups) {
        RestoreTmps::None => Ok(()),
        RestoreTmps::Leftover(found) => {
            for leftover in found {
                tracing::warn!(
                    "{} is in place and is used; {}",
                    db.display(),
                    leftover.advice()
                );
            }
            Ok(())
        }
        RestoreTmps::Interrupted(why) => bail!("{why}"),
    }
}

/// What [`restore_tmp`]s beside `db` mean (P70, R-c2): read by the store's open
/// ([`check_restore_tmps`]) and by `ferrite doctor`'s `fs.database_files`.
pub(crate) enum RestoreTmps {
    /// None is left.
    None,
    /// Left by runs that moved nothing away from `db`: it is complete. Delete them.
    Leftover(Vec<Leftover>),
    /// A run stopped halfway: opening `db` would create an empty site or lose its WAL.
    /// The text names the files and the commands that recover them.
    Interrupted(String),
}

/// A [`restore_tmp`] left by a run that moved nothing away from the database.
pub(crate) struct Leftover {
    pub(crate) tmp: PathBuf,
    /// `rm` steps for second names a kill inside a move left in `backups/` (Minor A).
    pub(crate) steps: Vec<String>,
}

impl Leftover {
    /// What to do about it, for the open's warning and doctor.
    pub(crate) fn advice(&self) -> String {
        let also = if self.steps.is_empty() {
            String::new()
        } else {
            format!(
                ", and remove the second names of the files in place it left in backups/: {}",
                self.steps.join("; ")
            )
        };
        format!(
            "{} is left from an interrupted `ferrite rollback --restore-snapshot` that moved \
             nothing away: delete it{also}",
            self.tmp.display()
        )
    }
}

pub(crate) fn restore_tmps(db: &Path, backups: &Path) -> RestoreTmps {
    let leftovers = leftover_restore_tmps(db);
    let Some((tmp, ts)) = leftovers.last() else {
        return RestoreTmps::None;
    };
    if std::fs::symlink_metadata(db).is_ok() {
        let mut found = Vec::new();
        for (tmp, ts) in &leftovers {
            let side = moved_side_files(db, backups, ts);
            if side.moved {
                let steps = side.steps;
                return RestoreTmps::Interrupted(format!(
                    "{} is in place, but {} is left from an interrupted `ferrite rollback \
                     --restore-snapshot` that had already moved the database's -wal/-shm into \
                     {}: opened without its WAL, the transactions committed there would be \
                     lost. Put the files back first: {}; then delete {}. Nothing was opened",
                    db.display(),
                    tmp.display(),
                    backups.display(),
                    steps.join("; "),
                    tmp.display()
                ));
            }
            found.push(Leftover {
                tmp: tmp.clone(),
                steps: side.steps,
            });
        }
        return RestoreTmps::Leftover(found);
    }
    let aside = backups.join(format!("rolled-back-{ts}.db"));
    let back = if aside.is_file() {
        let mut moves = vec![format!("`mv {} {}`", aside.display(), db.display())];
        for suffix in ["-wal", "-shm"] {
            let side = with_suffix(&aside, suffix);
            if std::fs::symlink_metadata(&side).is_ok() {
                moves.push(format!(
                    "`mv {} {}`",
                    side.display(),
                    with_suffix(db, suffix).display()
                ));
            }
        }
        format!(
            "; or, to keep the database that was current instead, {} and delete {}",
            moves.join(", "),
            tmp.display()
        )
    } else {
        String::new()
    };
    RestoreTmps::Interrupted(format!(
        "{} is missing and {} is left from an interrupted `ferrite rollback \
         --restore-snapshot` (the snapshot's copy was staged but not renamed into place). \
         Finish the restore with `mv {} {}`{back}; nothing was created",
        db.display(),
        tmp.display(),
        tmp.display(),
        db.display()
    ))
}

/// M4-4 R-c2 (amends P70): what a [`replace_db`] killed before `db` itself left
/// `backups/rolled-back-<ts>.db`: its `-wal` and `-shm` move first, so with `db` still in
/// place (`rolled-back-<ts>.db` absent, or the same file as `db` when the kill came inside
/// that move) a moved `-wal`/`-shm` means `db` lacks its WAL. Returns the shell steps that
/// put the files back, or `None` when nothing was moved away from `db`.
/// Second names a kill inside a move left (`rolled-back-<ts>.db*` that are the same file as
/// the one in place) only need an `rm`, even when nothing was moved away (review Minor A).
fn moved_side_files(db: &Path, backups: &Path, ts: &str) -> SideFiles {
    let aside_db = backups.join(format!("rolled-back-{ts}.db"));
    let db_linked = same_file(&aside_db, db);
    if std::fs::symlink_metadata(&aside_db).is_ok() && !db_linked {
        // `db` itself was moved; whatever is in place now is not this run's to judge.
        return SideFiles::default();
    }
    let mut steps = Vec::new();
    let mut moved = false;
    for suffix in ["-wal", "-shm"] {
        let aside = with_suffix(&aside_db, suffix);
        if std::fs::symlink_metadata(&aside).is_err() {
            continue;
        }
        let original = with_suffix(db, suffix);
        if std::fs::symlink_metadata(&original).is_err() {
            moved = true;
            steps.push(format!("`mv {} {}`", aside.display(), original.display()));
        } else if same_file(&aside, &original) {
            // A kill inside that move: the link is a second name of the file in place.
            steps.push(format!("`rm {}`", aside.display()));
        } else {
            moved = true;
            steps.push(format!(
                "{} and {} both exist and differ (the database was opened since the move): \
                 compare them by hand before going on",
                original.display(),
                aside.display()
            ));
        }
    }
    if db_linked {
        steps.push(format!("`rm {}`", aside_db.display()));
    }
    SideFiles { moved, steps }
}

/// What [`moved_side_files`] found for one run.
#[derive(Debug, Default)]
struct SideFiles {
    /// A `-wal`/`-shm` is no longer beside `db`: opening it would lose its WAL.
    moved: bool,
    /// The shell steps that put things back (`mv`), or remove second names (`rm`).
    steps: Vec<String>,
}

/// Whether `a` and `b` are the same file (device and inode); `false` when either is
/// missing, and on platforms without inodes.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

/// Moves `src` to `dst` without ever replacing an existing `dst`: a hard link (which
/// fails with `AlreadyExists` when `dst` exists) and then the removal of `src`.
fn move_no_clobber(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::hard_link(src, dst)?;
    if let Err(e) = std::fs::remove_file(src) {
        // Leave only the original name.
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    Ok(())
}

/// Flushes a folder's entries (best effort: not every platform can open a folder).
fn sync_dir(dir: &Path) {
    if let Ok(f) = File::open(dir) {
        let _ = f.sync_all();
    }
}

/// Moves `db`'s `-wal`, `-shm` and then `db` itself to `backups/rolled-back-<ts>.db…`
/// (never over an existing file), then renames a copy of `snapshot` (made first, beside
/// `db`) into place. The WAL moves first: a WAL left beside another database file could be
/// applied to it. A `-journal` beside `db` refuses (a hot rollback journal would be rolled
/// back into the snapshot's pages). On unix the copy gets `db`'s owner, group and mode
/// first (M4-2 R11: a root-run rollback must leave a database the service user can open).
/// On a failure the moved files are put back.
fn replace_db(db: &Path, snapshot: &Path, backups: &Path, ts: &str) -> Result<()> {
    let journal = with_suffix(db, "-journal");
    if std::fs::symlink_metadata(&journal).is_ok() {
        bail!(
            "{} exists (the rollback journal of an interrupted write): run `ferrite migrate \
             status` once with the binary that wrote it so SQLite recovers it, then retry; \
             nothing was changed",
            journal.display()
        );
    }
    std::fs::create_dir_all(backups)
        .with_context(|| format!("cannot create {}", backups.display()))?;
    let mut staged = Staged {
        path: restore_tmp(db, ts),
        done: false,
    };
    std::fs::copy(snapshot, &staged.path).with_context(|| {
        format!(
            "cannot copy {} to {}; nothing was changed",
            snapshot.display(),
            staged.path.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta =
            std::fs::metadata(db).with_context(|| format!("cannot read {}", db.display()))?;
        let kept = match_owner_and_mode(&staged.path, meta.uid(), meta.gid(), meta.mode())
            .with_context(|| {
                format!(
                    "cannot give {} the owner and mode of {} (run as root or as the \
                     database's owner); nothing was changed",
                    staged.path.display(),
                    db.display()
                )
            })?;
        if let Some(own) = kept {
            eprintln!(
                "warning: {} has group {}, which this user cannot give the restored copy: it \
                 keeps group {own}; run `chgrp {} {}` as root afterwards if the service reads \
                 the database through its group",
                db.display(),
                meta.gid(),
                meta.gid(),
                db.display()
            );
        }
    }
    File::open(&staged.path)
        .and_then(|f| f.sync_all())
        .with_context(|| format!("cannot flush {}", staged.path.display()))?;

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Puts the moved files back and says what state that left.
    let undo = |moved: &[(PathBuf, PathBuf)]| {
        let mut ok = true;
        for (src, dst) in moved.iter().rev() {
            if let Err(e) = move_no_clobber(dst, src) {
                ok = false;
                eprintln!(
                    "error: cannot move {} back to {}: {e}; move it by hand",
                    dst.display(),
                    src.display()
                );
            }
        }
        if ok {
            "nothing was changed".to_owned()
        } else {
            format!(
                "putting the files back failed too: the database that was current is in {} \
                 (see the errors above)",
                backups.display()
            )
        }
    };
    for suffix in ["-wal", "-shm", ""] {
        let src = with_suffix(db, suffix);
        if !suffix.is_empty() && std::fs::symlink_metadata(&src).is_err() {
            continue;
        }
        let dst = backups.join(format!("rolled-back-{ts}.db{suffix}"));
        if let Err(e) = move_no_clobber(&src, &dst) {
            let state = undo(&moved);
            if e.kind() == std::io::ErrorKind::CrossesDevices {
                bail!(
                    "cannot move {} to {}: they are on different filesystems, and \
                     --restore-snapshot needs the database on the data folder's filesystem \
                     (otherwise move the database files aside and copy {} into place by hand, \
                     with the site stopped); {state}",
                    src.display(),
                    dst.display(),
                    snapshot.display()
                );
            }
            return Err(e).with_context(|| {
                format!(
                    "cannot move {} to {}; {state}",
                    src.display(),
                    dst.display()
                )
            });
        }
        moved.push((src, dst));
    }
    if let Err(e) = std::fs::rename(&staged.path, db) {
        let state = undo(&moved);
        return Err(e).with_context(|| {
            format!(
                "cannot rename {} to {}; {state}",
                staged.path.display(),
                db.display()
            )
        });
    }
    staged.done = true;
    sync_dir(backups);
    if let Some(dir) = db.parent() {
        sync_dir(dir);
    }
    Ok(())
}

/// What [`match_owner_and_mode`] changes of a file owned by `current` (uid, gid) to give
/// it `want`.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerPlan {
    /// Owner and group match.
    Keep,
    /// The owner differs: a `chown` (only root may).
    Chown,
    /// Only the group differs: a `chgrp`, which the owner may do for a group it is in.
    GroupOnly,
}

#[cfg(unix)]
fn owner_plan(current: (u32, u32), want: (u32, u32)) -> OwnerPlan {
    if current == want {
        OwnerPlan::Keep
    } else if current.0 == want.0 {
        OwnerPlan::GroupOnly
    } else {
        OwnerPlan::Chown
    }
}

/// Gives `path` the owner `uid`:`gid` (a `chown` only when it differs, so a non-root run
/// on its own files needs none) and the permission bits of `mode`. A group change that is
/// refused (the owner is not in that group, P73) keeps the file's own group, which is
/// returned.
#[cfg(unix)]
fn match_owner_and_mode(
    path: &Path,
    uid: u32,
    gid: u32,
    mode: u32,
) -> std::io::Result<Option<u32>> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let current = std::fs::metadata(path)?;
    let mut kept = None;
    // Before the mode: a chown may clear set-id bits.
    match owner_plan((current.uid(), current.gid()), (uid, gid)) {
        OwnerPlan::Keep => {}
        OwnerPlan::Chown => std::os::unix::fs::chown(path, Some(uid), Some(gid))?,
        OwnerPlan::GroupOnly => match std::os::unix::fs::chown(path, None, Some(gid)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                kept = Some(current.gid());
            }
            Err(e) => return Err(e),
        },
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))?;
    Ok(kept)
}

/// The staged copy of the snapshot, removed on drop unless renamed into place.
struct Staged {
    path: PathBuf,
    done: bool,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Asks on a terminal; without one `--yes` is required.
fn confirm() -> Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("--restore-snapshot needs --yes when stdin is not a terminal; nothing was changed");
    }
    eprint!(
        "Restore the snapshot and lose everything written since the upgrade? Type 'yes' to continue: "
    );
    std::io::stderr().flush()?;
    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

fn print_restart() {
    println!(
        "restart to run it: `systemctl restart ferrite` under systemd, otherwise stop and \
         start `ferrite serve`"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_from_either_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = tmp.path().join("ferrite");
        let prev = tmp.path().join("ferrite.prev");
        std::fs::write(&installed, "b").unwrap();
        std::fs::write(&prev, "a").unwrap();
        let expected = |running_prev| Binaries {
            installed: installed.clone(),
            prev: prev.clone(),
            running_prev,
        };
        assert_eq!(Binaries::locate(&installed), expected(false));
        assert_eq!(Binaries::locate(&prev), expected(true));
        // A `.prev` with no installed binary beside it is the installed one.
        let lone = tmp.path().join("other.prev");
        assert_eq!(
            Binaries::locate(&lone),
            Binaries {
                installed: lone.clone(),
                prev: tmp.path().join("other.prev.prev"),
                running_prev: false,
            }
        );
    }

    /// Pins the store's real refusal (`plan_down`), not a copy of its text.
    #[test]
    fn lossy_refusal_is_recognised() {
        let lossy =
            ferrite_store::parse_directives("-- ferrite:lossy-down\nDROP TABLE x;").unwrap();
        let plain = ferrite_store::parse_directives("SELECT 1;").unwrap();
        let err =
            ferrite_store::plan_down(&[1, 2], &[(1, plain), (2, lossy)], 0, false).unwrap_err();
        assert!(is_lossy_refusal(&anyhow::Error::new(err)));
        let unknown = ferrite_store::plan_down(&[1, 9], &[], 0, true).unwrap_err();
        assert!(!is_lossy_refusal(&anyhow::Error::new(unknown)));
    }

    #[test]
    fn move_no_clobber_refuses_an_existing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let e = move_no_clobber(&a, &b).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        std::fs::remove_file(&b).unwrap();
        move_no_clobber(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "a");
    }

    #[test]
    fn replace_db_refuses_a_rollback_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("ferrite.db");
        let snap = tmp.path().join("snap.db");
        std::fs::write(&db, "current").unwrap();
        std::fs::write(with_suffix(&db, "-journal"), "hot").unwrap();
        std::fs::write(&snap, "snapshot").unwrap();
        let e = replace_db(&db, &snap, &tmp.path().join("backups"), "T").unwrap_err();
        assert!(format!("{e:#}").contains("-journal"), "{e:#}");
        assert_eq!(std::fs::read_to_string(&db).unwrap(), "current");
        assert!(!tmp.path().join("backups").exists());
    }

    /// The staged copy gets the replaced database's mode and owner (M4-2 R11): its own
    /// owner needs no `chown`; another owner is a `chown` (refused unless root).
    #[cfg(unix)]
    #[test]
    fn staged_copy_takes_the_owner_and_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("staged");
        std::fs::write(&f, "x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let me = std::fs::metadata(&f).unwrap();
        match_owner_and_mode(&f, me.uid(), me.gid(), 0o100_640).unwrap();
        let after = std::fs::metadata(&f).unwrap();
        assert_eq!(after.mode() & 0o7777, 0o640);
        assert_eq!((after.uid(), after.gid()), (me.uid(), me.gid()));
        if me.uid() != 0 {
            // Another owner: the chown is attempted, and refused to a non-root user.
            let e = match_owner_and_mode(&f, 0, 0, 0o100_600).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
        }
    }

    /// `replace_db` gives the restored file the current database's mode.
    #[cfg(unix)]
    #[test]
    fn replace_db_keeps_the_mode_of_the_replaced_database() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("ferrite.db");
        let snap = tmp.path().join("snap.db");
        std::fs::write(&db, "current").unwrap();
        std::fs::write(&snap, "snapshot").unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&snap, std::fs::Permissions::from_mode(0o644)).unwrap();
        replace_db(&db, &snap, &tmp.path().join("backups"), "T").unwrap();
        assert_eq!(std::fs::read_to_string(&db).unwrap(), "snapshot");
        assert_eq!(std::fs::metadata(&db).unwrap().mode() & 0o7777, 0o600);
    }

    /// P73: only a group that differs is a `chgrp`, which a non-root owner may be refused
    /// (and then keeps its own group with a warning); a different owner needs a `chown`.
    /// The refused `chgrp` itself needs a group the test user is not in and the owner of
    /// the file, so only this decision is tested here.
    #[cfg(unix)]
    #[test]
    fn owner_plan_tells_a_group_change_from_an_owner_change() {
        assert_eq!(owner_plan((1000, 100), (1000, 100)), OwnerPlan::Keep);
        assert_eq!(owner_plan((1000, 100), (1000, 200)), OwnerPlan::GroupOnly);
        assert_eq!(owner_plan((0, 0), (1000, 100)), OwnerPlan::Chown);
        assert_eq!(owner_plan((0, 100), (1000, 100)), OwnerPlan::Chown);
    }

    /// P70: the staged copies beside `db` are found by the name `replace_db` gives them.
    #[test]
    fn restore_tmps_are_found_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("ferrite.db");
        assert!(leftover_restore_tmps(&db).is_empty());
        for name in [
            "ferrite.db.restore-20260102T000000Z.tmp",
            "ferrite.db.restore-20260101T000000Z.tmp",
            "ferrite.db.restore-.tmp",
            "ferrite.db-wal",
            "other.db.restore-20260101T000000Z.tmp",
        ] {
            std::fs::write(tmp.path().join(name), "x").unwrap();
        }
        assert_eq!(
            leftover_restore_tmps(&db),
            vec![
                (
                    restore_tmp(&db, "20260101T000000Z"),
                    "20260101T000000Z".to_owned()
                ),
                (
                    restore_tmp(&db, "20260102T000000Z"),
                    "20260102T000000Z".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn replace_db_moves_aside_and_puts_back_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("ferrite.db");
        let snap = tmp.path().join("snap.db");
        let backups = tmp.path().join("backups");
        std::fs::write(&db, "current").unwrap();
        std::fs::write(with_suffix(&db, "-wal"), "wal").unwrap();
        std::fs::write(&snap, "snapshot").unwrap();
        // The target name is taken: nothing moves, nothing is left behind.
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("rolled-back-T.db"), "taken").unwrap();
        replace_db(&db, &snap, &backups, "T").unwrap_err();
        assert_eq!(std::fs::read_to_string(&db).unwrap(), "current");
        assert_eq!(
            std::fs::read_to_string(with_suffix(&db, "-wal")).unwrap(),
            "wal"
        );
        assert!(!with_suffix(&db, ".restore-T.tmp").exists());

        replace_db(&db, &snap, &backups, "U").unwrap();
        assert_eq!(std::fs::read_to_string(&db).unwrap(), "snapshot");
        assert_eq!(
            std::fs::read_to_string(backups.join("rolled-back-U.db")).unwrap(),
            "current"
        );
        assert_eq!(
            std::fs::read_to_string(backups.join("rolled-back-U.db-wal")).unwrap(),
            "wal"
        );
        assert!(!with_suffix(&db, "-wal").exists());
        assert!(!backups.join("rolled-back-U.db-shm").exists());
        assert_eq!(std::fs::read_to_string(&snap).unwrap(), "snapshot");
    }
}
