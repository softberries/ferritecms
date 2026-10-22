//! Connection setup: pragmas, writer + reader pools, data-folder lock (02 §6).

use std::{
    fs::{File, TryLockError},
    path::{Path, PathBuf},
    time::Duration,
};

use ferrite_store::StoreError;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::{SqliteStore, exec::map_db_err, repos::SqliteRepos};

/// How long a write or `begin()` waits for the single writer connection before failing
/// with `StoreError::Busy`. Never write through `read()` or call `begin()` while this
/// task holds a `Tx`: that waits for the writer the `Tx` itself holds (02 §3).
const WRITER_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Name of the lock file held for the store's lifetime.
pub(crate) const LOCK_FILE: &str = "ferrite.lock";

/// How to open a [`SqliteStore`].
#[derive(Debug, Clone)]
pub struct SqliteOptions {
    /// The database file.
    pub path: PathBuf,
    /// Reader pool size (default 4).
    pub readers: u32,
    /// Where `ferrite.lock` lives; default: the database's directory.
    pub lock_dir: Option<PathBuf>,
    /// Where pre-migration snapshots go; default: `<db dir>/backups`.
    pub backups_dir: Option<PathBuf>,
}

impl SqliteOptions {
    /// Defaults for a database file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            readers: 4,
            lock_dir: None,
            backups_dir: None,
        }
    }
}

impl SqliteStore {
    /// Takes the data-folder lock, then opens the writer and reader pools. Does not migrate.
    pub async fn open(opts: SqliteOptions) -> Result<Self, StoreError> {
        let db_dir = match opts.path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        std::fs::create_dir_all(&db_dir).map_err(|e| io_err(&db_dir, e))?;
        let lock = lock_folder(opts.lock_dir.as_deref().unwrap_or(&db_dir))?;

        let base = SqliteConnectOptions::new()
            .filename(&opts.path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_millis(5000))
            .pragma("temp_store", "MEMORY");
        // Writer first: it creates the file and switches it to WAL. It keeps one
        // connection open so the WAL index exists for the read-only readers.
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .acquire_timeout(WRITER_ACQUIRE_TIMEOUT)
            .connect_with(base.clone())
            .await
            .map_err(map_db_err)?;
        // Read-only, so a write sent to the reader pool by mistake fails loudly.
        let reader = SqlitePoolOptions::new()
            .max_connections(opts.readers.max(1))
            .connect_with(base.read_only(true))
            .await
            .map_err(map_db_err)?;

        Ok(Self {
            read: SqliteRepos::new(reader.clone(), writer.clone()),
            writer,
            reader,
            backups_dir: opts.backups_dir.unwrap_or_else(|| db_dir.join("backups")),
            _lock: lock,
        })
    }
}

/// Holds an exclusive OS lock on `<dir>/ferrite.lock`; a second store on the folder fails fast.
fn lock_folder(dir: &Path) -> Result<File, StoreError> {
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    let path = dir.join(LOCK_FILE);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| io_err(&path, e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(StoreError::Database(format!(
            "data folder is in use: {} is locked by another process",
            path.display()
        ))),
        Err(TryLockError::Error(e)) => Err(io_err(&path, e)),
    }
}

fn io_err(path: &Path, e: std::io::Error) -> StoreError {
    StoreError::Database(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_open_on_same_folder_fails() {
        let dir = tempfile::tempdir().unwrap();
        let opts = SqliteOptions::new(dir.path().join("ferrite.db"));
        let first = SqliteStore::open(opts.clone()).await.unwrap();
        let err = SqliteStore::open(opts.clone()).await.err().unwrap();
        assert!(
            matches!(&err, StoreError::Database(m) if m.contains("in use")),
            "{err:?}"
        );
        drop(first);
        SqliteStore::open(opts).await.unwrap();
    }

    #[tokio::test]
    async fn read_repos_write_on_writer_and_readers_are_read_only() {
        use ferrite_store::Store;
        use sqlx::Connection;

        let dir = tempfile::tempdir().unwrap();
        let mut opts = SqliteOptions::new(dir.path().join("ferrite.db"));
        opts.readers = 1;
        let store = SqliteStore::open(opts).await.unwrap();
        store.migrate_up(None).await.unwrap();

        // Hold the only reader connection inside a read transaction.
        let mut busy = store.reader.acquire().await.unwrap();
        let mut read_tx = busy.begin().await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT count(*) FROM schema_meta")
            .fetch_one(&mut *read_tx)
            .await
            .unwrap();
        let set = store.read().schema_meta().set("k", "v");
        tokio::time::timeout(Duration::from_secs(2), set)
            .await
            .expect("write must not wait for a reader")
            .unwrap();

        // A write sent straight to the reader pool is refused.
        let err = sqlx::query("INSERT INTO schema_meta (key, value) VALUES ('x', 'y')")
            .execute(&mut *read_tx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("readonly"), "{err}");
        drop(read_tx);
        drop(busy);

        assert_eq!(
            store
                .read()
                .schema_meta()
                .get("k")
                .await
                .unwrap()
                .as_deref(),
            Some("v")
        );
    }
}
