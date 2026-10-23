use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;

use ipnet::IpNet;
use toml::{Table, Value};

use crate::cx::{Cx, join, resolve};
use crate::error::{ConfigError, ConfigProblem};
use crate::model::*;
use crate::model::{is_sqlite, is_supported_db_url};

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
    let media = parse_media(&mut cx, &mut table);
    let cluster = parse_cluster(&mut cx, &mut table);
    let cache = parse_cache(&mut cx, &mut table);
    let plugins = parse_plugins(&mut cx, &mut table);
    cx.unknown(table, "");

    validate_cluster(&mut cx, &cluster, &database, &media);

    match site {
        Some(site) if cx.problems.is_empty() => Ok(Config {
            data_dir: data_dir.to_path_buf(),
            site,
            server,
            database,
            media,
            cluster,
            cache,
            plugins,
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
            "at most 48 characters, so the notification channel name fits Postgres' 63-byte \
             limit",
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

fn parse_media(cx: &mut Cx, root: &mut Table) -> MediaSection {
    let mut t = section(cx, root, "media");
    let backend = cx
        .string(&mut t, "media", "backend")
        .unwrap_or_else(|| "local".to_owned());
    let local_shared = cx.boolean(&mut t, "media", "local_shared");
    let media_root = cx.string(&mut t, "media", "root");
    let s3 = cx.section(&mut t, "media", "s3");
    cx.unknown(t, "media");

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
    let node_name = cx
        .string(&mut t, "cluster", "node_name")
        .filter(|s| !s.is_empty());
    cx.unknown(t, "cluster");
    ClusterConfig { mode, node_name }
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
    cx.unknown(t, "cache");
    CacheConfig {
        max_bytes,
        logged_in,
        poll_interval_ms,
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
    let failure_window_secs = cx.duration_secs(&mut t, b, "failure_window").unwrap_or(600);
    cx.unknown(t, b);
    PluginsConfig {
        default_memory,
        default_fuel,
        render_deadline_ms,
        action_deadline_ms,
        disable_after_failures,
        failure_window_secs,
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
            out.push_str("[media]\nbackend = \"s3\"\n\n[media.s3]\n");
            out.push_str(&format!("bucket = {}\n", q(bucket)));
            if !prefix.is_empty() {
                out.push_str(&format!("prefix = {}\n", q(prefix)));
            }
            out.push_str("region = \"us-east-1\"\n");
            out.push_str("# Set your region, endpoint and credentials:\n");
            out.push_str("# endpoint = \"https://s3.eu-central-1.amazonaws.com\"\n");
            out.push_str("# access_key_id_file = \"/run/secrets/s3_key\"\n");
            out.push_str("# secret_access_key_file = \"/run/secrets/s3_secret\"\n");
            out.push_str("# public_base_url = \"https://cdn.example.com\"\n\n");
        }
        InitMedia::Local => {
            out.push_str("[media]\nbackend = \"local\"\nroot = \"media\"\n\n");
        }
    }
    out.push_str("[cluster]\nmode = \"auto\"\n\n");
    out.push_str(
        "[cache]\nmax_bytes = \"256MB\"\nlogged_in = \"bypass\"\npoll_interval_ms = 250\n\n",
    );
    out.push_str(
        "[plugins]\ndefault_memory = \"32MB\"\ndefault_fuel = 50000000\nrender_deadline_ms = 200\naction_deadline_ms = 5000\ndisable_after_failures = 20\nfailure_window = \"10m\"\n",
    );
    out
}
