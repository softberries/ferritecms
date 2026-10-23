//! The permission report an admin approves before a plugin is installed or updated
//! (plan rulings P3, P5, P6): every capability the manifest asks for as a plain-language
//! line, high-risk ones flagged, what is new against the current grant, the limits, and
//! the approval fingerprint.
//!
//! **Fingerprint.** Lowercase hex SHA-256 of the compact JSON of
//! `{"artifact":<hash>,"grant":<Grant::of(manifest).to_json()>,"plugin":<name>,
//! "version":<version>}`. serde_json objects are sorted maps (the workspace does not
//! enable `preserve_order`; a unit test pins one fingerprint so a change there fails
//! loudly), so equal inputs give equal fingerprints on every node and release.

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::PluginError;
use crate::grant::{Grant, LimitBounds};
use crate::manifest::{Capabilities, HtmlMode, MailScope, Manifest, html_level, pattern_covered};

/// What a manifest asks for, as an admin sees it before approving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionReport {
    pub plugin: String,
    pub version: String,
    /// The component's artifact hash (64 lowercase hex) the report was made for.
    pub artifact: String,
    /// Every capability asked for, in a fixed order (see [`PermissionItem::key`]).
    pub items: Vec<PermissionItem>,
    /// The items (or the parts of them) the current grant does not cover; all of
    /// `items` when there is no current grant.
    pub added: Vec<PermissionItem>,
    /// Every limit the manifest's `[limits]` sets, memory first.
    pub limits: Vec<LimitItem>,
    /// What an approval must carry (see the module docs).
    pub fingerprint: String,
}

/// One capability asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionItem {
    /// `content.read`, `content.meta.read`, `content.meta.write`, `options.read`, `kv`,
    /// `http` (one item per host), `render.head`, `render.text`, `render.html`,
    /// `routes` (one item per route), `mail`, `limits.memory`, `limits.fuel` (only when
    /// above the default).
    pub key: &'static str,
    /// Plain language, e.g. "read published posts and pages".
    pub text: String,
    /// The patterns, types, host or methods the item covers.
    pub detail: Vec<String>,
    pub high_risk: bool,
}

/// One `[limits]` request against the host default and the approval ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitItem {
    /// `memory` (bytes) or `fuel`.
    pub name: &'static str,
    pub requested: u64,
    pub default: u64,
    pub ceiling: u64,
    pub above_default: bool,
}

impl Manifest {
    /// The report for installing this manifest with the component `artifact` (64 hex,
    /// `ArtifactHash::as_str()`), against the `current` grant of an installed version
    /// (`None` for a new plugin) and the host `defaults` `(memory bytes, fuel)`. A limit
    /// above its ceiling is refused here (`LimitAboveCeiling`), before anything is
    /// stored.
    pub fn permission_report(
        &self,
        artifact: &str,
        current: Option<&Grant>,
        defaults: (u64, u64),
    ) -> Result<PermissionReport, PluginError> {
        let bounds = [
            (
                "memory",
                self.limits.memory,
                LimitBounds::memory(defaults.0),
            ),
            ("fuel", self.limits.fuel, LimitBounds::fuel(defaults.1)),
        ];
        let mut limits = Vec::new();
        for (name, requested, b) in bounds {
            if let Some(requested) = requested {
                b.within_ceiling(name, requested)?;
                limits.push(LimitItem {
                    name,
                    requested,
                    default: b.default,
                    ceiling: b.ceiling,
                    above_default: requested > b.default,
                });
            }
        }
        let name = &self.plugin.name;
        let mut items = capability_items(self, &self.capabilities);
        items.extend(limits.iter().filter_map(limit_item));
        let added = match current {
            None => items.clone(),
            Some(current) => {
                let mut added =
                    capability_items(self, &uncovered(&self.capabilities, &current.capabilities));
                let approved = |l: &LimitItem| match l.name {
                    "memory" => current.limits.memory,
                    _ => current.limits.fuel,
                };
                added.extend(
                    limits
                        .iter()
                        .filter(|l| approved(l).is_none_or(|a| a < l.requested))
                        .filter_map(limit_item),
                );
                added
            }
        };
        Ok(PermissionReport {
            plugin: name.clone(),
            version: self.plugin.version.to_string(),
            artifact: artifact.to_owned(),
            items,
            added,
            limits,
            fingerprint: fingerprint(self, artifact),
        })
    }
}

fn fingerprint(m: &Manifest, artifact: &str) -> String {
    let doc = json!({
        "plugin": m.plugin.name,
        "version": m.plugin.version.to_string(),
        "artifact": artifact,
        "grant": Grant::of(m).to_json(),
    });
    Sha256::digest(doc.to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What of `want` `have` does not cover (the same rules as
/// [`Capabilities::is_subset_of`]).
fn uncovered(want: &Capabilities, have: &Capabilities) -> Capabilities {
    let patterns = |want: &[String], have: &[String]| -> Vec<String> {
        want.iter()
            .filter(|p| !pattern_covered(p, have))
            .cloned()
            .collect()
    };
    Capabilities {
        content_read: patterns(&want.content_read, &have.content_read),
        content_meta_read: patterns(&want.content_meta_read, &have.content_meta_read),
        content_meta_write: patterns(&want.content_meta_write, &have.content_meta_write),
        options_read: patterns(&want.options_read, &have.options_read),
        kv: want.kv && !have.kv,
        http: want
            .http
            .iter()
            .filter(|h| !have.http.contains(h))
            .cloned()
            .collect(),
        render_head: want.render_head && !have.render_head,
        render_text: want.render_text && !have.render_text,
        render_html: if html_level(want.render_html) > html_level(have.render_html) {
            want.render_html
        } else {
            HtmlMode::Off
        },
        routes: want.routes && !have.routes,
        mail: if want.mail == MailScope::Admins && have.mail != MailScope::Admins {
            MailScope::Admins
        } else {
            MailScope::None
        },
    }
}

fn item(
    key: &'static str,
    text: impl Into<String>,
    detail: Vec<String>,
    high_risk: bool,
) -> PermissionItem {
    PermissionItem {
        key,
        text: text.into(),
        detail,
        high_risk,
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn type_name(t: &str) -> String {
    match t {
        "post" => "posts".to_owned(),
        "page" => "pages".to_owned(),
        other => format!("`{other}` items"),
    }
}

fn quoted(patterns: &[String]) -> String {
    and_list(
        &patterns
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>(),
    )
}

/// The items for capabilities `c` of manifest `m` (routes come from `m`'s `[routes]`).
fn capability_items(m: &Manifest, c: &Capabilities) -> Vec<PermissionItem> {
    let name = &m.plugin.name;
    let own = format!("plugin.{name}.");
    let any_star = |p: &[String]| p.iter().any(|p| p == "*");
    let mut out = Vec::new();
    if !c.content_read.is_empty() {
        let text = if any_star(&c.content_read) {
            "read all published content, of every type".to_owned()
        } else {
            let types: Vec<_> = c.content_read.iter().map(|t| type_name(t)).collect();
            format!("read published {}", and_list(&types))
        };
        out.push(item(
            "content.read",
            text,
            c.content_read.clone(),
            any_star(&c.content_read),
        ));
    }
    if !c.content_meta_read.is_empty() {
        let text = if any_star(&c.content_meta_read) {
            "read every meta field of published content".to_owned()
        } else {
            format!(
                "read the meta fields {} of published content",
                quoted(&c.content_meta_read)
            )
        };
        let high = any_star(&c.content_meta_read);
        out.push(item(
            "content.meta.read",
            text,
            c.content_meta_read.clone(),
            high,
        ));
    }
    if !c.content_meta_write.is_empty() {
        let text = if any_star(&c.content_meta_write) {
            "change every meta field of content (outside other plugins' own fields)".to_owned()
        } else {
            format!(
                "change the meta fields {} of content",
                quoted(&c.content_meta_write)
            )
        };
        let high = c
            .content_meta_write
            .iter()
            .any(|p| p == "*" || !p.starts_with(&own));
        out.push(item(
            "content.meta.write",
            text,
            c.content_meta_write.clone(),
            high,
        ));
    }
    if !c.options_read.is_empty() {
        let text = if any_star(&c.options_read) {
            "read every site option".to_owned()
        } else {
            format!("read the site options {}", quoted(&c.options_read))
        };
        out.push(item(
            "options.read",
            text,
            c.options_read.clone(),
            any_star(&c.options_read),
        ));
    }
    if c.kv {
        out.push(item(
            "kv",
            "store its own data on the site",
            Vec::new(),
            false,
        ));
    }
    for host in &c.http {
        out.push(item(
            "http",
            format!("send HTTPS requests to {host}"),
            vec![host.clone()],
            true,
        ));
    }
    if c.render_head {
        out.push(item(
            "render.head",
            "add tags to the head of pages",
            Vec::new(),
            false,
        ));
    }
    if c.render_text {
        out.push(item(
            "render.text",
            "rewrite plain-text values such as excerpts",
            Vec::new(),
            false,
        ));
    }
    match c.render_html {
        HtmlMode::Off => {}
        HtmlMode::Sanitized => out.push(item(
            "render.html",
            "rewrite the HTML of post bodies and footers (sanitized)",
            Vec::new(),
            false,
        )),
        HtmlMode::Raw => out.push(item(
            "render.html",
            "insert unsanitized HTML, including scripts, into post bodies and footers",
            Vec::new(),
            true,
        )),
    }
    if c.routes {
        if m.routes.is_empty() {
            out.push(item(
                "routes",
                format!("answer requests under /_p/{name}/"),
                Vec::new(),
                true,
            ));
        }
        for (path, spec) in &m.routes {
            let mut text = format!(
                "answer {} requests at /_p/{name}{path}",
                and_list(&spec.methods)
            );
            match &spec.rate_limit {
                Some(limit) => text.push_str(&format!(" (at most {limit} per visitor)")),
                None => text.push_str(" (default rate limit)"),
            }
            if spec.cross_origin {
                text.push_str(", from other sites too");
            }
            out.push(item("routes", text, spec.methods.clone(), true));
        }
    }
    if c.mail == MailScope::Admins {
        out.push(item(
            "mail",
            "send e-mail to the site's administrators",
            Vec::new(),
            true,
        ));
    }
    out
}

/// A high-risk item for a limit above its default; `None` otherwise.
fn limit_item(l: &LimitItem) -> Option<PermissionItem> {
    if !l.above_default {
        return None;
    }
    Some(match l.name {
        "memory" => item(
            "limits.memory",
            format!(
                "use up to {} of memory per call (site default {})",
                bytes(l.requested),
                bytes(l.default)
            ),
            Vec::new(),
            true,
        ),
        _ => item(
            "limits.fuel",
            format!(
                "use up to {} fuel per call (site default {})",
                l.requested, l.default
            ),
            Vec::new(),
            true,
        ),
    })
}

fn bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    if n.is_multiple_of(MIB) {
        format!("{} MiB", n / MIB)
    } else if n.is_multiple_of(KIB) {
        format!("{} KiB", n / KIB)
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::DESIGN_EXAMPLE;

    const ARTIFACT: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const DEFAULTS: (u64, u64) = (32 * 1024 * 1024, 50_000_000);

    fn summary(items: &[PermissionItem]) -> Vec<(&'static str, Vec<String>, bool)> {
        items
            .iter()
            .map(|i| (i.key, i.detail.clone(), i.high_risk))
            .collect()
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn report_lists_capabilities_in_plain_language() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let r = m
            .permission_report(ARTIFACT, None, DEFAULTS)
            .expect("report");
        assert_eq!(r.plugin, "seo-pro");
        assert_eq!(r.version, "1.4.0");
        assert_eq!(r.artifact, ARTIFACT);
        assert_eq!(
            summary(&r.items),
            [
                ("content.read", v(&["post", "page"]), false),
                ("content.meta.read", v(&["seo.*"]), false),
                // Outside the plugin's own `plugin.seo-pro.*` namespace.
                ("content.meta.write", v(&["seo.*"]), true),
                ("options.read", v(&["site.title", "site.url"]), false),
                ("kv", v(&[]), false),
                ("http", v(&["api.indexnow.org"]), true),
                ("render.head", v(&[]), false),
                ("render.text", v(&[]), false),
                ("routes", v(&["POST"]), true),
                ("mail", v(&[]), true),
            ]
        );
        let text = |key: &str| {
            r.items
                .iter()
                .find(|i| i.key == key)
                .map(|i| i.text.as_str())
                .unwrap_or_default()
        };
        assert_eq!(text("content.read"), "read published posts and pages");
        assert_eq!(
            text("content.meta.write"),
            "change the meta fields `seo.*` of content"
        );
        assert_eq!(
            text("options.read"),
            "read the site options `site.title` and `site.url`"
        );
        assert_eq!(text("http"), "send HTTPS requests to api.indexnow.org");
        assert_eq!(
            text("routes"),
            "answer POST requests at /_p/seo-pro/submit (at most 10/min per visitor)"
        );
        assert_eq!(text("mail"), "send e-mail to the site's administrators");
        // No current grant: everything is new. The design example's limits lower the
        // defaults: listed, not high risk, no item.
        assert_eq!(r.added, r.items);
        assert_eq!(
            r.limits,
            [
                LimitItem {
                    name: "memory",
                    requested: 16 * 1024 * 1024,
                    default: DEFAULTS.0,
                    ceiling: 256 * 1024 * 1024,
                    above_default: false,
                },
                LimitItem {
                    name: "fuel",
                    requested: 20_000_000,
                    default: DEFAULTS.1,
                    ceiling: 500_000_000,
                    above_default: false,
                },
            ]
        );
    }

    #[test]
    fn high_risk_rules() {
        let caps = |src: &str| {
            let m = Manifest::parse(&format!(
                "[plugin]\nname = \"me\"\nversion = \"1.0.0\"\npublisher = \"x\"\n\
                 world = \"ferrite:plugin/plugin@0.1.0\"\n[capabilities]\n{src}\n"
            ))
            .expect("manifest");
            summary(
                &m.permission_report(ARTIFACT, None, DEFAULTS)
                    .expect("report")
                    .items,
            )
        };
        // Writes in the plugin's own namespace are not high risk; anything else is.
        assert_eq!(
            caps("content.meta.write = [\"plugin.me.*\", \"plugin.me.x\"]"),
            [(
                "content.meta.write",
                v(&["plugin.me.*", "plugin.me.x"]),
                false
            )]
        );
        assert!(caps("content.meta.write = [\"plugin.mee.*\"]")[0].2);
        assert!(caps("content.meta.write = [\"plugin.me.*\", \"*\"]")[0].2);
        // `*` anywhere is high risk; reads of named keys are not.
        for key in ["content.read", "content.meta.read", "options.read"] {
            assert!(caps(&format!("{key} = [\"*\"]"))[0].2, "{key}");
            assert!(!caps(&format!("{key} = [\"a.b\"]"))[0].2, "{key}");
        }
        assert_eq!(
            caps("render.html = \"raw\""),
            [("render.html", v(&[]), true)]
        );
        assert_eq!(
            caps("render.html = \"sanitized\""),
            [("render.html", v(&[]), false)]
        );
        assert_eq!(caps("render.html = \"off\""), []);
        // `routes = true` without a `[routes]` table still shows (and is high risk).
        assert_eq!(caps("routes = true"), [("routes", v(&[]), true)]);
    }

    /// Pre-flight F7: pins the fingerprint format. If this fails, a dependency changed
    /// how serde_json orders or writes objects (e.g. `preserve_order`), or the grant
    /// format changed: every stored approval would stop matching across a rolling
    /// upgrade. Change the constants only for a deliberate format change.
    #[test]
    fn fingerprint_is_pinned() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let doc = json!({
            "plugin": m.plugin.name,
            "version": m.plugin.version.to_string(),
            "artifact": ARTIFACT,
            "grant": Grant::of(&m).to_json(),
        });
        assert_eq!(doc.to_string(), PINNED_DOC);
        assert_eq!(fingerprint(&m, ARTIFACT), PINNED_FINGERPRINT);
        let r = m
            .permission_report(ARTIFACT, None, DEFAULTS)
            .expect("report");
        assert_eq!(r.fingerprint, PINNED_FINGERPRINT);
    }

    /// Checked independently: `printf '%s' "$PINNED_DOC" | shasum -a 256`.
    const PINNED_DOC: &str = concat!(
        r#"{"artifact":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","#,
        r#""grant":{"capabilities":{"content":{"meta":{"read":["seo.*"],"write":["seo.*"]},"#,
        r#""read":["post","page"]},"http":["api.indexnow.org"],"kv":true,"mail":"admins","#,
        r#""options":{"read":["site.title","site.url"]},"render":{"head":true,"text":true},"#,
        r#""routes":true},"limits":{"fuel":20000000,"memory":16777216},"v":1},"#,
        r#""plugin":"seo-pro","version":"1.4.0"}"#,
    );
    const PINNED_FINGERPRINT: &str =
        "8bbedd0caa89cc6f6ca9058ca1afee3fbe2735f0fae9b878cf5de80189dc54a5";
}
