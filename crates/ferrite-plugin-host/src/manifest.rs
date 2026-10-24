//! `ferrite-plugin.toml` parsing and validation (design 05 §3).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use serde_json::{Map, Value, json};

use crate::error::{ManifestError, ManifestProblem};

/// Largest `ferrite-plugin.toml` accepted (plan "Exact values": manifest part <= 64 KiB).
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Plugin names that are refused because a static admin API path takes them
/// (`/api/v1/plugins/<name>` would hit that route instead; ruling R9). A new static
/// segment under `/plugins/` goes here.
pub const RESERVED_PLUGIN_NAMES: &[&str] = &["inspect"];

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub plugin: PluginMeta,
    pub capabilities: Capabilities,
    /// Hook name to priority (lower runs first).
    pub hooks: BTreeMap<String, i32>,
    pub routes: BTreeMap<String, RouteSpec>,
    pub settings: BTreeMap<String, SettingSpec>,
    pub limits: LimitRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMeta {
    pub name: String,
    pub version: semver::Version,
    pub publisher: String,
    pub description: Option<String>,
    pub world: String,
    pub license: Option<String>,
    pub homepage: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub content_read: Vec<String>,
    pub content_meta_read: Vec<String>,
    pub content_meta_write: Vec<String>,
    pub options_read: Vec<String>,
    pub kv: bool,
    pub http: Vec<String>,
    pub render_head: bool,
    pub render_text: bool,
    pub render_html: HtmlMode,
    pub routes: bool,
    pub mail: MailScope,
}

impl Capabilities {
    /// Whether every capability requested here is also in `granted` (ruling R21: an
    /// admin-approved grant set). Key and type patterns are covered by `*`, by the same
    /// pattern, or by a `<prefix>.*` glob that matches them; HTTP hosts must be listed
    /// exactly; `render.html` levels order `off < sanitized < raw`.
    pub fn is_subset_of(&self, granted: &Capabilities) -> bool {
        let lists = [
            (&self.content_read, &granted.content_read),
            (&self.content_meta_read, &granted.content_meta_read),
            (&self.content_meta_write, &granted.content_meta_write),
            (&self.options_read, &granted.options_read),
        ];
        let flags = [
            (self.kv, granted.kv),
            (self.render_head, granted.render_head),
            (self.render_text, granted.render_text),
            (self.routes, granted.routes),
            (
                self.mail == MailScope::Admins,
                granted.mail == MailScope::Admins,
            ),
        ];
        lists
            .iter()
            .all(|(want, have)| want.iter().all(|p| pattern_covered(p, have)))
            && self.http.iter().all(|h| granted.http.contains(h))
            && flags.iter().all(|(want, have)| !want || *have)
            && html_level(self.render_html) <= html_level(granted.render_html)
    }
}

pub(crate) fn html_level(m: HtmlMode) -> u8 {
    match m {
        HtmlMode::Off => 0,
        HtmlMode::Sanitized => 1,
        HtmlMode::Raw => 2,
    }
}

/// Whether pattern `p` (`*`, `<prefix>.*` or an exact key) is within `granted`.
pub(crate) fn pattern_covered(p: &str, granted: &[String]) -> bool {
    granted.iter().any(|g| {
        g == "*"
            || g == p
            || (p != "*"
                && g.strip_suffix('*')
                    .is_some_and(|prefix| prefix.ends_with('.') && p.starts_with(prefix)))
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HtmlMode {
    #[default]
    Off,
    Sanitized,
    Raw,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MailScope {
    #[default]
    None,
    Admins,
}

/// Resource limits a plugin asks for. How they combine with host defaults is decided
/// at install/call time, not here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitRequest {
    /// Bytes. Parsed from strings such as `"16MB"`; `KB`/`KiB` are 1024 bytes and
    /// `MB`/`MiB` are 1024 * 1024 bytes.
    pub memory: Option<u64>,
    pub fuel: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    /// Uppercase HTTP methods; defaults to `["GET"]` when omitted.
    pub methods: Vec<String>,
    /// `"N/s"`, `"N/min"` or `"N/h"`.
    pub rate_limit: Option<String>,
    pub cross_origin: bool,
}

impl RouteSpec {
    /// The route's limit as (requests, window): its `rate_limit`, else
    /// [`DEFAULT_ROUTE_RATE_LIMIT`](crate::DEFAULT_ROUTE_RATE_LIMIT) (`N/s`, `N/min`,
    /// `N/h`; a manifest never holds another form).
    pub fn rate(&self) -> (u32, std::time::Duration) {
        self.rate_limit
            .as_deref()
            .and_then(parse_rate)
            .or_else(|| parse_rate(crate::DEFAULT_ROUTE_RATE_LIMIT))
            .unwrap_or((10, std::time::Duration::from_secs(60)))
    }
}

/// `(N, window)` of a valid `N/s|N/min|N/h` rate limit.
fn parse_rate(s: &str) -> Option<(u32, std::time::Duration)> {
    if !valid_rate_limit(s) {
        return None;
    }
    let (n, unit) = s.split_once('/')?;
    let secs = match unit {
        "s" => 1,
        "min" => 60,
        _ => 3600,
    };
    Some((n.parse().ok()?, std::time::Duration::from_secs(secs)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Text,
    Textarea,
    Bool,
    Number,
    Select,
    Color,
    Media,
    Secret,
}

impl SettingKind {
    /// The shared setting type, whose value rules the manifest's defaults follow.
    pub fn setting_type(self) -> ferrite_domain::SettingType {
        use ferrite_domain::SettingType as T;
        match self {
            Self::Text => T::Text,
            Self::Textarea => T::Textarea,
            Self::Bool => T::Bool,
            Self::Number => T::Number,
            Self::Select => T::Select,
            Self::Color => T::Color,
            Self::Media => T::Media,
            Self::Secret => T::Secret,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    /// TOML key `type`.
    pub kind: SettingKind,
    pub label: String,
    pub default: Option<toml::Value>,
    /// Choices; required (non-empty) for `select`, empty otherwise.
    pub options: Vec<String>,
}

impl Manifest {
    /// Parse and validate the `ferrite-plugin.toml` text as uploaded (install and
    /// update). All problems are collected before returning. A text larger than
    /// [`MAX_MANIFEST_BYTES`] is refused before parsing; stored manifests read back by
    /// [`Manifest::from_json`] are not size-checked again (they passed here).
    pub fn parse(src: &str) -> Result<Manifest, ManifestError> {
        if src.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::single(
                "",
                format!(
                    "manifest is {} bytes, more than the {} KiB allowed",
                    src.len(),
                    MAX_MANIFEST_BYTES / 1024
                ),
            ));
        }
        match src.parse() {
            Ok(table) => Manifest::from_table(table),
            Err(e) => Err(ManifestError::single("", format!("invalid TOML: {e}"))),
        }
    }

    /// Validate a parsed table (see [`Manifest::parse`]).
    fn from_table(mut table: toml::Table) -> Result<Manifest, ManifestError> {
        let mut cx = Cx::default();

        let plugin = match cx.section(&mut table, "plugin") {
            Some(mut t) => parse_plugin(&mut cx, &mut t),
            None => {
                cx.push("plugin", "missing required section [plugin]");
                None
            }
        };
        let capabilities = cx
            .section(&mut table, "capabilities")
            .map(|mut t| parse_capabilities(&mut cx, &mut t))
            .unwrap_or_default();
        let hooks_table = cx.section(&mut table, "hooks");
        let hooks = hooks_table
            .map(|t| parse_hooks(&mut cx, t, &capabilities))
            .unwrap_or_default();
        let routes = match cx.section(&mut table, "routes") {
            Some(t) => {
                if !capabilities.routes {
                    cx.push(
                        "routes",
                        "[routes] requires `routes = true` in [capabilities]",
                    );
                }
                parse_routes(&mut cx, t)
            }
            None => BTreeMap::new(),
        };
        let settings = cx
            .section(&mut table, "settings")
            .map(|t| parse_settings(&mut cx, t))
            .unwrap_or_default();
        let limits = cx
            .section(&mut table, "limits")
            .map(|mut t| parse_limits(&mut cx, &mut t))
            .unwrap_or_default();
        cx.unknown(table, "");

        match plugin {
            Some(plugin) if cx.problems.is_empty() => Ok(Manifest {
                plugin,
                capabilities,
                hooks,
                routes,
                settings,
                limits,
            }),
            _ => Err(ManifestError {
                problems: cx.problems,
            }),
        }
    }

    /// A manifest stored as JSON (the `plugins.manifest` column: the `ferrite-plugin.toml`
    /// table in JSON form, dotted keys as nested objects), turned into a TOML table and
    /// validated by the same rules as [`Manifest::parse`], except the size cap: a stored
    /// row passed it at install, and is not refused later because its JSON form is
    /// larger (review round 1, item 3).
    pub fn from_json(json: &Value) -> Result<Manifest, ManifestError> {
        let table: toml::Table = serde_json::from_value(json.clone())
            .map_err(|e| ManifestError::single("", format!("not a TOML table: {e}")))?;
        Manifest::from_table(table)
    }
}

/// The JSON form of a `[capabilities]` table holding `c`, with only its non-default
/// members (read back by [`Manifest::from_json`]).
pub(crate) fn capabilities_json(c: &Capabilities) -> Value {
    fn put(map: &mut Map<String, Value>, key: &str, value: Value) {
        let empty = match &value {
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Bool(b) => !b,
            _ => false,
        };
        if !empty {
            map.insert(key.to_owned(), value);
        }
    }
    let mut meta = Map::new();
    put(&mut meta, "read", json!(c.content_meta_read));
    put(&mut meta, "write", json!(c.content_meta_write));
    let mut content = Map::new();
    put(&mut content, "read", json!(c.content_read));
    put(&mut content, "meta", Value::Object(meta));
    let mut options = Map::new();
    put(&mut options, "read", json!(c.options_read));
    let mut render = Map::new();
    put(&mut render, "head", json!(c.render_head));
    put(&mut render, "text", json!(c.render_text));
    match c.render_html {
        HtmlMode::Off => {}
        HtmlMode::Sanitized => put(&mut render, "html", json!("sanitized")),
        HtmlMode::Raw => put(&mut render, "html", json!("raw")),
    }
    let mut out = Map::new();
    put(&mut out, "content", Value::Object(content));
    put(&mut out, "options", Value::Object(options));
    put(&mut out, "kv", json!(c.kv));
    put(&mut out, "http", json!(c.http));
    put(&mut out, "render", Value::Object(render));
    put(&mut out, "routes", json!(c.routes));
    if c.mail == MailScope::Admins {
        put(&mut out, "mail", json!("admins"));
    }
    Value::Object(out)
}

const WORLD: &str = "ferrite:plugin/plugin@0.1.0";
const HTTP_METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

#[derive(Default)]
struct Cx {
    problems: Vec<ManifestProblem>,
}

fn join(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_owned()
    } else {
        format!("{base}.{key}")
    }
}

impl Cx {
    fn push(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.problems.push(ManifestProblem {
            path: path.into(),
            message: message.into(),
        });
    }

    /// Report every key left in `table` as unknown.
    fn unknown(&mut self, table: toml::Table, base: &str) {
        for key in table.keys() {
            self.push(join(base, key), "unknown key");
        }
    }

    /// Remove a sub-table; a non-table value is reported.
    fn section(&mut self, table: &mut toml::Table, key: &str) -> Option<toml::Table> {
        self.section_at(table, key, key)
    }

    fn section_at(
        &mut self,
        table: &mut toml::Table,
        key: &str,
        path: &str,
    ) -> Option<toml::Table> {
        match table.remove(key)? {
            toml::Value::Table(t) => Some(t),
            other => {
                self.push(
                    path,
                    format!("expected a table, found {}", other.type_str()),
                );
                None
            }
        }
    }

    fn string(&mut self, table: &mut toml::Table, key: &str, path: &str) -> Option<String> {
        match table.remove(key)? {
            toml::Value::String(s) => Some(s),
            other => {
                self.push(
                    path,
                    format!("expected a string, found {}", other.type_str()),
                );
                None
            }
        }
    }

    fn boolean(&mut self, table: &mut toml::Table, key: &str, path: &str) -> Option<bool> {
        match table.remove(key)? {
            toml::Value::Boolean(b) => Some(b),
            other => {
                self.push(
                    path,
                    format!("expected a boolean, found {}", other.type_str()),
                );
                None
            }
        }
    }

    /// [`Cx::strings`] whose items must be key patterns (see [`valid_pattern`]).
    fn patterns(&mut self, table: &mut toml::Table, key: &str, path: &str) -> Option<Vec<String>> {
        let items = self.strings(table, key, path)?;
        let bad: Vec<_> = items.iter().filter(|p| !valid_pattern(p)).collect();
        if !bad.is_empty() {
            self.push(
                path,
                format!(
                    "patterns must be an exact key, \"*\" or \"<prefix>.*\"; invalid: {}",
                    bad.iter()
                        .map(|p| format!("`{p}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        Some(items)
    }

    fn strings(&mut self, table: &mut toml::Table, key: &str, path: &str) -> Option<Vec<String>> {
        match table.remove(key)? {
            toml::Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                let mut ok = true;
                for item in items {
                    match item {
                        toml::Value::String(s) if !s.is_empty() => out.push(s),
                        _ => ok = false,
                    }
                }
                if !ok {
                    self.push(path, "expected an array of non-empty strings");
                }
                ok.then_some(out)
            }
            other => {
                self.push(
                    path,
                    format!("expected an array, found {}", other.type_str()),
                );
                None
            }
        }
    }
}

fn parse_plugin(cx: &mut Cx, t: &mut toml::Table) -> Option<PluginMeta> {
    let name = cx.string(t, "name", "plugin.name");
    let version = cx.string(t, "version", "plugin.version");
    let publisher = cx.string(t, "publisher", "plugin.publisher");
    let world = cx.string(t, "world", "plugin.world");
    let description = cx.string(t, "description", "plugin.description");
    let license = cx.string(t, "license", "plugin.license");
    let homepage = cx.string(t, "homepage", "plugin.homepage");
    cx.unknown(std::mem::take(t), "plugin");

    // A wrongly typed field was already reported; only absent fields are reported here.
    let require = |cx: &mut Cx, v: &Option<String>, key: &str| {
        if v.is_none()
            && !cx
                .problems
                .iter()
                .any(|p| p.path == format!("plugin.{key}"))
        {
            cx.push(format!("plugin.{key}"), "missing required key");
        }
    };
    require(cx, &name, "name");
    require(cx, &version, "version");
    require(cx, &publisher, "publisher");
    require(cx, &world, "world");

    if let Some(n) = &name {
        if !ferrite_domain::valid_plugin_name(n) {
            cx.push("plugin.name", "must match [a-z0-9-]{2,48}");
        } else if RESERVED_PLUGIN_NAMES.contains(&n.as_str()) {
            cx.push("plugin.name", format!("`{n}` is reserved"));
        }
    }
    let version = version.and_then(|v| match semver::Version::parse(&v) {
        Ok(v) => Some(v),
        Err(e) => {
            cx.push("plugin.version", format!("invalid semver: {e}"));
            None
        }
    });
    if let Some(w) = &world
        && w != WORLD
    {
        cx.push(
            "plugin.world",
            format!("unsupported world `{w}`, expected `{WORLD}`"),
        );
    }
    if matches!(&publisher, Some(p) if p.trim().is_empty()) {
        cx.push("plugin.publisher", "must not be empty");
    }

    Some(PluginMeta {
        name: name?,
        version: version?,
        publisher: publisher?,
        description,
        world: world?,
        license,
        homepage,
    })
}

/// A capability key pattern: an exact key (no `*`), `"*"`, or `"<prefix>.*"` with a
/// non-empty, `*`-free prefix.
pub(crate) fn valid_pattern(p: &str) -> bool {
    if p == "*" {
        return true;
    }
    let prefix = p.strip_suffix(".*").unwrap_or(p);
    !prefix.is_empty() && !prefix.contains('*')
}

fn valid_hostname(h: &str) -> bool {
    h.len() <= 253
        && h.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// A hostname an allowlist must not name (P7, pre-flight F5, review round 1): it
/// parses as an IPv4 address, has no dot (single-label names resolve through search
/// domains), its last label is all digits (inet_aton forms such as `127.1` or
/// `0177.0.0.1`), a label starts with `0x` (hex forms), a label is `localhost`, or the
/// first label starts with `localhost` (`localhost4.localdomain4`,
/// `localhost6.localdomain6`). Case-insensitive. IPv6 literals never get here: `:`,
/// `[` and `%` already fail [`valid_hostname`].
fn bare_host(h: &str) -> bool {
    let lower = h.to_ascii_lowercase();
    let labels: Vec<&str> = lower.split('.').collect();
    lower.parse::<Ipv4Addr>().is_ok()
        || lower.starts_with("localhost")
        || labels.len() < 2
        || labels
            .last()
            .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
        || labels
            .iter()
            .any(|l| l.starts_with("0x") || *l == "localhost")
}

fn listed(items: &[&String]) -> String {
    items
        .iter()
        .map(|h| format!("`{h}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_capabilities(cx: &mut Cx, t: &mut toml::Table) -> Capabilities {
    let mut c = Capabilities::default();
    const P: &str = "capabilities";

    if let Some(mut content) = cx.section_at(t, "content", "capabilities.content") {
        c.content_read = cx
            .patterns(&mut content, "read", "capabilities.content.read")
            .unwrap_or_default();
        if let Some(mut meta) = cx.section_at(&mut content, "meta", "capabilities.content.meta") {
            c.content_meta_read = cx
                .patterns(&mut meta, "read", "capabilities.content.meta.read")
                .unwrap_or_default();
            c.content_meta_write = cx
                .patterns(&mut meta, "write", "capabilities.content.meta.write")
                .unwrap_or_default();
            cx.unknown(meta, "capabilities.content.meta");
        }
        cx.unknown(content, "capabilities.content");
    }
    if let Some(mut options) = cx.section_at(t, "options", "capabilities.options") {
        c.options_read = cx
            .patterns(&mut options, "read", "capabilities.options.read")
            .unwrap_or_default();
        cx.unknown(options, "capabilities.options");
    }
    c.kv = cx.boolean(t, "kv", "capabilities.kv").unwrap_or(false);
    if let Some(given) = cx.strings(t, "http", "capabilities.http") {
        // Canonical form (review round 1): lowercase, one trailing dot dropped, so
        // grants and fingerprints do not depend on spelling; duplicates removed.
        let mut hosts: Vec<String> = Vec::with_capacity(given.len());
        for h in given {
            let h = h.to_ascii_lowercase();
            let h = h.strip_suffix('.').map(str::to_owned).unwrap_or(h);
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
        let (names, bad): (Vec<_>, Vec<_>) = hosts.iter().partition(|h| valid_hostname(h));
        if !bad.is_empty() {
            cx.push(
                "capabilities.http",
                format!(
                    "must be plain hostnames (no scheme, port, path or wildcard); invalid: {}",
                    listed(&bad)
                ),
            );
        }
        let bare: Vec<_> = names.into_iter().filter(|h| bare_host(h)).collect();
        if !bare.is_empty() {
            cx.push(
                "capabilities.http",
                format!(
                    "bare IP addresses and localhost are not allowed (a hostname needs a dot \
                     and a last label that is not a number); invalid: {}",
                    listed(&bare)
                ),
            );
        }
        c.http = hosts;
    }
    if let Some(mut render) = cx.section_at(t, "render", "capabilities.render") {
        c.render_head = cx
            .boolean(&mut render, "head", "capabilities.render.head")
            .unwrap_or(false);
        c.render_text = cx
            .boolean(&mut render, "text", "capabilities.render.text")
            .unwrap_or(false);
        if let Some(v) = render.remove("html") {
            c.render_html = match v.as_str() {
                Some("off") => HtmlMode::Off,
                Some("sanitized") => HtmlMode::Sanitized,
                Some("raw") => HtmlMode::Raw,
                _ => {
                    cx.push(
                        "capabilities.render.html",
                        "must be the string \"off\", \"sanitized\" or \"raw\"",
                    );
                    HtmlMode::Off
                }
            };
        }
        cx.unknown(render, "capabilities.render");
    }
    c.routes = cx
        .boolean(t, "routes", "capabilities.routes")
        .unwrap_or(false);
    if let Some(m) = cx.string(t, "mail", "capabilities.mail") {
        if m == "admins" {
            c.mail = MailScope::Admins;
        } else {
            cx.push("capabilities.mail", "must be \"admins\" or absent");
        }
    }
    cx.unknown(std::mem::take(t), P);
    c
}

fn parse_hooks(cx: &mut Cx, t: toml::Table, caps: &Capabilities) -> BTreeMap<String, i32> {
    let mut out = BTreeMap::new();
    for (name, value) in t {
        let path = join("hooks", &name);
        let Some(spec) = crate::capability::hook_catalogue()
            .iter()
            .find(|h| h.name == name)
        else {
            cx.push(
                &path,
                "unknown hook (not in the ferrite:plugin@0.1.0 catalogue)",
            );
            continue;
        };
        let priority = match value {
            toml::Value::Integer(i) => i32::try_from(i).ok(),
            _ => None,
        };
        let Some(priority) = priority else {
            cx.push(&path, "priority must be an integer that fits in 32 bits");
            continue;
        };
        if !(spec.requires)(caps) {
            cx.push(
                &path,
                "hook requires a capability that is not declared in [capabilities]",
            );
            continue;
        }
        out.insert(name, priority);
    }
    out
}

pub(crate) fn valid_rate_limit(s: &str) -> bool {
    let Some((n, unit)) = s.split_once('/') else {
        return false;
    };
    matches!(unit, "s" | "min" | "h")
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
        && n.parse::<u32>().is_ok_and(|n| n > 0)
}

fn parse_routes(cx: &mut Cx, t: toml::Table) -> BTreeMap<String, RouteSpec> {
    let mut out = BTreeMap::new();
    for (key, value) in t {
        let path = join("routes", &key);
        let before = cx.problems.len();
        if !key.starts_with('/') {
            cx.push(&path, "route path must start with \"/\"");
        }
        let toml::Value::Table(mut rt) = value else {
            cx.push(&path, "expected a table");
            continue;
        };
        let mpath = format!("{path}.methods");
        let methods = match cx.strings(&mut rt, "methods", &mpath) {
            Some(m) if m.is_empty() => {
                cx.push(&mpath, "must not be empty");
                Vec::new()
            }
            Some(m) => {
                if m.iter().any(|x| !HTTP_METHODS.contains(&x.as_str())) {
                    cx.push(
                        &mpath,
                        format!(
                            "must be uppercase HTTP methods ({})",
                            HTTP_METHODS.join(", ")
                        ),
                    );
                }
                m
            }
            None => vec!["GET".to_owned()],
        };
        let rpath = format!("{path}.rate_limit");
        let rate_limit = cx.string(&mut rt, "rate_limit", &rpath);
        if let Some(r) = &rate_limit
            && !valid_rate_limit(r)
        {
            cx.push(
                &rpath,
                "must look like \"N/s\", \"N/min\" or \"N/h\" with N > 0",
            );
        }
        let cross_origin = cx
            .boolean(&mut rt, "cross_origin", &format!("{path}.cross_origin"))
            .unwrap_or(false);
        cx.unknown(rt, &path);
        if cx.problems.len() == before {
            out.insert(
                key,
                RouteSpec {
                    methods,
                    rate_limit,
                    cross_origin,
                },
            );
        }
    }
    out
}

fn parse_settings(cx: &mut Cx, t: toml::Table) -> BTreeMap<String, SettingSpec> {
    let mut out = BTreeMap::new();
    for (key, value) in t {
        let path = join("settings", &key);
        let before = cx.problems.len();
        // Pre-flight F23: the key becomes an option name, a seal purpose and an API
        // field, so it follows the theme rule.
        if !valid_setting_key(&key) {
            cx.push(
                &path,
                format!("setting keys must be 1-{MAX_SETTING_KEY} of [a-z0-9_-]"),
            );
            continue;
        }
        let toml::Value::Table(mut st) = value else {
            cx.push(
                &path,
                "expected a table like { type = \"text\", label = \"...\" }",
            );
            continue;
        };
        let tpath = format!("{path}.type");
        let kind = cx.string(&mut st, "type", &tpath).and_then(|s| {
            let k = match s.as_str() {
                "text" => SettingKind::Text,
                "textarea" => SettingKind::Textarea,
                "bool" => SettingKind::Bool,
                "number" => SettingKind::Number,
                "select" => SettingKind::Select,
                "color" => SettingKind::Color,
                "media" => SettingKind::Media,
                "secret" => SettingKind::Secret,
                _ => {
                    cx.push(&tpath, format!("unknown setting type `{s}`"));
                    return None;
                }
            };
            Some(k)
        });
        if kind.is_none() && !st.contains_key("type") && cx.problems.len() == before {
            cx.push(&tpath, "missing required key");
        }
        let lpath = format!("{path}.label");
        let label = cx.string(&mut st, "label", &lpath);
        if label.is_none() && !cx.problems.iter().any(|p| p.path == lpath) {
            cx.push(&lpath, "missing required key");
        }
        let default = st.remove("default");
        let opath = format!("{path}.options");
        let options = cx.strings(&mut st, "options", &opath).unwrap_or_default();
        if kind == Some(SettingKind::Select) && options.is_empty() {
            cx.push(&opath, "`select` settings need a non-empty `options` list");
        }
        if let (Some(kind), Some(d)) = (kind, &default) {
            // Review m3: a default must pass the value rules a PATCH of it would
            // (`ferrite_domain::check_setting_value`: type, options, colour, lengths).
            let problem = match kind {
                SettingKind::Media | SettingKind::Secret => Some(format!(
                    "{} settings have no default",
                    kind.setting_type().as_str()
                )),
                _ => match serde_json::to_value(d) {
                    Ok(json) => {
                        ferrite_domain::check_setting_value(kind.setting_type(), &options, &json)
                            .err()
                            .map(|e| format!("default {e}"))
                    }
                    Err(e) => Some(format!("default cannot be read: {e}")),
                },
            };
            if let Some(message) = problem {
                cx.push(format!("{path}.default"), message);
            }
        }
        cx.unknown(st, &path);
        if cx.problems.len() == before
            && let (Some(kind), Some(label)) = (kind, label)
        {
            out.insert(
                key,
                SettingSpec {
                    kind,
                    label,
                    default,
                    options,
                },
            );
        }
    }
    out
}

/// Longest `[settings]` key.
const MAX_SETTING_KEY: usize = 64;

/// `[a-z0-9_-]{1,64}` (the `theme.toml` settings rule).
fn valid_setting_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_SETTING_KEY
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `"16MB"`, `"32MiB"`, `"512KB"`, `"4KiB"`; MB = MiB = 1024 * 1024, KB = KiB = 1024.
fn parse_memory(s: &str) -> Option<u64> {
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let (num, unit) = s.split_at(split);
    let mult: u64 = match unit {
        "KB" | "KiB" => 1 << 10,
        "MB" | "MiB" => 1 << 20,
        _ => return None,
    };
    let n: u64 = num.parse().ok()?;
    n.checked_mul(mult).filter(|b| *b > 0)
}

fn parse_limits(cx: &mut Cx, t: &mut toml::Table) -> LimitRequest {
    let mut l = LimitRequest::default();
    if let Some(v) = t.remove("memory") {
        match v.as_str().and_then(parse_memory) {
            Some(b) => l.memory = Some(b),
            None => cx.push(
                "limits.memory",
                "must be a string like \"16MB\", \"32MiB\" or \"512KB\" (MB = MiB, KB = KiB; positive)",
            ),
        }
    }
    if let Some(v) = t.remove("fuel") {
        match v
            .as_integer()
            .and_then(|i| u64::try_from(i).ok())
            .filter(|f| *f > 0)
        {
            Some(f) => l.fuel = Some(f),
            None => cx.push("limits.fuel", "must be a positive integer"),
        }
    }
    cx.unknown(std::mem::take(t), "limits");
    l
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const DESIGN_EXAMPLE: &str = r#"
[plugin]
name        = "seo-pro"              # [a-z0-9-]{2,48}, unique per site
version     = "1.4.0"                # semver
publisher   = "example.dev"
description = "Meta tags, Open Graph, sitemap pings"
world       = "ferrite:plugin/plugin@0.1.0"
license     = "MIT"
homepage    = "https://example.dev/seo-pro"

[capabilities]
content.read       = ["post", "page"]       # content types readable; ["*"] = all public types
content.meta.read  = ["seo.*"]
content.meta.write = ["seo.*"]              # meta key globs; "plugin.<name>.*" always allowed
options.read       = ["site.title", "site.url"]
kv                 = true
http               = ["api.indexnow.org"]   # hostnames, HTTPS only; no wildcards in v1
render.head        = true                   # may contribute structured head tags
render.text        = true                   # may rewrite plain-text values (excerpt)
render.html        = "off"                  # "off" | "sanitized" | "raw" — rewrite body/footer HTML (§7)

routes             = true                   # may serve endpoints under /_p/<name>/ (§4b)
mail               = "admins"               # may e-mail site admins only (no arbitrary recipients)

[hooks]
"render.head" = 10                          # filter, priority (lower runs first)
"post.published" = 50                       # action
"request.served" = 100                      # action, batched (§4a)

[routes]
"/submit" = { methods = ["POST"], rate_limit = "10/min" }   # → /_p/seo-pro/submit

[settings]                                  # declarative; admin renders the form (§4c)
indexnow_key = { type = "secret", label = "IndexNow API key" }
default_og_image = { type = "media", label = "Default OG image" }
add_json_ld = { type = "bool", default = true, label = "Emit JSON-LD" }

[limits]                                    # may only lower the site defaults, or request
memory = "16MB"                             # higher values that the admin must approve
fuel   = 20_000_000
"#;

    const MINIMAL: &str = r#"
[plugin]
name = "mini"
version = "0.1.0"
publisher = "example.dev"
world = "ferrite:plugin/plugin@0.1.0"
"#;

    fn with(extra: &str) -> String {
        format!("{MINIMAL}\n{extra}")
    }

    fn paths(r: Result<Manifest, ManifestError>) -> Vec<String> {
        r.expect_err("expected failure")
            .problems
            .into_iter()
            .map(|p| p.path)
            .collect()
    }

    #[test]
    fn parses_design_example() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        assert_eq!(m.plugin.name, "seo-pro");
        assert_eq!(m.plugin.version, semver::Version::new(1, 4, 0));
        assert_eq!(m.hooks["render.head"], 10);
        assert_eq!(m.capabilities.http, ["api.indexnow.org"]);
        assert_eq!(m.capabilities.render_html, HtmlMode::Off);
        assert_eq!(m.capabilities.mail, MailScope::Admins);
        assert_eq!(m.capabilities.content_meta_write, ["seo.*"]);
        assert!(m.capabilities.routes && m.capabilities.kv);
        assert_eq!(m.routes["/submit"].methods, ["POST"]);
        assert_eq!(m.routes["/submit"].rate_limit.as_deref(), Some("10/min"));
        assert_eq!(
            m.routes["/submit"].rate(),
            (10, std::time::Duration::from_secs(60))
        );
        assert_eq!(m.settings["add_json_ld"].kind, SettingKind::Bool);
        assert_eq!(
            m.settings["add_json_ld"].default,
            Some(toml::Value::Boolean(true))
        );
        assert_eq!(m.limits.memory, Some(16 * 1024 * 1024));
        assert_eq!(m.limits.fuel, Some(20_000_000));
    }

    #[test]
    fn minimal_manifest_has_safe_defaults() {
        let m = Manifest::parse(MINIMAL).expect("parses");
        assert_eq!(m.capabilities, Capabilities::default());
        assert!(m.hooks.is_empty() && m.routes.is_empty() && m.settings.is_empty());
        assert_eq!(m.limits, LimitRequest::default());
    }

    #[test]
    fn rejects_unknown_keys() {
        let p = paths(Manifest::parse(&with(
            "[capabilities]\nfs = true\ncontent.nope = []\n",
        )));
        assert_eq!(p, ["capabilities.content.nope", "capabilities.fs"]);
        let p = paths(Manifest::parse(&format!("{MINIMAL}\n[bogus]\nx = 1\n")));
        assert_eq!(p, ["bogus"]);
        let p = paths(Manifest::parse(
            &MINIMAL.replace("[plugin]", "[plugin]\nauthor = \"x\""),
        ));
        assert_eq!(p, ["plugin.author"]);
    }

    #[test]
    fn rejects_bad_name() {
        for bad in ["A", "a", "has_underscore", &"a".repeat(49)] {
            let src = MINIMAL.replace("\"mini\"", &format!("\"{bad}\""));
            assert_eq!(paths(Manifest::parse(&src)), ["plugin.name"], "{bad}");
        }
    }

    #[test]
    fn rejects_reserved_names() {
        for reserved in RESERVED_PLUGIN_NAMES {
            let src = MINIMAL.replace("\"mini\"", &format!("\"{reserved}\""));
            let err = Manifest::parse(&src).unwrap_err();
            assert_eq!(err.problems.len(), 1, "{err}");
            assert_eq!(err.problems[0].path, "plugin.name");
            assert!(err.problems[0].message.contains("reserved"), "{err}");
        }
        // A name that only contains a reserved word is fine.
        let src = MINIMAL.replace("\"mini\"", "\"inspector\"");
        assert!(Manifest::parse(&src).is_ok());
    }

    #[test]
    fn rejects_bad_world_and_version() {
        let src = MINIMAL
            .replace("0.1.0\"\npublisher", "nope\"\npublisher")
            .replace("plugin@0.1.0", "plugin@0.2.0");
        assert_eq!(
            paths(Manifest::parse(&src)),
            ["plugin.version", "plugin.world"]
        );
    }

    #[test]
    fn missing_required_fields_are_reported() {
        let p = paths(Manifest::parse("[plugin]\nname = \"ok\"\n"));
        assert_eq!(p, ["plugin.version", "plugin.publisher", "plugin.world"]);
        assert_eq!(paths(Manifest::parse("")), ["plugin"]);
    }

    #[test]
    fn syntax_error_is_a_problem() {
        let e = Manifest::parse("[plugin\n").expect_err("syntax error");
        assert_eq!(e.problems.len(), 1);
    }

    #[test]
    fn collects_all_problems() {
        let src = with("[hooks]\n\"render.nope\" = 1\n\"content.body\" = 5\n");
        let e = Manifest::parse(&src).expect_err("fails");
        let got: Vec<_> = e.problems.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(e.problems.len(), 2, "{e}");
        assert_eq!(got, ["hooks.content.body", "hooks.render.nope"]);
        let shown = e.to_string();
        assert!(shown.contains("hooks.render.nope") && shown.contains("hooks.content.body"));
    }

    #[test]
    fn hooks_with_capability_are_accepted() {
        let src = with(
            "[capabilities]\nrender.html = \"sanitized\"\nrender.text = true\n\
             [hooks]\n\"content.body\" = -5\n\"content.excerpt\" = 1\n",
        );
        let m = Manifest::parse(&src).expect("parses");
        assert_eq!(m.hooks["content.body"], -5);
        assert_eq!(m.capabilities.render_html, HtmlMode::Sanitized);
    }

    #[test]
    fn hook_priority_must_be_i32() {
        let src = with("[hooks]\n\"post.saved\" = 99999999999\n");
        assert_eq!(paths(Manifest::parse(&src)), ["hooks.post.saved"]);
        let src = with("[hooks]\n\"post.saved\" = \"x\"\n");
        assert_eq!(paths(Manifest::parse(&src)), ["hooks.post.saved"]);
    }

    #[test]
    fn http_requires_plain_hostnames() {
        for bad in [
            "*.example.com",
            "https://x.com",
            "x.com/path",
            "x.com:8080",
            "",
            "-a.com",
        ] {
            let src = with(&format!("[capabilities]\nhttp = [\"{bad}\"]\n"));
            assert_eq!(paths(Manifest::parse(&src)), ["capabilities.http"], "{bad}");
        }
        let ok = with("[capabilities]\nhttp = [\"api.example.com\", \"x-y.io\"]\n");
        assert!(Manifest::parse(&ok).is_ok());
    }

    #[test]
    fn capability_patterns_are_validated() {
        for (key, path) in [
            ("content.read", "capabilities.content.read"),
            ("content.meta.read", "capabilities.content.meta.read"),
            ("content.meta.write", "capabilities.content.meta.write"),
            ("options.read", "capabilities.options.read"),
        ] {
            for bad in ["seo*", "*.seo", "a.*.b", "a**", ".*", "**"] {
                let src = with(&format!("[capabilities]\n{key} = [\"{bad}\"]\n"));
                assert_eq!(paths(Manifest::parse(&src)), [path], "{key} = {bad}");
            }
            let ok = with(&format!(
                "[capabilities]\n{key} = [\"*\", \"seo.*\", \"site.title\"]\n"
            ));
            assert!(Manifest::parse(&ok).is_ok(), "{key}");
        }
    }

    #[test]
    fn render_html_values() {
        for (v, want) in [("raw", HtmlMode::Raw), ("sanitized", HtmlMode::Sanitized)] {
            let src = with(&format!("[capabilities]\nrender.html = \"{v}\"\n"));
            assert_eq!(
                Manifest::parse(&src).expect("ok").capabilities.render_html,
                want
            );
        }
        for bad in ["false", "\"maybe\""] {
            let src = with(&format!("[capabilities]\nrender.html = {bad}\n"));
            assert_eq!(paths(Manifest::parse(&src)), ["capabilities.render.html"]);
        }
    }

    #[test]
    fn mail_only_admins() {
        let src = with("[capabilities]\nmail = \"anyone\"\n");
        assert_eq!(paths(Manifest::parse(&src)), ["capabilities.mail"]);
    }

    #[test]
    fn routes_require_capability() {
        let src = with("[routes]\n\"/x\" = { methods = [\"GET\"] }\n");
        assert_eq!(paths(Manifest::parse(&src)), ["routes"]);
        let ok = with("[capabilities]\nroutes = true\n[routes]\n\"/x\" = {}\n");
        let m = Manifest::parse(&ok).expect("ok");
        assert_eq!(m.routes["/x"].methods, ["GET"]);
        assert!(!m.routes["/x"].cross_origin);
        // No `rate_limit`: the default.
        assert_eq!(
            m.routes["/x"].rate(),
            (10, std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn route_validation() {
        let base = "[capabilities]\nroutes = true\n[routes]\n";
        let cases = [
            ("\"submit\" = {}", "routes.submit"),
            ("\"/a\" = { methods = [\"post\"] }", "routes./a.methods"),
            ("\"/a\" = { methods = [] }", "routes./a.methods"),
            (
                "\"/a\" = { rate_limit = \"10/day\" }",
                "routes./a.rate_limit",
            ),
            (
                "\"/a\" = { rate_limit = \"x/min\" }",
                "routes./a.rate_limit",
            ),
            (
                "\"/a\" = { rate_limit = \"0/min\" }",
                "routes./a.rate_limit",
            ),
            ("\"/a\" = { nope = 1 }", "routes./a.nope"),
        ];
        for (line, path) in cases {
            assert_eq!(
                paths(Manifest::parse(&with(&format!("{base}{line}\n")))),
                [path]
            );
        }
        let ok = with(&format!(
            "{base}\"/a\" = {{ methods = [\"GET\", \"POST\"], rate_limit = \"5/s\", cross_origin = true }}\n"
        ));
        let m = Manifest::parse(&ok).expect("ok");
        assert!(m.routes["/a"].cross_origin);
        assert_eq!(
            m.routes["/a"].rate(),
            (5, std::time::Duration::from_secs(1))
        );
    }

    #[test]
    fn setting_validation() {
        let cases = [
            ("a = { type = \"nope\", label = \"A\" }", "settings.a.type"),
            ("a = { type = \"text\" }", "settings.a.label"),
            ("a = { label = \"A\" }", "settings.a.type"),
            (
                "a = { type = \"select\", label = \"A\" }",
                "settings.a.options",
            ),
            (
                "a = { type = \"bool\", label = \"A\", default = \"x\" }",
                "settings.a.default",
            ),
            (
                "a = { type = \"text\", label = \"A\", extra = 1 }",
                "settings.a.extra",
            ),
            ("a = 1", "settings.a"),
            // Pre-flight F23: keys are option names and seal purposes.
            ("Ab = { type = \"text\", label = \"A\" }", "settings.Ab"),
            (
                "\"a.b\" = { type = \"text\", label = \"A\" }",
                "settings.a.b",
            ),
            (
                "\"a:b\" = { type = \"text\", label = \"A\" }",
                "settings.a:b",
            ),
            (
                "n = { type = \"number\", label = \"N\", default = nan }",
                "settings.n.default",
            ),
            // Review m3: defaults follow the value rules.
            (
                "s = { type = \"select\", label = \"S\", options = [\"x\"], default = \"y\" }",
                "settings.s.default",
            ),
            (
                "c = { type = \"color\", label = \"C\", default = \"red\" }",
                "settings.c.default",
            ),
            (
                "k = { type = \"secret\", label = \"K\", default = \"x\" }",
                "settings.k.default",
            ),
        ];
        for (line, path) in cases {
            let src = with(&format!("[settings]\n{line}\n"));
            assert_eq!(paths(Manifest::parse(&src)), [path], "{line}");
        }
        let ok = with(
            "[settings]\na = { type = \"select\", label = \"A\", options = [\"x\", \"y\"], default = \"x\" }\n",
        );
        let m = Manifest::parse(&ok).expect("ok");
        assert_eq!(m.settings["a"].options, ["x", "y"]);
        let long = "k".repeat(65);
        let src = with(&format!(
            "[settings]\n{long} = {{ type = \"text\", label = \"A\" }}\n"
        ));
        assert_eq!(paths(Manifest::parse(&src)), [format!("settings.{long}")]);
        let ok = with(&format!(
            "[settings]\n{} = {{ type = \"text\", label = \"A\" }}\nmy_key-2 = {{ type = \"bool\", label = \"B\" }}\n",
            "k".repeat(64)
        ));
        assert_eq!(Manifest::parse(&ok).expect("ok").settings.len(), 2);
        let long = with(&format!(
            "[settings]\nt = {{ type = \"text\", label = \"T\", default = \"{}\" }}\n",
            "a".repeat(1_001)
        ));
        assert_eq!(paths(Manifest::parse(&long)), ["settings.t.default"]);
        let ok = with("[settings]\nc = { type = \"color\", label = \"C\", default = \"#abc\" }\n");
        assert!(Manifest::parse(&ok).is_ok());
    }

    #[test]
    fn limit_parsing() {
        for (v, want) in [
            ("16MB", 16u64 << 20),
            ("32MiB", 32 << 20),
            ("512KB", 512 << 10),
            ("4KiB", 4 << 10),
        ] {
            let src = with(&format!("[limits]\nmemory = \"{v}\"\n"));
            assert_eq!(
                Manifest::parse(&src).expect("ok").limits.memory,
                Some(want),
                "{v}"
            );
        }
        for bad in [
            "\"16GB\"",
            "\"MB\"",
            "\"1.5MB\"",
            "\"0MB\"",
            "16",
            "\"-1MB\"",
        ] {
            let src = with(&format!("[limits]\nmemory = {bad}\n"));
            assert_eq!(paths(Manifest::parse(&src)), ["limits.memory"], "{bad}");
        }
        let src = with("[limits]\nfuel = -1\n");
        assert_eq!(paths(Manifest::parse(&src)), ["limits.fuel"]);
    }

    #[test]
    fn capability_subsets() {
        let caps = |src: &str| {
            Manifest::parse(&with(&format!("[capabilities]\n{src}")))
                .expect("ok")
                .capabilities
        };
        let want = caps(
            "content.read = [\"post\"]\ncontent.meta.read = [\"seo.title\", \"seo.x.*\"]\n\
             kv = true\nhttp = [\"api.example.org\"]\nrender.html = \"sanitized\"\n",
        );
        assert!(want.is_subset_of(&want));
        let wider = caps(
            "content.read = [\"*\"]\ncontent.meta.read = [\"seo.*\"]\nkv = true\n\
             http = [\"api.example.org\", \"b.example.org\"]\nrender.html = \"raw\"\n",
        );
        assert!(want.is_subset_of(&wider));
        assert!(!wider.is_subset_of(&want));
        for narrower in [
            "content.read = [\"page\"]\ncontent.meta.read = [\"seo.*\"]\nkv = true\nhttp = [\"api.example.org\"]\nrender.html = \"raw\"\n",
            "content.read = [\"*\"]\ncontent.meta.read = [\"seo.title\"]\nkv = true\nhttp = [\"api.example.org\"]\nrender.html = \"raw\"\n",
            "content.read = [\"*\"]\ncontent.meta.read = [\"seo.*\"]\nhttp = [\"api.example.org\"]\nrender.html = \"raw\"\n",
            "content.read = [\"*\"]\ncontent.meta.read = [\"seo.*\"]\nkv = true\nrender.html = \"raw\"\n",
            "content.read = [\"*\"]\ncontent.meta.read = [\"seo.*\"]\nkv = true\nhttp = [\"api.example.org\"]\n",
        ] {
            assert!(!want.is_subset_of(&caps(narrower)), "{narrower}");
        }
        assert!(
            !caps("options.read = [\"*\"]\n").is_subset_of(&caps("options.read = [\"site.*\"]\n"))
        );
        assert!(Capabilities::default().is_subset_of(&Capabilities::default()));
    }
}
