//! `ferrite restore` (M4-1 P14): a backup archive or a repository snapshot into an empty
//! data folder, or, with `--force`, in place of an existing one, which is moved aside to
//! `<dir>.pre-restore-<ts>` (never deleted) unless a running process holds its lock.
//!
//! Everything is first written to a staging folder: next to an absent target
//! (`.<name>.restoring-<ts>-<pid>`, renamed to the target at the end; inside the target,
//! then created, when the parent is not writable), inside an empty one
//! (`<dir>/.restoring-<ts>-<pid>`, so a mount point such as a container volume works; its
//! entries are moved in with `ferrite.db` last, so a folder holding the database is
//! complete), or next to an occupied target.
//! The files go there first, then the blobs through a media store opened from the restored
//! `ferrite.toml` (a relative local media root lands inside the staging folder). Only when
//! all of it is in place are the staged entries moved into the empty target, or the old
//! folder moved aside and the staging folder renamed to the target, so a failed restore
//! leaves the data folder as it was. The manifest is checked before anything is written: a
//! backup whose schema is newer than this binary stops with `archive::Newer` (exit 3).

use std::fs::{File, TryLockError};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Args;
use ferrite_config::{Config, DbKind, MediaSection};
use ferrite_media::{MediaError, MediaStore};
use ferrite_store::SCHEMA_META_INSTALL_ID;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode};
use tokio::runtime::Handle;

use super::repo::{Repo, SNAPSHOT_FORMAT, read_hash_list};
use super::target::{RepoLocation, Target};
use super::{ARTIFACT_LIST, CONFIG_FILE, DB_FILE};
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
/// Staging folders of killed restores older than this are removed at the start of a run.
const STALE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

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
    /// (refused while a running ferrite holds its lock, and for a folder that cannot be
    /// renamed, such as a mount point).
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

/// The target folder, checked before anything is read.
enum TargetState {
    /// Absent: staged next to it (or inside it, created, when that fails).
    Absent,
    /// Empty: staged inside it.
    Empty,
    /// Holds something (only with `force`): staged next to it, then moved aside; its
    /// `ferrite.lock` is held until then.
    Occupied { _lock: Option<File> },
}

/// [`TargetState`] without the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Absent,
    Empty,
    Occupied,
}

impl TargetState {
    fn shape(&self) -> Shape {
        match self {
            TargetState::Absent => Shape::Absent,
            TargetState::Empty => Shape::Empty,
            TargetState::Occupied { .. } => Shape::Occupied,
        }
    }
}

fn check_target(dir: &Path, force: bool) -> anyhow::Result<TargetState> {
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok(TargetState::Absent);
    }
    if !dir.is_dir() {
        bail!("{} exists and is not a folder", dir.display());
    }
    // A filesystem's `lost+found` does not count (a fresh volume has one, B-M7).
    if super::repo::dir_is_empty(dir)? {
        return Ok(TargetState::Empty);
    }
    if !force {
        bail!(
            "{} is not empty; restore into a new folder, or pass --force to move it aside",
            dir.display()
        );
    }
    check_movable(dir)?;
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
    Ok(TargetState::Occupied { _lock: lock })
}

/// `--force` renames the folder; a mount point (or a folder on another device than its
/// parent) cannot be renamed, so it is refused before any work.
fn check_movable(dir: &Path) -> anyhow::Result<()> {
    let (parent, _) = parent_and_name(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let own = std::fs::metadata(dir)?.dev();
        let up = std::fs::metadata(&parent)?.dev();
        if own != up {
            bail!(
                "{} is a mount point (or on another device than its parent), so --force cannot \
                 move it aside; empty it (keep a copy of what it holds) and restore into the \
                 empty folder instead",
                dir.display()
            );
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
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
    if let Some(f) = files.iter().find(|f| {
        ![DB_FILE, CONFIG_FILE, SECRET_KEY_FILE, ARTIFACT_LIST].contains(&f.path.as_str())
    }) {
        bail!("the backup lists an unexpected file {:?}", f.path);
    }
    if !files.iter().any(|f| f.path == DB_FILE) {
        bail!("the backup has no {DB_FILE}");
    }
    if manifest.artifacts.is_some() != files.iter().any(|f| f.path == ARTIFACT_LIST) {
        bail!("the backup's artifacts and {ARTIFACT_LIST} do not match");
    }
    Ok(())
}

/// The staging folder, removed when dropped unless [`Staging::keep`] was called (with the
/// target folder, when this run created it to stage inside).
struct Staging {
    path: PathBuf,
    /// Inside the target (its entries are moved in), or next to it (renamed).
    inside: bool,
    /// The target folder this run created.
    created: Option<PathBuf>,
    keep: bool,
}

impl Staging {
    /// Next to an absent target (inside it, created, when the parent refuses) or an
    /// occupied one; inside an empty one.
    fn create(dir: &Path, shape: Shape, ts: &str) -> anyhow::Result<Staging> {
        let pid = std::process::id();
        let (parent, name) = parent_and_name(dir)?;
        let sibling = parent.join(format!(".{name}.restoring-{ts}-{pid}"));
        let inside = dir.join(format!(".restoring-{ts}-{pid}"));
        let new = |path: PathBuf, inside: bool, created: Option<PathBuf>| {
            std::fs::create_dir(&path)
                .with_context(|| format!("cannot create {}", path.display()))?;
            anyhow::Ok(Staging {
                path,
                inside,
                created,
                keep: false,
            })
        };
        match shape {
            Shape::Occupied => new(sibling, false, None),
            Shape::Empty => new(inside, true, None),
            Shape::Absent => {
                let _ = std::fs::create_dir_all(&parent);
                if std::fs::create_dir(&sibling).is_ok() {
                    return Ok(Staging {
                        path: sibling,
                        inside: false,
                        created: None,
                        keep: false,
                    });
                }
                std::fs::create_dir(dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
                new(inside, true, Some(dir.to_path_buf()))
            }
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep {
            // Only this run's own staging folder (and the empty target it created).
            let _ = std::fs::remove_dir_all(&self.path);
            if let Some(dir) = &self.created {
                let _ = std::fs::remove_dir(dir);
            }
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
    let parent = abs
        .parent()
        .map(Path::to_path_buf)
        .with_context(|| format!("{} has no parent folder", dir.display()))?;
    Ok((parent, name))
}

/// Removes staging folders of killed restores older than a day: `<dir>/.restoring-*` and
/// `<parent>/.<name>.restoring-*` (ours by name).
fn sweep_stale(dir: &Path) {
    let Ok((parent, name)) = parent_and_name(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let sibling = format!(".{name}.restoring-");
    // `<prefix><yyyymmddThhmmssZ>-<pid>`, exactly as `Staging::create` names them.
    let is_staging = |file_name: &str, prefix: &str| {
        file_name
            .strip_prefix(prefix)
            .and_then(|rest| rest.split_once('-'))
            .is_some_and(|(ts, pid)| {
                super::repo::is_snapshot_name(ts)
                    && !pid.is_empty()
                    && pid.bytes().all(|b| b.is_ascii_digit())
            })
    };
    for (folder, prefix) in [
        (dir.to_path_buf(), ".restoring-"),
        (parent, sibling.as_str()),
    ] {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let ours = file_name.to_str().is_some_and(|n| is_staging(n, prefix));
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > STALE);
            if ours && old && entry.path().is_dir() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
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

/// M4-4 P68: refuses a restored local media root that is absolute and inside `dir` when
/// `dir` is replaced (an occupied target, moved aside) or created by renaming the staging
/// folder (an absent one): the blobs would go into the old folder (or make the target
/// appear), and the restored site would have none. Staged inside the target (an empty
/// one), the target stays, and so does a root inside it.
fn check_media_root(config: &Config, staging: &Staging, dir: &Path) -> anyhow::Result<()> {
    if staging.inside {
        return Ok(());
    }
    let MediaSection::Local { root, .. } = &config.media else {
        return Ok(());
    };
    let (Ok(resolved), Ok(target)) = (super::resolve(root), super::resolve(dir)) else {
        return Ok(());
    };
    if !resolved.starts_with(&target) {
        return Ok(());
    }
    let (what, first) = if occupied(dir) {
        (
            "which --force moves aside",
            format!("move {} aside yourself, then ", dir.display()),
        )
    } else {
        ("which does not exist yet", String::new())
    };
    bail!(
        "the restored configuration keeps the media at {}, inside {}, {what}: the restored \
         site would be left without its media. Restore into an empty folder instead ({first}\
         create {} empty and run the restore again), or point [media] root \
         (FERRITE_MEDIA__ROOT) elsewhere; nothing was changed",
        root.display(),
        dir.display(),
        dir.display()
    )
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
    sweep_stale(dir);
    let state = check_target(dir, opts.force)?;
    let ts = chrono::Utc::now().format(SNAPSHOT_FORMAT).to_string();
    let (from, manifest, staging, stats) = match source {
        Target::Repo(loc) => {
            let repo = Repo::open(loc, false).await?;
            let name = pick_snapshot(&repo, opts.snapshot.as_deref()).await?;
            let manifest = repo.read_manifest(&name).await?;
            let from = format!("snapshot {name} of {}", repo.display());
            check_manifest(&manifest, &from)?;
            let staging = Staging::create(dir, state.shape(), &ts)?;
            let stats = restore_repo(&repo, &name, &manifest, &staging, dir).await?;
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
            let shape = state.shape();
            let (dir, ts, from2) = (dir.to_owned(), ts.clone(), from.clone());
            let handle = Handle::current();
            let (manifest, staging, stats) = tokio::task::spawn_blocking(move || {
                restore_archive(&handle, input, &dir, shape, &ts, &from2)
            })
            .await??;
            (from, manifest, staging, stats)
        }
    };
    new_install(&staging.path.join(DB_FILE)).await?;
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

/// Makes the staged database a new install (R16): a fresh `schema_meta.install_id`, so the
/// restored site's media GC never deletes from a store the original site owns, no
/// artifact keeps a GC mark of the original's (the grace period starts over here), and the
/// snapshot's own running `site.backup` row is done (M4-4 P69).
async fn new_install(db: &Path) -> anyhow::Result<()> {
    let opts = SqliteConnectOptions::new()
        .filename(db)
        .journal_mode(SqliteJournalMode::Delete);
    let mut conn = SqliteConnection::connect_with(&opts)
        .await
        .context("cannot open the restored database")?;
    let result = async {
        let mut tx = conn.begin().await?;
        sqlx::query(
            "INSERT INTO schema_meta (key, value) VALUES (?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(SCHEMA_META_INSTALL_ID)
        .bind(uuid::Uuid::now_v7().to_string())
        .execute(&mut *tx)
        .await?;
        // A backup older than migration 0011 has no marks.
        let marks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pragma_table_info('artifacts') WHERE name = 'orphaned_at'",
        )
        .fetch_one(&mut *tx)
        .await?;
        if marks > 0 {
            sqlx::query("UPDATE artifacts SET orphaned_at = NULL WHERE orphaned_at IS NOT NULL")
                .execute(&mut *tx)
                .await?;
        }
        // M4-4 P69: a snapshot the `site.backup` job wrote holds that job's own row as
        // `running`; it is done (it made this snapshot), and its lease must not run the
        // backup again here. Its key stays taken, so the period gets no second backup.
        // Other running rows are left to lease reclaim, as after a crash. A backup older
        // than migration 0004 has no `jobs`.
        if ferrite_store_sqlite::table_exists(&mut tx, "jobs").await? {
            sqlx::query(
                "UPDATE jobs SET status = 'done', locked_by = NULL, locked_until = NULL, \
                 finished_at = ? WHERE kind = ? AND status = 'running'",
            )
            .bind(ferrite_domain::Timestamp::now().as_micros())
            .bind(crate::jobs::SITE_BACKUP_KIND)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        anyhow::Ok(())
    }
    .await;
    let _ = conn.close().await;
    result.context("cannot give the restored database its own install id")
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
        None => match snapshots.last() {
            Some(name) => Ok(name.clone()),
            None if !repo.exports().await?.is_empty() => bail!(
                "{} has no snapshots, only export archives (in {}/): a Postgres site's \
                 scheduled backups; load one with `ferrite import-site`",
                repo.display(),
                super::repo::EXPORTS
            ),
            None => bail!("{} has no snapshots", repo.display()),
        },
    }
}

async fn restore_repo(
    repo: &Repo,
    name: &str,
    manifest: &Manifest,
    staging: &Staging,
    dir: &Path,
) -> anyhow::Result<LoadStats> {
    for entry in manifest.files.iter().flatten() {
        if entry.path != ARTIFACT_LIST {
            repo.fetch_file(name, entry, &staging.path.join(&entry.path))
                .await?;
        }
    }
    let config = staged_config(&staging.path)?;
    check_media_root(&config, staging, dir)?;
    let mut stats = LoadStats::default();
    let scratch = tempfile::Builder::new()
        .prefix("ferrite-restore-")
        .tempdir()
        .context("cannot create a temporary folder")?;
    let Some(list) = repo
        .fetch_artifact_list(name, manifest, scratch.path())
        .await?
    else {
        return Ok(stats);
    };
    let media = open_media(&config)?;
    let count = manifest.artifacts.map_or(0, |a| a.count);
    for hash in read_hash_list(File::open(&list)?, count) {
        let hash = hash.with_context(|| format!("snapshot {name}"))?;
        if media.head(&hash).await?.is_some() {
            stats.blobs_present += 1;
            continue;
        }
        match repo.blobs().copy_to(&media, &hash).await {
            Ok(_) => stats.blobs_stored += 1,
            Err(MediaError::NotFound) => bail!(
                "{} lacks artifact {hash}, which snapshot {name} lists: the repository is \
                 damaged, or a backup with --keep pruned this snapshot while it was being \
                 restored (then restore again, picking a snapshot that is still there)",
                repo.display()
            ),
            Err(e) => return Err(e).with_context(|| format!("cannot restore artifact {hash}")),
        }
    }
    Ok(stats)
}

/// Reads an archive into a new staging folder (blocking).
fn restore_archive(
    handle: &Handle,
    input: Box<dyn Read + Send>,
    dir: &Path,
    shape: Shape,
    ts: &str,
    from: &str,
) -> anyhow::Result<(Manifest, Staging, LoadStats)> {
    let mut reader = ArchiveReader::new(input)?;
    let mut entries = reader.entries()?;
    let manifest = entries.manifest().clone();
    check_manifest(&manifest, from)?;
    let staging = Staging::create(dir, shape, ts)?;
    let files = manifest.files.as_deref().unwrap_or_default().len();
    let mut seen = 0usize;
    let mut media: Option<Arc<MediaStore>> = None;
    let mut stats = LoadStats::default();
    while let Some(mut entry) = entries.next_entry()? {
        match entry.kind.clone() {
            EntryKind::File(name) if name == ARTIFACT_LIST => {
                // Checked (line by line, then its sha256), not restored.
                let count = manifest.artifacts.map_or(0, |a| a.count);
                for hash in read_hash_list(&mut entry, count) {
                    hash?;
                }
                entry.finish()?;
                seen += 1;
            }
            EntryKind::File(name) => {
                let dest = staging.path.join(&name);
                let mut file = crate::init::create_private(&dest)?;
                std::io::copy(&mut entry, &mut file)
                    .with_context(|| format!("cannot restore {name}"))?;
                entry.finish()?;
                file.sync_all()?;
                seen += 1;
            }
            EntryKind::Artifact(hash) => {
                if seen < files {
                    bail!("the archive has artifacts before its files");
                }
                store_blob(handle, media.as_ref(), &hash, entry, &mut stats)?;
                continue;
            }
            EntryKind::Table { .. } => bail!("a backup archive holds no tables"),
        }
        if seen == files {
            let config = staged_config(&staging.path)?;
            check_media_root(&config, &staging, dir)?;
            if manifest.artifacts.is_some() {
                media = Some(Arc::new(open_media(&config)?));
            }
        }
    }
    Ok((manifest, staging, stats))
}

/// Whether anything (a file, folder or link) is at `path`.
fn occupied(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Puts the staged site in place.
///
/// - Staged inside the target: moves the entries into it, `ferrite.db` last (a folder
///   holding the database is complete), refusing any name that appeared meanwhile.
/// - Staged next to an absent target: renames the staging folder to it.
/// - Occupied target: renames it aside, then the staging folder to it; if that fails it is
///   renamed back.
///
/// Returns where the old folder went.
fn put_in_place(
    mut staging: Staging,
    dir: &Path,
    state: &TargetState,
    ts: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if staging.inside {
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(&staging.path)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<std::io::Result<_>>()?;
        names.sort_by_key(|n| n == DB_FILE);
        let mut moved: Vec<std::ffi::OsString> = Vec::new();
        let result = (|| -> anyhow::Result<()> {
            for name in names {
                let to = dir.join(&name);
                if occupied(&to) {
                    bail!(
                        "{} appeared during the restore; refusing to replace it",
                        to.display()
                    );
                }
                std::fs::rename(staging.path.join(&name), &to).with_context(|| {
                    format!("cannot move the restored files into {}", dir.display())
                })?;
                moved.push(name);
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Back into staging, which is then removed.
            for name in moved.iter().rev() {
                let _ = std::fs::rename(dir.join(name), staging.path.join(name));
            }
            return Err(e);
        }
        std::fs::remove_dir(&staging.path)
            .with_context(|| format!("cannot remove {}", staging.path.display()))?;
        staging.keep = true;
        return Ok(None);
    }
    if let TargetState::Absent = state {
        if occupied(dir) {
            bail!(
                "{} appeared during the restore; refusing to replace it",
                dir.display()
            );
        }
        std::fs::rename(&staging.path, dir)
            .with_context(|| format!("cannot move the restored site to {}", dir.display()))?;
        staging.keep = true;
        return Ok(None);
    }
    let (parent, name) = parent_and_name(dir)?;
    let aside = parent.join(format!("{name}.pre-restore-{ts}"));
    if occupied(&aside) {
        bail!("{} already exists", aside.display());
    }
    std::fs::rename(dir, &aside).with_context(|| {
        format!(
            "cannot move {} aside to {} (for a mount point, restore into an empty folder \
             instead)",
            dir.display(),
            aside.display()
        )
    })?;
    if occupied(dir) {
        let _ = std::fs::rename(&aside, dir);
        bail!(
            "{} appeared during the restore; the old site stays at {}",
            dir.display(),
            aside.display()
        );
    }
    if let Err(e) = std::fs::rename(&staging.path, dir) {
        if let Err(back) = std::fs::rename(&aside, dir) {
            return Err(e).with_context(|| {
                format!(
                    "cannot move the restored site to {}, nor the old folder back ({back}); the \
                     old site is at {}",
                    dir.display(),
                    aside.display()
                )
            });
        }
        return Err(e)
            .with_context(|| format!("cannot move the restored site to {}", dir.display()));
    }
    staging.keep = true;
    Ok(Some(aside))
}

/// `ferrite restore`: exit 0, 2 when the site is restored but the check afterwards failed
/// (M4-4 P68), 3 when the backup is newer than this binary, 1 on any other error (nothing
/// was put in place).
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
    // M4-4 P68: the site is in place now (the old folder moved aside); a failure from here
    // on is "done, with something to look at", not "nothing happened".
    match after_restore(&args.data).await {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            eprintln!(
                "error: the site was restored into {}, but restore could not check it \
                 afterwards: {e:#}. Look at it with `ferrite migrate status --data {}` and \
                 `ferrite doctor --data {}` before serving it.",
                args.data.display(),
                args.data.display(),
                args.data.display()
            );
            Ok(ExitCode::from(2))
        }
    }
}

/// What `restore` prints about the restored folder: config warnings and the migration
/// status of the restored database.
async fn after_restore(data: &Path) -> anyhow::Result<()> {
    let config = load_config(data)?;
    if let Some(warning) = super::shared_media_warning(&config) {
        eprintln!("{warning}");
    }
    // R14: the restored copy would back itself up into the same repository.
    if config.backup.schedule != ferrite_config::BackupSchedule::Off {
        let to = config.backup.to.as_deref().unwrap_or_default();
        let to = match Target::parse(to, &config.data_dir) {
            // The folder without `.` and `..` (lexically), as the operator would name it.
            Ok(Target::Repo(RepoLocation::Dir(dir))) => {
                let mut out = PathBuf::new();
                for c in std::path::absolute(&dir).unwrap_or(dir).components() {
                    match c {
                        std::path::Component::CurDir => {}
                        std::path::Component::ParentDir => {
                            out.pop();
                        }
                        c => out.push(c),
                    }
                }
                out.display().to_string()
            }
            Ok(t) => t.describe(),
            Err(_) => to.to_owned(),
        };
        eprintln!(
            "warning: the restored ferrite.toml schedules backups ([backup] schedule = \"{}\") \
             to {to}; a `serve` of this folder writes into that repository too. Set \
             FERRITE_BACKUP__SCHEDULE=off (or change [backup]) before serving a copy that must \
             not.",
            config.backup.schedule.as_str()
        );
    }
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
    Ok(())
}
