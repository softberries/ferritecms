use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::bail;
use ferrite_store::{Compat, MigrationStatus, Phase};

use crate::cli::{DataArg, MigrateCommand};
use std::time::Duration;

use ferrite_store::Store;

use crate::open::{
    MIGRATE_LOCK_WAIT, OpenMode, migrate_lock_with, open_store, release_migrate_lock,
};
use crate::{binary_version, load_config};

pub async fn run(cmd: MigrateCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        MigrateCommand::Status(DataArg { data }) => {
            let store = open_store(&load_config(&data)?, OpenMode::MustExist).await?;
            let binary = binary_version()?;
            let status = store.migration_status(&binary).await?;
            print!("{}", render_status(&status, &binary));
            return Ok(match status.verdict {
                Compat::UpToDate | Compat::NewerSchemaOk => ExitCode::SUCCESS,
                Compat::NeedsUp => ExitCode::from(2),
                Compat::Incompatible { .. } => ExitCode::from(3),
            });
        }
        MigrateCommand::Up {
            data: DataArg { data },
            to,
        } => {
            let config = load_config(&data)?;
            let store = open_store(&config, OpenMode::CreateIfMissing).await?;
            let applied = up_recorded(
                store.as_ref(),
                to,
                &config.data_dir,
                &binary_version()?,
                MIGRATE_LOCK_WAIT,
            )
            .await?;
            if applied.is_empty() {
                println!("Nothing to apply.");
            } else {
                println!("Applied migrations: {}", join(&applied));
            }
        }
        MigrateCommand::Down {
            data: DataArg { data },
            to,
            allow_lossy,
            yes,
        } => {
            if !yes && !confirm(to)? {
                bail!("aborted");
            }
            let store = open_store(&load_config(&data)?, OpenMode::MustExist).await?;
            let reverted = down_locked(store.as_ref(), to, allow_lossy, MIGRATE_LOCK_WAIT).await?;
            if reverted.is_empty() {
                println!("Nothing to revert.");
            } else {
                println!("Reverted migrations: {}", join(&reverted));
            }
        }
        MigrateCommand::Verify(DataArg { data }) => {
            let store = open_store(&load_config(&data)?, OpenMode::MustExist).await?;
            store.verify_migrations().await?;
            println!("Migration checksums OK.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `migrate_up` under the migrate lock (02 §7, 04 §7), waiting up to `wait` for it.
pub(crate) async fn up_locked(
    store: &dyn Store,
    to: Option<i64>,
    wait: Duration,
) -> anyhow::Result<Vec<i64>> {
    let guard = migrate_lock_with(store, wait).await?;
    let applied = store.migrate_up(to).await;
    release_migrate_lock(guard).await;
    Ok(applied?.applied)
}

/// [`up_locked`] for the site's own database (`ferrite migrate up`, `init`): also records
/// the change in `<data_dir>/upgrade.json` and `schema_meta.last_binary_version`
/// ([`crate::upgrade::migrate_and_record`]) under the same lock. Returns the applied versions.
pub(crate) async fn up_recorded(
    store: &dyn Store,
    to: Option<i64>,
    data_dir: &Path,
    binary: &semver::Version,
    wait: Duration,
) -> anyhow::Result<Vec<i64>> {
    let guard = migrate_lock_with(store, wait).await?;
    let result = async {
        let (report, _) = crate::upgrade::migrate_and_record(
            store,
            data_dir,
            binary,
            crate::upgrade::Migrate::UpTo(to),
        )
        .await?;
        anyhow::Ok(report.applied)
    }
    .await;
    release_migrate_lock(guard).await;
    result
}

/// `migrate_down` under the migrate lock, waiting up to `wait` for it.
pub(crate) async fn down_locked(
    store: &dyn Store,
    to: i64,
    allow_lossy: bool,
    wait: Duration,
) -> anyhow::Result<Vec<i64>> {
    let guard = migrate_lock_with(store, wait).await?;
    let reverted = store.migrate_down(to, allow_lossy).await;
    release_migrate_lock(guard).await;
    Ok(reverted?)
}

fn join(versions: &[i64]) -> String {
    versions
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Asks on a terminal; without a terminal `--yes` is required.
fn confirm(to: i64) -> anyhow::Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("`migrate down` needs --yes when stdin is not a terminal");
    }
    eprint!(
        "Revert all migrations above version {to}? This can lose data. Type 'yes' to continue: "
    );
    std::io::stderr().flush()?;
    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

pub(crate) fn render_status(status: &MigrationStatus, binary: &semver::Version) -> String {
    let mut out = String::new();
    out.push_str(&format!("database version: {}\n", status.db_version));
    out.push_str(&format!(
        "binary version: {} (ferrite {binary})\n",
        status.binary_version
    ));
    if let Some(min) = &status.min_binary {
        out.push_str(&format!("schema requires ferrite >= {min}\n"));
    }
    if status.pending.is_empty() {
        out.push_str("pending migrations: none\n");
    } else {
        out.push_str("pending migrations:\n");
        for m in &status.pending {
            let phase = match m.directives.phase {
                Phase::Expand => "expand",
                Phase::Contract => "contract",
            };
            let min = m
                .directives
                .min_binary
                .as_ref()
                .map_or_else(|| "-".to_owned(), ToString::to_string);
            out.push_str(&format!(
                "  {:04} {} (phase={phase}, min-binary={min}, lossy-down={})\n",
                m.version,
                m.description,
                if m.directives.lossy_down { "yes" } else { "no" }
            ));
        }
    }
    if !status.unknown_applied.is_empty() {
        out.push_str(&format!(
            "unknown applied migrations: {}\n",
            join(&status.unknown_applied)
        ));
    }
    let verdict = match &status.verdict {
        Compat::UpToDate => "up to date".to_owned(),
        Compat::NeedsUp => "needs migrate up".to_owned(),
        Compat::NewerSchemaOk => "newer schema, compatible".to_owned(),
        Compat::Incompatible { requires } => {
            format!("incompatible, schema requires ferrite >= {requires}")
        }
    };
    out.push_str(&format!("verdict: {verdict}\n"));
    out
}

#[cfg(test)]
mod tests {
    use ferrite_store::LOCK_MIGRATE;
    use ferrite_store_sqlite::{SqliteOptions, SqliteStore};

    use super::*;

    #[tokio::test]
    async fn migrate_up_and_down_take_the_migrate_lock() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
            .await
            .unwrap();
        let short = Duration::from_millis(300);
        let guard = store.try_lock(LOCK_MIGRATE).await.unwrap().unwrap();
        let e = up_locked(&store, None, short).await.unwrap_err();
        let msg = format!("{e:#}");
        assert!(
            msg.contains("has held the migration lock for 300ms"),
            "{msg}"
        );
        let e = down_locked(&store, 1, true, short).await.unwrap_err();
        assert!(format!("{e:#}").contains("migration lock"), "{e:#}");
        assert_eq!(
            store
                .migration_status(&crate::binary_version().unwrap())
                .await
                .unwrap()
                .db_version,
            0
        );
        guard.release().await.unwrap();

        let applied = up_locked(&store, None, short).await.unwrap();
        assert!(!applied.is_empty());
        // Released again afterwards.
        store
            .try_lock(LOCK_MIGRATE)
            .await
            .unwrap()
            .unwrap()
            .release()
            .await
            .unwrap();
    }
}
