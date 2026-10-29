use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::config_cmd::ConfigCommand;
use crate::ext_cmd::{PluginCommand, ThemeCommand};
use crate::{
    config_cmd, export_cmd, ext_cmd, healthcheck, import_cmd, init, media_cmd, migrate, seed,
    serve, user_cmd,
};

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
    /// Import content from another CMS.
    #[command(subcommand)]
    Import(import_cmd::ImportCommand),
    /// Media storage tools.
    #[command(subcommand)]
    Media(media_cmd::MediaCommand),
    /// Export the site's database (and optionally its media) to a portable archive.
    ///
    /// The archive (`tar.zst`, mode 0600) holds every table except transient and derived
    /// ones (sessions, jobs, events, search, …), read from one consistent snapshot, so a
    /// running Postgres site can be exported; with SQLite stop `serve` first. It holds
    /// password hashes and sealed secrets but never secret.key. The database must be at
    /// this binary's schema.
    ///
    /// Exit code: 0 ok, 3 when the database schema is newer than this binary, 1 on any
    /// other error (an older schema: run `ferrite migrate up` first).
    Export(export_cmd::ExportArgs),
    /// Load an export archive into a fresh site (made by `ferrite init`).
    ///
    /// The database must be fresh: no rows but the built-in content types and taxonomies.
    /// An archive from an older schema is loaded at that schema, then migrated up; the
    /// search index is rebuilt. Everything lands in one transaction, so a refused, damaged
    /// or truncated archive leaves the database as it was.
    ///
    /// Exit code: 0 ok, 3 when the archive's format or schema is newer than this binary,
    /// 1 on any other error.
    ImportSite(export_cmd::ImportSiteArgs),
    /// Database tools.
    #[command(subcommand)]
    Db(export_cmd::DbCommand),
    /// Validate the configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Manage user accounts (the operator path: no login needed).
    #[command(subcommand)]
    User(user_cmd::UserCommand),
    /// Install and manage themes (the operator path: no login needed).
    #[command(subcommand)]
    Theme(ThemeCommand),
    /// Install and manage plugins (the operator path: no login needed).
    #[command(subcommand)]
    Plugin(PluginCommand),
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
        Command::Import(cmd) => import_cmd::run(cmd).await,
        Command::Media(cmd) => media_cmd::run(cmd).await,
        Command::Export(args) => export_cmd::run_export(args).await,
        Command::ImportSite(args) => export_cmd::run_import_site(args).await,
        Command::Db(cmd) => export_cmd::run_db(cmd).await,
        Command::Config(cmd) => config_cmd::run(cmd),
        Command::Healthcheck(args) => healthcheck::run(args).await,
        Command::Theme(cmd) => ext_cmd::run_theme(cmd).await.map(|()| ExitCode::SUCCESS),
        Command::Plugin(cmd) => ext_cmd::run_plugin(cmd).await.map(|()| ExitCode::SUCCESS),
        Command::User(cmd) => user_cmd::run(cmd).await.map(|()| ExitCode::SUCCESS),
    }
}
