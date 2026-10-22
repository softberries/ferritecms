use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::{init, migrate};

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

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Init(args) => init::run(args).await,
        Command::Migrate(cmd) => migrate::run(cmd).await,
    }
}
