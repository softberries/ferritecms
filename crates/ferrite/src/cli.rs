use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::config_cmd::ConfigCommand;
use crate::ext_cmd::{PluginCommand, ThemeCommand};
use crate::{
    backup, config_cmd, doctor, export_cmd, ext_cmd, healthcheck, import_cmd, init, media_cmd,
    migrate, seed, serve, update, user_cmd,
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
    /// Import content from another CMS or a folder of Markdown files.
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
    /// With `--with-media`, an artifact whose file is missing from the media store is left
    /// out (one `missing artifact <hash>` line each on stderr, listed in the manifest); a
    /// file whose content does not match its hash fails the export.
    ///
    /// Exit code: 0 ok, 2 done but artifacts were missing from the media store, 3 when the
    /// database schema is newer than this binary, 1 on any other error (an older schema:
    /// run `ferrite migrate up` first); a failed export removes its file.
    Export(export_cmd::ExportArgs),
    /// Load an export archive into a fresh site (made by `ferrite init`).
    ///
    /// The database must be fresh: no rows but the built-in content types and taxonomies.
    /// An archive from an older schema is loaded at that schema, then migrated up; the
    /// search index is rebuilt. The rows land in one transaction, so a refused, damaged or
    /// truncated archive leaves no rows; a schema step taken for the archive is undone
    /// (best effort, a warning says when it failed), so the import can be retried.
    ///
    /// Exit code: 0 ok, 3 when the archive's format or schema is newer than this binary,
    /// 1 on any other error.
    ImportSite(export_cmd::ImportSiteArgs),
    /// Back up a solo (SQLite) site: database snapshot, ferrite.toml, secret.key and media.
    ///
    /// The database is copied with `VACUUM INTO` and checked with `PRAGMA integrity_check`
    /// before anything is written. `--to` is a `.tar.zst` file (mode 0600), `-` for stdout,
    /// or a repository: a folder or `s3://bucket[/prefix]` holding shared
    /// `artifacts/<ab>/<hash>` (copied only when missing) and
    /// `snapshots/<yyyymmddThhmmssZ>/`. Like every SQLite command it needs `serve` stopped
    /// (a running site is backed up by the `[backup]` schedule). Postgres sites are refused:
    /// use the provider's backups and `ferrite export`. Nothing is encrypted: pipe `--to -`
    /// to age or gpg.
    ///
    /// An artifact whose file is missing from the media store is left out (one `missing
    /// artifact <hash>` line each on stderr, listed in the manifest); a file whose content
    /// does not match its hash fails the backup.
    ///
    /// Exit code: 0 ok, 2 done but artifacts were missing from the media store, 3 when the
    /// repository is of a newer format than this binary, 1 on any other error (a failed
    /// backup leaves no archive or snapshot, except that a failed `--keep` prune keeps the
    /// snapshot it follows). A repository folder or prefix must be empty or carry
    /// ferrite-backup-repo.json, and lie outside the data folder and the media store.
    Backup(backup::BackupArgs),
    /// Restore a backup into an empty data folder (or, with --force, in place of one).
    ///
    /// The source is a backup `.tar.zst`, `-` for stdin, or a repository (folder or
    /// `s3://`; the newest snapshot unless `--snapshot`). With `--force` an existing folder
    /// is moved aside to `<data>.pre-restore-<ts>` (never deleted), refused while a running
    /// ferrite holds its lock. Blobs go to the media store the restored ferrite.toml names
    /// (those present are skipped). The database is not migrated: the command prints the
    /// migration status and `serve` migrates under `database.auto_migrate`. A failed restore
    /// leaves the data folder as it was. An empty folder (a mount point too) is restored into
    /// in place; `--force` refuses a mount point.
    ///
    /// Exit code: 0 ok, 3 when the backup's schema is newer than this binary (nothing is
    /// written), 1 on any other error.
    Restore(backup::RestoreArgs),
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
    /// Update this binary to a newer signed release (it does not restart anything).
    ///
    /// Looks up the newest stable release (or `--version X`) of softberries/ferritecms on
    /// GitHub, downloads this target's asset with `SHA256SUMS` and `SHA256SUMS.minisig`,
    /// checks the minisign signature with the key built into this binary, then the asset's
    /// sha256, unpacks it beside the binary and runs the new binary's `--version` (with
    /// `--data`, also its `migrate status`). After confirmation (`--yes` without a
    /// terminal) the running binary is copied to `ferrite.prev` and the new one is renamed
    /// over it. Restart the service afterwards. Any refusal leaves the binary untouched; a
    /// build without a release key refuses.
    ///
    /// Exit code: 0 updated or already up to date, 2 a newer release exists (`--check`),
    /// 1 on any error (an older or equal `--version`: use `ferrite rollback`).
    SelfUpdate(update::SelfUpdateArgs),
    /// Go back to the previous binary (`ferrite.prev`), and with `--schema` the schema too.
    ///
    /// Without `--schema` it swaps `ferrite` and `ferrite.prev` (run it with either; running
    /// it again re-upgrades). With `--data` it first runs the kept binary's `migrate status`
    /// and refuses (exit 3) when that binary cannot open the schema: roll the schema back
    /// with `--schema` first.
    ///
    /// `--schema` (needs `--data`) reverts to the `schema_from` of `<data>/upgrade.json` and
    /// must run as the binary the record names as `to` (after a binary rollback that is
    /// `ferrite.prev`). It runs the down migrations under the migrate lock (with SQLite
    /// `serve` must be stopped); a lossy step needs `--allow-lossy`. With
    /// `--restore-snapshot` (SQLite only) the recorded pre-upgrade snapshot replaces the
    /// database instead: everything written since the upgrade is lost, so it asks first
    /// (`--yes` without a terminal), and the current database with its `-wal`/`-shm` is
    /// moved to `backups/rolled-back-<ts>.db`, never deleted. Then it swaps the binaries
    /// (unless run as `ferrite.prev`) and renames `upgrade.json` to
    /// `upgrade-rolled-back-<ts>.json`. It restarts nothing.
    ///
    /// Exit code: 0 rolled back, 3 the previous binary cannot open this schema, 1 on any
    /// other error (nothing changed, unless the message says otherwise).
    Rollback(update::rollback::RollbackArgs),
    /// Probe a node's HTTP endpoint (for container health checks): exit 0 on 200, else 1.
    Healthcheck(healthcheck::HealthcheckArgs),
    /// Check a site and report what is wrong, changing nothing.
    ///
    /// Prints one line per check, `<status>  <check>  <detail>`, with the status one of
    /// ok, info, warn, error or skip, then `doctor: N ok, N warn, N error, N skip`. It
    /// checks the version, the configuration, file modes and ownership, the database, the
    /// media store, the cluster, the clock, the backup target, SMTP, TLS and the latest
    /// release (09 §8). It creates and writes no file, folder, key or schema, works while
    /// `serve` runs, and never prints a secret. `--offline` skips the checks that reach
    /// other services (S3, SMTP, the ACME directory, the release lookup); the database is
    /// still opened.
    ///
    /// Exit code: 3 when the database schema is newer than this binary allows, else 1 when
    /// a check is an error, else 2 when one is a warning, else 0.
    Doctor(doctor::DoctorArgs),
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
        Command::Backup(args) => backup::run_backup_cmd(args).await,
        Command::Restore(args) => backup::run_restore(args).await,
        Command::Config(cmd) => config_cmd::run(cmd),
        Command::Healthcheck(args) => healthcheck::run(args).await,
        Command::Doctor(args) => doctor::execute(args).await,
        Command::SelfUpdate(args) => update::run(args).await,
        Command::Rollback(args) => update::rollback::run(args).await,
        Command::Theme(cmd) => ext_cmd::run_theme(cmd).await.map(|()| ExitCode::SUCCESS),
        Command::Plugin(cmd) => ext_cmd::run_plugin(cmd).await.map(|()| ExitCode::SUCCESS),
        Command::User(cmd) => user_cmd::run(cmd).await.map(|()| ExitCode::SUCCESS),
    }
}
