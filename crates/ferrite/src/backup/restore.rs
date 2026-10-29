//! `ferrite restore` (M4-1 P14): a backup archive or a repository snapshot into an empty
//! data folder, or, with `--force`, in place of an existing one, which is moved aside to
//! `<dir>.pre-restore-<ts>` (never deleted) unless a running process holds its lock.
//!
//! Everything is first written to a staging folder next to the target
//! (`.<name>.restoring-<ts>`): the files, then the blobs through a media store opened from
//! the restored `ferrite.toml` (a relative local media root lands inside the staging
//! folder). Only when all of it is in place is the old folder moved aside and the staging
//! folder renamed to the target, so a failed restore changes nothing. The manifest is
//! checked before anything is written: a backup whose schema is newer than this binary
//! stops with `archive::Newer` (exit 3).

use std::fs::{File, TryLockError};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Args;
use ferrite_config::{Config, DbKind};
use ferrite_domain::ArtifactHash;
use ferrite_media::{MediaError, MediaStore};
use tokio::runtime::Handle;

use super::repo::{Repo, SNAPSHOT_FORMAT};
use super::target::Target;
use super::{CONFIG_FILE, DB_FILE};
use crate::archive::{ArchiveReader, EntryKind, Kind, Manifest, Newer};
use crate::keys::SECRET_KEY_FILE;
use crate::open::{OpenMode, open_store};
use crate::site::open_media;
use crate::site_copy::{LoadStats, store_blob};
use crate::{binary_version, load_config};

/// Exit code for a backup newer than this binary.
const NEWER: u8 = 3;
/// The SQLite folder lock `serve` holds.
const LOCK_FILE: &str = "ferrite.lock";

#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// A backup `.tar.zst`, `-` for stdin, or a repository (a folder or
    /// `s3://bucket[/prefix]`, credentials and endpoint from the AWS_* environment).
    pub source: String,
    /// The data folder to restore into: absent or empty, unless `--force`.
    #[arg(long)]
    pub data: PathBuf,
    /// Repository only: the snapshot to restore (`yyyymmddThhmmssZ`); default the newest.
    #[arg(long)]
    pub snapshot: Option<String>,
    /// Move an existing, non-empty data folder aside to `<data>.pre-restore-<ts>` first
    /// (refused while a running ferrite holds its lock).
    #[arg(long)]
    pub force: bool,
}

/// How to restore.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Repository only: the snapshot name; `None` is the newest.
    pub snapshot: Option<String>,
    /// Move a non-empty target aside.
    pub force: bool,
}

/// What a restore did.
#[derive(Debug, Clone)]
pub struct RestoreReport {
    /// The archive, or the repository snapshot.
    pub from: String,
    pub manifest: Manifest,
    /// Blobs written to the media store.
    pub blobs_stored: u64,
    /// Blobs the media store already had.
    pub blobs_present: u64,
    /// Where the old folder went.
    pub moved_aside: Option<PathBuf>,
}

/// The target folder's state, checked before anything is read.
struct TargetState {
    /// Exists and holds something (only with `force`).
    occupied: bool,
    /// Exists and is empty.
    empty: bool,
    /// The held `ferrite.lock` of an occupied target.
    _lock: Option<File>,
}

fn check_target(dir: &Path, force: bool) -> anyhow::Result<TargetState> {
    if !dir.exists() {
        return Ok(TargetState {
            occupied: false,
            empty: false,
            _lock: None,
        });
    }
    if !dir.is_dir() {
        bail!("{} exists and is not a folder", dir.display());
    }
    let empty = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .next()
        .is_none();
    if empty {
        return Ok(TargetState {
            occupied: false,
            empty: true,
            _lock: None,
        });
    }
    if !force {
        bail!(
            "{} is not empty; restore into a new folder, or pass --force to move it aside",
            dir.display()
        );
    }
    let lock_path = dir.join(LOCK_FILE);
    let lock = match File::options().read(true).write(true).open(&lock_path) {
        Ok(file) => {
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => bail!(
                    "data folder is in use: {} is locked by another process (stop `ferrite \
                     serve` first)",
                    lock_path.display()
                ),
                Err(TryLockError::Error(e)) => {
                    return Err(e).with_context(|| format!("cannot lock {}", lock_path.display()));
                }
            }
            Some(file)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("cannot open {}", lock_path.display())),
    };
    Ok(TargetState {
        occupied: true,
        empty: false,
        _lock: lock,
    })
}

/// Refuses what this binary cannot restore, before anything is written.
fn check_manifest(manifest: &Manifest, source: &str) -> anyhow::Result<()> {
    if manifest.kind != Kind::Backup {
        bail!("{source} is an export archive, not a backup; load it with `ferrite import-site`");
    }
    let latest = ferrite_store_sqlite::latest_schema_version();
    if manifest.schema_version > latest {
        return Err(Newer(format!(
            "the backup is at schema {}, newer than this binary ({latest}); restore it with \
             ferrite {} or newer",
            manifest.schema_version, manifest.ferrite_version
        ))
        .into());
    }
    if manifest.source_backend != "sqlite" {
        bail!(
            "the backup comes from a {} site; only SQLite backups can be restored",
            manifest.source_backend
        );
    }
    let files = manifest.files.as_deref().unwrap_or_default();
    if let Some(f) = files
        .iter()
        .find(|f| ![DB_FILE, CONFIG_FILE, SECRET_KEY_FILE].contains(&f.path.as_str()))
    {
        bail!("the backup lists an unexpected file {:?}", f.path);
    }
    if !files.iter().any(|f| f.path == DB_FILE) {
        bail!("the backup has no {DB_FILE}");
    }
    Ok(())
}

/// The staging folder, removed when dropped unless [`Staging::keep`] was called.
struct Staging {
    path: PathBuf,
    keep: bool,
}

impl Staging {
    fn create(dir: &Path, ts: &str) -> anyhow::Result<Staging> {
        let (parent, name) = parent_and_name(dir)?;
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        let path = parent.join(format!(".{name}.restoring-{ts}-{}", std::process::id()));
        std::fs::create_dir(&path).with_context(|| format!("cannot create {}", path.display()))?;
        Ok(Staging { path, keep: false })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep {
            // Only this run's own staging folder.
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn parent_and_name(dir: &Path) -> anyhow::Result<(PathBuf, String)> {
    let abs = std::path::absolute(dir)?;
    let name = abs
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no usable folder name", dir.display()))?
        .to_owned();
    let parent = abs.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok((parent, name))
}

/// Loads the restored configuration from the staging folder; its SQLite database must be
/// the folder's `ferrite.db`.
fn staged_config(staging: &Path) -> anyhow::Result<Config> {
    let config = load_config(staging).context("the restored ferrite.toml is not usable")?;
    match config.database_kind() {
        DbKind::Sqlite { path } if path == staging.join(DB_FILE) => Ok(config),
        DbKind::Sqlite { path } => bail!(
            "the restored configuration puts the database at {}, outside the data folder; \
             restore needs database.url = \"sqlite://ferrite.db\" (check ferrite.toml and \
             FERRITE_DATABASE__URL)",
            path.display()
        ),
        DbKind::Postgres { .. } => bail!(
            "the restored configuration (with the environment) names a Postgres database; \
             unset FERRITE_DATABASE__URL"
        ),
    }
}

/// Restores `source` into `dir` (P14). Prints nothing.
pub async fn restore(
    source: &Target,
    dir: &Path,
    opts: &RestoreOptions,
) -> anyhow::Result<RestoreReport> {
    if opts.snapshot.is_some() && !matches!(source, Target::Repo(_)) {
        bail!("--snapshot applies to a repository, not to an archive");
    }
    let state = check_target(dir, opts.force)?;
    let ts = chrono::Utc::now().format(SNAPSHOT_FORMAT).to_string();
    let (from, manifest, staging, stats) = match source {
        Target::Repo(loc) => {
            let repo = Repo::open(loc, false)?;
            let name = pick_snapshot(&repo, opts.snapshot.as_deref()).await?;
            let manifest = repo.read_manifest(&name).await?;
            let from = format!("snapshot {name} of {}", repo.display());
            check_manifest(&manifest, &from)?;
            let staging = Staging::create(dir, &ts)?;
            let stats = restore_repo(&repo, &name, &manifest, &staging.path).await?;
            (from, manifest, staging, stats)
        }
        Target::File(_) | Target::Stdout => {
            let input: Box<dyn Read + Send> = match source {
                Target::File(path) => Box::new(
                    File::open(path).with_context(|| format!("cannot open {}", path.display()))?,
                ),
                _ => Box::new(std::io::stdin()),
            };
            let from = source.describe();
            let (dir, ts, from2) = (dir.to_owned(), ts.clone(), from.clone());
            let handle = Handle::current();
            let (manifest, staging, stats) = tokio::task::spawn_blocking(move || {
                restore_archive(&handle, input, &dir, &ts, &from2)
            })
            .await??;
            (from, manifest, staging, stats)
        }
    };
    let moved_aside = put_in_place(staging, dir, &state, &ts)?;
    drop(state);
    Ok(RestoreReport {
        from,
        manifest,
        blobs_stored: stats.blobs_stored,
        blobs_present: stats.blobs_present,
        moved_aside,
    })
}

async fn pick_snapshot(repo: &Repo, wanted: Option<&str>) -> anyhow::Result<String> {
    let snapshots = repo.snapshots().await?;
    match wanted {
        Some(name) if snapshots.iter().any(|s| s == name) => Ok(name.to_owned()),
        Some(name) => bail!(
            "{} has no snapshot {name}; it has: {}",
            repo.display(),
            if snapshots.is_empty() {
                "none".to_owned()
            } else {
                snapshots.join(", ")
            }
        ),
        None => snapshots
            .last()
            .cloned()
            .with_context(|| format!("{} has no snapshots", repo.display())),
    }
}

async fn restore_repo(
    repo: &Repo,
    name: &str,
    manifest: &Manifest,
    staging: &Path,
) -> anyhow::Result<LoadStats> {
    for entry in manifest.files.iter().flatten() {
        repo.fetch_file(name, entry, &staging.join(&entry.path))
            .await?;
    }
    let config = staged_config(staging)?;
    let mut stats = LoadStats::default();
    if manifest.artifact_hashes.is_empty() {
        return Ok(stats);
    }
    let media = open_media(&config)?;
    for h in &manifest.artifact_hashes {
        let hash = ArtifactHash::parse(h).context("invalid artifact hash")?;
        if media.head(&hash).await?.is_some() {
            stats.blobs_present += 1;
            continue;
        }
        match repo.blobs().copy_to(&media, &hash).await {
            Ok(_) => stats.blobs_stored += 1,
            Err(MediaError::NotFound) => bail!(
                "{} lacks artifact {hash}, which snapshot {name} lists; the repository is damaged",
                repo.display()
            ),
            Err(e) => return Err(e).with_context(|| format!("cannot restore artifact {hash}")),
        }
    }
    Ok(stats)
}

/// Reads an archive into a new staging folder next to `dir` (blocking).
fn restore_archive(
    handle: &Handle,
    input: Box<dyn Read + Send>,
    dir: &Path,
    ts: &str,
    from: &str,
) -> anyhow::Result<(Manifest, Staging, LoadStats)> {
    let mut reader = ArchiveReader::new(input)?;
    let mut entries = reader.entries()?;
    let manifest = entries.manifest().clone();
    check_manifest(&manifest, from)?;
    let staging = Staging::create(dir, ts)?;
    let files = manifest.files.as_deref().unwrap_or_default().len();
    let mut seen = 0usize;
    let mut media: Option<Arc<MediaStore>> = None;
    let mut stats = LoadStats::default();
    while let Some(mut entry) = entries.next_entry()? {
        match entry.kind.clone() {
            EntryKind::File(name) => {
                let dest = staging.path.join(&name);
                let mut file = crate::init::create_private(&dest)?;
                std::io::copy(&mut entry, &mut file)
                    .with_context(|| format!("cannot restore {name}"))?;
                entry.finish()?;
                file.sync_all()?;
                seen += 1;
                if seen == files {
                    let config = staged_config(&staging.path)?;
                    if manifest.artifacts.is_some() {
                        media = Some(Arc::new(open_media(&config)?));
                    }
                }
            }
            EntryKind::Artifact(hash) => {
                if seen < files {
                    bail!("the archive has artifacts before its files");
                }
                store_blob(handle, media.as_ref(), &hash, entry, &mut stats)?;
            }
            EntryKind::Table { .. } => bail!("a backup archive holds no tables"),
        }
    }
    Ok((manifest, staging, stats))
}

/// Moves an occupied target aside (or removes an empty one), then renames the staging
/// folder to the target. Returns where the old folder went.
fn put_in_place(
    mut staging: Staging,
    dir: &Path,
    state: &TargetState,
    ts: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let mut aside = None;
    if state.occupied {
        let (parent, name) = parent_and_name(dir)?;
        let to = parent.join(format!("{name}.pre-restore-{ts}"));
        if to.exists() {
            bail!("{} already exists", to.display());
        }
        std::fs::rename(dir, &to)
            .with_context(|| format!("cannot move {} aside to {}", dir.display(), to.display()))?;
        aside = Some(to);
    } else if state.empty {
        std::fs::remove_dir(dir).with_context(|| format!("cannot replace {}", dir.display()))?;
    }
    if let Err(e) = std::fs::rename(&staging.path, dir) {
        if let Some(to) = &aside {
            let _ = std::fs::rename(to, dir);
        }
        return Err(e)
            .with_context(|| format!("cannot move the restored site to {}", dir.display()));
    }
    staging.keep = true;
    Ok(aside)
}

/// `ferrite restore`: exit 0, 3 when the backup is newer than this binary, 1 on any other
/// error.
pub async fn run_restore(args: RestoreArgs) -> anyhow::Result<ExitCode> {
    match restore_cmd(args).await {
        Ok(code) => Ok(code),
        Err(e) if e.downcast_ref::<Newer>().is_some() => {
            eprintln!("error: {e:#}");
            Ok(ExitCode::from(NEWER))
        }
        Err(e) => Err(e),
    }
}

async fn restore_cmd(args: RestoreArgs) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let source = Target::parse(&args.source, &cwd)?;
    let opts = RestoreOptions {
        snapshot: args.snapshot,
        force: args.force,
    };
    let report = restore(&source, &args.data, &opts).await?;
    let m = &report.manifest;
    println!(
        "Restored {} into {} (taken {} by ferrite {}, schema {}).",
        report.from,
        args.data.display(),
        m.created_at,
        m.ferrite_version,
        m.schema_version
    );
    if let Some(aside) = &report.moved_aside {
        println!("The previous folder was moved to {}.", aside.display());
    }
    match m.artifacts {
        None => println!(
            "The backup holds no media artifacts; copy them with `ferrite media sync` if the \
             media store does not have them."
        ),
        Some(_) => println!(
            "Stored {} artifacts in the media store ({} were already there).",
            report.blobs_stored, report.blobs_present
        ),
    }
    if !m.missing.is_empty() {
        println!(
            "note: {} artifacts were missing from the site's media store at backup time and are \
             not in the backup (manifest.json `missing`).",
            m.missing.len()
        );
    }
    let has_key = m.files.iter().flatten().any(|f| f.path == SECRET_KEY_FILE);
    if !has_key {
        println!(
            "note: the backup holds no secret.key. Copy the site's key into {} (or set \
             auth.secret_key), or two-factor sign-in secrets cannot be opened (`ferrite user \
             disable-totp` resets them).",
            args.data.display()
        );
    }
    let config = load_config(&args.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let binary = binary_version()?;
    let status = store.migration_status(&binary).await;
    store.close().await;
    let status = status?;
    print!("{}", crate::migrate::render_status(&status, &binary));
    if !status.pending.is_empty() {
        println!(
            "`ferrite serve` applies the pending migrations when database.auto_migrate is on; \
             otherwise run `ferrite migrate up`."
        );
    }
    Ok(ExitCode::SUCCESS)
}
