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
    /// Prints `ok: mode=… database=… media=… workers=… mail=… backup=…` and exits 0, or prints every
    /// problem to stderr and exits 1.
    Check(DataArg),
}

pub fn run(cmd: ConfigCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        ConfigCommand::Check(args) => match ferrite_config::load(&args.data, utf8_env()?) {
            Ok(config) => {
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
            "media.s3.copy_if_not_exists = \"multipart\" places uploads with a multipart copy, which does not copy object attributes (content type, tags); prefer a header mode if your provider has one (03 §2)"
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
