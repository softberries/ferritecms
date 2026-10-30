//! `<data>/upgrade.json`: what the last binary or schema change on this data folder was
//! (09 §1, M4-2 P30). `ferrite rollback` reads it.
//!
//! The startup check (`serve`, and every other command that migrates on start) and
//! `ferrite migrate up` call [`migrate_and_record`] under the migrate lock. It compares
//! `schema_meta.last_binary_version` with this binary and looks at what `migrate_up`
//! applied; when either changed it replaces the file, then stores this binary's version.
//! A database that had no schema before (a new site, `ferrite init`) gets the version but
//! no file: there is nothing to roll back to. The rules are in [`migrate_and_record`].

use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, bail};
use ferrite_domain::Timestamp;
use ferrite_store::{MigrateReport, SCHEMA_META_LAST_BINARY, Store};
pub use ferrite_store_sqlite::KeepSnapshot;
use serde::{Deserialize, Serialize};

/// The file's name inside the data folder.
pub const UPGRADE_FILE: &str = "upgrade.json";

/// The format version written in `v`.
pub const UPGRADE_FORMAT: u32 = 1;

/// The contents of `upgrade.json`:
/// `{"v":1,"from":"<prev binary or null>","to":"<this binary>","schema_from":N,
/// "schema_to":M,"snapshot":"<relative path or null>","at":"<RFC 3339>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeRecord {
    /// Format version ([`UPGRADE_FORMAT`]).
    pub v: u32,
    /// The binary that started on this database before (`schema_meta.last_binary_version`);
    /// `None` when none was stored (a database from before M4-2).
    pub from: Option<String>,
    /// This binary's version.
    pub to: String,
    /// The schema version before this start (or `migrate up`).
    pub schema_from: i64,
    /// The schema version after it.
    pub schema_to: i64,
    /// SQLite's pre-migration snapshot, relative to the data folder (absolute only when
    /// the backups folder lies outside it); `None` when nothing was migrated or on Postgres.
    pub snapshot: Option<PathBuf>,
    /// When the change happened.
    pub at: Timestamp,
}

/// Reads `<data_dir>/upgrade.json`; `None` when there is none. Strict: an unreadable file
/// or another format version is an error (`ferrite rollback` refuses on it; the startup
/// path only warns, see [`keep_snapshot`]).
pub fn load(data_dir: &Path) -> anyhow::Result<Option<UpgradeRecord>> {
    let path = data_dir.join(UPGRADE_FILE);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let rec: UpgradeRecord = serde_json::from_slice(&raw)
        .with_context(|| format!("{} is not a valid upgrade record", path.display()))?;
    if rec.v != UPGRADE_FORMAT {
        bail!(
            "{} has format version {}; this binary reads version {UPGRADE_FORMAT}",
            path.display(),
            rec.v
        );
    }
    Ok(Some(rec))
}

/// Temp files of [`write`] older than this are swept by the next write (a crash between
/// creating and renaming one leaves it behind).
const STALE_TEMP: Duration = Duration::from_secs(24 * 3600);
const TEMP_PREFIX: &str = ".upgrade.json.";
const TEMP_SUFFIX: &str = ".tmp";

/// Replaces `<data_dir>/upgrade.json` atomically: a temp file in the same folder, synced,
/// then renamed over the old one, so a reader sees the old or the new record, never a
/// partial one. Also removes temp files of earlier writes older than a day.
pub fn write(data_dir: &Path, rec: &UpgradeRecord) -> anyhow::Result<()> {
    let path = data_dir.join(UPGRADE_FILE);
    sweep_stale_temps(data_dir);
    let mut json = serde_json::to_vec_pretty(rec)?;
    json.push(b'\n');
    let mut tmp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .suffix(TEMP_SUFFIX)
        .tempfile_in(data_dir)
        .with_context(|| format!("cannot create a temp file in {}", data_dir.display()))?;
    tmp.write_all(&json)
        .and_then(|()| tmp.as_file().sync_all())
        .with_context(|| format!("cannot write {}", tmp.path().display()))?;
    tmp.persist(&path)
        .map_err(|e| e.error)
        .with_context(|| format!("cannot write {}", path.display()))?;
    // Makes the rename itself durable.
    #[cfg(unix)]
    std::fs::File::open(data_dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("cannot sync {}", data_dir.display()))?;
    Ok(())
}

/// Best effort: a temp file that cannot be listed or removed is left for the next write.
fn sweep_stale_temps(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX)) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_TEMP);
        if old && let Err(e) = std::fs::remove_file(entry.path()) {
            tracing::warn!(path = %entry.path().display(), error = %e, "cannot remove a stale temp file");
        }
    }
}

/// What the store's snapshot pruning must spare, from `upgrade.json`: the snapshot it
/// names; for a provisional record (a move that has not finished: `snapshot` null,
/// `schema_from < schema_to`) the snapshot its move took, found by name (M4-2 R9: a crash
/// loop on a partly applied upgrade takes a new snapshot on every start and must not
/// prune the one from before the move; this is for pruning only, `rollback
/// --restore-snapshot` never resolves a snapshot by name); nothing when there is no file
/// or no such snapshot; every snapshot when the file cannot be read (a stray edit, or a
/// newer binary's format after a binary rollback), with a warning: keeping all is the
/// safe direction, and the site still opens.
pub fn keep_snapshot(data_dir: &Path) -> Option<KeepSnapshot> {
    match load(data_dir) {
        Ok(rec) => rec
            .and_then(|r| match r.snapshot {
                Some(s) => Some(s),
                None if r.schema_from < r.schema_to => {
                    find_snapshot(data_dir, r.schema_from, r.schema_to)
                }
                None => None,
            })
            .map(|s| KeepSnapshot::Keep(data_dir.join(s))),
        Err(e) => {
            tracing::warn!(
                error = format!("{e:#}"),
                "keeping every pre-migration snapshot: {UPGRADE_FILE} cannot be read"
            );
            Some(KeepSnapshot::KeepAll)
        }
    }
}

/// Whether [`migrate_and_record`] migrates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migrate {
    /// Records only (the schema is current, or newer and compatible).
    No,
    /// `migrate_up(to)` first.
    UpTo(Option<i64>),
}

/// Migrates (per `migrate`) and records the start of `binary`: `upgrade.json` and
/// `schema_meta.last_binary_version` (M4-2 P30, 09 §1). Call it under the migrate lock.
/// Returns the migrate report and the record written, if any.
///
/// Before migrating it writes a provisional record (`schema_to` = the target, `snapshot`
/// null); after migrating it finalises it with the snapshot. A start that dies in
/// between leaves the provisional record, and the next start (the same move, nothing left
/// to apply) finalises it, finding the snapshot by its name
/// `backups/pre-<schema_from>-to-<schema_to>-*.db`.
///
/// When `migrate_up` fails and the schema did not move, the record `begin` replaced is
/// written back (or the provisional one removed when there was none): a failed start
/// changes nothing. When it moved partway, the provisional record stays and the next
/// start carries its start forward.
pub async fn migrate_and_record(
    store: &dyn Store,
    data_dir: &Path,
    binary: &semver::Version,
    migrate: Migrate,
) -> anyhow::Result<(MigrateReport, Option<UpgradeRecord>)> {
    let status = store.migration_status(binary).await?;
    let before = status.db_version;
    let mut report = MigrateReport::default();
    let mut carry = None;
    if let Migrate::UpTo(to) = migrate {
        let target = status
            .pending
            .iter()
            .map(|p| p.version)
            .filter(|v| to.is_none_or(|t| *v <= t))
            .max();
        let begun = match target {
            Some(target) => begin(store, data_dir, binary, before, target).await?,
            None => None,
        };
        report = match store.migrate_up(to).await {
            Ok(report) => report,
            Err(e) => {
                if let Some(begun) = begun {
                    undo_begin(store, data_dir, binary, before, begun).await;
                }
                return Err(e.into());
            }
        };
        #[cfg(feature = "test-seams")]
        seams::check_fail_after_migrate()?;
        carry = begun.and_then(|b| b.carry);
    }
    let written = finish(store, data_dir, binary, before, &report, carry).await?;
    Ok((report, written))
}

/// What [`begin`] did: the record it replaced, and the start of an unfinished earlier move
/// it carried into the provisional record.
struct Begun {
    previous: Option<UpgradeRecord>,
    carry: Option<Carry>,
}

/// The start of an unfinished earlier move: its `schema_from` and snapshot.
#[derive(Debug, Clone)]
struct Carry {
    schema_from: i64,
    snapshot: Option<PathBuf>,
}

/// After a failed `migrate_up`: if the schema did not move, puts back what [`begin`]
/// replaced (or removes the provisional record), with the snapshot of a carried start
/// filled in. Best effort: the migrate error is what the caller reports.
async fn undo_begin(
    store: &dyn Store,
    data_dir: &Path,
    binary: &semver::Version,
    before: i64,
    begun: Begun,
) {
    match store.migration_status(binary).await {
        Ok(s) if s.db_version == before => {}
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "cannot tell whether the failed migration moved the schema; {UPGRADE_FILE} left provisional");
            return;
        }
    }
    // A carried start keeps the snapshot `begin` found for it (M4-2 R9): the record put
    // back is the unfinished move it came from, now naming its snapshot, so the next
    // start's pruning spares it even if that snapshot is no longer findable by name.
    let previous = begun
        .previous
        .map(|prev| match (&prev.snapshot, begun.carry) {
            (
                None,
                Some(Carry {
                    schema_from,
                    snapshot: Some(s),
                }),
            ) if schema_from == prev.schema_from => UpgradeRecord {
                snapshot: Some(s),
                ..prev
            },
            _ => prev,
        });
    let result = match &previous {
        Some(prev) => write(data_dir, prev),
        None => match std::fs::remove_file(data_dir.join(UPGRADE_FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        },
    };
    if let Err(e) = result {
        tracing::warn!(
            error = format!("{e:#}"),
            "cannot restore {UPGRADE_FILE} after the failed migration"
        );
    }
}

/// The record names this move: same target binary, same previous one.
fn same_move(rec: &UpgradeRecord, to: &str, stored: Option<&str>) -> bool {
    rec.to == to && rec.from.as_deref() == stored
}

/// An earlier attempt at this move stopped partway: the schema is past its start but
/// short of its target. Its `schema_from` (and snapshot) are the move's real start.
fn incomplete(rec: &UpgradeRecord, before: i64) -> bool {
    rec.schema_from < before && before < rec.schema_to
}

/// The provisional record, written before migrating from `before` (> 0) to `target`.
///
/// An unfinished earlier move is carried forward (its `schema_from` and snapshot become
/// this move's): an attempt at this same move that stopped partway, or a move from the
/// stored binary to another one that never stored its version (it died after migrating,
/// and now a third binary starts: A→B crashed, then C) whose record is unfinished
/// (`schema_to` beyond the current schema, or a provisional `snapshot: null`). A finished
/// A→B kept by a binary rollback to A is not carried into A→C.
async fn begin(
    store: &dyn Store,
    data_dir: &Path,
    binary: &semver::Version,
    before: i64,
    target: i64,
) -> anyhow::Result<Option<Begun>> {
    if before == 0 {
        return Ok(None);
    }
    let stored = store
        .read()
        .schema_meta()
        .get(SCHEMA_META_LAST_BINARY)
        .await?;
    let to = binary.to_string();
    let previous = load(data_dir).ok().flatten();
    let carry = previous.as_ref().and_then(|e| {
        let unfinished = if e.to == to {
            same_move(e, &to, stored.as_deref()) && incomplete(e, before)
        } else {
            // Only an unfinished record: one that stopped short of its target, or a
            // provisional one never finalised. A finished move kept by a binary rollback
            // (target reached, snapshot named) is not carried: the next move's own
            // snapshot is the one to restore.
            e.from.as_deref() == stored.as_deref()
                && e.schema_from < before
                && (e.schema_to > before || e.snapshot.is_none() && e.schema_from < e.schema_to)
        };
        unfinished.then(|| Carry {
            schema_from: e.schema_from,
            snapshot: e
                .snapshot
                .clone()
                .or_else(|| find_snapshot(data_dir, e.schema_from, e.schema_to)),
        })
    });
    replace(
        data_dir,
        &UpgradeRecord {
            v: UPGRADE_FORMAT,
            from: stored,
            to,
            schema_from: carry.as_ref().map_or(before, |c| c.schema_from),
            schema_to: target,
            snapshot: carry.as_ref().and_then(|c| c.snapshot.clone()),
            at: Timestamp::now(),
        },
    )?;
    Ok(Some(Begun { previous, carry }))
}

/// Writes the final record when something changed, then stores `binary` as the last
/// version. `before` is the schema before `migrate_up` ran.
///
/// - Migrations applied: the record of this move, with its snapshot; with `carry` (an
///   unfinished earlier move, see [`begin`]) its start and snapshot instead.
/// - Nothing applied, and the file already names this move (a retry after a start that
///   failed after migrating, or a re-upgrade after a binary rollback): kept, and a
///   provisional one gets its snapshot.
/// - Nothing applied, another binary: a binary-only record, unless this binary is older
///   than the stored one (a binary rollback): then the file is kept, so a schema
///   rollback after it, or the re-upgrade, still finds the move.
/// - A database that had no schema: no record (nothing to roll back to).
async fn finish(
    store: &dyn Store,
    data_dir: &Path,
    binary: &semver::Version,
    before: i64,
    report: &MigrateReport,
    carry: Option<Carry>,
) -> anyhow::Result<Option<UpgradeRecord>> {
    let after = store.migration_status(binary).await?.db_version;
    if after == 0 {
        // No schema_meta table to store the version in.
        return Ok(None);
    }
    let meta = store.read().schema_meta();
    let stored = meta.get(SCHEMA_META_LAST_BINARY).await?;
    let to = binary.to_string();
    let version_changed = stored.as_deref() != Some(to.as_str());
    let same = load(data_dir)
        .ok()
        .flatten()
        .filter(|e| same_move(e, &to, stored.as_deref()));
    let mut rec = None;
    if !report.applied.is_empty() {
        if before > 0 {
            // A carried start keeps its own snapshot (or none): a later one would not
            // match its `schema_from`.
            let (schema_from, snapshot) = match carry {
                Some(c) => (c.schema_from, c.snapshot),
                None => (
                    before,
                    report.snapshot.as_ref().map(|s| relative(data_dir, s)),
                ),
            };
            rec = Some(UpgradeRecord {
                v: UPGRADE_FORMAT,
                from: stored.clone(),
                to: to.clone(),
                schema_from,
                schema_to: after,
                snapshot,
                at: Timestamp::now(),
            });
        }
    } else if let Some(e) = same {
        if e.snapshot.is_none()
            && e.schema_from < e.schema_to
            && let Some(snapshot) = find_snapshot(data_dir, e.schema_from, e.schema_to)
        {
            rec = Some(UpgradeRecord {
                snapshot: Some(snapshot),
                ..e
            });
        }
    } else if version_changed && before > 0 {
        if is_downgrade(binary, stored.as_deref()) {
            tracing::info!(
                stored = stored.as_deref().unwrap_or_default(),
                this = %binary,
                "older binary started: {UPGRADE_FILE} kept for the re-upgrade or a schema rollback"
            );
        } else {
            rec = Some(UpgradeRecord {
                v: UPGRADE_FORMAT,
                from: stored.clone(),
                to: to.clone(),
                schema_from: before,
                schema_to: after,
                snapshot: None,
                at: Timestamp::now(),
            });
        }
    }
    if let Some(rec) = &rec {
        replace(data_dir, rec)?;
        tracing::info!(
            from = rec.from.as_deref().unwrap_or("unknown"),
            to = %rec.to,
            schema_from = rec.schema_from,
            schema_to = rec.schema_to,
            "wrote {UPGRADE_FILE}"
        );
    }
    if version_changed {
        meta.set(SCHEMA_META_LAST_BINARY, &to).await?;
    }
    Ok(rec)
}

/// `binary` is older than the stored last version (semver; an unparsable one is not).
fn is_downgrade(binary: &semver::Version, stored: Option<&str>) -> bool {
    stored
        .and_then(|s| semver::Version::parse(s).ok())
        .is_some_and(|s| *binary < s)
}

/// [`write`], first moving an unreadable `upgrade.json` aside to
/// `upgrade.json.invalid-<UTC yyyymmddThhmmssZ>` (never deleted).
fn replace(data_dir: &Path, rec: &UpgradeRecord) -> anyhow::Result<()> {
    let path = data_dir.join(UPGRADE_FILE);
    if path.exists() && load(data_dir).is_err() {
        let aside = data_dir.join(format!(
            "{UPGRADE_FILE}.invalid-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        ));
        std::fs::rename(&path, &aside).with_context(|| {
            format!(
                "cannot move {} aside to {}",
                path.display(),
                aside.display()
            )
        })?;
        tracing::warn!(path = %aside.display(), "moved an unreadable {UPGRADE_FILE} aside");
    }
    write(data_dir, rec)
}

/// The newest `backups/pre-<from>-to-<to>-*.db` (relative to `data_dir`), if any.
fn find_snapshot(data_dir: &Path, from: i64, to: i64) -> Option<PathBuf> {
    let prefix = format!("pre-{from}-to-{to}-");
    let dir = data_dir.join("backups");
    std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&prefix) && name.ends_with(".db")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.file_name())))
        .max()
        .map(|(_, name)| Path::new("backups").join(name))
}

/// Test seams (feature `test-seams`).
#[cfg(feature = "test-seams")]
pub mod seams {
    tokio::task_local! {
        static FAIL_AFTER_MIGRATE: ();
    }

    /// Runs `f` with [`super::migrate_and_record`] failing right after `migrate_up`, before
    /// the record is finalised: a start that dies in that window. Scoped to this task.
    pub async fn fail_after_migrate<F: std::future::Future>(f: F) -> F::Output {
        FAIL_AFTER_MIGRATE.scope((), f).await
    }

    pub(super) fn check_fail_after_migrate() -> anyhow::Result<()> {
        if FAIL_AFTER_MIGRATE.try_with(|()| ()).is_ok() {
            anyhow::bail!("test seam: failing after migrate_up");
        }
        Ok(())
    }
}

/// `path` relative to `data_dir` when it lies inside it (compared canonically).
fn relative(data_dir: &Path, path: &Path) -> PathBuf {
    if let Ok(rel) = path.strip_prefix(data_dir) {
        return rel.to_owned();
    }
    match (std::fs::canonicalize(data_dir), std::fs::canonicalize(path)) {
        (Ok(dir), Ok(p)) => p.strip_prefix(&dir).map_or(p.clone(), Path::to_path_buf),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(to: &str) -> UpgradeRecord {
        UpgradeRecord {
            v: 1,
            from: Some("0.1.0".into()),
            to: to.into(),
            schema_from: 10,
            schema_to: 11,
            snapshot: Some(PathBuf::from("backups/pre-10-to-11-20261030T120000Z.db")),
            at: Timestamp::now(),
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The new record replaces the file by a rename: a hard link to the old file keeps
    /// the old content (an in-place rewrite would change it), and no temp file is left.
    #[test]
    fn upgrade_json_write_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &sample("0.2.0")).unwrap();
        let old_raw = std::fs::read(dir.path().join(UPGRADE_FILE)).unwrap();
        std::fs::hard_link(dir.path().join(UPGRADE_FILE), dir.path().join("old.json")).unwrap();

        write(dir.path(), &sample("0.3.0")).unwrap();
        assert_eq!(std::fs::read(dir.path().join("old.json")).unwrap(), old_raw);
        assert_eq!(load(dir.path()).unwrap().unwrap().to, "0.3.0");
        assert_eq!(entries(dir.path()), vec!["old.json", UPGRADE_FILE]);
    }

    #[test]
    fn json_shape_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
        let mut rec = sample("0.2.0");
        rec.from = None;
        write(dir.path(), &rec).unwrap();
        assert_eq!(load(dir.path()).unwrap(), Some(rec.clone()));
        let raw = std::fs::read_to_string(dir.path().join(UPGRADE_FILE)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let mut keys = keys;
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "at",
                "from",
                "schema_from",
                "schema_to",
                "snapshot",
                "to",
                "v"
            ]
        );
        assert!(v["from"].is_null());
        assert_eq!(v["at"], rec.at.to_rfc3339().replace("+00:00", "Z"));
        assert_eq!(
            keep_snapshot(dir.path()),
            Some(KeepSnapshot::Keep(
                dir.path().join("backups/pre-10-to-11-20261030T120000Z.db")
            ))
        );
    }

    /// A provisional record (an unfinished move) spares the snapshot its move took, found
    /// by name; a record that changed no schema spares nothing.
    #[test]
    fn provisional_record_keeps_its_moves_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = sample("0.2.0");
        rec.snapshot = None;
        write(dir.path(), &rec).unwrap();
        assert_eq!(keep_snapshot(dir.path()), None, "no snapshot yet");
        let backups = dir.path().join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("pre-10-to-11-20261030T120000Z.db"), b"x").unwrap();
        std::fs::write(backups.join("pre-9-to-11-20261030T130000Z.db"), b"x").unwrap();
        assert_eq!(
            keep_snapshot(dir.path()),
            Some(KeepSnapshot::Keep(
                dir.path().join("backups/pre-10-to-11-20261030T120000Z.db")
            ))
        );
        rec.schema_from = 11;
        write(dir.path(), &rec).unwrap();
        assert_eq!(keep_snapshot(dir.path()), None, "a binary-only record");
    }

    #[test]
    fn unreadable_file_keeps_every_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(keep_snapshot(dir.path()), None);
        let mut v = serde_json::to_value(sample("0.2.0")).unwrap();
        v["v"] = 2.into();
        std::fs::write(dir.path().join(UPGRADE_FILE), v.to_string()).unwrap();
        assert_eq!(keep_snapshot(dir.path()), Some(KeepSnapshot::KeepAll));
        std::fs::write(dir.path().join(UPGRADE_FILE), "{").unwrap();
        assert_eq!(keep_snapshot(dir.path()), Some(KeepSnapshot::KeepAll));
    }

    #[test]
    fn replace_moves_an_unreadable_file_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(UPGRADE_FILE), "{").unwrap();
        replace(dir.path(), &sample("0.2.0")).unwrap();
        assert_eq!(load(dir.path()).unwrap().unwrap().to, "0.2.0");
        let names = entries(dir.path());
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[1].starts_with("upgrade.json.invalid-"), "{names:?}");
    }

    #[test]
    fn write_sweeps_stale_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".upgrade.json.abc.tmp");
        let young = dir.path().join(".upgrade.json.def.tmp");
        let other = dir.path().join(".other.tmp");
        for p in [&stale, &young, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        let old = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(old)
            .unwrap();
        write(dir.path(), &sample("0.2.0")).unwrap();
        assert!(!stale.exists());
        assert!(young.exists() && other.exists());
    }

    #[test]
    fn downgrade_is_semver() {
        let v = |s| semver::Version::parse(s).unwrap();
        assert!(is_downgrade(&v("0.9.0"), Some("0.10.0")));
        assert!(!is_downgrade(&v("0.10.0"), Some("0.9.0")));
        assert!(!is_downgrade(&v("0.10.0"), Some("0.10.0")));
        assert!(!is_downgrade(&v("0.10.0"), Some("garbage")));
        assert!(!is_downgrade(&v("0.10.0"), None));
    }

    #[test]
    fn finds_the_newest_snapshot_of_a_move() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        assert_eq!(find_snapshot(dir.path(), 10, 12), None);
        let now = std::time::SystemTime::now();
        for (name, age) in [
            ("pre-10-to-12-a.db", 60),
            ("pre-10-to-12-b.db", 0),
            ("pre-10-to-11-c.db", 0),
            ("pre-1-to-12-d.db", 0),
        ] {
            let f = std::fs::File::create(backups.join(name)).unwrap();
            f.set_modified(now - Duration::from_secs(age)).unwrap();
        }
        assert_eq!(
            find_snapshot(dir.path(), 10, 12),
            Some(PathBuf::from("backups/pre-10-to-12-b.db"))
        );
    }

    #[test]
    fn load_refuses_garbage_and_other_formats() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(UPGRADE_FILE), "{").unwrap();
        let e = format!("{:#}", load(dir.path()).unwrap_err());
        assert!(e.contains("not a valid upgrade record"), "{e}");
        let mut v = serde_json::to_value(sample("0.2.0")).unwrap();
        v["v"] = 2.into();
        std::fs::write(dir.path().join(UPGRADE_FILE), v.to_string()).unwrap();
        let e = format!("{:#}", load(dir.path()).unwrap_err());
        assert!(e.contains("format version 2"), "{e}");
    }

    #[test]
    fn snapshot_path_made_relative() {
        let dir = tempfile::tempdir().unwrap();
        let snap = dir.path().join("backups").join("pre-1-to-2-x.db");
        assert_eq!(
            relative(dir.path(), &snap),
            PathBuf::from("backups/pre-1-to-2-x.db")
        );
        let outside = PathBuf::from("/elsewhere/pre-1-to-2-x.db");
        assert_eq!(relative(dir.path(), &outside), outside);
    }
}
