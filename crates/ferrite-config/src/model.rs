use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ferrite_domain::Secret;
use ferrite_media::ConditionalPut;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The data folder this configuration was loaded from.
    pub data_dir: PathBuf,
    pub site: SiteConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub media: MediaSection,
    /// `[media] max_upload`, `image_workers` (M2a-2b P8/P9), `strip_metadata` and
    /// `gc_grace_days` (M4-1); either backend.
    pub uploads: UploadConfig,
    pub cluster: ClusterConfig,
    pub cache: CacheConfig,
    pub plugins: PluginsConfig,
    pub auth: AuthConfig,
    /// `[mail]`; `None` when `smtp_url` is not set (mail is then unavailable).
    pub mail: Option<MailConfig>,
    /// `[backup]`: the scheduled backup (M4-1 P15); off by default.
    pub backup: BackupConfig,
    /// Valid settings that have no effect (for example `[server.tls]` keys while its
    /// `mode` is `"off"`); `ferrite config check` prints them as warnings.
    pub warnings: Vec<crate::ConfigProblem>,
}

/// How often `serve` runs the `site.backup` job (`backup.schedule`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackupSchedule {
    #[default]
    Off,
    Daily,
    Weekly,
}

impl BackupSchedule {
    /// The config value.
    pub fn as_str(self) -> &'static str {
        match self {
            BackupSchedule::Off => "off",
            BackupSchedule::Daily => "daily",
            BackupSchedule::Weekly => "weekly",
        }
    }

    /// The period in seconds; `None` when off.
    pub fn every_secs(self) -> Option<u64> {
        match self {
            BackupSchedule::Off => None,
            BackupSchedule::Daily => Some(86_400),
            BackupSchedule::Weekly => Some(604_800),
        }
    }
}

/// Default of `backup.keep`.
pub const DEFAULT_BACKUP_KEEP: u32 = 7;
/// Largest `backup.keep`.
pub const MAX_BACKUP_KEEP: u32 = 1000;

/// `[backup]` (M4-1 P15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    pub schedule: BackupSchedule,
    /// `backup.to` as written: a folder (relative paths are relative to the data folder,
    /// and it must lie outside it) or `s3://bucket[/prefix]`. Required unless the schedule
    /// is off.
    pub to: Option<String>,
    /// Snapshots (SQLite) or export archives (Postgres) kept: 1..=[`MAX_BACKUP_KEEP`],
    /// default [`DEFAULT_BACKUP_KEEP`].
    pub keep: u32,
    /// Include the media artifacts (default true).
    pub with_media: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            schedule: BackupSchedule::Off,
            to: None,
            keep: DEFAULT_BACKUP_KEEP,
            with_media: true,
        }
    }
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
    /// Plain HTTP. With TLS on it keeps serving; bind it to loopback (healthcheck).
    pub listen: SocketAddr,
    pub trusted_proxies: Vec<ipnet::IpNet>,
    /// Where `GET /metrics` (Prometheus text) is served, on a listener of its own
    /// (M4-2 P34); `None` (the default): no metrics listener. Never on `listen`.
    pub metrics_listen: Option<SocketAddr>,
    /// `[server.tls]` (M4-2 P33).
    pub tls: TlsConfig,
}

/// `[server.tls] mode`: TLS terminated by the binary itself (solo only), or not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TlsConfig {
    /// No TLS listener (the default: a reverse proxy terminates TLS).
    #[default]
    Off,
    /// Certificates from an ACME CA (TLS-ALPN-01 on the TLS listener).
    Acme(AcmeTls),
}

impl TlsConfig {
    /// The `mode` value.
    pub fn mode(&self) -> &'static str {
        match self {
            TlsConfig::Off => "off",
            TlsConfig::Acme(_) => "acme",
        }
    }
}

/// Default of `server.tls.listen`.
pub const DEFAULT_TLS_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 443);

/// `[server.tls]` with `mode = "acme"` (M4-2 P33), validated: `site.url` is https and
/// its host is in `domains`, the mode is solo, the listeners differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeTls {
    /// The certificate's DNS names (default: the host of `site.url`), lowercase.
    pub domains: Vec<String>,
    /// The ACME account contact, a `mailto:` URI.
    pub contact: String,
    pub directory: AcmeDirectory,
    /// The HTTPS listener (default [`DEFAULT_TLS_LISTEN`]).
    pub listen: SocketAddr,
    /// An optional plain HTTP listener that only answers `301` to the https site.
    pub redirect_http: Option<SocketAddr>,
}

/// `server.tls.directory`: the ACME CA.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AcmeDirectory {
    /// Let's Encrypt (the default).
    #[default]
    Production,
    /// Let's Encrypt's staging CA (untrusted certificates, higher rate limits).
    Staging,
    /// Any other ACME directory, an `https://` URL.
    Url(url::Url),
}

impl AcmeDirectory {
    /// The directory URL.
    pub fn url(&self) -> &str {
        match self {
            AcmeDirectory::Production => "https://acme-v02.api.letsencrypt.org/directory",
            AcmeDirectory::Staging => "https://acme-staging-v02.api.letsencrypt.org/directory",
            AcmeDirectory::Url(u) => u.as_str(),
        }
    }
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
        /// object_store's encoding (`header:…`, `header-with-status:…`, `multipart`),
        /// already validated.
        copy_if_not_exists: Option<String>,
        conditional_put: Option<ConditionalPut>,
        prefix: String,
        public_base_url: Option<url::Url>,
    },
}

/// Default `media.max_upload`: 256 MiB.
pub const DEFAULT_MAX_UPLOAD: u64 = 256 << 20;
/// Largest `media.max_upload`: 1 GiB (below the store's 5 GB copy ceiling, 03 §3).
pub const MAX_UPLOAD_CEILING: u64 = 1 << 30;
/// Largest `media.image_workers`.
pub const MAX_IMAGE_WORKERS: usize = 16;
/// Default `media.upload_idle_timeout`: 30 s.
pub const DEFAULT_UPLOAD_IDLE_SECS: u64 = 30;
/// Default `media.gc_grace_days` (M4-1 P11, 03 §6).
pub const DEFAULT_GC_GRACE_DAYS: u32 = 7;
/// Largest `media.gc_grace_days`.
pub const MAX_GC_GRACE_DAYS: u32 = 365;

/// The upload settings of `[media]` (M2a-2b P8/P9), and the artifacts GC's grace period
/// (M4-1 P11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadConfig {
    /// Largest uploaded file in bytes (`media.max_upload`): 1..=[`MAX_UPLOAD_CEILING`],
    /// default [`DEFAULT_MAX_UPLOAD`].
    pub max_upload: u64,
    /// Image variant jobs at once per node (`media.image_workers`):
    /// 1..=[`MAX_IMAGE_WORKERS`], default 1.
    pub image_workers: usize,
    /// An upload that receives no bytes for this long is aborted and its slot freed
    /// (`media.upload_idle_timeout`, seconds > 0, default [`DEFAULT_UPLOAD_IDLE_SECS`]).
    pub idle_timeout_secs: u64,
    /// Strip EXIF and XMP from JPEG, PNG and WebP originals on ingest, losslessly
    /// (`media.strip_metadata`, default false: the original is kept byte for byte).
    pub strip_metadata: bool,
    /// Days an artifact stays after nothing references it, and a blob with no row after
    /// it was written, before the artifacts GC deletes it (`media.gc_grace_days`,
    /// 1..=[`MAX_GC_GRACE_DAYS`], default [`DEFAULT_GC_GRACE_DAYS`]).
    pub gc_grace_days: u32,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_upload: DEFAULT_MAX_UPLOAD,
            image_workers: 1,
            idle_timeout_secs: DEFAULT_UPLOAD_IDLE_SECS,
            strip_metadata: false,
            gc_grace_days: DEFAULT_GC_GRACE_DAYS,
        }
    }
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
    /// Cache TTL of search result pages in seconds (`cache.search_ttl`, default 60);
    /// `0` means search pages are not cached.
    pub search_ttl_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsConfig {
    pub default_memory: u64,
    pub default_fuel: u64,
    pub render_deadline_ms: u64,
    pub action_deadline_ms: u64,
    pub disable_after_failures: u32,
    pub failure_window_secs: u64,
    /// Most plugin calls running at once per node (`plugins.max_concurrent_calls`,
    /// P22): default `max(8, 4 × available_parallelism)`, explicit values > 0.
    pub max_concurrent_calls: usize,
    /// Time all plugin filters of one page may take together, from the render slot on
    /// (`plugins.render_budget_ms`, P22): default 1 000, > 0.
    pub render_budget_ms: u64,
    /// Cache TTL in seconds of pages whose plugin calls were volatile or failed
    /// (`plugins.volatile_ttl`, 05 §7a): default 60, 1..=3 600.
    pub volatile_ttl_secs: u64,
    /// Signatures and the registry (M6-2, design 14 §3.2, §5.2).
    pub trust: PluginTrustConfig,
}

/// The default plugin registry (design 14 §5.2): the official plugins on GitHub Pages,
/// verified with the key embedded from `crates/ferrite/registry.pub`.
pub const DEFAULT_REGISTRY_URL: &str =
    "https://softberries.github.io/ferritecms/registry/v1/index.json";

/// `[plugins] signatures`, `registry`, `registry_key`, `registry_check` (M6-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTrustConfig {
    /// `any` (default), `signed` or `trusted` (design 14 §3.2).
    pub signatures: ferrite_domain::plugin_trust::SignaturePolicy,
    /// The registry index URL (`https://…` or `file:///abs/…`); `None` when set to `""`.
    /// Default [`DEFAULT_REGISTRY_URL`].
    pub registry: Option<String>,
    /// The registry's minisign public key (the base64 line); required for any registry
    /// but the default, which uses the embedded key.
    pub registry_key: Option<String>,
    /// Whether `serve` checks the registry daily (ruling P5: off by default; the Plugins
    /// screen's "Check for updates" and the CLI fetch on demand).
    pub registry_check: RegistryCheck,
}

impl Default for PluginTrustConfig {
    fn default() -> Self {
        Self {
            signatures: Default::default(),
            registry: Some(DEFAULT_REGISTRY_URL.to_owned()),
            registry_key: None,
            registry_check: RegistryCheck::Off,
        }
    }
}

/// `plugins.registry_check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegistryCheck {
    #[default]
    Off,
    Daily,
}

impl RegistryCheck {
    pub fn as_str(self) -> &'static str {
        match self {
            RegistryCheck::Off => "off",
            RegistryCheck::Daily => "daily",
        }
    }
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
