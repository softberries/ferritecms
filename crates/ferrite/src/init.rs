use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Args;
use ferrite_config::{ClusterMode, DbKind, InitMedia, MediaSection, render_default_toml};
use sqlx::{AssertSqlSafe, Connection, PgConnection};

use crate::keys::SECRET_KEY_FILE;
use crate::load_config;
use crate::open::{MIGRATE_LOCK_WAIT, OpenMode, open_store};

const CONFIG_FILE: &str = "ferrite.toml";

#[derive(Args)]
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

// Manual Debug: `db` may embed a password.
impl std::fmt::Debug for InitArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitArgs")
            .field("data", &self.data)
            .field("url", &self.url)
            .field("title", &self.title)
            .field("db", &self.db.as_ref().map(|_| "[redacted]"))
            .field("schema", &self.schema)
            .field("media", &self.media)
            .finish()
    }
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
    let data_existed = args.data.exists();
    std::fs::create_dir_all(&args.data)
        .with_context(|| format!("cannot create {}", args.data.display()))?;
    write_private(&config_path, &rendered)?;

    let mut created = Vec::new();
    if let Err(e) = prepare(&args.data, &mut created).await {
        // Remove only what this run created, so a retry starts clean.
        let _ = std::fs::remove_file(&config_path);
        for p in created {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
        if !data_existed {
            let _ = std::fs::remove_dir(&args.data);
        }
        return Err(e);
    }

    let data = args.data.display();
    println!("Initialised {data}");
    println!("Next steps:");
    println!("  ferrite migrate status --data {data}");
    println!("  ferrite seed demo --data {data}   (optional sample content)");
    println!("  ferrite serve --data {data}");
    if matches!(media, InitMedia::S3 { .. }) {
        println!(
            "Edit {data}/{CONFIG_FILE} to set the S3 region, endpoint and credentials before serving."
        );
    }
    Ok(())
}

/// Validates the written config, refuses an existing database, creates media/backups
/// folders and migrates. Paths this run creates are pushed to `created` before they are.
async fn prepare(data: &Path, created: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let config = load_config(data)?;
    match config.database_kind() {
        DbKind::Sqlite { path } => {
            if path.exists() {
                bail!(
                    "existing database found at {}; use `ferrite migrate` instead",
                    path.display()
                );
            }
            let suffixed = |s: &str| {
                let mut o = path.clone().into_os_string();
                o.push(s);
                PathBuf::from(o)
            };
            track(created, suffixed("-wal"));
            track(created, suffixed("-shm"));
            track(created, data.join("ferrite.lock"));
            track(created, path);
        }
        DbKind::Postgres { url } => {
            ensure_pg_unmigrated(url.expose(), config.database.schema.as_deref()).await?;
        }
    }
    if let MediaSection::Local { root, .. } = &config.media {
        track(created, root.clone());
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create {}", root.display()))?;
    }
    // Solo without a configured key: create `secret.key` now (F30). A cluster config
    // must carry the key itself, so no unused key file is written next to it.
    if config.auth.secret_key.is_none() && config.effective_cluster_mode() == ClusterMode::Solo {
        track(created, data.join(SECRET_KEY_FILE));
        crate::keys::site_key(&config)?;
    }
    let backups = data.join("backups");
    track(created, backups.clone());
    std::fs::create_dir_all(&backups)
        .with_context(|| format!("cannot create {}", backups.display()))?;
    let store = open_store(&config, OpenMode::CreateIfMissing).await?;
    let applied = crate::migrate::up_locked(store.as_ref(), None, MIGRATE_LOCK_WAIT).await?;
    tracing::info!(?applied, "database migrated");
    Ok(())
}

/// Records `path` for cleanup if it does not exist yet.
fn track(created: &mut Vec<PathBuf>, path: PathBuf) {
    if !path.exists() {
        created.push(path);
    }
}

/// Refuses a Postgres schema that already has applied migrations.
async fn ensure_pg_unmigrated(url: &str, schema: Option<&str>) -> anyhow::Result<()> {
    let mut conn = PgConnection::connect(url)
        .await
        .context("cannot connect to the Postgres database")?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = COALESCE($1, current_schema()) AND table_name = '_sqlx_migrations')",
    )
    .bind(schema)
    .fetch_one(&mut conn)
    .await?;
    let rows: i64 = if exists {
        let table = match schema {
            Some(s) => format!("\"{}\"._sqlx_migrations", s.replace('"', "\"\"")),
            None => "_sqlx_migrations".to_owned(),
        };
        sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&mut conn)
            .await?
    } else {
        0
    };
    conn.close().await?;
    if rows > 0 {
        bail!(
            "existing database found{}; use `ferrite migrate` instead",
            schema.map_or_else(String::new, |s| format!(" in schema {s}"))
        );
    }
    Ok(())
}

/// Creates `path` exclusively with mode 0600 (it can hold the database URL).
fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut file = create_private(path)?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Creates `path` exclusively (an existing file is refused) with mode 0600, for files that
/// hold secrets: `ferrite.toml`, archives.
pub(crate) fn create_private(path: &Path) -> anyhow::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
        .with_context(|| format!("cannot create {}", path.display()))
}
