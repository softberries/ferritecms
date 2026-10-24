use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ferrite_domain::Secret;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The data folder this configuration was loaded from.
    pub data_dir: PathBuf,
    pub site: SiteConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub media: MediaSection,
    pub cluster: ClusterConfig,
    pub cache: CacheConfig,
    pub plugins: PluginsConfig,
    pub auth: AuthConfig,
    /// `[mail]`; `None` when `smtp_url` is not set (mail is then unavailable).
    pub mail: Option<MailConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteConfig {
    pub url: url::Url,
    pub title: String,
    pub locale: String,
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub trusted_proxies: Vec<ipnet::IpNet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub url: Secret,
    pub max_connections: u32,
    pub auto_migrate: bool,
    pub allow_contract: bool,
    pub schema: Option<String>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "interface fixed by the brief; one value exists per process"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSection {
    Local {
        /// Already resolved against the data folder.
        root: PathBuf,
        /// Operator asserts the folder is a shared volume (04 §3).
        local_shared: bool,
    },
    S3 {
        bucket: String,
        region: String,
        endpoint: Option<String>,
        access_key_id: Option<String>,
        secret_access_key: Option<Secret>,
        allow_http: bool,
        virtual_hosted_style: bool,
        prefix: String,
        public_base_url: Option<url::Url>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterMode {
    Solo,
    Cluster,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConfig {
    pub mode: ClusterMode,
    pub node_name: Option<String>,
    /// Concurrent job handlers per node (`cluster.workers`): default
    /// `max(2, available_parallelism)`, explicit values 1..=256 (P16).
    pub workers: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggedInCache {
    Bypass,
    PerUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    pub max_bytes: u64,
    pub logged_in: LoggedInCache,
    /// How often the invalidation loop polls the event outbox (milliseconds, >= 1).
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsConfig {
    pub default_memory: u64,
    pub default_fuel: u64,
    pub render_deadline_ms: u64,
    pub action_deadline_ms: u64,
    pub disable_after_failures: u32,
    pub failure_window_secs: u64,
}

/// `[auth]` (P4). `secret_key` is the site key as 64 hex characters; `None` means solo
/// mode uses `<data>/secret.key`. Cluster mode requires it (validated in `load`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub secret_key: Option<Secret>,
}

/// `[mail]` (M2a-2 P17). `smtp_url` is `smtps://` (implicit TLS), `smtp://…?tls=required`
/// (STARTTLS) or plain `smtp://`, credentials in the URL only over TLS or to a loopback
/// relay ([`check_smtp_url`](crate::check_smtp_url), R16); `from` is the sender mailbox (`a@example.com` or `Name <a@example.com>`).
/// Both are shape-checked here; the SMTP mailer parses them again at boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailConfig {
    pub smtp_url: Secret,
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbKind {
    /// Path resolved against the data folder.
    Sqlite {
        path: PathBuf,
    },
    Postgres {
        url: Secret,
    },
}

impl Config {
    pub fn database_kind(&self) -> DbKind {
        db_kind(self.database.url.expose(), &self.data_dir)
    }

    /// `Auto` resolves to `Cluster` only when the database is Postgres and media is
    /// shareable between nodes (S3, or local with `local_shared = true`); otherwise `Solo`.
    pub fn effective_cluster_mode(&self) -> ClusterMode {
        effective_mode(self.cluster.mode, self.database.url.expose(), &self.media)
    }
}

/// [`Config::effective_cluster_mode`] from its parts (also used while validating).
pub(crate) fn effective_mode(mode: ClusterMode, db_url: &str, media: &MediaSection) -> ClusterMode {
    match mode {
        ClusterMode::Auto => {
            let shared_media = matches!(
                media,
                MediaSection::S3 { .. }
                    | MediaSection::Local {
                        local_shared: true,
                        ..
                    }
            );
            if is_postgres(db_url) && shared_media {
                ClusterMode::Cluster
            } else {
                ClusterMode::Solo
            }
        }
        mode => mode,
    }
}

fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

pub(crate) fn is_sqlite(url: &str) -> bool {
    url.starts_with("sqlite:")
}

pub(crate) fn is_supported_db_url(url: &str) -> bool {
    is_postgres(url) || is_sqlite(url)
}

fn db_kind(url: &str, data_dir: &Path) -> DbKind {
    if is_postgres(url) {
        return DbKind::Postgres {
            url: Secret::new(url),
        };
    }
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);
    let rest = rest.split('?').next().unwrap_or(rest);
    let path = PathBuf::from(rest);
    let path = if path.is_absolute() || rest == ":memory:" {
        path
    } else {
        data_dir.join(path)
    };
    DbKind::Sqlite { path }
}
