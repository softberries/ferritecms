//! `ferrite config check`: validate `ferrite.toml` + `FERRITE_*` without opening anything.

use std::process::ExitCode;

use clap::Subcommand;
use ferrite_config::{ClusterMode, Config, ConfigError, ConfigProblem, DbKind, MediaSection};

use crate::cli::DataArg;
use crate::utf8_env;

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Load ferrite.toml and FERRITE_* variables exactly as `serve` does, without
    /// touching the database or media.
    ///
    /// Prints `ok: mode=… database=… media=… workers=… mail=… backup=…` and exits 0, or prints every
    /// problem to stderr and exits 1.
    Check(DataArg),
}

pub fn run(cmd: ConfigCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        ConfigCommand::Check(args) => match ferrite_config::load(&args.data, utf8_env()?) {
            Ok(config) => {
                // The repository checks `serve` runs before it starts (no database needed).
                if let Err(e) = crate::jobs::backup_location(&config) {
                    let e = ConfigError {
                        problems: vec![ConfigProblem {
                            path: "backup.to".to_owned(),
                            message: format!("{e:#}"),
                        }],
                    };
                    eprintln!("{}", problems(&e));
                    return Ok(ExitCode::FAILURE);
                }
                // A scheduled SQLite backup must be restorable (B-I2).
                if config.backup.schedule != ferrite_config::BackupSchedule::Off
                    && let Err(e) = crate::backup::check_backup_database(&config)
                {
                    let e = ConfigError {
                        problems: vec![ConfigProblem {
                            path: "database.url".to_owned(),
                            message: format!("{e:#}"),
                        }],
                    };
                    eprintln!("{}", problems(&e));
                    return Ok(ExitCode::FAILURE);
                }
                for warning in warnings(&config) {
                    eprintln!("warning: {warning}");
                }
                println!("{}", summary(&config));
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => {
                eprintln!("{}", problems(&e));
                Ok(ExitCode::FAILURE)
            }
        },
    }
}

/// Settings that work but cost something; printed to stderr, the exit code stays 0.
fn warnings(config: &Config) -> Vec<String> {
    let mut out = Vec::new();
    if let MediaSection::S3 {
        copy_if_not_exists: Some(mode),
        ..
    } = &config.media
        && mode.trim() == "multipart"
    {
        out.push(
            "media.s3.copy_if_not_exists = \"multipart\" places uploads of 8 MiB and more with a multipart copy, which does not copy object attributes (content type, tags); Ferrite sets none on stored artifacts today, so this matters only if that changes (09 §4a lists \"multipart\" for AWS S3)"
                .to_owned(),
        );
    }
    out
}

fn summary(config: &Config) -> String {
    let mode = match config.effective_cluster_mode() {
        ClusterMode::Cluster => "cluster",
        // `effective_cluster_mode` never returns Auto.
        ClusterMode::Solo | ClusterMode::Auto => "solo",
    };
    let database = match config.database_kind() {
        DbKind::Sqlite { .. } => "sqlite",
        DbKind::Postgres { .. } => "postgres",
    };
    let media = match config.media {
        MediaSection::Local { .. } => "local",
        MediaSection::S3 { .. } => "s3",
    };
    let mail = if config.mail.is_some() {
        "configured"
    } else {
        "off"
    };
    format!(
        "ok: mode={mode} database={database} media={media} workers={} mail={mail} backup={}",
        config.cluster.workers,
        config.backup.schedule.as_str()
    )
}

/// One line per problem; `ConfigProblem` messages never contain secret values.
fn problems(e: &ConfigError) -> String {
    let mut out = format!(
        "error: invalid configuration ({} problem(s))",
        e.problems.len()
    );
    for p in &e.problems {
        out.push_str(&format!("\n  {}: {}", p.path, p.message));
    }
    out
}
