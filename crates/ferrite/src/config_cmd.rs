//! `ferrite config check`: validate `ferrite.toml` + `FERRITE_*` without opening anything.

use std::process::ExitCode;

use clap::Subcommand;
use ferrite_config::{ClusterMode, Config, ConfigError, DbKind, MediaSection};

use crate::cli::DataArg;
use crate::utf8_env;

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Load ferrite.toml and FERRITE_* variables exactly as `serve` does, without
    /// touching the database or media.
    ///
    /// Prints `ok: mode=… database=… media=… workers=…` and exits 0, or prints every
    /// problem to stderr and exits 1.
    Check(DataArg),
}

pub fn run(cmd: ConfigCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        ConfigCommand::Check(args) => match ferrite_config::load(&args.data, utf8_env()?) {
            Ok(config) => {
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
    format!(
        "ok: mode={mode} database={database} media={media} workers={}",
        config.cluster.workers
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
