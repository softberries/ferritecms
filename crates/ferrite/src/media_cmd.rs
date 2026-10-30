//! `ferrite media …`: tools over the media store.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::{Args, Subcommand};
use ferrite_core::{ContentService, MediaService, SwappablePermalinks};
use ferrite_domain::{ContentId, ContentQuery, MAX_PAGE, Media};
use ferrite_http::media::{MAX_PROCESS_BYTES, variant_specs};
use ferrite_media::{MediaStore, VariantSpec, is_raster_image, process_image};
use ferrite_store::Store;
use futures::StreamExt;
use tokio::sync::Semaphore;

use crate::cli::DataArg;
use crate::gc::{self, GcOptions};
use crate::jobs::media::{ImageProcessor, VariantMaker};
use crate::media_url::parse_media_url;
use crate::open::{OpenMode, open_store, startup_check};
use crate::site::{open_media, permalink_rules};
use crate::{binary_version, load_config, load_theme};

/// Copies running at once.
const SYNC_CONCURRENCY: usize = 8;

#[derive(Debug, Subcommand)]
pub enum MediaCommand {
    /// Copy every artifact from one media store to another (a backend migration).
    ///
    /// A store is `local:<path>` or `s3://bucket[/prefix]`. With `--data` the source is the
    /// site's own store from its ferrite.toml (no database is opened, so this works while the
    /// site runs); every `s3://` URL takes its credentials, endpoint and options from the
    /// environment (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`,
    /// `AWS_ENDPOINT_URL`, `AWS_ALLOW_HTTP`, `AWS_COPY_IF_NOT_EXISTS`, `AWS_CONDITIONAL_PUT`).
    ///
    /// Copies every object under `artifacts/`, recorded in the database or not; skips keys
    /// already at the destination with the same size and checks the size after each copy;
    /// never deletes and never copies `tmp/`. Prints `copied N (B bytes), skipped M,
    /// failed F`. Exit code: 0, or 2 when some copies failed, or 1 on an error.
    Sync(SyncArgs),
    /// Delete artifacts nothing references, and staging leftovers (design 03 §6).
    ///
    /// One pass of the artifacts GC, as the daily `artifacts.gc` job runs it: marks
    /// unreferenced artifact rows, deletes rows marked longer than the grace period (then
    /// their files), files under `artifacts/` with no row that are older than the grace
    /// period, and `tmp/` objects older than 24 h. Files named by a queued or running job
    /// or by an import report are kept. With SQLite, stop `serve` first.
    ///
    /// Deletes happen only on a pass that reads the media store's owner marker
    /// (`ferrite-owner.json`) naming this site's database. A pass that finds no marker
    /// writes it and deletes nothing yet; a pass on a store another site owns (or whose
    /// marker cannot be read) marks and cleans `tmp/` but deletes nothing else; both say so.
    /// `--claim` makes this site the owner first and prints the owner it replaced: use it
    /// only when this site replaced the other one (a restore in place, a moved database).
    ///
    /// Prints `marked N, unmarked M, deleted rows R (B bytes), deleted unrecorded blobs U
    /// (B bytes), deleted tmp T` (with `--dry-run`, after `dry run: `, counting what would
    /// happen; `, failed F` is appended when some deletes failed, retried by the next pass;
    /// `; claimed the media store from …` after a claim; `; deletes skipped: …` with the
    /// reason when steps 2 and 3 did not run).
    /// Exit code: 0, or 3 when another GC or a backup's blob copy holds the GC lock, or 1 on
    /// an error, or 2 for a usage error (clap).
    Gc(GcArgs),
    /// Make the image size variants again, in process (after a theme adds sizes).
    ///
    /// The sizes are the core sizes plus the `[images]` of the theme active in the database,
    /// as `seed demo` and a running node compute them. By default only raster items whose
    /// recorded variants lack a size that applies to their width are processed (a size wider
    /// than the original is never made, so it is never missing). Only size names are compared, so
    /// a theme that changes the width of an existing size needs `--all`, which redoes every
    /// raster item. `--item ID` limits the run to one media item; a complete item is skipped
    /// unless `--all` is given too. Originals over 64 MiB and
    /// non-raster items are skipped. The replaced variant files stay for `media gc`.
    ///
    /// With SQLite the data folder must not be in use, so stop `serve` first. Progress goes to
    /// stderr every 50 items; prints `regenerated N, skipped S, failed F`. Exit code: 0, or 2
    /// when some items failed, or 1 on an error (an unknown `--item`, an unreadable store).
    Regenerate(RegenerateArgs),
}

#[derive(Debug, Args)]
pub struct GcArgs {
    #[command(flatten)]
    data: DataArg,
    /// Report what would be marked and deleted; change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Days to keep unreferenced files (1..=365); default `media.gc_grace_days` (7).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=365))]
    grace_days: Option<u32>,
    /// Make this site the owner of its media store first (rewrites `ferrite-owner.json`),
    /// for a site that replaced the store's previous owner. With `--dry-run`, nothing is
    /// written; the counts are those the claim would allow.
    #[arg(long)]
    claim: bool,
}

#[derive(Debug, Args)]
pub struct RegenerateArgs {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    data: PathBuf,
    /// Redo every raster item, not only those with a missing size.
    #[arg(long)]
    all: bool,
    /// Only this media item (a content id).
    #[arg(long)]
    item: Option<ContentId>,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// The destination store.
    #[arg(long)]
    to: String,
    /// The source store.
    #[arg(long, conflicts_with = "data", required_unless_present = "data")]
    from: Option<String>,
    /// Use the media store configured in this data folder as the source.
    #[arg(long)]
    data: Option<PathBuf>,
}

pub async fn run(cmd: MediaCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        MediaCommand::Sync(args) => sync(args).await,
        MediaCommand::Gc(args) => collect(args).await,
        MediaCommand::Regenerate(args) => regenerate(args).await,
    }
}

async fn collect(args: GcArgs) -> anyhow::Result<ExitCode> {
    let config = load_config(&args.data.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let done = async {
        startup_check(
            store.as_ref(),
            &binary_version()?,
            &config.database,
            &config.data_dir,
        )
        .await?;
        let media = open_media(&config)?;
        let opts = GcOptions {
            grace: gc::grace_days(args.grace_days.unwrap_or(config.uploads.gc_grace_days)),
            dry_run: args.dry_run,
            now: ferrite_domain::Timestamp::now(),
            claim: args.claim,
        };
        gc::run(store.as_ref(), &media, &opts).await
    }
    .await;
    store.close().await;
    match done? {
        Some(report) if args.dry_run => println!("dry run: {report}"),
        Some(report) => println!("{report}"),
        None => {
            eprintln!("another artifacts GC (or a backup copying artifacts) is running");
            return Ok(ExitCode::from(3));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// How one object fared.
enum Outcome {
    Copied(u64),
    Skipped,
    /// Not an artifact key: neither copied nor counted.
    Ignored,
    Failed(String),
}

async fn sync(args: SyncArgs) -> anyhow::Result<ExitCode> {
    let src = match (&args.from, &args.data) {
        (Some(url), _) => parse_media_url(url, false).context("--from")?,
        (None, Some(data)) => open_media(&load_config(data)?).context("--data")?,
        (None, None) => anyhow::bail!("one of --from and --data is required"),
    };
    let dst = parse_media_url(&args.to, true).context("--to")?;
    // Fail fast (exit 1) on a destination that cannot be reached or written.
    dst.check()
        .await
        .context("--to: the destination store is not usable")?;

    let (mut copied, mut bytes, mut skipped, mut failed, mut ignored) = (0u64, 0, 0u64, 0u64, 0u64);
    let mut outcomes = src
        .list_artifacts()
        .map(|listed| {
            let (src, dst) = (&src, &dst);
            async move {
                let object = listed.context("listing the source store")?;
                let outcome = match &object.hash {
                    None => {
                        tracing::warn!(key = %object.key, "not an artifact key; ignored");
                        Outcome::Ignored
                    }
                    Some(hash) => match dst.head(hash).await {
                        Ok(Some(size)) if size == object.size => Outcome::Skipped,
                        Ok(_) => match src.copy_to(dst, hash).await {
                            Ok(n) => Outcome::Copied(n),
                            Err(e) => Outcome::Failed(e.to_string()),
                        },
                        Err(e) => Outcome::Failed(e.to_string()),
                    },
                };
                anyhow::Ok((object.key, outcome))
            }
        })
        .buffer_unordered(SYNC_CONCURRENCY);
    while let Some(item) = outcomes.next().await {
        let (key, outcome) = item?;
        match outcome {
            Outcome::Failed(why) => {
                failed += 1;
                eprintln!("failed {key}: {why}");
            }
            Outcome::Copied(n) => {
                copied += 1;
                bytes += n;
            }
            Outcome::Skipped => skipped += 1,
            Outcome::Ignored => ignored += 1,
        }
    }
    if ignored > 0 {
        eprintln!("ignored {ignored} key(s) under artifacts/ that are not artifacts");
    }
    println!("copied {copied} ({bytes} bytes), skipped {skipped}, failed {failed}");
    Ok(if failed > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

/// Items between two progress lines.
const PROGRESS_EVERY: usize = 50;

/// What `regenerate` did with the items.
#[derive(Default)]
struct Counts {
    regenerated: u64,
    skipped: u64,
    failed: u64,
}

async fn regenerate(args: RegenerateArgs) -> anyhow::Result<ExitCode> {
    let config = load_config(&args.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let done = async {
        startup_check(
            store.as_ref(),
            &binary_version()?,
            &config.database,
            &config.data_dir,
        )
        .await?;
        let media = open_media(&config)?;
        let theme = load_theme(
            store.as_ref(),
            &media,
            &config.data_dir,
            config.effective_cluster_mode(),
        )
        .await?
        .theme;
        // The sizes a `media.variants` job would make on a node with this theme.
        let specs = variant_specs(&theme);
        let permalinks = Arc::new(SwappablePermalinks::new(
            permalink_rules(store.as_ref()).await?,
        ));
        let content = Arc::new(ContentService::new(store.clone(), permalinks.clone()));
        let service = MediaService::new(store.clone(), content, permalinks);
        let rows = media_rows(store.as_ref(), args.item).await?;
        let counts = regenerate_rows(&media, &service, &specs, rows, args.all).await;
        println!(
            "regenerated {}, skipped {}, failed {}",
            counts.regenerated, counts.skipped, counts.failed
        );
        anyhow::Ok(if counts.failed > 0 {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        })
    }
    .await;
    store.close().await;
    done
}

/// Every media row (whatever the status of its item), or the one of `item` (an error
/// when the id is no media item).
async fn media_rows(store: &dyn Store, item: Option<ContentId>) -> anyhow::Result<Vec<Media>> {
    let repos = store.read();
    if let Some(id) = item {
        let row = repos.media().get(id).await?;
        return Ok(vec![
            row.with_context(|| format!("{id} is not a media item"))?,
        ]);
    }
    let mut rows = Vec::new();
    let mut query = ContentQuery {
        kind: Some("media".to_owned()),
        limit: MAX_PAGE,
        ..ContentQuery::default()
    };
    loop {
        let page = repos.content().list(&query).await?;
        let ids: Vec<ContentId> = page.items.iter().map(|i| i.id).collect();
        rows.extend(repos.media().get_many(&ids).await?);
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(cursor),
            None => return Ok(rows),
        }
    }
}

/// Whether `row` lacks a size of `specs` that applies to its width. A row without
/// dimensions has not been processed yet.
fn lacks_variant(row: &Media, specs: &[VariantSpec]) -> bool {
    let Some(width) = row.width.and_then(|w| u32::try_from(w).ok()) else {
        return true;
    };
    let have = row.variants.as_object();
    specs
        .iter()
        .any(|s| s.is_made_for(width) && !have.is_some_and(|v| v.contains_key(&s.name)))
}

async fn regenerate_rows(
    media: &MediaStore,
    service: &MediaService,
    specs: &[VariantSpec],
    rows: Vec<Media>,
    all: bool,
) -> Counts {
    // One image at a time: the memory bound of `media.image_workers = 1`.
    let slots = Arc::new(Semaphore::new(1));
    let processor: ImageProcessor = Arc::new(process_image);
    let maker = VariantMaker {
        media,
        service,
        processor: &processor,
    };
    let total = rows.len();
    let mut counts = Counts::default();
    for (n, row) in rows.into_iter().enumerate() {
        let id = row.item_id;
        if !is_raster_image(&row.mime) || !(all || lacks_variant(&row, specs)) {
            counts.skipped += 1;
        } else {
            match one(media, &maker, &slots, specs, &row).await {
                Ok(true) => counts.regenerated += 1,
                Ok(false) => counts.skipped += 1,
                Err(why) => {
                    counts.failed += 1;
                    eprintln!("failed {id}: {why}");
                }
            }
        }
        if (n + 1) % PROGRESS_EVERY == 0 {
            eprintln!("{} of {total} items", n + 1);
        }
    }
    counts
}

/// Makes the variants of one raster item: `Ok(false)` when the original is over
/// [`MAX_PROCESS_BYTES`] or the row was replaced meanwhile.
async fn one(
    media: &MediaStore,
    maker: &VariantMaker<'_>,
    slots: &Arc<Semaphore>,
    specs: &[VariantSpec],
    row: &Media,
) -> anyhow::Result<bool> {
    let hash = &row.artifact_hash;
    let size = media
        .head(hash)
        .await?
        .context("the original is not stored")?;
    if size > MAX_PROCESS_BYTES {
        eprintln!(
            "skipped {}: original larger than {MAX_PROCESS_BYTES} bytes",
            row.item_id
        );
        return Ok(false);
    }
    // The slot first, as the job does: buffered originals stay bounded by the slots.
    let slot = slots.clone().acquire_owned().await?;
    let data = media.get(hash).await?;
    Ok(maker
        .make(row.item_id, hash, data, specs.to_vec(), slot)
        .await?)
}
