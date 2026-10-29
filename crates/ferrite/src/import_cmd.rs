//! `ferrite import wordpress`: imports a WordPress export (WXR) into the site (M3-1).
//!
//! Opens the site as `seed demo` does (config, store, startup check, media store,
//! permalink rules), runs [`ferrite_import_wp::run::run`], writes `report.json` and
//! `report.html`, and prints the counts. A failed run writes no report: the error is
//! printed and the exit code is 1.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use async_trait::async_trait;
use clap::{Args, Subcommand};
use ferrite_core::{Actor, CoreError, SwappablePermalinks, import::ImportService};
use ferrite_domain::{Role, UserStatus};
use ferrite_http::import_fetch::HttpSource;
use ferrite_import_wp::ImportError;
use ferrite_import_wp::report::Report;
use ferrite_import_wp::run::{
    ByteStream, DirSource, ImportContext, ImportOptions, MediaSource, run as import,
};
use ferrite_store::Store;

use crate::{
    binary_version, load_config,
    open::{OpenMode, open_store, startup_check},
    site::{open_media, permalink_rules},
};

/// Done, with nothing refused or missing.
const DONE: u8 = 0;
/// Done, but the report lists refused redirects, missing media or refused attachments.
const DONE_WITH_PROBLEMS: u8 = 2;
/// Another import is running.
const BUSY: u8 = 3;

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Import a WordPress export file (WXR) into the site.
    ///
    /// Exit code: 0 done, 2 done with refused redirects, missing media or attachments
    /// refused for their size or type (the report lists them), 3 when another import is
    /// running, 1 on any error. With SQLite,
    /// `ferrite serve` holds the data folder's lock: stop it first.
    Wordpress(WordpressArgs),
}

#[derive(Debug, Args)]
pub struct WordpressArgs {
    /// The WXR file (Tools → Export in WordPress).
    pub file: PathBuf,
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
    /// A copy of the old site's `wp-content/uploads` folder.
    #[arg(long)]
    pub uploads: Option<PathBuf>,
    /// Fetch attachments missing from `--uploads` from the old site (public addresses
    /// only, 30 s per file with its redirects, at most `media.max_upload` each).
    #[arg(long)]
    pub fetch_media: bool,
    /// Run everything, then roll every write back.
    #[arg(long)]
    pub dry_run: bool,
    /// Where to write report.json and report.html (default:
    /// <data>/import/reports/<run-id>/).
    #[arg(long)]
    pub report: Option<PathBuf>,
    /// Keep the site's post permalink structure instead of the old site's.
    #[arg(long)]
    pub no_keep_permalinks: bool,
    /// Do not import the site title and description.
    #[arg(long)]
    pub no_site_options: bool,
    /// The old site's category base, for redirects.
    #[arg(long, default_value = "category")]
    pub category_base: String,
    /// The old site's tag base, for redirects.
    #[arg(long, default_value = "tag")]
    pub tag_base: String,
    /// The admin who imports (default: the first active admin).
    #[arg(long = "as")]
    pub as_login: Option<String>,
    /// Take over an import whose process stopped (no heartbeat for 120 s).
    #[arg(long)]
    pub takeover: bool,
    /// Test seam: fetch attachments from any address (local test servers).
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub allow_private_fetch: bool,
}

pub async fn run(cmd: ImportCommand) -> anyhow::Result<ExitCode> {
    let ImportCommand::Wordpress(args) = cmd;
    let config = load_config(&args.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let done = async {
        startup_check(store.as_ref(), &binary_version()?, &config.database).await?;
        let media = Arc::new(open_media(&config)?);
        let permalinks = Arc::new(SwappablePermalinks::new(
            permalink_rules(store.as_ref()).await?,
        ));
        let actor = importer(store.as_ref(), args.as_login.as_deref()).await?;
        let ctx = ImportContext {
            import: Arc::new(ImportService::new(store.clone(), permalinks.clone())),
            permalinks,
            media_store: media,
            actor,
            work_dir: args.data.join("import"),
            media_source: media_source(&args)?,
            site_url: config.site.url.as_str().trim_end_matches('/').to_owned(),
        };
        let opts = ImportOptions {
            dry_run: args.dry_run,
            keep_permalinks: !args.no_keep_permalinks,
            site_options: !args.no_site_options,
            category_base: args.category_base.clone(),
            tag_base: args.tag_base.clone(),
            takeover: args.takeover,
            max_upload: config.uploads.max_upload,
            #[cfg(feature = "test-seams")]
            batch_items: None,
            #[cfg(feature = "test-seams")]
            fail_after_batches: None,
            #[cfg(feature = "test-seams")]
            step_hook: None,
            #[cfg(feature = "test-seams")]
            stop_after: None,
        };
        match import(ctx, &args.file, opts).await {
            Ok(report) => finish(&args, &report),
            Err(ImportError::Core(CoreError::Unavailable(why))) => {
                eprintln!("error: {why}");
                Ok(ExitCode::from(BUSY))
            }
            Err(e) => Err(anyhow::Error::new(e).context("the import failed")),
        }
    }
    .await;
    store.close().await;
    done
}

/// The admin `login` names, or the first active admin.
async fn importer(store: &dyn Store, login: Option<&str>) -> anyhow::Result<Actor> {
    let user = match login {
        Some(login) => store
            .read()
            .users()
            .get_by_login(login)
            .await?
            .with_context(|| format!("no user {login:?}"))?,
        None => store
            .read()
            .users()
            .list_active_admins(1)
            .await?
            .into_iter()
            .next()
            .context("the site has no active admin (create one with `ferrite user create`)")?,
    };
    if user.role != Role::Admin || user.status != UserStatus::Active {
        bail!(
            "{:?} is not an active admin; only active admins may import",
            user.login
        );
    }
    Ok(Actor::new(user.id, user.role))
}

/// The uploads folder, then the old site when `--fetch-media` asks for it.
fn media_source(args: &WordpressArgs) -> anyhow::Result<Arc<dyn MediaSource>> {
    let mut sources: Vec<Arc<dyn MediaSource>> = Vec::new();
    if let Some(dir) = &args.uploads {
        if !dir.is_dir() {
            bail!("--uploads {} is not a folder", dir.display());
        }
        sources.push(Arc::new(DirSource::new(dir)));
    }
    if args.fetch_media {
        let version = binary_version()?.to_string();
        #[cfg(feature = "test-seams")]
        let http = if args.allow_private_fetch {
            HttpSource::allowing_private(&version)
        } else {
            HttpSource::new(&version)
        };
        #[cfg(not(feature = "test-seams"))]
        let http = HttpSource::new(&version);
        sources.push(Arc::new(http.map_err(anyhow::Error::msg)?));
    }
    Ok(Arc::new(Sources(sources)))
}

/// Media sources tried in order; none at all means every attachment is missing.
struct Sources(Vec<Arc<dyn MediaSource>>);

#[async_trait]
impl MediaSource for Sources {
    async fn open(
        &self,
        rel_path: &str,
        url: &str,
        max: u64,
    ) -> Result<Option<ByteStream>, ImportError> {
        for source in &self.0 {
            if let Some(stream) = source.open(rel_path, url, max).await? {
                return Ok(Some(stream));
            }
        }
        Ok(None)
    }
}

/// Writes the report files, prints the summary and picks the exit code.
fn finish(args: &WordpressArgs, report: &Report) -> anyhow::Result<ExitCode> {
    let dir = args.report.clone().unwrap_or_else(|| {
        args.data
            .join("import")
            .join("reports")
            .join(&report.run_id)
    });
    write_reports(&dir, report)?;
    if report.dry_run
        && let Some(note) = report.notes.iter().find(|n| n.starts_with("dry run:"))
    {
        println!("{note}");
    }
    println!("Imported from {}:", report.source);
    for (kind, c) in &report.counts {
        println!(
            "  {kind}: {} created, {} updated, {} unchanged, {} kept, {} skipped, {} failed",
            c.created, c.updated, c.unchanged, c.kept, c.skipped, c.failed
        );
    }
    let missing = total(report, "missing_media", report.missing_media.len());
    let refused = total(report, "redirects.refused", report.redirects.refused.len());
    let failed = report.counts.get("media").map_or(0, |c| c.failed);
    if missing > 0 {
        println!("  {missing} attachments missing (the items keep their old URLs)");
    }
    if failed > 0 {
        println!("  {failed} attachments refused (size or type)");
    }
    if refused > 0 {
        println!("  {refused} redirects refused");
    }
    if report.counts.get("media").is_some_and(|c| c.created > 0) && !report.dry_run {
        println!("media variants are made by the next `ferrite serve`");
    }
    println!("Report: {}", dir.join("report.json").display());
    println!("        {}", dir.join("report.html").display());
    let code = if missing > 0 || refused > 0 || failed > 0 {
        DONE_WITH_PROBLEMS
    } else {
        DONE
    };
    Ok(ExitCode::from(code))
}

/// The total of a capped report list (its length when nothing was dropped).
fn total(report: &Report, field: &str, len: usize) -> u64 {
    report
        .totals
        .get(field)
        .copied()
        .unwrap_or(len as u64)
        .max(len as u64)
}

fn write_reports(dir: &Path, report: &Report) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    for (name, body) in [
        ("report.json", report.to_json()),
        ("report.html", report.to_html()),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, body).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}
