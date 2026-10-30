use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use ferrite_config::{Config, DatabaseConfig, DbKind};
use ferrite_store::{
    Backend, Compat, LOCK_MIGRATE, LockGuard, MigrationStatus, Phase, Store, StoreError, lock,
};
use ferrite_store_postgres::{PgOptions, PgStore};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};

use crate::upgrade::{Migrate, UpgradeRecord, migrate_and_record};

/// Whether opening may create a missing database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    CreateIfMissing,
    /// Fail instead of creating anything (SQLite: the database file must exist).
    MustExist,
}

/// Opens the configured database (SQLite takes the data-folder lock). Does not migrate.
/// With `MustExist` and SQLite, nothing (dir, lock, db) is created when the file is missing.
pub async fn open_store(config: &Config, mode: OpenMode) -> anyhow::Result<Arc<dyn Store>> {
    match config.database_kind() {
        DbKind::Sqlite { path } => {
            if mode == OpenMode::MustExist && !path.is_file() {
                bail!(
                    "database not found at {}; run `ferrite init` first",
                    path.display()
                );
            }
            let mut opts = SqliteOptions::new(path);
            opts.lock_dir = Some(config.data_dir.clone());
            opts.backups_dir = Some(config.data_dir.join("backups"));
            // Pruning never deletes the snapshot a rollback may restore (M4-2 P30).
            opts.keep_snapshot = crate::upgrade::keep_snapshot(&config.data_dir);
            let store = SqliteStore::open(opts)
                .await
                .context("cannot open the SQLite database")?;
            Ok(Arc::new(store))
        }
        DbKind::Postgres { url } => {
            let store = PgStore::open(PgOptions {
                url: url.expose().to_owned(),
                max_connections: config.database.max_connections,
                schema: config.database.schema.clone(),
                application_name: "ferrite".into(),
            })
            .await
            .context("cannot open the Postgres database")?;
            Ok(Arc::new(store))
        }
    }
}

/// Opens a database from a bare URL (`ferrite db copy`, M4-1 P8): `postgres://…` (or
/// `postgresql://…`) with an optional `schema`, or `sqlite:<path to ferrite.db>`, whose
/// folder lock is taken on the file's parent. With `CreateIfMissing` an absent SQLite file
/// is created (its folder must exist). Does not migrate.
pub async fn open_store_url(
    url: &str,
    schema: Option<&str>,
    mode: OpenMode,
) -> anyhow::Result<Arc<dyn Store>> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let store = PgStore::open(PgOptions {
            url: url.to_owned(),
            max_connections: 4,
            schema: schema.map(str::to_owned),
            application_name: "ferrite".into(),
        })
        .await
        .context("cannot open the Postgres database")?;
        return Ok(Arc::new(store));
    }
    let Some(rest) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    else {
        bail!("unsupported database URL (use postgres://… or sqlite:<path to ferrite.db>)");
    };
    if schema.is_some() {
        bail!("a schema applies to Postgres only");
    }
    let path = std::path::PathBuf::from(rest);
    if mode == OpenMode::MustExist && !path.is_file() {
        bail!("database not found at {}", path.display());
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_owned(),
        _ => std::path::PathBuf::from("."),
    };
    if !parent.is_dir() {
        bail!("folder {} does not exist", parent.display());
    }
    let mut opts = SqliteOptions::new(&path);
    opts.lock_dir = Some(parent);
    let store = SqliteStore::open(opts)
        .await
        .context("cannot open the SQLite database")?;
    Ok(Arc::new(store))
}

/// A read-only view of a SQLite database, only for `ferrite doctor` (M4-3a P49/P50): one
/// connection that takes no data-folder lock, opens no writer, runs no migration and
/// creates no database, so it works while `serve` holds the store and never stops one
/// from starting. Being read-only it never checkpoints the WAL; SQLite still updates the
/// shared-memory index in `-shm` as every reader of a WAL database does. With no `-wal`
/// or `-shm` file (a clean shutdown removed them) it opens `immutable`, since a read-only
/// open would create both and leave them; nobody else has the file open then.
pub(crate) struct SqliteReadOnly {
    conn: tokio::sync::Mutex<sqlx::SqliteConnection>,
    immutable: bool,
}

impl SqliteReadOnly {
    /// Opens the database at `path`, which must exist.
    pub(crate) async fn open(path: &Path) -> anyhow::Result<Self> {
        use sqlx::ConnectOptions;
        if !path.is_file() {
            bail!(
                "database not found at {}; run `ferrite init` first",
                path.display()
            );
        }
        let side = |suffix: &str| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            std::path::PathBuf::from(name).exists()
        };
        let immutable = !side("-wal") && !side("-shm");
        let conn = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false)
            .immutable(immutable)
            .busy_timeout(Duration::from_secs(5))
            .connect()
            .await
            .context("cannot open the SQLite database read-only")?;
        Ok(Self {
            conn: tokio::sync::Mutex::new(conn),
            immutable,
        })
    }

    /// Whether it was opened `immutable` (no WAL files existed).
    pub(crate) fn immutable(&self) -> bool {
        self.immutable
    }

    /// What [`Store::migration_status`] reports, read the same way: the applied versions
    /// (a migration recorded as failed is an error) and `schema_meta.min_binary_version`
    /// against the embedded migrations.
    pub(crate) async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> anyhow::Result<MigrationStatus> {
        let mut conn = self.conn.lock().await;
        let applied: Vec<i64> = if table_exists(&mut conn, "_sqlx_migrations").await? {
            let rows: Vec<(i64, bool)> =
                sqlx::query_as("SELECT version, success FROM _sqlx_migrations ORDER BY version")
                    .fetch_all(&mut *conn)
                    .await?;
            if let Some((v, _)) = rows.iter().find(|(_, ok)| !ok) {
                bail!("migration {v} failed partway (dirty); repair the database manually");
            }
            rows.into_iter().map(|(v, _)| v).collect()
        } else {
            Vec::new()
        };
        let min_binary = match meta(&mut conn, ferrite_store::SCHEMA_META_MIN_BINARY).await? {
            Some(raw) => Some(semver::Version::parse(&raw).with_context(|| {
                format!("invalid {} `{raw}`", ferrite_store::SCHEMA_META_MIN_BINARY)
            })?),
            None => None,
        };
        Ok(MigrationStatus::compute(
            &applied,
            &ferrite_store_sqlite::embedded_status()?,
            binary,
            min_binary,
        ))
    }

    /// A `schema_meta` value (`None` without the table or the key).
    pub(crate) async fn schema_meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        let mut conn = self.conn.lock().await;
        meta(&mut conn, key).await
    }
}

async fn table_exists(conn: &mut sqlx::SqliteConnection, name: &str) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?)",
    )
    .bind(name)
    .fetch_one(&mut *conn)
    .await?)
}

async fn meta(conn: &mut sqlx::SqliteConnection, key: &str) -> anyhow::Result<Option<String>> {
    if !table_exists(conn, "schema_meta").await? {
        return Ok(None);
    }
    Ok(
        sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// What the startup check wants done (02 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Schema matches this binary.
    Proceed,
    /// Schema is newer but compatible with this binary.
    ProceedNewerSchema { db: i64, bin: i64 },
    /// Pending migrations should be applied now.
    MigrateUp,
}

/// Pure part of the startup check: maps a migration status to a decision or an error.
pub fn decide(
    status: &MigrationStatus,
    backend: Backend,
    db: &DatabaseConfig,
    binary: &semver::Version,
) -> anyhow::Result<Decision> {
    match &status.verdict {
        Compat::UpToDate => Ok(Decision::Proceed),
        Compat::NewerSchemaOk => Ok(Decision::ProceedNewerSchema {
            db: status.db_version,
            bin: status.binary_version,
        }),
        Compat::Incompatible { requires } => bail!(
            "the database schema requires ferrite >= {requires} (this is {binary}); run \
             `ferrite migrate down --to {}` with the newer binary, or upgrade this binary",
            status.binary_version
        ),
        Compat::NeedsUp => {
            if !db.auto_migrate {
                bail!(
                    "the database has {} pending migration(s) and database.auto_migrate is off; \
                     run `ferrite migrate up`",
                    status.pending.len()
                );
            }
            let contract = status
                .pending
                .iter()
                .any(|m| m.directives.phase == Phase::Contract);
            if backend == Backend::Postgres && contract && !db.allow_contract {
                bail!(
                    "pending migrations include a contract (destructive) migration; other nodes \
                     may still run an older binary. Run `ferrite migrate up` once all nodes are \
                     upgraded, or set database.allow_contract = true"
                );
            }
            Ok(Decision::MigrateUp)
        }
    }
}

/// How long startup and `ferrite migrate` wait for the migrate lock (02 §7, 04 §7).
pub const MIGRATE_LOCK_WAIT: Duration = Duration::from_secs(5 * 60);
/// How often a wait for the migrate lock is logged.
const MIGRATE_LOCK_LOG_EVERY: Duration = Duration::from_secs(10);

/// Takes [`LOCK_MIGRATE`] (both backends: another node or `ferrite migrate` may hold it),
/// waiting up to [`MIGRATE_LOCK_WAIT`] and logging every 10 s while waiting.
pub async fn migrate_lock(store: &dyn Store) -> anyhow::Result<Box<dyn LockGuard>> {
    migrate_lock_with(store, MIGRATE_LOCK_WAIT).await
}

/// [`migrate_lock`] with another wait (the CLI passes the default; tests shorten it).
pub(crate) async fn migrate_lock_with(
    store: &dyn Store,
    max_wait: Duration,
) -> anyhow::Result<Box<dyn LockGuard>> {
    let started = tokio::time::Instant::now();
    let wait = lock(store, LOCK_MIGRATE, max_wait);
    tokio::pin!(wait);
    let mut log =
        tokio::time::interval_at(started + MIGRATE_LOCK_LOG_EVERY, MIGRATE_LOCK_LOG_EVERY);
    loop {
        tokio::select! {
            r = &mut wait => {
                return match r {
                    Ok(guard) => Ok(guard),
                    Err(StoreError::Busy) => bail!(
                        "another node or `ferrite migrate` has held the migration lock for {}: \
                         a long migration may still be running elsewhere, or a hung `ferrite` \
                         process holds it (on Postgres: the session with application_name \
                         `ferrite:lock:migrate` in pg_stat_activity); stop that process and retry",
                        describe_wait(max_wait)
                    ),
                    Err(e) => Err(anyhow::Error::new(e).context("cannot take the migration lock")),
                };
            }
            _ = log.tick() => tracing::info!(
                waited_secs = started.elapsed().as_secs(),
                "waiting for the migration lock (another node or `ferrite migrate` holds it)"
            ),
        }
    }
}

fn describe_wait(wait: Duration) -> String {
    let secs = wait.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) && wait.subsec_nanos() == 0 {
        let m = secs / 60;
        format!("{m} minute{}", if m == 1 { "" } else { "s" })
    } else {
        format!("{wait:?}")
    }
}

/// Releases a migrate lock, logging (not failing on) a dirty release: the lock is gone
/// either way once the guard is consumed.
pub async fn release_migrate_lock(guard: Box<dyn LockGuard>) {
    if let Err(e) = guard.release().await {
        tracing::warn!(error = %e, "releasing the migration lock failed");
    }
}

/// Startup compatibility check (02 §7-§8): applies pending migrations when allowed,
/// refuses to start when the schema is incompatible with `binary`, then records the start
/// in `<data_dir>/upgrade.json` and `schema_meta.last_binary_version` (M4-2 P30,
/// [`crate::upgrade::migrate_and_record`]); returns the record when one was written. Runs under the
/// migrate lock, so concurrent starts migrate (and record) once and none reads a
/// half-migrated schema.
pub async fn startup_check(
    store: &dyn Store,
    binary: &semver::Version,
    db: &DatabaseConfig,
    data_dir: &Path,
) -> anyhow::Result<Option<UpgradeRecord>> {
    let guard = migrate_lock(store).await?;
    let result = check_and_migrate(store, binary, db, data_dir).await;
    release_migrate_lock(guard).await;
    result
}

async fn check_and_migrate(
    store: &dyn Store,
    binary: &semver::Version,
    db: &DatabaseConfig,
    data_dir: &Path,
) -> anyhow::Result<Option<UpgradeRecord>> {
    let status = store.migration_status(binary).await?;
    let migrate = match decide(&status, store.backend(), db, binary)? {
        Decision::Proceed => Migrate::No,
        Decision::ProceedNewerSchema { db, bin } => {
            tracing::warn!(db, bin, "running against newer schema");
            Migrate::No
        }
        Decision::MigrateUp => Migrate::UpTo(None),
    };
    let (report, written) = migrate_and_record(store, data_dir, binary, migrate).await?;
    if !report.applied.is_empty() {
        tracing::info!(applied = ?report.applied, "applied pending migrations");
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use ferrite_domain::Secret;
    use ferrite_store::{Directives, PendingMigration};

    use super::*;

    fn dbconf(auto: bool, allow_contract: bool) -> DatabaseConfig {
        DatabaseConfig {
            url: Secret::new("postgres://h/d"),
            max_connections: 4,
            auto_migrate: auto,
            allow_contract,
            schema: None,
        }
    }

    fn pending(version: i64, phase: Phase) -> PendingMigration {
        PendingMigration {
            version,
            description: format!("m{version}"),
            directives: Directives {
                phase,
                min_binary: None,
                lossy_down: false,
            },
        }
    }

    fn status(verdict: Compat, pending: Vec<PendingMigration>) -> MigrationStatus {
        MigrationStatus {
            db_version: 2,
            binary_version: 4,
            min_binary: None,
            pending,
            unknown_applied: vec![],
            verdict,
        }
    }

    fn bin() -> semver::Version {
        semver::Version::new(0, 1, 0)
    }

    fn err_text(r: anyhow::Result<Decision>) -> String {
        format!("{:#}", r.unwrap_err())
    }

    #[test]
    fn lock_wait_wording() {
        assert_eq!(describe_wait(MIGRATE_LOCK_WAIT), "5 minutes");
        assert_eq!(describe_wait(Duration::from_secs(60)), "1 minute");
        assert_eq!(describe_wait(Duration::from_millis(300)), "300ms");
    }

    #[test]
    fn up_to_date_and_newer_schema_proceed() {
        let c = dbconf(true, false);
        let s = status(Compat::UpToDate, vec![]);
        assert_eq!(
            decide(&s, Backend::Sqlite, &c, &bin()).unwrap(),
            Decision::Proceed
        );
        let s = status(Compat::NewerSchemaOk, vec![]);
        assert_eq!(
            decide(&s, Backend::Postgres, &c, &bin()).unwrap(),
            Decision::ProceedNewerSchema { db: 2, bin: 4 }
        );
    }

    #[test]
    fn needs_up_migrates_when_auto() {
        let s = status(Compat::NeedsUp, vec![pending(3, Phase::Expand)]);
        for backend in [Backend::Sqlite, Backend::Postgres] {
            assert_eq!(
                decide(&s, backend, &dbconf(true, false), &bin()).unwrap(),
                Decision::MigrateUp
            );
        }
    }

    #[test]
    fn needs_up_refused_without_auto_migrate() {
        let s = status(Compat::NeedsUp, vec![pending(3, Phase::Expand)]);
        let e = err_text(decide(&s, Backend::Sqlite, &dbconf(false, false), &bin()));
        assert!(e.contains("ferrite migrate up"), "{e}");
    }

    #[test]
    fn startup_check_contract_refusal() {
        let s = status(
            Compat::NeedsUp,
            vec![pending(3, Phase::Expand), pending(4, Phase::Contract)],
        );
        let e = err_text(decide(&s, Backend::Postgres, &dbconf(true, false), &bin()));
        assert!(
            e.contains("ferrite migrate up") && e.contains("contract"),
            "{e}"
        );
        // Allowed explicitly, or on SQLite (single process), it proceeds.
        assert_eq!(
            decide(&s, Backend::Postgres, &dbconf(true, true), &bin()).unwrap(),
            Decision::MigrateUp
        );
        assert_eq!(
            decide(&s, Backend::Sqlite, &dbconf(true, false), &bin()).unwrap(),
            Decision::MigrateUp
        );
    }

    #[test]
    fn incompatible_schema_refused_with_guidance() {
        let s = status(
            Compat::Incompatible {
                requires: semver::Version::new(0, 5, 0),
            },
            vec![],
        );
        let e = err_text(decide(&s, Backend::Sqlite, &dbconf(true, false), &bin()));
        assert!(e.contains("requires ferrite >= 0.5.0"), "{e}");
        assert!(e.contains("ferrite migrate down --to 4"), "{e}");
    }

    #[tokio::test]
    async fn read_only_view_matches_the_store_while_it_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferrite.db");
        let store = SqliteStore::open(SqliteOptions::new(&path)).await.unwrap();
        store.migrate_up(None).await.unwrap();
        store
            .read()
            .schema_meta()
            .set("min_binary_version", "0.0.1")
            .await
            .unwrap();
        // The store holds the folder lock; the view needs none.
        let view = SqliteReadOnly::open(&path).await.unwrap();
        assert!(!view.immutable(), "the store's WAL files exist");
        let binary = semver::Version::new(0, 1, 0);
        assert_eq!(
            view.migration_status(&binary).await.unwrap(),
            store.migration_status(&binary).await.unwrap()
        );
        for key in ["site_id", "install_id", "nope"] {
            assert_eq!(
                view.schema_meta(key).await.unwrap(),
                store.read().schema_meta().get(key).await.unwrap(),
                "{key}"
            );
        }
        // A write through the view is refused.
        let mut conn = view.conn.lock().await;
        let err = sqlx::query("INSERT INTO schema_meta (key, value) VALUES ('x', 'y')")
            .execute(&mut *conn)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("readonly"), "{err}");
    }

    #[tokio::test]
    async fn read_only_view_needs_an_existing_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferrite.db");
        let err = SqliteReadOnly::open(&path).await.err().unwrap();
        assert!(format!("{err:#}").contains("not found"), "{err:#}");
        assert!(!path.exists());
    }
}
