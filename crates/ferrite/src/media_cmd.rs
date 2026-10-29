//! `ferrite media …`: tools over the media store.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Args, Subcommand};
use futures::StreamExt;

use crate::load_config;
use crate::media_url::parse_media_url;
use crate::site::open_media;

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
    }
}

/// How one object fared.
enum Outcome {
    Copied(u64),
    Skipped,
    Failed(String),
}

async fn sync(args: SyncArgs) -> anyhow::Result<ExitCode> {
    let src = match (&args.from, &args.data) {
        (Some(url), _) => parse_media_url(url, false).context("--from")?,
        (None, Some(data)) => open_media(&load_config(data)?).context("--data")?,
        (None, None) => anyhow::bail!("one of --from and --data is required"),
    };
    let dst = parse_media_url(&args.to, true).context("--to")?;

    let (mut copied, mut bytes, mut skipped, mut failed) = (0u64, 0u64, 0u64, 0u64);
    let mut outcomes = src
        .list_artifacts()
        .map(|listed| {
            let (src, dst) = (&src, &dst);
            async move {
                let object = listed.context("listing the source store")?;
                let outcome = match &object.hash {
                    None => {
                        tracing::warn!(key = %object.key, "not an artifact key; ignored");
                        Outcome::Skipped
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
        }
    }
    println!("copied {copied} ({bytes} bytes), skipped {skipped}, failed {failed}");
    Ok(if failed > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}
