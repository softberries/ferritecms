use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;

use ferrite_media::ConditionalPut;
use ipnet::IpNet;
use toml::{Table, Value};

use crate::cx::{Cx, join, resolve};
use crate::error::{ConfigError, ConfigProblem};
use crate::model::*;
use crate::model::{effective_mode, is_sqlite, is_supported_db_url};

const FILE_NAME: &str = "ferrite.toml";
const ENV_PREFIX: &str = "FERRITE_";

/// Load `data_dir/ferrite.toml` (missing file means defaults), apply `FERRITE_*`
/// overrides from `env` (name, value pairs), then validate. All problems are collected.
///
/// `FERRITE_SITE__URL` sets `site.url`; a double underscore nests and keys are
/// lowercased. `FERRITE_LOG` and `FERRITE_TEST_*` are not configuration and are ignored,
/// as is anything not starting with `FERRITE_`.
pub fn load(
    data_dir: &Path,
    env: impl IntoIterator<Item = (String, String)>,
) -> Result<Config, ConfigError> {
    let path = data_dir.join(FILE_NAME);
    let mut table = match std::fs::read_to_string(&path) {
        Ok(src) => match src.parse::<Table>() {
            Ok(t) => t,
            Err(e) => {
                // Do not print the source snippet: it may contain secrets.
                let (line, col) = e.span().map(|s| line_col(&src, s.start)).unwrap_or((0, 0));
                return Err(single(
                    FILE_NAME,
                    format!("invalid TOML at line {line}, column {col}: {}", e.message()),
                ));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Table::new(),
        Err(e) => {
            return Err(single(FILE_NAME, format!("cannot read file: {}", e.kind())));
        }
    };

    let mut cx = Cx {
        problems: Vec::new(),
        data_dir: data_dir.to_path_buf(),
        env_paths: HashMap::new(),
        env_raw: HashMap::new(),
    };
    apply_env(&mut cx, &mut table, env);

    let site = parse_site(&mut cx, &mut table);
    let server = parse_server(&mut cx, &mut table);
    let database = parse_database(&mut cx, &mut table);
    let (media, uploads) = parse_media(&mut cx, &mut table);
    let cluster = parse_cluster(&mut cx, &mut table);
    let cache = parse_cache(&mut cx, &mut table);
    let plugins = parse_plugins(&mut cx, &mut table);
    let (auth, key_given) = parse_auth(&mut cx, &mut table);
    let mail = parse_mail(&mut cx, &mut table);
    cx.unknown(table, "");

    validate_cluster(&mut cx, &cluster, &database, &media);
    // A given but invalid key is already a problem; only a missing one is reported here.
    if !key_given
        && effective_mode(cluster.mode, database.url.expose(), &media) == ClusterMode::Cluster
    {
        cx.push(
            "auth.secret_key",
            "required in cluster mode (every node must use the same key)",
        );
    }

    match site {
        Some(site) if cx.problems.is_empty() => Ok(Config {
            data_dir: data_dir.to_path_buf(),
            site,
            server,
            database,
            media,
            uploads,
            cluster,
            cache,
            plugins,
            auth,
            mail,
        }),
        _ => Err(ConfigError {
            problems: cx.problems,
        }),
    }
}

fn single(path: &str, message: String) -> ConfigError {
    ConfigError {
        problems: vec![ConfigProblem {
            path: path.to_owned(),
            message,
        }],
    }
}

fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

fn apply_env(cx: &mut Cx, table: &mut Table, env: impl IntoIterator<Item = (String, String)>) {
    let mut vars: Vec<(String, String)> = env
        .into_iter()
        .filter(|(k, _)| {
            k.starts_with(ENV_PREFIX) && k != "FERRITE_LOG" && !k.starts_with("FERRITE_TEST_")
        })
        .collect();
    vars.sort();
    for (name, raw) in vars {
        let segs: Vec<String> = name[ENV_PREFIX.len()..]
            .split("__")
            .map(str::to_lowercase)
            .collect();
        if segs.iter().any(String::is_empty) {
            cx.problems.push(ConfigProblem {
                path: format!("env:{name}"),
                message: "invalid variable name (empty key segment)".to_owned(),
            });
            continue;
        }
        let dotted = segs.join(".");
        let mut prefix = String::new();
        for seg in &segs {
            prefix = join(&prefix, seg);
            cx.env_paths.entry(prefix.clone()).or_insert(name.clone());
        }
        cx.env_paths.insert(dotted.clone(), name.clone());
        cx.env_raw.insert(dotted, raw.clone());
        insert(table, &segs, parse_env_value(&raw));
    }
}

/// TOML scalar/array syntax when it parses, otherwise the raw string.
fn parse_env_value(raw: &str) -> Value {
    format!("v = {raw}")
        .parse::<Table>()
        .ok()
        .and_then(|mut t| t.remove("v"))
        .filter(|v| !matches!(v, Value::Table(_)))
        .unwrap_or_else(|| Value::String(raw.to_owned()))
}

fn insert(table: &mut Table, segs: &[String], value: Value) {
    match segs {
        [] => {}
        [last] => {
            table.insert(last.clone(), value);
        }
        [first, rest @ ..] => {
            let entry = table
                .entry(first.clone())
                .or_insert_with(|| Value::Table(Table::new()));
            if !entry.is_table() {
                *entry = Value::Table(Table::new());
            }
            if let Value::Table(sub) = entry {
                insert(sub, rest, value);
            }
        }
    }
}

fn section(cx: &mut Cx, table: &mut Table, key: &str) -> Table {
    cx.section(table, "", key).unwrap_or_default()
}

fn parse_site(cx: &mut Cx, root: &mut Table) -> Option<SiteConfig> {
    let mut t = section(cx, root, "site");
    let present = t.contains_key("url");
    let url = match cx.string(&mut t, "site", "url") {
        None => {
            if !present {
                cx.push("site.url", "required");
            }
            None
        }
        Some(s) => match url::Url::parse(&s) {
            Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host() => Some(u),
            Ok(_) => {
                cx.push("site.url", "must be an absolute http:// or https:// URL");
                None
            }
            Err(e) => {
                cx.push("site.url", format!("invalid URL: {e}"));
                None
            }
        },
    };
    let title = cx.string(&mut t, "site", "title").unwrap_or_default();
    let locale = non_empty(cx, &mut t, "site", "locale", "en");
    let timezone = non_empty(cx, &mut t, "site", "timezone", "UTC");
    cx.unknown(t, "site");
    Some(SiteConfig {
        url: url?,
        title,
        locale,
        timezone,
    })
}

fn non_empty(cx: &mut Cx, t: &mut Table, base: &str, key: &str, default: &str) -> String {
    match cx.string(t, base, key) {
        Some(s) if s.trim().is_empty() => {
            cx.push(&join(base, key), "must not be empty");
            default.to_owned()
        }
        Some(s) => s,
        None => default.to_owned(),
    }
}

fn parse_server(cx: &mut Cx, root: &mut Table) -> ServerConfig {
    let mut t = section(cx, root, "server");
    let listen = match cx.string(&mut t, "server", "listen") {
        Some(s) => match s.parse::<SocketAddr>() {
            Ok(a) => a,
            Err(_) => {
                cx.push(
                    "server.listen",
                    "must be a socket address such as \"0.0.0.0:8080\"",
                );
                default_listen()
            }
        },
        None => default_listen(),
    };
    let mut trusted_proxies = Vec::new();
    if let Some(v) = t.remove("trusted_proxies") {
        let items: Option<Vec<String>> = match v {
            Value::Array(a) => a
                .into_iter()
                .map(|i| match i {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect(),
            // Env convenience: comma separated.
            Value::String(s) => Some(
                s.split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => None,
        };
        match items {
            None => cx.push(
                "server.trusted_proxies",
                "expected an array of CIDR strings",
            ),
            Some(items) => {
                for (i, item) in items.iter().enumerate() {
                    match parse_net(item) {
                        Some(net) => trusted_proxies.push(net),
                        None => cx.push(
                            &format!("server.trusted_proxies[{i}]"),
                            "must be a CIDR such as \"10.0.0.0/8\" or an IP address",
                        ),
                    }
                }
            }
        }
    }
    cx.unknown(t, "server");
    ServerConfig {
        listen,
        trusted_proxies,
    }
}

fn parse_net(s: &str) -> Option<IpNet> {
    s.parse::<IpNet>()
        .ok()
        .or_else(|| s.parse::<std::net::IpAddr>().ok().map(IpNet::from))
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}

fn parse_database(cx: &mut Cx, root: &mut Table) -> DatabaseConfig {
    let mut t = section(cx, root, "database");
    let url = match cx.secret(&mut t, "database", "url") {
        Some(s) => {
            if !is_supported_db_url(s.expose()) {
                cx.push(
                    "database.url",
                    "unsupported URL: expected sqlite:… or postgres://…",
                );
            }
            if is_sqlite(s.expose()) && s.expose().contains('?') {
                cx.push(
                    "database.url",
                    "query options are not supported on sqlite URLs",
                );
            }
            s
        }
        None => ferrite_domain::Secret::new("sqlite://ferrite.db"),
    };
    let max_connections = cx
        .positive_u32(&mut t, "database", "max_connections")
        .unwrap_or(16);
    let auto_migrate = cx
        .boolean(&mut t, "database", "auto_migrate")
        .unwrap_or(true);
    let allow_contract = cx
        .boolean(&mut t, "database", "allow_contract")
        .unwrap_or(false);
    let schema = cx.string(&mut t, "database", "schema");
    if let Some(s) = &schema
        && (s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        cx.push(
            "database.schema",
            "must be a non-empty identifier of letters, digits and underscores",
        );
    }
    if let Some(s) = &schema
        && s.len() > MAX_SCHEMA_LEN
    {
        cx.push(
            "database.schema",
            format!(
                "at most {MAX_SCHEMA_LEN} characters, so the notification channel name fits \
                 Postgres' 63-byte limit"
            ),
        );
    }
    if schema.is_some() && is_sqlite(url.expose()) {
        cx.push("database.schema", "only valid with a Postgres database");
    }
    cx.unknown(t, "database");
    DatabaseConfig {
        url,
        max_connections,
        auto_migrate,
        allow_contract,
        schema,
    }
}

/// Longest `database.schema` (P2): `ferrite_events_` + 48 = 63 bytes, Postgres'
/// identifier limit for the notification channel.
const MAX_SCHEMA_LEN: usize = 48;

fn parse_media(cx: &mut Cx, root: &mut Table) -> (MediaSection, UploadConfig) {
    let mut t = section(cx, root, "media");
    let backend = cx
        .string(&mut t, "media", "backend")
        .unwrap_or_else(|| "local".to_owned());
    let local_shared = cx.boolean(&mut t, "media", "local_shared");
    let media_root = cx.string(&mut t, "media", "root");
    let s3 = cx.section(&mut t, "media", "s3");
    let uploads = parse_uploads(cx, &mut t);
    cx.unknown(t, "media");
    (
        media_backend(cx, backend, local_shared, media_root, s3),
        uploads,
    )
}

/// `media.max_upload` (1 B..=1 GiB), `media.image_workers` (1..=16) and
/// `media.upload_idle_timeout` (a positive duration), `media.strip_metadata` (a
/// boolean, default false) and `media.gc_grace_days` (1..=365, default 7), for either
/// backend.
fn parse_uploads(cx: &mut Cx, t: &mut Table) -> UploadConfig {
    let defaults = UploadConfig::default();
    let max_upload = match cx.size(t, "media", "max_upload", false) {
        None => defaults.max_upload,
        Some(n) if n > MAX_UPLOAD_CEILING => {
            cx.push("media.max_upload", "must be at most 1GiB");
            defaults.max_upload
        }
        Some(n) => n,
    };
    const MAX_WORKERS: u64 = MAX_IMAGE_WORKERS as u64;
    let image_workers = match cx.uint(t, "media", "image_workers") {
        None => defaults.image_workers,
        Some(n @ 1..=MAX_WORKERS) => n as usize,
        Some(_) => {
            cx.push("media.image_workers", "must be between 1 and 16");
            defaults.image_workers
        }
    };
    let idle_timeout_secs = cx
        .duration_secs(t, "media", "upload_idle_timeout", false)
        .unwrap_or(defaults.idle_timeout_secs);
    let strip_metadata = cx
        .boolean(t, "media", "strip_metadata")
        .unwrap_or(defaults.strip_metadata);
    const MAX_GRACE: u64 = MAX_GC_GRACE_DAYS as u64;
    let gc_grace_days = match cx.uint(t, "media", "gc_grace_days") {
        None => defaults.gc_grace_days,
        Some(n @ 1..=MAX_GRACE) => n as u32,
        Some(_) => {
            cx.push("media.gc_grace_days", "must be between 1 and 365");
            defaults.gc_grace_days
        }
    };
    UploadConfig {
        max_upload,
        image_workers,
        idle_timeout_secs,
        strip_metadata,
        gc_grace_days,
    }
}

fn media_backend(
    cx: &mut Cx,
    backend: String,
    local_shared: Option<bool>,
    media_root: Option<String>,
    s3: Option<Table>,
) -> MediaSection {
    let data_dir = cx.data_dir.clone();
    let default_local = |root: Option<String>, shared| MediaSection::Local {
        root: resolve(&data_dir, root.as_deref().unwrap_or("media")),
        local_shared: shared,
    };
    match backend.as_str() {
        "local" => {
            if s3.is_some() {
                cx.push("media.s3", "only valid with backend = \"s3\"");
            }
            default_local(media_root, local_shared.unwrap_or(false))
        }
        "s3" => {
            if media_root.is_some() {
                cx.push("media.root", "only valid with backend = \"local\"");
            }
            if local_shared.is_some() {
                cx.push("media.local_shared", "only valid with backend = \"local\"");
            }
            parse_s3(cx, s3.unwrap_or_default())
        }
        _ => {
            cx.push("media.backend", "must be \"local\" or \"s3\"");
            default_local(None, false)
        }
    }
}

fn parse_s3(cx: &mut Cx, mut t: Table) -> MediaSection {
    let b = "media.s3";
    let present = (t.contains_key("bucket"), t.contains_key("region"));
    let bucket = cx.string(&mut t, b, "bucket");
    let region = cx.string(&mut t, b, "region");
    for (key, v, present) in [
        ("bucket", &bucket, present.0),
        ("region", &region, present.1),
    ] {
        match v {
            None => {
                if !present {
                    cx.push(&join(b, key), "required when backend = \"s3\"");
                }
            }
            Some(s) if s.trim().is_empty() => cx.push(&join(b, key), "must not be empty"),
            Some(_) => {}
        }
    }
    let endpoint = cx.string(&mut t, b, "endpoint");
    if let Some(e) = &endpoint
        && !matches!(url::Url::parse(e), Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host())
    {
        cx.push(
            "media.s3.endpoint",
            "must be an absolute http:// or https:// URL",
        );
    }
    let access_key_id = cx.secret_string(&mut t, b, "access_key_id");
    let secret_access_key = cx.secret(&mut t, b, "secret_access_key");
    if access_key_id.is_some() != secret_access_key.is_some() {
        cx.push(
            b,
            "access_key_id and secret_access_key must be set together (or both omitted to use the environment's credentials)",
        );
    }
    let allow_http = cx.boolean(&mut t, b, "allow_http").unwrap_or(false);
    let virtual_hosted_style = cx
        .boolean(&mut t, b, "virtual_hosted_style")
        .unwrap_or(false);
    let copy_if_not_exists = cx.string(&mut t, b, "copy_if_not_exists");
    if let Some(mode) = &copy_if_not_exists
        && let Err(message) = ferrite_media::validate_copy_if_not_exists(mode)
    {
        cx.push("media.s3.copy_if_not_exists", message);
    }
    let conditional_put = match cx.string(&mut t, b, "conditional_put").as_deref() {
        None => None,
        Some("etag") => Some(ConditionalPut::Etag),
        Some("disabled") => Some(ConditionalPut::Disabled),
        Some(_) => {
            cx.push(
                "media.s3.conditional_put",
                "must be \"etag\" or \"disabled\"",
            );
            None
        }
    };
    let prefix = cx.string(&mut t, b, "prefix").unwrap_or_default();
    let public_base_url =
        cx.string(&mut t, b, "public_base_url")
            .and_then(|s| match url::Url::parse(&s) {
                Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host() => Some(u),
                _ => {
                    cx.push(
                        "media.s3.public_base_url",
                        "must be an absolute http:// or https:// URL",
                    );
                    None
                }
            });
    cx.unknown(t, b);
    MediaSection::S3 {
        bucket: bucket.unwrap_or_default(),
        region: region.unwrap_or_default(),
        endpoint,
        access_key_id,
        secret_access_key,
        allow_http,
        virtual_hosted_style,
        copy_if_not_exists,
        conditional_put,
        prefix,
        public_base_url,
    }
}

fn parse_cluster(cx: &mut Cx, root: &mut Table) -> ClusterConfig {
    let mut t = section(cx, root, "cluster");
    let mode = match cx.string(&mut t, "cluster", "mode").as_deref() {
        None | Some("auto") => ClusterMode::Auto,
        Some("solo") => ClusterMode::Solo,
        Some("cluster") => ClusterMode::Cluster,
        Some(_) => {
            cx.push("cluster.mode", "must be \"solo\", \"cluster\" or \"auto\"");
            ClusterMode::Auto
        }
    };
    // Trimmed; 1..=64 bytes (P14). Omit the key for the $HOSTNAME-<pid> default.
    let node_name = cx.string(&mut t, "cluster", "node_name").and_then(|s| {
        let name = s.trim();
        if name.is_empty() {
            cx.push(
                "cluster.node_name",
                "must not be empty (omit it to use $HOSTNAME-<pid>)",
            );
            None
        } else if name.len() > MAX_NODE_NAME {
            cx.push("cluster.node_name", "must be at most 64 bytes");
            None
        } else {
            Some(name.to_owned())
        }
    });
    let workers = match cx.uint(&mut t, "cluster", "workers") {
        None => default_workers(),
        Some(n @ 1..=MAX_WORKERS) => n as usize,
        Some(_) => {
            cx.push("cluster.workers", "must be between 1 and 256");
            default_workers()
        }
    };
    cx.unknown(t, "cluster");
    ClusterConfig {
        mode,
        node_name,
        workers,
    }
}

/// Longest `cluster.node_name`, in bytes (`ferrite_cluster::MAX_NODE_NAME`).
const MAX_NODE_NAME: usize = 64;

/// Upper bound for an explicit `cluster.workers` (P16).
const MAX_WORKERS: u64 = 256;

/// `max(2, available_parallelism)` (04 §6 "default = CPU count, min 2", P16).
fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .max(2)
}

fn parse_cache(cx: &mut Cx, root: &mut Table) -> CacheConfig {
    let mut t = section(cx, root, "cache");
    let max_bytes = cx
        .size(&mut t, "cache", "max_bytes", true)
        .unwrap_or(256 << 20);
    let logged_in = match cx.string(&mut t, "cache", "logged_in").as_deref() {
        None | Some("bypass") => LoggedInCache::Bypass,
        Some("per-user") => LoggedInCache::PerUser,
        Some(_) => {
            cx.push("cache.logged_in", "must be \"bypass\" or \"per-user\"");
            LoggedInCache::Bypass
        }
    };
    let poll_interval_ms = cx
        .positive(&mut t, "cache", "poll_interval_ms")
        .unwrap_or(250);
    // 0: search pages are not cached (F34).
    let search_ttl_secs = match cx.duration_secs(&mut t, "cache", "search_ttl", true) {
        None => DEFAULT_SHORT_TTL_SECS,
        Some(n) if n > MAX_SHORT_TTL_SECS => {
            cx.push(
                "cache.search_ttl",
                format!("must be at most {MAX_SHORT_TTL_SECS} seconds (1h)"),
            );
            DEFAULT_SHORT_TTL_SECS
        }
        Some(n) => n,
    };
    cx.unknown(t, "cache");
    CacheConfig {
        max_bytes,
        logged_in,
        poll_interval_ms,
        search_ttl_secs,
    }
}

fn parse_plugins(cx: &mut Cx, root: &mut Table) -> PluginsConfig {
    let mut t = section(cx, root, "plugins");
    let b = "plugins";
    let default_memory = cx
        .size(&mut t, b, "default_memory", false)
        .unwrap_or(32 << 20);
    let default_fuel = cx.positive(&mut t, b, "default_fuel").unwrap_or(50_000_000);
    let render_deadline_ms = cx.positive(&mut t, b, "render_deadline_ms").unwrap_or(200);
    let action_deadline_ms = cx.positive(&mut t, b, "action_deadline_ms").unwrap_or(5000);
    let disable_after_failures = cx
        .positive_u32(&mut t, b, "disable_after_failures")
        .unwrap_or(20);
    let failure_window_secs = cx
        .duration_secs(&mut t, b, "failure_window", false)
        .unwrap_or(600);
    let max_concurrent_calls = cx
        .positive(&mut t, b, "max_concurrent_calls")
        .and_then(|n| match usize::try_from(n) {
            Ok(n) => Some(n),
            Err(_) => {
                cx.push("plugins.max_concurrent_calls", "value is too large");
                None
            }
        })
        .unwrap_or_else(default_max_concurrent_calls);
    let render_budget_ms = match cx.positive(&mut t, b, "render_budget_ms") {
        None => 1000,
        Some(n) if n > MAX_RENDER_BUDGET_MS => {
            cx.push(
                "plugins.render_budget_ms",
                format!("must be at most {MAX_RENDER_BUDGET_MS}"),
            );
            1000
        }
        // Below the render deadline every render call would be cut by the budget, and
        // a cut call is never counted, so a looping plugin would never be disabled.
        Some(n) if n < render_deadline_ms => {
            cx.push(
                "plugins.render_budget_ms",
                format!("must be at least plugins.render_deadline_ms ({render_deadline_ms})"),
            );
            1000
        }
        Some(n) => n,
    };
    let volatile_ttl_secs = match cx.duration_secs(&mut t, b, "volatile_ttl", false) {
        None => DEFAULT_SHORT_TTL_SECS,
        Some(n) if n > MAX_SHORT_TTL_SECS => {
            cx.push(
                "plugins.volatile_ttl",
                format!("must be at most {MAX_SHORT_TTL_SECS} seconds (1h)"),
            );
            DEFAULT_SHORT_TTL_SECS
        }
        Some(n) => n,
    };
    cx.unknown(t, b);
    PluginsConfig {
        default_memory,
        default_fuel,
        render_deadline_ms,
        action_deadline_ms,
        disable_after_failures,
        failure_window_secs,
        max_concurrent_calls,
        render_budget_ms,
        volatile_ttl_secs,
    }
}

/// Default `cache.search_ttl` and `plugins.volatile_ttl` (the HTTP layer's `SHORT_TTL`).
const DEFAULT_SHORT_TTL_SECS: u64 = 60;
/// Largest `cache.search_ttl` and `plugins.volatile_ttl` (1 h).
const MAX_SHORT_TTL_SECS: u64 = 3600;

/// Largest `plugins.render_budget_ms` (one minute: a page is not worth waiting longer).
const MAX_RENDER_BUDGET_MS: u64 = 60_000;

/// `max(8, 4 × available_parallelism)` (P22; the plugin host's
/// `default_max_concurrent_calls`, which this crate does not depend on).
fn default_max_concurrent_calls() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .saturating_mul(4)
        .max(8)
}

/// Site key length in bytes; written as twice as many hex characters (P4).
const SITE_KEY_BYTES: usize = 32;

/// The `[auth]` section, and whether a key (inline or `_file`) was given at all.
fn parse_auth(cx: &mut Cx, root: &mut Table) -> (AuthConfig, bool) {
    let mut t = section(cx, root, "auth");
    let given = t.contains_key("secret_key") || t.contains_key("secret_key_file");
    let from_file = t.contains_key("secret_key_file") && !t.contains_key("secret_key");
    let secret_key = cx.secret(&mut t, "auth", "secret_key").and_then(|key| {
        let v = key.expose();
        if v.len() == SITE_KEY_BYTES * 2 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(key)
        } else {
            let path = if from_file {
                "auth.secret_key_file"
            } else {
                "auth.secret_key"
            };
            cx.push(path, "must be 64 hex characters (32 bytes)");
            None
        }
    });
    cx.unknown(t, "auth");
    (AuthConfig { secret_key }, given)
}

/// The `[mail]` section (P17); `None` without `smtp_url`. Messages never echo the URL.
fn parse_mail(cx: &mut Cx, root: &mut Table) -> Option<MailConfig> {
    let mut t = section(cx, root, "mail");
    let url_path = if t.contains_key("smtp_url_file") && !t.contains_key("smtp_url") {
        "mail.smtp_url_file"
    } else {
        "mail.smtp_url"
    };
    let url_given = t.contains_key("smtp_url") || t.contains_key("smtp_url_file");
    let smtp_url = cx.secret(&mut t, "mail", "smtp_url");
    let from = cx.string(&mut t, "mail", "from");
    cx.unknown(t, "mail");
    let url_ok = smtp_url
        .as_ref()
        .is_some_and(|u| match check_smtp_url(u.expose()) {
            Ok(()) => true,
            Err(message) => {
                cx.push(url_path, message);
                false
            }
        });
    match (smtp_url, from) {
        (None, None) => None,
        // Already a problem (both forms, or an unreadable or empty file).
        (None, Some(_)) if url_given => None,
        (None, Some(_)) => {
            cx.push("mail.from", "only valid together with mail.smtp_url");
            None
        }
        (Some(_), None) => {
            cx.push("mail.from", "required when mail.smtp_url is set");
            None
        }
        (Some(smtp_url), Some(from)) => {
            if !valid_mailbox(&from) {
                cx.push(
                    "mail.from",
                    "must be an address such as \"noreply@example.com\" or \
                     \"Site <noreply@example.com>\"",
                );
                return None;
            }
            url_ok.then_some(MailConfig { smtp_url, from })
        }
    }
}

const SMTP_URL_SHAPE: &str = "must be smtps://host[:port] (implicit TLS), \
     smtp://host[:port]?tls=required (STARTTLS) or plain smtp://host[:port], with any \
     credentials in the URL";

/// `smtps://` or `smtp://` with a host; `tls` only `required`, only on `smtp` (R16:
/// no opportunistic mode, which falls back to plaintext). Credentials (URL userinfo)
/// only over TLS (`smtps`, `tls=required`) or to a loopback relay (`localhost`,
/// `127.0.0.0/8`, `::1`), since lettre authenticates on whatever connection it has.
/// The messages never contain the URL.
pub fn check_smtp_url(s: &str) -> Result<(), &'static str> {
    let u = url::Url::parse(s).map_err(|_| SMTP_URL_SHAPE)?;
    if !matches!(u.scheme(), "smtp" | "smtps") || u.host_str().is_none_or(str::is_empty) {
        return Err(SMTP_URL_SHAPE);
    }
    let mut required = false;
    for (k, v) in u.query_pairs() {
        if k == "tls" {
            if u.scheme() != "smtp" || v != "required" {
                return Err("`tls` may only be `required`, and only on smtp:// \
                     (smtps:// is always TLS; plaintext fallback is not supported)");
            }
            required = true;
        }
    }
    let credentials = !u.username().is_empty() || u.password().is_some();
    let encrypted = u.scheme() == "smtps" || required;
    if credentials && !encrypted && !is_loopback(&u) {
        return Err(
            "credentials need TLS: use smtps:// or smtp://…?tls=required \
             (plain smtp:// with credentials is allowed only to a loopback relay)",
        );
    }
    Ok(())
}

/// `localhost`, `127.0.0.0/8` or `::1`. `smtp` is not a "special" URL scheme, so the
/// `url` crate keeps an IPv4 host as an opaque domain: parse it here.
fn is_loopback(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => {
            d.eq_ignore_ascii_case("localhost")
                || d.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `local@domain` or `Display Name <local@domain>`, without control characters.
/// Header-safety copy 3 of 3 (keep in step): `ferrite_core::mail::{header_safe,
/// check_address}` (1) and the plugin-host `mail` import's subject check (2); this crate
/// cannot depend on core.
fn valid_mailbox(s: &str) -> bool {
    if s.chars().any(char::is_control) {
        return false;
    }
    let addr = match s.strip_suffix('>') {
        Some(rest) => match rest.rsplit_once('<') {
            Some((_name, addr)) => addr,
            None => return false,
        },
        None => s,
    };
    let forbidden = |c: char| c.is_whitespace() || "<>()[],;:\"\\".contains(c);
    match addr.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !addr.chars().any(forbidden)
        }
        None => false,
    }
}

/// Startup validation per design 04 §3 (the checks that need only configuration).
fn validate_cluster(
    cx: &mut Cx,
    cluster: &ClusterConfig,
    database: &DatabaseConfig,
    media: &MediaSection,
) {
    if cluster.mode != ClusterMode::Cluster {
        return;
    }
    if is_sqlite(database.url.expose()) {
        cx.push(
            "cluster.mode",
            "mode = \"cluster\" requires a Postgres database, not SQLite",
        );
    }
    if matches!(
        media,
        MediaSection::Local {
            local_shared: false,
            ..
        }
    ) {
        cx.push(
            "media.backend",
            "mode = \"cluster\" requires backend = \"s3\" (or local_shared = true if the folder is a shared volume)",
        );
    }
}

/// Media backend written by `ferrite init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitMedia {
    /// Files under `<data>/media`.
    Local,
    /// S3-compatible bucket; `prefix` is empty or ends with `/`.
    S3 { bucket: String, prefix: String },
}

impl InitMedia {
    /// Parses `s3://bucket[/prefix]`; the prefix is normalised to end with `/` (or be empty).
    pub fn parse_s3_url(s: &str) -> Result<Self, String> {
        let rest = s
            .strip_prefix("s3://")
            .ok_or_else(|| "expected s3://bucket/prefix".to_owned())?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() {
            return Err("missing bucket name in s3:// URL".to_owned());
        }
        let prefix = prefix.trim_matches('/');
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        Ok(Self::S3 {
            bucket: bucket.to_owned(),
            prefix,
        })
    }
}

/// The upload keys of `[media]` in the `ferrite init` template (commented defaults).
const UPLOAD_TEMPLATE: &str = "# Largest uploaded file (1 byte to 1GiB).\n# max_upload = \"256MB\"\n# Image variant jobs at once per node (1..=16).\n# image_workers = 1\n# An upload that sends nothing for this long is aborted.\n# upload_idle_timeout = \"30s\"\n# Strip EXIF and XMP (GPS included) from uploaded JPEG, PNG and WebP originals.\n# strip_metadata = false\n# Days the artifacts GC keeps unreferenced files before deleting them (1..=365).\n# gc_grace_days = 7\n";

/// Template for `ferrite init`. For S3 the bucket and prefix are concrete and region
/// defaults to `us-east-1`; the operator must still set credentials/endpoint.
pub fn render_default_toml(
    site_url: &str,
    title: &str,
    db_url: Option<&str>,
    schema: Option<&str>,
    media: &InitMedia,
) -> String {
    let q = |s: &str| Value::String(s.to_owned()).to_string();
    let mut out = String::new();
    out.push_str("[site]\n");
    out.push_str(&format!("url = {}\n", q(site_url)));
    out.push_str(&format!("title = {}\n", q(title)));
    out.push_str("locale = \"en\"\ntimezone = \"UTC\"\n\n");
    out.push_str("[server]\nlisten = \"127.0.0.1:8080\"\ntrusted_proxies = []\n\n");
    out.push_str("[database]\n");
    out.push_str(&format!(
        "url = {}\n",
        q(db_url.unwrap_or("sqlite://ferrite.db"))
    ));
    if let Some(schema) = schema {
        out.push_str(&format!("schema = {}\n", q(schema)));
    }
    out.push_str("max_connections = 16\n# url_file = \"/run/secrets/db_url\"\n\n");
    match media {
        InitMedia::S3 { bucket, prefix } => {
            out.push_str("[media]\nbackend = \"s3\"\n");
            out.push_str(UPLOAD_TEMPLATE);
            out.push_str("\n[media.s3]\n");
            out.push_str(&format!("bucket = {}\n", q(bucket)));
            if !prefix.is_empty() {
                out.push_str(&format!("prefix = {}\n", q(prefix)));
            }
            out.push_str("region = \"us-east-1\"\n");
            out.push_str("# Set your region, endpoint and credentials:\n");
            out.push_str("# endpoint = \"https://s3.eu-central-1.amazonaws.com\"\n");
            out.push_str("# access_key_id_file = \"/run/secrets/s3_key\"\n");
            out.push_str("# secret_access_key_file = \"/run/secrets/s3_secret\"\n");
            out.push_str("# public_base_url = \"https://cdn.example.com\"\n");
            out.push_str(
                "# How uploads are placed without a race, if your provider supports it (03 §2):\n",
            );
            out.push_str("# copy_if_not_exists = \"header:x-amz-copy-source-if-none-match:*\"\n");
            out.push_str("# For a store that rejects If-None-Match on create:\n");
            out.push_str("# conditional_put = \"disabled\"\n\n");
        }
        InitMedia::Local => {
            out.push_str("[media]\nbackend = \"local\"\nroot = \"media\"\n");
            out.push_str(UPLOAD_TEMPLATE);
            out.push('\n');
        }
    }
    out.push_str("[cluster]\nmode = \"auto\"\n");
    out.push_str("# Concurrent background jobs per node (1..=256); default max(2, CPU count).\n");
    out.push_str("# workers = 4\n\n");
    out.push_str(
        "[cache]\nmax_bytes = \"256MB\"\nlogged_in = \"bypass\"\npoll_interval_ms = 250\n",
    );
    out.push_str(
        "# Cache lifetime of search result pages (0..=1h); 0 = search pages are not cached.\n",
    );
    out.push_str("search_ttl = \"60s\"\n\n");
    out.push_str("[auth]\n");
    out.push_str("# Site key (64 hex characters) that seals stored secrets. Unset, a solo site\n");
    out.push_str(
        "# keeps a generated one in secret.key in the data folder; cluster mode requires\n",
    );
    out.push_str("# it here (the same key on every node), e.g. from `openssl rand -hex 32`.\n");
    out.push_str("# secret_key_file = \"/run/secrets/ferrite_site_key\"\n\n");
    out.push_str(
        "[plugins]\ndefault_memory = \"32MB\"\ndefault_fuel = 50000000\nrender_deadline_ms = 200\naction_deadline_ms = 5000\ndisable_after_failures = 20\nfailure_window = \"10m\"\n",
    );
    out.push_str("# All plugin filters of one page together; past it the rest are skipped.\n");
    out.push_str("render_budget_ms = 1000\n");
    out.push_str(
        "# Cache lifetime of pages whose plugin calls were volatile or failed (1s..=1h).\n",
    );
    out.push_str("volatile_ttl = \"60s\"\n");
    out.push_str(
        "# Plugin calls at once per node (filters, actions, routes); default\n# max(8, 4 x CPU count). Plugin action jobs use at most cluster.workers - 1 of them.\n",
    );
    out.push_str("# max_concurrent_calls = 32\n\n");
    out.push_str("# [mail]\n");
    out.push_str("# Outgoing mail (password resets, plugin notices to admins). Unset: no mail.\n");
    out.push_str(
        "# smtps:// or smtp://…?tls=required (STARTTLS); credentials in the URL need TLS\n",
    );
    out.push_str("# (plain smtp:// with credentials only to a loopback relay).\n");
    out.push_str(
        "# smtp_url_file = \"/run/secrets/smtp_url\"  # or smtp_url = \"smtps://user:pass@host\"\n",
    );
    out.push_str("# from = \"Site <noreply@example.com>\"\n");
    out
}
