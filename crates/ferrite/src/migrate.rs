use std::io::{IsTerminal, Write};

use anyhow::bail;
use ferrite_store::{Compat, MigrationStatus, Phase};

use crate::cli::{DataArg, MigrateCommand};
use crate::open::open_store;
use crate::{binary_version, load_config};

pub async fn run(cmd: MigrateCommand) -> anyhow::Result<()> {
    match cmd {
        MigrateCommand::Status(DataArg { data }) => {
            let store = open_store(&load_config(&data)?).await?;
            let status = store.migration_status(&binary_version()?).await?;
            print!("{}", render_status(&status, &binary_version()?));
        }
        MigrateCommand::Up {
            data: DataArg { data },
            to,
        } => {
            let store = open_store(&load_config(&data)?).await?;
            let applied = store.migrate_up(to).await?;
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
            let store = open_store(&load_config(&data)?).await?;
            let reverted = store.migrate_down(to, allow_lossy).await?;
            if reverted.is_empty() {
                println!("Nothing to revert.");
            } else {
                println!("Reverted migrations: {}", join(&reverted));
            }
        }
        MigrateCommand::Verify(DataArg { data }) => {
            let store = open_store(&load_config(&data)?).await?;
            store.verify_migrations().await?;
            println!("Migration checksums OK.");
        }
    }
    Ok(())
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

fn render_status(status: &MigrationStatus, binary: &semver::Version) -> String {
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
