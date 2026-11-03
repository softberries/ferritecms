//! The permission report an admin approves before a plugin is installed or updated
//! (plan rulings P3, P5, P6): every capability the manifest asks for as a plain-language
//! line with its [`Risk`], what is new against the current grant, the limits, and the
//! approval fingerprint. Action-hook subscriptions are listed too (ruling R22, M1).
//! `request.served` is part of the grant (R23): it is in the fingerprint and, when the
//! current grant lacks it, in `added`. The other action hooks are low risk and not part
//! of the grant: they do not change the fingerprint, and against a current grant (an
//! update, or an installed row) they are never in `added`.
//!
//! **Fingerprint.** Lowercase hex SHA-256 of the compact JSON of
//! `{"artifact":<hash>,"grant":<Grant::of(manifest).to_json()>,"plugin":<name>,
//! "version":<version>}`, plus `"signer":<KEY_ID>` when the bundle is signed (M6-2 D11:
//! unsigned fingerprints stay byte-identical, and an approval of a signed report cannot be
//! replayed with the signature stripped or swapped). serde_json objects are sorted maps (the workspace does not
//! enable `preserve_order`; a unit test pins one fingerprint so a change there fails
//! loudly), so equal inputs give equal fingerprints on every node and release.

use ferrite_domain::plugin_trust::KeyId;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::HostConfig;
use crate::error::PluginError;
use crate::grant::{GRANTED_HOOK, Grant, LimitBounds, RoutePolicy};
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
    /// The key that signed the bundle, when it is signed (part of the fingerprint).
    pub signer: Option<KeyId>,
}

/// One capability asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionItem {
    /// `content.read`, `content.meta.read`, `content.meta.write`, `options.read`, `kv`,
    /// `http` (one item per host), `render.head`, `render.text`, `render.html`,
    /// `routes` (one item per route), `mail`, `hooks` (one item per action hook),
    /// `limits.memory`, `limits.fuel` (only when above the default).
    pub key: &'static str,
    /// Plain language, e.g. "read published posts and pages".
    pub text: String,
    /// The patterns, types, host, methods or hook name the item covers.
    pub detail: Vec<String>,
    pub risk: Risk,
}

impl PermissionItem {
    /// Whether the item is [`Risk::High`] (the CLI's `!` lines).
    pub fn high_risk(&self) -> bool {
        self.risk == Risk::High
    }
}

/// How much an item can do to the site or its visitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Low,
    /// Sees site-wide data no single page shows (`request.served`).
    Medium,
    High,
}

impl Risk {
    /// `low`, `medium` or `high`.
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        }
    }
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
    /// (`None` for a new plugin), for a host configured as `host` (its defaults and its
    /// real ceilings: a pooling host's memory ceiling is its slot size, so an approval
    /// the report allows always loads). A limit above its ceiling is refused here
    /// (`LimitAboveCeiling`), before anything is stored.
    pub fn permission_report(
        &self,
        artifact: &str,
        current: Option<&Grant>,
        host: &HostConfig,
    ) -> Result<PermissionReport, PluginError> {
        self.permission_report_signed(artifact, None, current, host)
    }

    /// [`Manifest::permission_report`] for a bundle signed by `signer` (its key id is
    /// part of the fingerprint, M6-2 D11).
    pub fn permission_report_signed(
        &self,
        artifact: &str,
        signer: Option<&KeyId>,
        current: Option<&Grant>,
        host: &HostConfig,
    ) -> Result<PermissionReport, PluginError> {
        let (memory, fuel) = LimitBounds::of_host(host);
        let bounds = [
            ("memory", self.limits.memory, memory),
            ("fuel", self.limits.fuel, fuel),
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
        let mut items = capability_items(self, &self.capabilities, None);
        items.extend(hook_items(self));
        items.extend(limits.iter().filter_map(limit_item));
        let added = match current {
            None => items.clone(),
            Some(current) => {
                let uncovered = uncovered(&self.capabilities, &current.capabilities);
                let relaxed = (self.capabilities.routes && !uncovered.routes).then_some(current);
                let mut added = capability_items(self, &uncovered, relaxed);
                if !current.request_served {
                    added.extend(
                        hook_items(self)
                            .into_iter()
                            .filter(|i| i.detail.iter().any(|h| h == GRANTED_HOOK)),
                    );
                }
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
            fingerprint: fingerprint(self, artifact, signer),
            signer: signer.copied(),
        })
    }
}

fn fingerprint(m: &Manifest, artifact: &str, signer: Option<&KeyId>) -> String {
    let mut doc = json!({
        "plugin": m.plugin.name,
        "version": m.plugin.version.to_string(),
        "artifact": artifact,
        "grant": Grant::of(m).to_json(),
    });
    if let Some(signer) = signer {
        doc["signer"] = json!(signer.to_string());
    }
    ferrite_domain::to_hex(&Sha256::digest(doc.to_string().as_bytes()))
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
        risk: if high_risk { Risk::High } else { Risk::Low },
    }
}

/// One item per action hook the manifest subscribes to, in hook-name order (filter
/// hooks are covered by their `render.*` items). Payloads per design 05 §4.
fn hook_items(m: &Manifest) -> Vec<PermissionItem> {
    m.hooks
        .keys()
        .filter_map(|hook| {
            let (text, risk) = match hook.as_str() {
                "post.saved" => ("is told the id of every post or page saved", Risk::Low),
                "post.published" => ("is told the id of every post or page published", Risk::Low),
                "user.created" => ("is told the id of every new user", Risk::Low),
                "request.served" => (
                    "receives every page view of the site (path, status, time, referrer host, \
                     browser class, cache outcome)",
                    Risk::Medium,
                ),
                _ => return None,
            };
            Some(PermissionItem {
                key: "hooks",
                text: text.to_owned(),
                detail: vec![hook.clone()],
                risk,
            })
        })
        .collect()
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
fn capability_items(
    m: &Manifest,
    c: &Capabilities,
    relaxed: Option<&Grant>,
) -> Vec<PermissionItem> {
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
    // High (ruling R22): a head script runs with the site's origin on every page, as a
    // WordPress plugin's would, and can act as a logged-in admin who views the site.
    if c.render_head {
        out.push(item(
            "render.head",
            "add tags to the head of every page, including scripts from any https:// host, \
             which run with the site's origin (as a logged-in admin when one views the site)",
            Vec::new(),
            true,
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
    // Routes: all of them when `c` asks for routes; otherwise (an `added` list under an
    // already-granted `routes`) those whose policy `relaxed` says is not approved (R7).
    if c.routes || relaxed.is_some() {
        if c.routes && m.routes.is_empty() {
            out.push(item(
                "routes",
                format!("answer requests under /_p/{name}/"),
                Vec::new(),
                true,
            ));
        }
        for (path, spec) in &m.routes {
            if !c.routes && !relaxed.is_some_and(|g| !g.covers_route(&RoutePolicy::of(spec))) {
                continue;
            }
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
            .map(|i| (i.key, i.detail.clone(), i.high_risk()))
            .collect()
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn report_lists_capabilities_in_plain_language() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let r = m
            .permission_report(ARTIFACT, None, &HostConfig::default())
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
                ("render.head", v(&[]), true),
                ("render.text", v(&[]), false),
                ("routes", v(&["POST"]), true),
                ("mail", v(&[]), true),
                ("hooks", v(&["post.published"]), false),
                ("hooks", v(&["request.served"]), false),
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
                &m.permission_report(ARTIFACT, None, &HostConfig::default())
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

    /// Ruling R22 (M1): action hooks are listed with their risk; `request.served` is
    /// medium. Only `request.served` is part of the grant (R23): an update adding it is
    /// `added` and changes the fingerprint; the other hooks do neither.
    #[test]
    fn hook_subscriptions_are_listed_with_their_risk() {
        let src = |hooks: &str| {
            format!(
                "[plugin]\nname = \"me\"\nversion = \"1.0.0\"\npublisher = \"x\"\n\
                 world = \"ferrite:plugin/plugin@0.1.0\"\n[hooks]\n{hooks}\n"
            )
        };
        let all = Manifest::parse(&src(
            "\"post.saved\" = 1\n\"post.published\" = 1\n\"user.created\" = 1\n\
             \"request.served\" = 1",
        ))
        .expect("manifest");
        let r = all
            .permission_report(ARTIFACT, None, &HostConfig::default())
            .expect("report");
        let got: Vec<_> = r
            .items
            .iter()
            .map(|i| (i.key, i.detail.clone(), i.risk))
            .collect();
        assert_eq!(
            got,
            [
                ("hooks", v(&["post.published"]), Risk::Low),
                ("hooks", v(&["post.saved"]), Risk::Low),
                ("hooks", v(&["request.served"]), Risk::Medium),
                ("hooks", v(&["user.created"]), Risk::Low),
            ]
        );
        assert!(r.items[2].text.contains("every page view"));
        assert_eq!(r.added, r.items, "a new plugin: everything is new");
        let none = Manifest::parse(&src("")).expect("manifest");
        let update = all
            .permission_report(ARTIFACT, Some(&Grant::of(&none)), &HostConfig::default())
            .expect("report");
        let added: Vec<_> = update.added.iter().map(|i| i.detail.clone()).collect();
        assert_eq!(
            added,
            [v(&["request.served"])],
            "only request.served widens"
        );
        let base = none
            .permission_report(ARTIFACT, None, &HostConfig::default())
            .expect("report");
        assert_ne!(
            update.fingerprint, base.fingerprint,
            "request.served is approved"
        );
        // The low-risk hooks alone: not added, fingerprint unchanged.
        let low = Manifest::parse(&src(
            "\"post.saved\" = 1\n\"post.published\" = 1\n\"user.created\" = 1",
        ))
        .expect("manifest");
        let update = low
            .permission_report(ARTIFACT, Some(&Grant::of(&none)), &HostConfig::default())
            .expect("report");
        assert!(update.added.is_empty(), "{:?}", update.added);
        assert_eq!(update.fingerprint, base.fingerprint);
        // Approved: an installed row's report adds nothing.
        let row = all
            .permission_report(ARTIFACT, Some(&Grant::of(&all)), &HostConfig::default())
            .expect("report");
        assert!(row.added.is_empty(), "{:?}", row.added);
        assert_eq!(Risk::Medium.as_str(), "medium");
    }

    /// Pre-flight F7: pins the fingerprint format. If this fails, a dependency changed
    /// how serde_json orders or writes objects (e.g. `preserve_order`), or the grant
    /// format changed: every stored approval would stop matching across a rolling
    /// upgrade. Change the constants only for a deliberate format change.
    #[test]
    fn fingerprint_is_pinned() {
        // R23: without `request.served` the grant has no `hooks` member, so this is the
        // pre-R23 pin, unchanged.
        let src = DESIGN_EXAMPLE.replace("\"request.served\" = 100", "");
        let m = Manifest::parse(&src).expect("parses");
        assert!(!m.hooks.contains_key("request.served"));
        let doc = json!({
            "plugin": m.plugin.name,
            "version": m.plugin.version.to_string(),
            "artifact": ARTIFACT,
            "grant": Grant::of(&m).to_json(),
        });
        assert_eq!(doc.to_string(), PINNED_DOC);
        assert_eq!(fingerprint(&m, ARTIFACT, None), PINNED_FINGERPRINT);
        let r = m
            .permission_report(ARTIFACT, None, &HostConfig::default())
            .expect("report");
        assert_eq!(r.fingerprint, PINNED_FINGERPRINT);

        // With it (the full design example): `"hooks":["request.served"]` in the grant.
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let doc = json!({
            "plugin": m.plugin.name,
            "version": m.plugin.version.to_string(),
            "artifact": ARTIFACT,
            "grant": Grant::of(&m).to_json(),
        });
        assert_eq!(doc.to_string(), PINNED_SERVED_DOC);
        assert_eq!(fingerprint(&m, ARTIFACT, None), PINNED_SERVED_FINGERPRINT);
    }

    /// M6-2 D11: a signer changes the fingerprint; the same signer gives the same one.
    #[test]
    fn signer_changes_fingerprint() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let host = HostConfig::default();
        let a = KeyId::parse("432051207B9BA065").unwrap();
        let b = KeyId::parse("0123456789ABCDEF").unwrap();
        let unsigned = m.permission_report(ARTIFACT, None, &host).unwrap();
        let signed_a = m
            .permission_report_signed(ARTIFACT, Some(&a), None, &host)
            .unwrap();
        let signed_b = m
            .permission_report_signed(ARTIFACT, Some(&b), None, &host)
            .unwrap();
        assert_ne!(unsigned.fingerprint, signed_a.fingerprint);
        assert_ne!(signed_a.fingerprint, signed_b.fingerprint);
        assert_eq!(signed_a.signer, Some(a));
        assert_eq!(unsigned.signer, None);
        assert_eq!(unsigned.fingerprint, PINNED_SERVED_FINGERPRINT);
    }

    #[test]
    fn same_signer_same_fingerprint() {
        let m = Manifest::parse(DESIGN_EXAMPLE).expect("parses");
        let host = HostConfig::default();
        let a = KeyId::parse("432051207B9BA065").unwrap();
        let one = m
            .permission_report_signed(ARTIFACT, Some(&a), None, &host)
            .unwrap();
        let two = m
            .permission_report_signed(ARTIFACT, Some(&a), None, &host)
            .unwrap();
        assert_eq!(one.fingerprint, two.fingerprint);
    }

    /// Checked independently: `printf '%s' "$PINNED_DOC" | shasum -a 256`.
    const PINNED_DOC: &str = concat!(
        r#"{"artifact":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","#,
        r#""grant":{"capabilities":{"content":{"meta":{"read":["seo.*"],"write":["seo.*"]},"#,
        r#""read":["post","page"]},"http":["api.indexnow.org"],"kv":true,"mail":"admins","#,
        r#""options":{"read":["site.title","site.url"]},"render":{"head":true,"text":true},"#,
        r#""routes":true},"limits":{"fuel":20000000,"memory":16777216},"#,
        r#""routes":[{"cross_origin":false,"rate_limit":"10/min"}],"v":1},"#,
        r#""plugin":"seo-pro","version":"1.4.0"}"#,
    );
    const PINNED_FINGERPRINT: &str =
        "6fb42abbeba14e3a6954e699435216c68faa0a0b09deb9ac6c7b9c117d8049c8";

    /// R23: [`PINNED_DOC`] with `"hooks":["request.served"]` in the grant; checked the
    /// same way.
    const PINNED_SERVED_DOC: &str = concat!(
        r#"{"artifact":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","#,
        r#""grant":{"capabilities":{"content":{"meta":{"read":["seo.*"],"write":["seo.*"]},"#,
        r#""read":["post","page"]},"http":["api.indexnow.org"],"kv":true,"mail":"admins","#,
        r#""options":{"read":["site.title","site.url"]},"render":{"head":true,"text":true},"#,
        r#""routes":true},"hooks":["request.served"],"#,
        r#""limits":{"fuel":20000000,"memory":16777216},"#,
        r#""routes":[{"cross_origin":false,"rate_limit":"10/min"}],"v":1},"#,
        r#""plugin":"seo-pro","version":"1.4.0"}"#,
    );
    const PINNED_SERVED_FINGERPRINT: &str =
        "d822dda9e9f1b508e273bbc176674cd6b23f28677dfb9c7e5fb1d44129e66fae";
}
