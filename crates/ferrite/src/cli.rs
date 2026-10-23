use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::config_cmd::ConfigCommand;
use crate::{config_cmd, healthcheck, init, migrate, seed, serve};

#[derive(Debug, Parser)]
#[command(name = "ferrite", version, about = "FerriteCMS")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a data folder with ferrite.toml and an up-to-date database.
    Init(init::InitArgs),
    /// Inspect and run database migrations.
    #[command(subcommand)]
    Migrate(MigrateCommand),
    /// Serve the site (stops cleanly on SIGINT/SIGTERM).
    Serve(DataArg),
    /// Add sample content.
    #[command(subcommand)]
    Seed(seed::SeedCommand),
    /// Validate the configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Probe a node's HTTP endpoint (for container health checks): exit 0 on 200, else 1.
    Healthcheck(healthcheck::HealthcheckArgs),
}

#[derive(Debug, Args)]
pub struct DataArg {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    /// Show database and binary versions, pending migrations and the verdict.
    ///
    /// Exit code: 0 when up to date or the schema is newer but compatible, 2 when
    /// migrations are pending, 3 when the schema is incompatible with this binary,
    /// 1 on any error.
    Status(DataArg),
    /// Apply pending migrations.
    Up {
        #[command(flatten)]
        data: DataArg,
        /// Stop after this migration version.
        #[arg(long)]
        to: Option<i64>,
    },
    /// Revert migrations down to a version.
    Down {
        #[command(flatten)]
        data: DataArg,
        /// Revert everything above this version.
        #[arg(long)]
        to: i64,
        /// Allow migrations whose down loses data.
        #[arg(long)]
        allow_lossy: bool,
        /// Do not ask for confirmation (required when stdin is not a terminal).
        #[arg(long)]
        yes: bool,
    },
    /// Check applied migration checksums against this binary.
    Verify(DataArg),
}

pub async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match cli.command {
        Command::Init(args) => init::run(args).await.map(|()| ExitCode::SUCCESS),
        Command::Migrate(cmd) => migrate::run(cmd).await,
        Command::Serve(args) => serve::run(args).await.map(|()| ExitCode::SUCCESS),
        Command::Seed(cmd) => seed::run(cmd).await.map(|()| ExitCode::SUCCESS),
        Command::Config(cmd) => config_cmd::run(cmd),
        Command::Healthcheck(args) => healthcheck::run(args).await,
    }
}
