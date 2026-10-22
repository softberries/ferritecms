//! Embedded migrations plus the 02 §7–§8 rules: pre-migration snapshots, lossy-down
//! refusal, `schema_meta` bookkeeping and checksum verification.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use ferrite_store::{
    MigrationStatus, SCHEMA_META_MIN_BINARY, StoreError, plan_down, required_min_binary,
};
use semver::Version;
use sqlx::migrate::{MigrateError, Migrator};

use crate::{SqliteStore, exec::db_err};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Pre-migration snapshots kept in the backups folder.
const KEEP_SNAPSHOTS: usize = 5;

/// `(version, up SQL)` of every embedded migration.
fn embedded_up() -> Vec<(i64, &'static str)> {
    MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .map(|m| (m.version, m.sql.as_str()))
        .collect()
}

fn embedded_versions() -> Vec<i64> {
    embedded_up().into_iter().map(|(v, _)| v).collect()
}

fn mig_err(e: MigrateError) -> StoreError {
    StoreError::Migration(e.to_string())
}

fn io_err(path: &Path, e: std::io::Error) -> StoreError {
    StoreError::Migration(format!("snapshot {}: {e}", path.display()))
}

/// One row of `_sqlx_migrations`.
struct Applied {
    version: i64,
    checksum: Vec<u8>,
}

impl SqliteStore {
    async fn table_exists(&self, name: &str) -> Result<bool, StoreError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?)",
        )
        .bind(name)
        .fetch_one(&self.writer)
        .await
        .map_err(db_err)
    }

    /// Applied migrations, oldest first; errors if one is recorded as failed (dirty).
    async fn applied(&self) -> Result<Vec<Applied>, StoreError> {
        if !self.table_exists("_sqlx_migrations").await? {
            return Ok(Vec::new());
        }
        let rows: Vec<(i64, Vec<u8>, bool)> = sqlx::query_as(
            "SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&self.writer)
        .await
        .map_err(db_err)?;
        if let Some((v, _, _)) = rows.iter().find(|(_, _, ok)| !ok) {
            return Err(StoreError::Migration(format!(
                "migration {v} failed partway (dirty); repair the database manually"
            )));
        }
        Ok(rows
            .into_iter()
            .map(|(version, checksum, _)| Applied { version, checksum })
            .collect())
    }

    async fn applied_versions(&self) -> Result<Vec<i64>, StoreError> {
        Ok(self.applied().await?.iter().map(|a| a.version).collect())
    }

    pub(crate) async fn status(&self, binary: &Version) -> Result<MigrationStatus, StoreError> {
        let applied = self.applied_versions().await?;
        let min_binary = if self.table_exists("schema_meta").await? {
            let raw: Option<String> =
                sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = ?")
                    .bind(SCHEMA_META_MIN_BINARY)
                    .fetch_optional(&self.writer)
                    .await
                    .map_err(db_err)?;
            raw.map(|s| {
                Version::parse(&s).map_err(|e| {
                    StoreError::Migration(format!("invalid {SCHEMA_META_MIN_BINARY} `{s}`: {e}"))
                })
            })
            .transpose()?
        } else {
            None
        };
        Ok(MigrationStatus::compute(
            &applied,
            &embedded_versions(),
            binary,
            min_binary,
        ))
    }

    pub(crate) async fn up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError> {
        let applied = self.applied_versions().await?;
        let pending: Vec<i64> = embedded_versions()
            .into_iter()
            .filter(|v| !applied.contains(v) && to.is_none_or(|t| *v <= t))
            .collect();
        if let Some(&target) = pending.iter().max() {
            let from = applied.iter().copied().max().unwrap_or(0);
            self.pre_migration_snapshot(from, target).await?;
            match to {
                Some(t) => MIGRATOR.run_to(t, &self.writer).await,
                None => MIGRATOR.run(&self.writer).await,
            }
            .map_err(mig_err)?;
            tracing::info!(from, to = target, "applied migrations {pending:?}");
        }
        self.sync_schema_meta().await?;
        Ok(pending)
    }

    pub(crate) async fn down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        let applied = self.applied_versions().await?;
        let steps = plan_down(&applied, &embedded_up(), to, allow_lossy)?;
        if steps.is_empty() {
            return Ok(steps);
        }
        MIGRATOR.undo(&self.writer, to).await.map_err(mig_err)?;
        tracing::info!(to, "reverted migrations {steps:?}");
        self.sync_schema_meta().await?;
        Ok(steps)
    }

    pub(crate) async fn verify(&self) -> Result<(), StoreError> {
        let embedded = MIGRATOR
            .iter()
            .filter(|m| m.migration_type.is_up_migration());
        let applied = self.applied().await?;
        for m in embedded {
            if let Some(a) = applied.iter().find(|a| a.version == m.version)
                && a.checksum != *m.checksum
            {
                return Err(StoreError::Migration(format!(
                    "checksum mismatch for applied migration {}: its file changed after it was applied",
                    m.version
                )));
            }
        }
        Ok(())
    }

    /// Writes `site_id` and `created_by_version` when absent and sets
    /// `min_binary_version` from the applied contract migrations.
    async fn sync_schema_meta(&self) -> Result<(), StoreError> {
        if !self.table_exists("schema_meta").await? {
            return Ok(());
        }
        let site_id = uuid::Uuid::now_v7().to_string();
        for (key, value) in [
            ("site_id", site_id.as_str()),
            ("created_by_version", env!("CARGO_PKG_VERSION")),
        ] {
            sqlx::query(
                "INSERT INTO schema_meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO NOTHING",
            )
            .bind(key)
            .bind(value)
            .execute(&self.writer)
            .await
            .map_err(db_err)?;
        }
        let applied = self.applied_versions().await?;
        match required_min_binary(&applied, &embedded_up())? {
            Some(min) => sqlx::query(
                "INSERT INTO schema_meta (key, value) VALUES (?, ?) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )
            .bind(SCHEMA_META_MIN_BINARY)
            .bind(min.to_string()),
            None => {
                sqlx::query("DELETE FROM schema_meta WHERE key = ?").bind(SCHEMA_META_MIN_BINARY)
            }
        }
        .execute(&self.writer)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// `VACUUM INTO dest` (dest must not exist).
    pub(crate) async fn vacuum_into(&self, dest: &Path) -> Result<(), StoreError> {
        let dest = dest
            .to_str()
            .ok_or_else(|| StoreError::Invalid(format!("non-UTF-8 path {}", dest.display())))?;
        sqlx::query("VACUUM INTO ?")
            .bind(dest)
            .execute(&self.writer)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// `backups/pre-<from>-to-<to>-<UTC yyyymmddThhmmssZ>.db`, keeping the newest five.
    async fn pre_migration_snapshot(&self, from: i64, to: i64) -> Result<(), StoreError> {
        let dir = &self.backups_dir;
        std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
        let stem = format!(
            "pre-{from}-to-{to}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        // Several migrations in one second must not collide.
        let mut dest = dir.join(format!("{stem}.db"));
        let mut n = 1;
        while dest.exists() {
            n += 1;
            dest = dir.join(format!("{stem}-{n}.db"));
        }
        self.vacuum_into(&dest).await?;
        tracing::info!(path = %dest.display(), "wrote pre-migration snapshot");
        prune_snapshots(dir)
    }
}

fn prune_snapshots(dir: &Path) -> Result<(), StoreError> {
    let mut snaps: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("pre-") && name.ends_with(".db")) {
            continue;
        }
        let path = entry.path();
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .map_err(|e| io_err(&path, e))?;
        snaps.push((modified, path));
    }
    snaps.sort_by(|a, b| b.cmp(a));
    for (_, old) in snaps.into_iter().skip(KEEP_SNAPSHOTS) {
        std::fs::remove_file(&old).map_err(|e| io_err(&old, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ferrite_store::Store;

    use crate::{SqliteOptions, SqliteStore};

    fn snapshots(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn sqlite_snapshot_written_before_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
            .await
            .unwrap();
        let backups = dir.path().join("backups");
        store.migrate_up(None).await.unwrap();
        store.migrate_down(0, false).await.unwrap();
        store.migrate_up(None).await.unwrap();
        let names = snapshots(&backups);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            names.iter().all(|n| n.starts_with("pre-0-to-1-")),
            "{names:?}"
        );

        // Nothing pending: no snapshot.
        store.migrate_up(None).await.unwrap();
        assert_eq!(snapshots(&backups).len(), 2);

        // Each snapshot is a readable database holding the state before 0001 ran.
        for name in &names {
            let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(backups.join(name));
            let mut conn = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&opts)
                .await
                .unwrap();
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'content_items'",
            )
            .fetch_one(&mut conn)
            .await
            .unwrap();
            assert_eq!(n, 0, "{name} was taken after migrating");
        }

        for _ in 0..4 {
            store.migrate_down(0, false).await.unwrap();
            store.migrate_up(None).await.unwrap();
        }
        // Six snapshots written, five kept.
        assert_eq!(snapshots(&backups).len(), 5);
    }
}
