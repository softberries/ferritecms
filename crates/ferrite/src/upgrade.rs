//! `<data>/upgrade.json`: what the last binary or schema change on this data folder was
//! (09 §1, M4-2 P30). `ferrite rollback` reads it.
//!
//! The startup check (`serve`, and every other command that migrates on start) and
//! `ferrite migrate up` call [`record`] under the migrate lock. It compares
//! `schema_meta.last_binary_version` with this binary and looks at what `migrate_up`
//! applied; when either changed it replaces the file, then stores this binary's version.
//! A database that had no schema before (a new site, `ferrite init`) gets the version but
//! no file: there is nothing to roll back to.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use ferrite_domain::Timestamp;
use ferrite_store::{MigrateReport, SCHEMA_META_LAST_BINARY, Store};
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

/// Reads `<data_dir>/upgrade.json`; `None` when there is none.
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

/// Replaces `<data_dir>/upgrade.json` atomically: a temp file in the same folder, synced,
/// then renamed over the old one, so a reader sees the old or the new record, never a
/// partial one.
pub fn write(data_dir: &Path, rec: &UpgradeRecord) -> anyhow::Result<()> {
    let path = data_dir.join(UPGRADE_FILE);
    let mut json = serde_json::to_vec_pretty(rec)?;
    json.push(b'\n');
    let mut tmp = tempfile::Builder::new()
        .prefix(".upgrade.json.")
        .suffix(".tmp")
        .tempfile_in(data_dir)
        .with_context(|| format!("cannot create a temp file in {}", data_dir.display()))?;
    tmp.write_all(&json)?;
    tmp.as_file().sync_all()?;
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

/// The snapshot `upgrade.json` names, as a path to pass to the store's pruning
/// (`SqliteOptions::keep_snapshot`).
pub fn protected_snapshot(data_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    Ok(load(data_dir)?
        .and_then(|r| r.snapshot)
        .map(|s| data_dir.join(s)))
}

/// Records this start (or `migrate up`) of `binary`: replaces `upgrade.json` when the
/// stored `last_binary_version` differs from `binary` or `report` applied migrations,
/// then stores `binary` as the last version. `schema_from` is the schema before
/// `migrate_up` ran. Returns the record written, if any. Call it under the migrate lock.
///
/// Not written: for a database that had no schema (nothing to roll back to), and on a
/// retry after a failed start whose record already names this move (it may name the
/// snapshot that the retry, having nothing left to migrate, would lose).
pub async fn record(
    store: &dyn Store,
    data_dir: &Path,
    binary: &semver::Version,
    schema_from: i64,
    report: &MigrateReport,
) -> anyhow::Result<Option<UpgradeRecord>> {
    let schema_to = store.migration_status(binary).await?.db_version;
    if schema_to == 0 {
        // No schema_meta table to store the version in.
        return Ok(None);
    }
    let meta = store.read().schema_meta();
    let stored = meta.get(SCHEMA_META_LAST_BINARY).await?;
    let to = binary.to_string();
    let version_changed = stored.as_deref() != Some(to.as_str());
    let mut written = None;
    if schema_from > 0 && (version_changed || !report.applied.is_empty()) {
        let retry = report.applied.is_empty()
            && load(data_dir)?.is_some_and(|old| old.to == to && old.from == stored);
        if !retry {
            let rec = UpgradeRecord {
                v: UPGRADE_FORMAT,
                from: stored,
                to: to.clone(),
                schema_from,
                schema_to,
                snapshot: report.snapshot.as_ref().map(|s| relative(data_dir, s)),
                at: Timestamp::now(),
            };
            write(data_dir, &rec)?;
            tracing::info!(
                from = rec.from.as_deref().unwrap_or("unknown"),
                to = %rec.to,
                schema_from,
                schema_to,
                "wrote {UPGRADE_FILE}"
            );
            written = Some(rec);
        }
    }
    if version_changed {
        meta.set(SCHEMA_META_LAST_BINARY, &to).await?;
    }
    Ok(written)
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
            protected_snapshot(dir.path()).unwrap(),
            Some(dir.path().join("backups/pre-10-to-11-20261030T120000Z.db"))
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
