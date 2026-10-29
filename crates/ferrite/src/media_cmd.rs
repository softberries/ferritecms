//! `ferrite media …`: tools over the media store.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Args, Subcommand};
use futures::StreamExt;

use crate::cli::DataArg;
use crate::gc::{self, GcOptions};
use crate::media_url::parse_media_url;
use crate::open::{OpenMode, open_store, startup_check};
use crate::site::open_media;
use crate::{binary_version, load_config};

/// Copies running at once.
const SYNC_CONCURRENCY: usize = 8;

#[derive(Debug, Subcommand)]
pub enum MediaCommand {
    /// Copy every artifact from one media store to another (a backend migration).
    ///
    /// A store is `local:<path>` or `s3://bucket[/prefix]`. With `--data` the source is the
    /// site's own store from its ferrite.toml (no database is opened, so this works while the
    /// site runs); every `s3://` URL takes its credentials and endpoint from the environment
    /// (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ENDPOINT_URL`,
    /// `AWS_ALLOW_HTTP`).
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
    /// Prints `marked N, unmarked M, deleted rows R (B bytes), deleted unrecorded blobs U
    /// (B bytes), deleted tmp T` (with `--dry-run`, after `dry run: `, counting what would
    /// happen). Exit code: 0, or 3 when another GC is running, or 1 on an error.
    Gc(GcArgs),
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
    }
}

async fn collect(args: GcArgs) -> anyhow::Result<ExitCode> {
    let config = load_config(&args.data.data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let done = async {
        startup_check(store.as_ref(), &binary_version()?, &config.database).await?;
        let media = open_media(&config)?;
        let opts = GcOptions {
            grace: gc::grace_days(args.grace_days.unwrap_or(config.uploads.gc_grace_days)),
            dry_run: args.dry_run,
            now: ferrite_domain::Timestamp::now(),
        };
        gc::run(store.as_ref(), &media, &opts).await
    }
    .await;
    store.close().await;
    match done? {
        Some(report) if args.dry_run => println!("dry run: {report}"),
        Some(report) => println!("{report}"),
        None => {
            eprintln!("another artifacts GC is running");
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
