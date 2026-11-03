//! `ferrite config check`: validate `ferrite.toml` + `FERRITE_*` without opening anything.

use std::path::Path;
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
    /// Prints `ok: mode=… database=… media=… workers=… mail=… metrics=… tls=… backup=… webhooks=…` and exits 0, or prints every
    /// problem to stderr and exits 1.
    Check(DataArg),
}

pub fn run(cmd: ConfigCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        ConfigCommand::Check(args) => match check(&args.data, utf8_env()?) {
            Checked::Ok { config, warnings } => {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
                println!("{}", summary(&config));
                Ok(ExitCode::SUCCESS)
            }
            Checked::Refused { problem, .. } => {
                let e = ConfigError {
                    problems: vec![problem],
                };
                eprintln!("{}", problems(&e));
                Ok(ExitCode::FAILURE)
            }
            Checked::Invalid(e) => {
                eprintln!("{}", problems(&e));
                Ok(ExitCode::FAILURE)
            }
        },
    }
}

/// What loading and checking `ferrite.toml` + `FERRITE_*` found; shared by `config check`
/// and `ferrite doctor`.
pub(crate) enum Checked {
    /// Loaded, and the offline repository checks passed; `warnings` are settings that work
    /// but cost something (or do nothing).
    Ok {
        config: Config,
        warnings: Vec<String>,
    },
    /// Loaded, but one of the checks `serve` runs before it starts refused it.
    Refused {
        config: Config,
        problem: ConfigProblem,
    },
    /// Did not load: every problem found.
    Invalid(ConfigError),
}

/// Loads the configuration exactly as `serve` does, then runs the offline checks `serve`
/// runs before it starts. Touches no database, media store or network.
pub(crate) fn check(data_dir: &Path, env: Vec<(String, String)>) -> Checked {
    let config = match ferrite_config::load(data_dir, env) {
        Ok(config) => config,
        Err(e) => return Checked::Invalid(e),
    };
    // The repository checks `serve` runs before it starts (no database needed).
    if let Err(e) = crate::jobs::backup_location(&config) {
        let problem = ConfigProblem {
            path: "backup.to".to_owned(),
            message: format!("{e:#}"),
        };
        return Checked::Refused { config, problem };
    }
    // A scheduled SQLite backup must be restorable (B-I2).
    if config.backup.schedule != ferrite_config::BackupSchedule::Off
        && let Err(e) = crate::backup::check_backup_database(&config)
    {
        let problem = ConfigProblem {
            path: "database.url".to_owned(),
            message: format!("{e:#}"),
        };
        return Checked::Refused { config, problem };
    }
    let warnings = warnings(&config);
    Checked::Ok { config, warnings }
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
    // M6-2: a daily registry check that has no registry to check (this build has no
    // embedded key and none is configured).
    if config.plugins.trust.registry_check == ferrite_config::RegistryCheck::Daily
        && matches!(crate::registry::configured(&config.plugins.trust), Ok(None))
    {
        out.push(
            "plugins.registry_check = daily has no effect: this build has no key for the default registry; set plugins.registry_key"
                .to_owned(),
        );
    }
    out.extend(
        config
            .warnings
            .iter()
            .map(|w| format!("{}: {}", w.path, w.message)),
    );
    out
}

fn summary(config: &Config) -> String {
    format!("ok: {}", summary_fields(config))
}

/// `mode=… database=… media=… workers=… mail=… metrics=… tls=… backup=… webhooks=…`.
pub(crate) fn summary_fields(config: &Config) -> String {
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
    let metrics = config
        .server
        .metrics_listen
        .map_or_else(|| "off".to_owned(), |at| at.to_string());
    format!(
        "mode={mode} database={database} media={media} workers={} mail={mail} metrics={metrics} tls={} backup={} webhooks={}",
        config.cluster.workers,
        config.server.tls.mode(),
        config.backup.schedule.as_str(),
        if config.webhooks.local_targets {
            "local"
        } else {
            "off"
        }
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
