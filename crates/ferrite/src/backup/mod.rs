//! `ferrite backup` and `ferrite restore` for solo (SQLite) sites (M4-1 P13–P14).
//!
//! A backup is a `VACUUM INTO` snapshot of the database, checked with
//! `PRAGMA integrity_check`, plus `ferrite.toml`, `secret.key` (unless left out) and the
//! blobs of every `artifacts` row. It goes to one `.tar.zst` archive (P7, `kind: "backup"`)
//! or to a repository ([`repo`]). [`run_backup`] takes an open store and media store and
//! returns a [`BackupReport`] without printing, so the CLI and the scheduled job share it.

pub mod repo;
mod restore;
pub mod target;

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Args;
use ferrite_config::{Config, DbKind};
use ferrite_domain::{ArtifactHash, to_hex};
use ferrite_media::MediaStore;
use ferrite_store::{Backend, Store};
use sha2::{Digest, Sha256};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use tokio::runtime::Handle;

use crate::archive::{ArchiveWriter, ArtifactsInfo, FORMAT, FileEntry, Kind, Manifest};
use crate::keys::SECRET_KEY_FILE;
use crate::open::{OpenMode, open_store};
use crate::site::open_media;
use crate::{binary_version, load_config};

pub use restore::{RestoreArgs, RestoreOptions, RestoreReport, restore, run_restore};
use target::Target;

/// The database file in a backup.
pub(crate) const DB_FILE: &str = "ferrite.db";
/// The configuration file in a backup.
pub(crate) const CONFIG_FILE: &str = "ferrite.toml";
/// Exit code of a backup that lacks blobs missing from the media store (R10).
const MISSING_BLOBS: u8 = 2;
/// Why a Postgres site is refused.
const POSTGRES: &str = "`ferrite backup` backs up SQLite sites; back up a Postgres site with \
                        the provider's automated backups and point-in-time recovery, and use \
                        `ferrite export --with-media` for a portable copy";

#[derive(Debug, Args)]
pub struct BackupArgs {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
    /// Where to write: a `.tar.zst` file (refused if it exists), `-` for stdout, or a
    /// repository (a folder, created if missing, or `s3://bucket[/prefix]`, credentials
    /// and endpoint from the AWS_* environment).
    #[arg(long)]
    pub to: String,
    /// Leave the media artifacts out.
    #[arg(long)]
    pub no_media: bool,
    /// Leave secret.key out (TOTP secrets then cannot be opened after a restore without it).
    #[arg(long)]
    pub no_secret_key: bool,
    /// Repository only: keep the newest N snapshots, delete older ones and the artifacts no
    /// kept snapshot lists.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1000))]
    pub keep: Option<u32>,
    /// Test seam: damage the local snapshot before it is checked.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub test_corrupt_snapshot: bool,
}

/// What to back up besides the database.
#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// Include `<data>/secret.key` when it exists.
    pub secret_key: bool,
    /// Repository only: prune to this many snapshots afterwards.
    pub keep: Option<usize>,
    /// Test seam: damage the local snapshot before the integrity check.
    #[cfg(feature = "test-seams")]
    pub corrupt_snapshot: bool,
}

/// Blob counts of a backup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobCounts {
    /// Blobs written (to the archive, or copied to the repository).
    pub written: u64,
    /// Blobs the repository already had.
    pub present: u64,
    /// Bytes written.
    pub bytes: u64,
}

/// What a backup did.
#[derive(Debug, Clone)]
pub struct BackupReport {
    /// The file, `stdout`, or the repository.
    pub location: String,
    /// The repository snapshot's name.
    pub snapshot: Option<String>,
    pub schema_version: i64,
    /// The size of the database snapshot.
    pub db_bytes: u64,
    /// The files besides the blobs (`ferrite.toml`, `secret.key`, `ferrite.db`).
    pub files: Vec<String>,
    /// `None` without media.
    pub blobs: Option<BlobCounts>,
    /// Artifacts whose blob was missing from the media store (R10): the backup lacks them.
    pub missing: Vec<ArtifactHash>,
    /// What `keep` pruned.
    pub pruned: Option<repo::PruneReport>,
}

/// The snapshot's name in the backups folder: `backup-<ts>.db`, never `pre-*.db`, which the
/// pre-migration snapshot pruning deletes.
fn local_snapshot_name(ts: &str) -> String {
    format!("backup-{ts}.db")
}

/// Removes the local snapshot (and any SQLite side files) when dropped.
struct LocalSnapshot(PathBuf);

impl Drop for LocalSnapshot {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut p = self.0.as_os_str().to_owned();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }
}

/// What the checked snapshot holds.
struct SnapshotInfo {
    hashes: Vec<ArtifactHash>,
    schema_version: i64,
    site_id: Option<String>,
}

/// Opens the snapshot read-only, runs `PRAGMA integrity_check` (it must say `ok`), then reads
/// the artifact hashes, the schema version and the site id.
async fn inspect(path: &Path) -> anyhow::Result<SnapshotInfo> {
    let opts = SqliteConnectOptions::new().filename(path).read_only(true);
    let mut conn = SqliteConnection::connect_with(&opts)
        .await
        .context("cannot open the snapshot")?;
    let result = async {
        let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&mut conn)
            .await
            .context("the snapshot failed its integrity check (PRAGMA integrity_check)")?;
        if rows != ["ok"] {
            bail!(
                "the snapshot failed its integrity check (PRAGMA integrity_check): {}",
                rows.join("; ")
            );
        }
        let hashes: Vec<String> = sqlx::query_scalar("SELECT hash FROM artifacts ORDER BY hash")
            .fetch_all(&mut conn)
            .await?;
        let hashes = hashes
            .iter()
            .map(|h| ArtifactHash::parse(h).with_context(|| format!("invalid artifact hash {h:?}")))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let schema_version: Option<i64> =
            sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
                .fetch_one(&mut conn)
                .await?;
        let site_id: Option<String> =
            sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'site_id'")
                .fetch_optional(&mut conn)
                .await?;
        anyhow::Ok(SnapshotInfo {
            hashes,
            schema_version: schema_version.unwrap_or(0),
            site_id,
        })
    }
    .await;
    let _ = conn.close().await;
    result.context("cannot read the snapshot")
}

/// Damages the middle of the snapshot (the integrity check must catch it).
#[cfg(feature = "test-seams")]
fn corrupt(path: &Path) -> anyhow::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    let len = file.metadata()?.len();
    let page = 4096;
    let at = (len / 2 / page) * page;
    file.seek(SeekFrom::Start(at))?;
    file.write_all(&vec![0xA5; usize::try_from(page.min(len - at))?])?;
    Ok(())
}

/// The sha256 and size of a file.
fn file_entry(name: &str, path: &Path) -> anyhow::Result<FileEntry> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 256 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok(FileEntry {
        path: name.to_owned(),
        bytes,
        sha256: to_hex(&hasher.finalize()),
    })
}

/// Backs up the SQLite site `store` (whose configuration is `config`) to `target` (P13):
/// writes a `VACUUM INTO` snapshot to `<data>/backups/backup-<ts>.db`, checks it with
/// `PRAGMA integrity_check`, then writes the archive or repository snapshot with the blobs
/// of the snapshot's `artifacts` rows from `media` (`None`: no media). The local snapshot is
/// deleted afterwards, also on failure. A blob missing from `media` is left out and listed
/// in [`BackupReport::missing`]; one whose content does not match its hash fails the
/// backup. Safe next to writers (`Store::snapshot` works next to an open transaction).
pub async fn run_backup(
    config: &Config,
    store: &dyn Store,
    media: Option<Arc<MediaStore>>,
    target: &Target,
    opts: &BackupOptions,
) -> anyhow::Result<BackupReport> {
    if store.backend() != Backend::Sqlite {
        bail!("{POSTGRES}");
    }
    if opts.keep.is_some() && !matches!(target, Target::Repo(_)) {
        bail!("--keep applies to a repository (a folder or s3://), not to an archive");
    }
    let dir = config.data_dir.join("backups");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let ts = chrono::Utc::now().format(repo::SNAPSHOT_FORMAT).to_string();
    let local = LocalSnapshot(dir.join(local_snapshot_name(&ts)));
    // A leftover of an interrupted run in the same second: ours by its name.
    drop(LocalSnapshot(local.0.clone()));
    store
        .snapshot(&local.0)
        .await
        .context("cannot write the database snapshot")?;
    #[cfg(feature = "test-seams")]
    if opts.corrupt_snapshot {
        corrupt(&local.0)?;
    }
    let info = inspect(&local.0).await?;

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let toml = config.data_dir.join(CONFIG_FILE);
    if toml.is_file() {
        files.push((CONFIG_FILE.to_owned(), toml));
    }
    let key = config.data_dir.join(SECRET_KEY_FILE);
    if opts.secret_key && key.is_file() {
        files.push((SECRET_KEY_FILE.to_owned(), key));
    }
    files.push((DB_FILE.to_owned(), local.0.clone()));
    let entries = files
        .iter()
        .map(|(name, path)| file_entry(name, path))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let db_bytes = entries.last().map_or(0, |e| e.bytes);
    let manifest = Manifest {
        format: FORMAT,
        kind: Kind::Backup,
        ferrite_version: binary_version()?.to_string(),
        schema_version: info.schema_version,
        source_backend: "sqlite".to_owned(),
        site_id: info.site_id,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        tables: None,
        files: Some(entries),
        artifacts: None,
        missing: Vec::new(),
        artifact_hashes: Vec::new(),
    };
    let mut report = BackupReport {
        location: target.describe(),
        snapshot: None,
        schema_version: info.schema_version,
        db_bytes,
        files: files.iter().map(|(n, _)| n.clone()).collect(),
        blobs: None,
        missing: Vec::new(),
        pruned: None,
    };
    match target {
        Target::File(path) => {
            let blobs = archive_blobs(media.as_deref(), &info.hashes, &mut report).await?;
            write_archive_file(path, manifest, files, media, blobs, &mut report).await?;
        }
        Target::Stdout => {
            report.location = "stdout".to_owned();
            let blobs = archive_blobs(media.as_deref(), &info.hashes, &mut report).await?;
            let manifest = archive_manifest(manifest, &blobs, &report);
            let out = BufWriter::new(std::io::stdout());
            let mut out = write_archive(out, manifest, files, media, blobs).await?;
            out.flush()?;
        }
        Target::Repo(loc) => {
            let repo = repo::Repo::open(loc, true)?;
            let mut manifest = manifest;
            if let Some(media) = &media {
                let copy = repo.copy_blobs(media, &info.hashes).await?;
                manifest.artifacts = Some(ArtifactsInfo {
                    count: copy.listed.len() as u64,
                    bytes: copy.listed_bytes,
                });
                manifest.artifact_hashes = copy.listed.iter().map(ToString::to_string).collect();
                manifest.missing = copy.missing.iter().map(ToString::to_string).collect();
                report.missing = copy.missing;
                report.blobs = Some(BlobCounts {
                    written: copy.written,
                    present: copy.present,
                    bytes: copy.bytes,
                });
            }
            let name = repo.write_snapshot(&files, &manifest).await?;
            report.location = format!("{}/snapshots/{name}", repo.display());
            report.snapshot = Some(name);
            if let Some(keep) = opts.keep {
                report.pruned = Some(repo.prune(keep).await?);
            }
        }
    }
    drop(local);
    Ok(report)
}

/// The size of each blob of `hashes` in `media`; missing ones go to `report.missing`.
async fn archive_blobs(
    media: Option<&MediaStore>,
    hashes: &[ArtifactHash],
    report: &mut BackupReport,
) -> anyhow::Result<Vec<(ArtifactHash, u64)>> {
    let Some(media) = media else {
        return Ok(Vec::new());
    };
    let mut blobs = Vec::new();
    for hash in hashes {
        match media.head(hash).await? {
            Some(size) => blobs.push((hash.clone(), size)),
            None => report.missing.push(hash.clone()),
        }
    }
    report.blobs = Some(BlobCounts {
        written: blobs.len() as u64,
        present: 0,
        bytes: blobs.iter().map(|(_, s)| s).sum(),
    });
    Ok(blobs)
}

fn archive_manifest(
    mut manifest: Manifest,
    blobs: &[(ArtifactHash, u64)],
    report: &BackupReport,
) -> Manifest {
    manifest.artifacts = report.blobs.as_ref().map(|b| ArtifactsInfo {
        count: blobs.len() as u64,
        bytes: b.bytes,
    });
    manifest.missing = report.missing.iter().map(ToString::to_string).collect();
    manifest
}

/// Writes the archive to a new file (mode 0600), removed again on failure.
async fn write_archive_file(
    path: &Path,
    manifest: Manifest,
    files: Vec<(String, PathBuf)>,
    media: Option<Arc<MediaStore>>,
    blobs: Vec<(ArtifactHash, u64)>,
    report: &mut BackupReport,
) -> anyhow::Result<()> {
    let manifest = archive_manifest(manifest, &blobs, report);
    let file = crate::init::create_private(path)?;
    let result = async {
        let out = write_archive(BufWriter::new(file), manifest, files, media, blobs).await?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        anyhow::Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Writes a backup archive: the manifest, the files in order, then the blobs.
async fn write_archive<W: Write + Send + 'static>(
    out: W,
    manifest: Manifest,
    files: Vec<(String, PathBuf)>,
    media: Option<Arc<MediaStore>>,
    blobs: Vec<(ArtifactHash, u64)>,
) -> anyhow::Result<W> {
    let handle = Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut writer = ArchiveWriter::new(out, &manifest)?;
        for (name, path) in &files {
            writer.append_file(name, path)?;
        }
        if let Some(media) = media {
            for (hash, size) in blobs {
                crate::site_copy::append_blob(&mut writer, &handle, &media, &hash, size)?;
            }
        }
        writer.finish()
    })
    .await?
}

/// `ferrite backup`: exit 0, 2 when artifacts were missing from the media store, 1 on any
/// error.
pub async fn run_backup_cmd(args: BackupArgs) -> anyhow::Result<ExitCode> {
    let config = load_config(&args.data)?;
    if let DbKind::Postgres { .. } = config.database_kind() {
        bail!("{POSTGRES}");
    }
    let cwd = std::env::current_dir()?;
    let target = Target::parse(&args.to, &cwd)?;
    if args.keep.is_some() && !matches!(target, Target::Repo(_)) {
        bail!("--keep applies to a repository (a folder or s3://), not to an archive");
    }
    if let Target::File(path) = &target
        && path.exists()
    {
        bail!("{} already exists", path.display());
    }
    let media = if args.no_media {
        None
    } else {
        Some(Arc::new(open_media(&config)?))
    };
    let store = open_store(&config, OpenMode::MustExist).await?;
    let opts = BackupOptions {
        secret_key: !args.no_secret_key,
        keep: args.keep.map(|k| k as usize),
        #[cfg(feature = "test-seams")]
        corrupt_snapshot: args.test_corrupt_snapshot,
    };
    let result = run_backup(&config, store.as_ref(), media, &target, &opts).await;
    store.close().await;
    let report = result?;
    print_report(&report, &target);
    if report.missing.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(MISSING_BLOBS))
    }
}

fn print_report(report: &BackupReport, target: &Target) {
    // stdout may be the archive itself.
    let to_stdout = !matches!(target, Target::Stdout);
    let say = |line: String| {
        if to_stdout {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    };
    for hash in &report.missing {
        eprintln!("missing artifact {hash}");
    }
    say(format!(
        "Backed up {} (database at schema {}, {} bytes) to {}.",
        report.files.join(", "),
        report.schema_version,
        report.db_bytes,
        report.location
    ));
    match (&report.blobs, matches!(target, Target::Repo(_))) {
        (None, _) => say("Artifacts: not included (--no-media).".to_owned()),
        (Some(b), true) => say(format!(
            "Artifacts: {} copied, {} already in the repository ({} bytes copied).",
            b.written, b.present, b.bytes
        )),
        (Some(b), false) => say(format!("Artifacts: {} ({} bytes).", b.written, b.bytes)),
    }
    if !report.missing.is_empty() {
        eprintln!(
            "{} artifacts are missing from the media store and are not in the backup (listed \
             above and in manifest.json `missing`).",
            report.missing.len()
        );
    }
    if let Some(p) = &report.pruned {
        say(format!(
            "Pruned {} snapshots and {} artifacts.",
            p.snapshots.len(),
            p.artifacts
        ));
    }
    let key = if report.files.iter().any(|f| f == SECRET_KEY_FILE) {
        "password hashes, sealed secrets and the site key (secret.key)"
    } else {
        "password hashes and sealed secrets"
    };
    eprintln!(
        "The backup holds {key} unencrypted; keep it private (pipe `--to -` to age or gpg to \
         encrypt it)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_snapshot_is_never_pruned_as_pre_migration() {
        let name = local_snapshot_name("20261029T101500Z");
        assert_eq!(name, "backup-20261029T101500Z.db");
        assert!(!name.starts_with("pre-"));
    }
}
