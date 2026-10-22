//! `ferrite-plugin.toml` parsing and validation (design 05 §3).

use std::collections::BTreeMap;

use crate::error::{ManifestError, ManifestProblem};

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

fn html_level(m: HtmlMode) -> u8 {
    match m {
        HtmlMode::Off => 0,
        HtmlMode::Sanitized => 1,
        HtmlMode::Raw => 2,
    }
}

/// Whether pattern `p` (`*`, `<prefix>.*` or an exact key) is within `granted`.
fn pattern_covered(p: &str, granted: &[String]) -> bool {
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
    /// Parse and validate. All problems are collected before returning.
    pub fn parse(src: &str) -> Result<Manifest, ManifestError> {
        let mut table: toml::Table = match src.parse() {
            Ok(t) => t,
            Err(e) => {
                return Err(ManifestError {
                    problems: vec![ManifestProblem {
                        path: String::new(),
                        message: format!("invalid TOML: {e}"),
                    }],
                });
            }
        };
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

fn valid_name(name: &str) -> bool {
    (2..=48).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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

    if let Some(n) = &name
        && !valid_name(n)
    {
        cx.push("plugin.name", "must match [a-z0-9-]{2,48}");
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
    if let Some(hosts) = cx.strings(t, "http", "capabilities.http") {
        let bad: Vec<_> = hosts.iter().filter(|h| !valid_hostname(h)).collect();
        if !bad.is_empty() {
            cx.push(
                "capabilities.http",
                format!(
                    "must be plain hostnames (no scheme, port, path or wildcard); invalid: {}",
                    bad.iter()
                        .map(|h| format!("`{h}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
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

fn valid_rate_limit(s: &str) -> bool {
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
            let ok = match kind {
                SettingKind::Bool => d.is_bool(),
                SettingKind::Number => d.is_integer() || d.is_float(),
                SettingKind::Text
                | SettingKind::Textarea
                | SettingKind::Select
                | SettingKind::Color => d.is_str(),
                SettingKind::Media | SettingKind::Secret => false,
            };
            if !ok {
                cx.push(
                    format!("{path}.default"),
                    format!(
                        "default of type {} is not valid for this setting type",
                        d.type_str()
                    ),
                );
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
