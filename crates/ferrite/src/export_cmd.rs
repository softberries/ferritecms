//! `ferrite export`, `ferrite import-site` and `ferrite db copy` (M4-1 P5–P8): the CLI over
//! [`crate::site_copy`].

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::{Args, Subcommand};
use ferrite_config::DatabaseConfig;
use ferrite_domain::Secret;

use crate::archive::Newer;
use crate::open::{OpenMode, open_store, open_store_url};
use crate::site::open_media;
use crate::site_copy::{self, LoadStats};
use crate::{binary_version, load_config};

/// Exit code for an archive or database newer than this binary.
const NEWER: u8 = 3;

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
    /// The archive to write (`.tar.zst`; refused if it exists), or `-` for stdout.
    #[arg(long)]
    pub out: PathBuf,
    /// Also put every stored artifact (media, themes, plugins) into the archive.
    #[arg(long)]
    pub with_media: bool,
}

#[derive(Debug, Args)]
pub struct ImportSiteArgs {
    /// The archive `ferrite export` wrote, or `-` for stdin.
    pub archive: PathBuf,
    /// The data folder to import into (made by `ferrite init`; its database must be fresh).
    #[arg(long)]
    pub data: PathBuf,
    /// Store the archive's artifacts in the site's media store (those present are skipped).
    #[arg(long)]
    pub with_media: bool,
    /// Test seam: report an error after the load committed.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub test_fail_after_commit: bool,
}

#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// Copy every exported table from one database into a fresh one, streamed.
    ///
    /// A URL is `postgres://…` or `sqlite:<path to ferrite.db>` (an absent SQLite file is
    /// created; the folder lock is taken in its folder). The source must be at this
    /// binary's schema; the target must be fresh (an empty or unmigrated database, or one
    /// just made by `ferrite init`). Media is not copied (use `ferrite media sync`) and no
    /// ferrite.toml is written: the command prints the `[database]` lines to use.
    ///
    /// Exit code: 0 ok, 3 when the source (or target) schema is newer than this binary,
    /// 1 on any other error.
    Copy(CopyArgs),
}

#[derive(Args)]
pub struct CopyArgs {
    /// The source database URL.
    #[arg(long)]
    pub from: String,
    /// The target database URL.
    #[arg(long)]
    pub to: String,
    /// The source's Postgres schema.
    #[arg(long)]
    pub from_schema: Option<String>,
    /// The target's Postgres schema (created if missing).
    #[arg(long)]
    pub to_schema: Option<String>,
}

// Manual Debug: the URLs may embed passwords.
impl std::fmt::Debug for CopyArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopyArgs")
            .field("from", &"[redacted]")
            .field("to", &"[redacted]")
            .field("from_schema", &self.from_schema)
            .field("to_schema", &self.to_schema)
            .finish()
    }
}

/// Exit code of an export whose archive lacks blobs missing from the media store.
const MISSING_BLOBS: u8 = 2;

/// Maps a [`Newer`] refusal to exit code 3 (printed here), other errors to exit 1.
fn exit(result: anyhow::Result<ExitCode>) -> anyhow::Result<ExitCode> {
    match result {
        Ok(code) => Ok(code),
        Err(e) if e.downcast_ref::<Newer>().is_some() => {
            eprintln!("error: {e:#}");
            Ok(ExitCode::from(NEWER))
        }
        Err(e) => Err(e),
    }
}

fn is_dash(p: &Path) -> bool {
    p.as_os_str() == "-"
}

pub async fn run_export(args: ExportArgs) -> anyhow::Result<ExitCode> {
    exit(export(args).await)
}

async fn export(args: ExportArgs) -> anyhow::Result<ExitCode> {
    let config = load_config(&args.data)?;
    let media = if args.with_media {
        Some(Arc::new(open_media(&config)?))
    } else {
        None
    };
    let store = open_store(&config, OpenMode::MustExist).await?;
    let binary = binary_version()?;
    let result = if is_dash(&args.out) {
        let out = std::io::BufWriter::new(std::io::stdout());
        site_copy::export_to(store.as_ref(), media, out, &binary)
            .await
            .and_then(|(m, mut out)| {
                out.flush()?;
                Ok(m)
            })
    } else {
        export_file(store.as_ref(), media, &args.out, &binary).await
    };
    store.close().await;
    let manifest = result?;
    let rows: u64 = manifest.tables.iter().flatten().map(|t| t.rows).sum();
    let tables = manifest.tables.as_ref().map_or(0, Vec::len);
    let media = manifest.artifacts.map_or_else(String::new, |a| {
        format!(" and {} artifacts ({} bytes)", a.count, a.bytes)
    });
    let target = if is_dash(&args.out) {
        "stdout".to_owned()
    } else {
        args.out.display().to_string()
    };
    // stdout may be the archive itself.
    eprintln!(
        "Exported {rows} rows from {tables} tables{media} at schema {} to {target}.",
        manifest.schema_version
    );
    eprintln!(
        "The archive holds password hashes and sealed secrets but not secret.key; keep it \
         private (pipe `--out -` to age or gpg to encrypt it)."
    );
    if manifest.missing.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "{} artifacts are missing from the media store and are not in the archive (listed \
         above and in manifest.json `missing`).",
        manifest.missing.len()
    );
    Ok(ExitCode::from(MISSING_BLOBS))
}

/// Writes the archive to a new file (mode 0600), removed again on failure.
async fn export_file(
    store: &dyn ferrite_store::Store,
    media: Option<Arc<ferrite_media::MediaStore>>,
    path: &Path,
    binary: &semver::Version,
) -> anyhow::Result<crate::archive::Manifest> {
    let file = crate::init::create_private(path)?;
    let result = async {
        let (manifest, out) =
            site_copy::export_to(store, media, std::io::BufWriter::new(file), binary).await?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        anyhow::Ok(manifest)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

pub async fn run_import_site(args: ImportSiteArgs) -> anyhow::Result<ExitCode> {
    exit(import_site(args).await)
}

async fn import_site(args: ImportSiteArgs) -> anyhow::Result<ExitCode> {
    let config = load_config(&args.data)?;
    let input: Box<dyn Read + Send> = if is_dash(&args.archive) {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(&args.archive)
                .with_context(|| format!("cannot open {}", args.archive.display()))?,
        )
    };
    let media = if args.with_media {
        Some(Arc::new(open_media(&config)?))
    } else {
        None
    };
    let store = open_store(&config, OpenMode::MustExist).await?;
    let binary = binary_version()?;
    let result = async {
        let (manifest, stats) = site_copy::import_from(
            input,
            store.clone(),
            media,
            config.database.clone(),
            binary.clone(),
            site_copy::LoadSeams {
                #[cfg(feature = "test-seams")]
                fail_after_commit: args.test_fail_after_commit,
            },
        )
        .await?;
        site_copy::finalize(store.as_ref(), &config.database, &binary, &stats).await?;
        anyhow::Ok((manifest, stats))
    }
    .await;
    store.close().await;
    let (manifest, stats) = result?;
    println!(
        "Imported {} rows (schema {}, exported from {} by ferrite {} at {}).",
        stats.rows,
        manifest.schema_version,
        manifest.source_backend,
        manifest.ferrite_version,
        manifest.created_at
    );
    print_blobs(&stats);
    if !manifest.missing.is_empty() {
        println!(
            "note: {} artifacts were missing from the source's media store at export time and \
             are not in the archive (manifest.json `missing`).",
            manifest.missing.len()
        );
    }
    site_copy::print_totp_hint(stats.totp_users, Some(&args.data));
    Ok(ExitCode::SUCCESS)
}

fn print_blobs(stats: &LoadStats) {
    if stats.blobs_stored + stats.blobs_present > 0 {
        println!(
            "Stored {} artifacts in the media store ({} were already there).",
            stats.blobs_stored, stats.blobs_present
        );
    }
    if stats.blobs_ignored > 0 {
        println!(
            "The archive holds {} artifacts that were not stored; pass --with-media to store \
             them, or copy the media with `ferrite media sync`.",
            stats.blobs_ignored
        );
    }
}

pub async fn run_db(cmd: DbCommand) -> anyhow::Result<ExitCode> {
    let DbCommand::Copy(args) = cmd;
    exit(db_copy(args).await)
}

async fn db_copy(args: CopyArgs) -> anyhow::Result<ExitCode> {
    let binary = binary_version()?;
    // Before anything is opened, so a bad URL creates nothing.
    let (url, note) = config_url(&args.to)?;
    let source = open_store_url(&args.from, args.from_schema.as_deref(), OpenMode::MustExist)
        .await
        .context("--from")?;
    // Refuse an older or newer source before the target is opened (and maybe created).
    let checked = match source.migration_status(&binary).await {
        Ok(status) => site_copy::check_source(&status),
        Err(e) => Err(e.into()),
    };
    if let Err(e) = checked {
        source.close().await;
        return Err(e);
    }
    let created = new_sqlite_file(&args.to);
    let target = match open_store_url(
        &args.to,
        args.to_schema.as_deref(),
        OpenMode::CreateIfMissing,
    )
    .await
    .context("--to")
    {
        Ok(t) => t,
        Err(e) => {
            source.close().await;
            remove_created(created.as_deref());
            return Err(e);
        }
    };
    let db = DatabaseConfig {
        url: Secret::new(args.to.clone()),
        max_connections: 4,
        auto_migrate: true,
        allow_contract: false,
        schema: args.to_schema.clone(),
    };
    let result = async {
        let stats = site_copy::copy(source.as_ref(), target.as_ref(), &db, &binary).await?;
        site_copy::finalize(target.as_ref(), &db, &binary, &stats).await?;
        anyhow::Ok(stats)
    }
    .await;
    source.close().await;
    target.close().await;
    let stats = match result {
        Ok(stats) => stats,
        Err(e) => {
            remove_created(created.as_deref());
            return Err(e);
        }
    };
    println!(
        "Copied {} rows. Use these lines in the target site's ferrite.toml:",
        stats.rows
    );
    println!();
    println!("[database]");
    println!("url = {}", toml_string(&url));
    if let Some(schema) = &args.to_schema {
        println!("schema = {}", toml_string(schema));
    }
    println!();
    if note {
        println!(
            "The password is shown as PASSWORD: put the real one there, or the whole URL in a \
             file named by database.url_file."
        );
    }
    site_copy::print_totp_hint(stats.totp_users, None);
    Ok(ExitCode::SUCCESS)
}

/// The path of the SQLite file `url` names when it does not exist yet (this run creates it).
fn new_sqlite_file(url: &str) -> Option<PathBuf> {
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let path = PathBuf::from(rest);
    (!path.exists()).then_some(path)
}

/// Removes a SQLite database this run created (with its `-wal`/`-shm` files).
fn remove_created(path: Option<&Path>) {
    let Some(path) = path else { return };
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

/// The `database.url` for `url` and whether a password was hidden: a SQLite path made
/// absolute; a Postgres password (in the user info or a `password` query parameter)
/// shown as `PASSWORD`.
fn config_url(url: &str) -> anyhow::Result<(String, bool)> {
    if let Some(rest) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    {
        let path = std::path::absolute(rest)?;
        return Ok((format!("sqlite://{}", path.display()), false));
    }
    let mut parsed = url::Url::parse(url).context("--to is not a valid URL")?;
    let mut hidden = false;
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some("PASSWORD"));
        hidden = true;
    }
    if parsed.query_pairs().any(|(k, _)| k == "password") {
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| {
                let v = if k == "password" {
                    "PASSWORD".into()
                } else {
                    v.into_owned()
                };
                (k.into_owned(), v)
            })
            .collect();
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
        hidden = true;
    }
    Ok((parsed.to_string(), hidden))
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_urls() {
        let url = |u: &str| config_url(u).unwrap();
        assert_eq!(
            url("postgres://u:secret@h:5432/db"),
            ("postgres://u:PASSWORD@h:5432/db".into(), true)
        );
        assert_eq!(
            url("postgres://u@h/db"),
            ("postgres://u@h/db".into(), false)
        );
        assert_eq!(
            url("postgres://u@h/db?sslmode=require&password=secret"),
            (
                "postgres://u@h/db?sslmode=require&password=PASSWORD".into(),
                true
            )
        );
        assert!(url("sqlite:/x/ferrite.db").0.starts_with("sqlite:///x"));
        assert_eq!(toml_string("s1"), "\"s1\"");
        // A quote needs a literal string (still valid TOML).
        assert_eq!(toml_string("a\"b"), "'a\"b'");
    }
}
