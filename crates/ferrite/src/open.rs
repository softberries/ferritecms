use std::sync::Arc;

use anyhow::{Context, bail};
use ferrite_config::{Config, DatabaseConfig, DbKind};
use ferrite_store::{Backend, Compat, MigrationStatus, Phase, Store};
use ferrite_store_postgres::{PgOptions, PgStore};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};

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

/// Startup compatibility check (02 §7-§8): applies pending migrations when allowed,
/// refuses to start when the schema is incompatible with `binary`.
pub async fn startup_check(
    store: &dyn Store,
    binary: &semver::Version,
    db: &DatabaseConfig,
) -> anyhow::Result<()> {
    let status = store.migration_status(binary).await?;
    match decide(&status, store.backend(), db, binary)? {
        Decision::Proceed => {}
        Decision::ProceedNewerSchema { db, bin } => {
            tracing::warn!(db, bin, "running against newer schema");
        }
        Decision::MigrateUp => {
            let applied = store.migrate_up(None).await?;
            tracing::info!(?applied, "applied pending migrations");
        }
    }
    Ok(())
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
}
