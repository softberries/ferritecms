use std::io::Write;
use std::path::Path;

use anyhow::{Context, bail};
use clap::Args;
use ferrite_config::{InitMedia, MediaSection, render_default_toml};

use crate::{load_config, open::open_store};

const CONFIG_FILE: &str = "ferrite.toml";

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Data folder to create.
    #[arg(long)]
    pub data: std::path::PathBuf,
    /// Public URL of the site, e.g. https://example.com.
    #[arg(long)]
    pub url: String,
    /// Site title.
    #[arg(long, default_value = "My Site")]
    pub title: String,
    /// Database URL (default: SQLite in the data folder), e.g. postgres://user@host/db.
    #[arg(long)]
    pub db: Option<String>,
    /// Postgres schema to keep all tables in (created if missing).
    #[arg(long)]
    pub schema: Option<String>,
    /// Store media in S3: s3://bucket/prefix (default: the data folder).
    #[arg(long)]
    pub media: Option<String>,
}

pub async fn run(args: InitArgs) -> anyhow::Result<()> {
    let config_path = args.data.join(CONFIG_FILE);
    if config_path.exists() {
        bail!(
            "{} already exists; refusing to overwrite it",
            config_path.display()
        );
    }
    let media = match &args.media {
        Some(url) => InitMedia::parse_s3_url(url).map_err(|e| anyhow::anyhow!("--media: {e}"))?,
        None => InitMedia::Local,
    };
    let rendered = render_default_toml(
        &args.url,
        &args.title,
        args.db.as_deref(),
        args.schema.as_deref(),
        &media,
    );
    std::fs::create_dir_all(&args.data)
        .with_context(|| format!("cannot create {}", args.data.display()))?;
    write_private(&config_path, &rendered)?;

    match prepare(&args.data).await {
        Ok(()) => {}
        Err(e) => {
            // Leave no half-initialised folder behind that blocks a retry.
            let _ = std::fs::remove_file(&config_path);
            return Err(e);
        }
    }

    let data = args.data.display();
    println!("Initialised {data}");
    println!("Next steps:");
    println!("  ferrite migrate status --data {data}");
    println!("  ferrite serve --data {data}   (arrives in a later milestone)");
    if matches!(media, InitMedia::S3 { .. }) {
        println!(
            "Edit {data}/{CONFIG_FILE} to set the S3 region, endpoint and credentials before serving."
        );
    }
    Ok(())
}

/// Validates the written config, creates media/backups folders and migrates the database.
async fn prepare(data: &Path) -> anyhow::Result<()> {
    let config = load_config(data)?;
    if let MediaSection::Local { root, .. } = &config.media {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create {}", root.display()))?;
    }
    std::fs::create_dir_all(data.join("backups"))
        .with_context(|| format!("cannot create {}", data.join("backups").display()))?;
    let store = open_store(&config).await?;
    let applied = store.migrate_up(None).await?;
    tracing::info!(?applied, "database migrated");
    Ok(())
}

/// Creates `path` exclusively with mode 0600 (it can hold the database URL).
fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))
}
