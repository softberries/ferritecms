//! Embedded migrations plus the 02 §7–§8 rules: lossy-down refusal, `schema_meta`
//! bookkeeping and checksum verification. No automatic snapshot on Postgres (02 §7).

use ferrite_store::{
    Directives, MigrationStatus, PendingMigration, SCHEMA_META_MIN_BINARY, StoreError,
    next_min_binary, parse_directives, plan_down,
};
use semver::Version;
use sqlx::migrate::{MigrateError, Migrator};

use crate::{PgStore, exec::map_db_err};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Every embedded migration's version with the directives parsed from its up SQL.
fn embedded() -> Result<Vec<(i64, Directives)>, StoreError> {
    MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .map(|m| Ok((m.version, parse_directives(m.sql.as_str())?)))
        .collect()
}

/// Every embedded migration as a status entry.
fn embedded_status() -> Result<Vec<PendingMigration>, StoreError> {
    MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .map(|m| {
            Ok(PendingMigration {
                version: m.version,
                description: m.description.to_string(),
                directives: parse_directives(m.sql.as_str())?,
            })
        })
        .collect()
}

fn embedded_versions() -> Vec<i64> {
    MIGRATOR
        .iter()
        .filter(|m| m.migration_type.is_up_migration())
        .map(|m| m.version)
        .collect()
}

fn mig_err(e: MigrateError) -> StoreError {
    StoreError::Migration(e.to_string())
}

/// One row of `_sqlx_migrations`.
struct Applied {
    version: i64,
    checksum: Vec<u8>,
}

impl PgStore {
    /// Whether `name` resolves to a table in the configured `search_path`.
    async fn table_exists(&self, name: &str) -> Result<bool, StoreError> {
        sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(map_db_err)
    }

    /// Applied migrations, oldest first; errors if one is recorded as failed (dirty).
    async fn applied(&self) -> Result<Vec<Applied>, StoreError> {
        if !self.table_exists("_sqlx_migrations").await? {
            return Ok(Vec::new());
        }
        let rows: Vec<(i64, Vec<u8>, bool)> = sqlx::query_as(
            "SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_db_err)?;
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

    /// `schema_meta.min_binary_version`, if the table and key exist.
    async fn stored_min_binary(&self) -> Result<Option<Version>, StoreError> {
        if !self.table_exists("schema_meta").await? {
            return Ok(None);
        }
        let raw: Option<String> =
            sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = $1")
                .bind(SCHEMA_META_MIN_BINARY)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_db_err)?;
        raw.map(|s| {
            Version::parse(&s).map_err(|e| {
                StoreError::Migration(format!("invalid {SCHEMA_META_MIN_BINARY} `{s}`: {e}"))
            })
        })
        .transpose()
    }

    pub(crate) async fn status(&self, binary: &Version) -> Result<MigrationStatus, StoreError> {
        let applied = self.applied_versions().await?;
        let min_binary = self.stored_min_binary().await?;
        Ok(MigrationStatus::compute(
            &applied,
            &embedded_status()?,
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
            // sqlx takes an advisory lock, so concurrent nodes apply each migration once.
            let pool = self.migration_pool().await?;
            let res = match to {
                Some(t) => MIGRATOR.run_to(t, &pool).await,
                None => MIGRATOR.run(&pool).await,
            };
            pool.close().await;
            res.map_err(mig_err)?;
            tracing::info!(from, to = target, "applied migrations {pending:?}");
        }
        self.sync_schema_meta(&[]).await?;
        Ok(pending)
    }

    pub(crate) async fn down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        let applied = self.applied_versions().await?;
        let steps = plan_down(&applied, &embedded()?, to, allow_lossy)?;
        if steps.is_empty() {
            return Ok(steps);
        }
        let pool = self.migration_pool().await?;
        let res = MIGRATOR.undo(&pool, to).await;
        pool.close().await;
        res.map_err(mig_err)?;
        tracing::info!(to, "reverted migrations {steps:?}");
        self.sync_schema_meta(&steps).await?;
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

    /// Writes `site_id` and `created_by_version` when absent and updates
    /// `min_binary_version` per [`next_min_binary`]; `reverted` = versions just undone.
    async fn sync_schema_meta(&self, reverted: &[i64]) -> Result<(), StoreError> {
        if !self.table_exists("schema_meta").await? {
            return Ok(());
        }
        let site_id = uuid::Uuid::now_v7().to_string();
        for (key, value) in [
            ("site_id", site_id.as_str()),
            ("created_by_version", env!("CARGO_PKG_VERSION")),
        ] {
            sqlx::query(
                "INSERT INTO schema_meta (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING",
            )
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await
            .map_err(map_db_err)?;
        }
        let current = self.stored_min_binary().await?;
        let applied = self.applied_versions().await?;
        let next = next_min_binary(current.as_ref(), &applied, &embedded()?, reverted);
        if next == current {
            return Ok(());
        }
        match &next {
            Some(min) => sqlx::query(
                "INSERT INTO schema_meta (key, value) VALUES ($1, $2) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )
            .bind(SCHEMA_META_MIN_BINARY)
            .bind(min.to_string()),
            None => {
                sqlx::query("DELETE FROM schema_meta WHERE key = $1").bind(SCHEMA_META_MIN_BINARY)
            }
        }
        .execute(&self.pool)
        .await
        .map_err(map_db_err)?;
        tracing::info!(from = ?current, to = ?next, "updated {SCHEMA_META_MIN_BINARY}");
        Ok(())
    }
}
