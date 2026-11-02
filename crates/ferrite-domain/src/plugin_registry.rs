//! The plugin registry index, schema 1 (M6-2, design 14 §5.1): one signed static JSON
//! file. Pure: types and validation only; the signature is checked by
//! `ferrite_plugin_host::trust::verify_index` before [`RegistryIndex::parse`] runs.
//! Everything here is untrusted input: every rule below refuses the whole index.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::Timestamp;
use crate::plugin_trust::{KeyId, MAX_SIGNATURE_BYTES, PublisherKey};
use crate::valid_plugin_name;

/// The largest index accepted (and fetched).
pub const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;
/// The largest manifest file an index entry may name (the upload cap).
pub const MAX_REGISTRY_MANIFEST_BYTES: u64 = 64 * 1024;
/// The largest component an index entry may name (the upload cap).
pub const MAX_REGISTRY_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;
/// At most this many plugins per index.
pub const MAX_REGISTRY_PLUGINS: usize = 1000;
/// At most this many versions per plugin.
pub const MAX_REGISTRY_VERSIONS: usize = 100;
/// The longest file URL.
pub const MAX_REGISTRY_URL: usize = 2048;
/// The schema this binary reads.
pub const REGISTRY_SCHEMA: u64 = 1;

const MAX_NAME_CHARS: usize = 100;
const MAX_DESCRIPTION_CHARS: usize = 500;
const MAX_REASON_CHARS: usize = 500;
const MAX_LICENSE_CHARS: usize = 100;
const MAX_WORLD_CHARS: usize = 200;

/// Why an index was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the registry index is invalid: {0}")]
pub struct RegistryError(pub String);

fn bad<T>(msg: impl Into<String>) -> Result<T, RegistryError> {
    Err(RegistryError(msg.into()))
}

/// A verified, validated index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndex {
    pub name: String,
    pub serial: u64,
    pub generated_at: Timestamp,
    pub publishers: Vec<RegistryPublisher>,
    pub plugins: Vec<RegistryPlugin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPublisher {
    /// `[a-z0-9][a-z0-9.-]{1,63}`.
    pub id: String,
    pub name: String,
    pub homepage: Option<String>,
    /// The publisher's current keys.
    pub keys: Vec<PublisherKey>,
    pub revoked_keys: Vec<RevokedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedKey {
    pub key: PublisherKey,
    pub revoked_at: Timestamp,
    pub reason: String,
    pub compromised: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPlugin {
    pub name: String,
    /// A publisher id of the index.
    pub publisher: String,
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub versions: Vec<RegistryVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryVersion {
    /// Semver, validated.
    pub version: String,
    pub world: String,
    pub published_at: Option<Timestamp>,
    pub manifest: RegistryFile,
    pub component: RegistryFile,
    /// The publisher's minisig over the signing payload (design 14 §2.1).
    pub signature: String,
    /// The reason, when yanked.
    pub yanked: Option<String>,
}

/// One file of a version: where it is, its sha256 (64 lowercase hex) and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryFile {
    /// An absolute `https://` URL, or a path relative to the index.
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
}

// The wire form. Unknown members are ignored (forward-compatible within schema 1, D14).
#[derive(Deserialize)]
struct RawIndex {
    schema: u64,
    name: String,
    serial: u64,
    generated_at: String,
    #[serde(default)]
    publishers: Vec<RawPublisher>,
    #[serde(default)]
    plugins: Vec<RawPlugin>,
}

#[derive(Deserialize)]
struct RawPublisher {
    id: String,
    name: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    revoked_keys: Vec<RawRevoked>,
}

#[derive(Deserialize)]
struct RawRevoked {
    key: String,
    revoked_at: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    compromised: bool,
}

#[derive(Deserialize)]
struct RawPlugin {
    name: String,
    publisher: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    versions: Vec<RawVersion>,
}

#[derive(Deserialize)]
struct RawVersion {
    version: String,
    world: String,
    #[serde(default)]
    published_at: Option<String>,
    manifest: RawFile,
    component: RawFile,
    signature: String,
    #[serde(default)]
    yanked: Option<String>,
}

#[derive(Deserialize)]
struct RawFile {
    url: String,
    sha256: String,
    bytes: u64,
}

fn text(field: &str, value: String, max: usize) -> Result<String, RegistryError> {
    if value.chars().count() > max {
        return bad(format!("{field} is longer than {max} characters"));
    }
    if value.chars().any(|c| c.is_control() && c != '\n') {
        return bad(format!("{field} holds control characters"));
    }
    Ok(value)
}

fn time(field: &str, value: &str) -> Result<Timestamp, RegistryError> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| Timestamp::from(t.with_timezone(&Utc)))
        .or_else(|_| bad(format!("{field} is not an RFC 3339 time")))
}

fn valid_publisher_id(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'-')
}

fn homepage(field: &str, value: Option<String>) -> Result<Option<String>, RegistryError> {
    match value {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => {
            if v.len() > MAX_REGISTRY_URL
                || !(v.starts_with("https://") || v.starts_with("http://"))
            {
                return bad(format!("{field} must be an http(s) URL"));
            }
            text(field, v, MAX_REGISTRY_URL).map(Some)
        }
    }
}

/// Checks a file URL: an absolute `https://` URL with a host, or a relative path with no
/// scheme, no `..` segment, no leading `/` and no backslash.
pub fn check_file_url(url: &str) -> Result<(), RegistryError> {
    if url.is_empty() || url.len() > MAX_REGISTRY_URL {
        return bad("a file url is empty or longer than 2048 bytes");
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad("a file url holds spaces or control characters");
    }
    if url.contains('\\') {
        return bad("a file url holds a backslash");
    }
    if let Some(rest) = url.strip_prefix("https://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() || host.contains('@') {
            return bad("a file url has no host, or holds credentials");
        }
        return Ok(());
    }
    if url.contains(':') && url.split('/').next().is_some_and(|s| s.contains(':')) {
        return bad("a file url is neither https:// nor a relative path");
    }
    if url.starts_with('/') {
        return bad("a relative file url starts with /");
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    if path
        .split('/')
        .any(|s| s == ".." || s == "." || s.is_empty())
    {
        return bad("a relative file url has a `..`, `.` or empty segment");
    }
    Ok(())
}

fn file(field: &str, raw: RawFile, max: u64) -> Result<RegistryFile, RegistryError> {
    check_file_url(&raw.url).map_err(|e| RegistryError(format!("{field}: {}", e.0)))?;
    if raw.sha256.len() != 64
        || !raw
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return bad(format!("{field}: sha256 must be 64 lowercase hex digits"));
    }
    if raw.bytes == 0 || raw.bytes > max {
        return bad(format!("{field}: bytes must be 1..={max}"));
    }
    Ok(RegistryFile {
        url: raw.url,
        sha256: raw.sha256,
        bytes: raw.bytes,
    })
}

fn key(field: &str, b64: &str) -> Result<PublisherKey, RegistryError> {
    PublisherKey::from_base64(b64).or_else(|e| bad(format!("{field}: {e}")))
}

/// Compares two version strings already validated as semver.
fn cmp_versions(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

impl RegistryIndex {
    /// Parses and validates an index's bytes (design 14 §5.1).
    pub fn parse(bytes: &[u8]) -> Result<RegistryIndex, RegistryError> {
        if bytes.len() as u64 > MAX_INDEX_BYTES {
            return bad(format!("larger than {MAX_INDEX_BYTES} bytes"));
        }
        // The schema first, so a schema-2 index gets the plain message.
        let probe: serde_json::Value =
            serde_json::from_slice(bytes).or_else(|e| bad(format!("not JSON: {e}")))?;
        match probe.get("schema").and_then(serde_json::Value::as_u64) {
            Some(REGISTRY_SCHEMA) => {}
            _ => return bad("this binary reads registry schema 1"),
        }
        let raw: RawIndex =
            serde_json::from_value(probe).or_else(|e| bad(format!("malformed: {e}")))?;
        debug_assert_eq!(raw.schema, REGISTRY_SCHEMA);
        if raw.serial == 0 || raw.serial > i64::MAX as u64 {
            return bad("serial must be a positive integer");
        }
        if raw.publishers.len() > MAX_REGISTRY_PLUGINS {
            return bad("too many publishers");
        }
        if raw.plugins.len() > MAX_REGISTRY_PLUGINS {
            return bad(format!("more than {MAX_REGISTRY_PLUGINS} plugins"));
        }
        let mut seen_keys: Vec<KeyId> = Vec::new();
        let mut publishers: Vec<RegistryPublisher> = Vec::with_capacity(raw.publishers.len());
        for p in raw.publishers {
            if !valid_publisher_id(&p.id) {
                return bad(format!(
                    "publisher id `{}` is not valid",
                    p.id.escape_debug()
                ));
            }
            if publishers.iter().any(|q| q.id == p.id) {
                return bad(format!("publisher `{}` is listed twice", p.id));
            }
            if p.keys.len() + p.revoked_keys.len() > 32 {
                return bad(format!("publisher `{}` lists too many keys", p.id));
            }
            let mut keys = Vec::new();
            for k in &p.keys {
                let k = key("publisher key", k)?;
                if seen_keys.contains(&k.key_id) {
                    return bad(format!("key {} is listed twice", k.key_id));
                }
                seen_keys.push(k.key_id);
                keys.push(k);
            }
            let mut revoked_keys = Vec::new();
            for r in p.revoked_keys {
                let k = key("revoked key", &r.key)?;
                if seen_keys.contains(&k.key_id) {
                    return bad(format!("key {} is listed twice", k.key_id));
                }
                seen_keys.push(k.key_id);
                revoked_keys.push(RevokedKey {
                    key: k,
                    revoked_at: time("revoked_at", &r.revoked_at)?,
                    reason: text("reason", r.reason, MAX_REASON_CHARS)?,
                    compromised: r.compromised,
                });
            }
            publishers.push(RegistryPublisher {
                name: text("publisher name", p.name, MAX_NAME_CHARS)?,
                homepage: homepage("publisher homepage", p.homepage)?,
                id: p.id,
                keys,
                revoked_keys,
            });
        }
        let mut plugins: Vec<RegistryPlugin> = Vec::with_capacity(raw.plugins.len());
        for p in raw.plugins {
            if !valid_plugin_name(&p.name) {
                return bad(format!(
                    "plugin name `{}` is not valid",
                    p.name.escape_debug()
                ));
            }
            if plugins.iter().any(|q| q.name == p.name) {
                return bad(format!("plugin `{}` is listed twice", p.name));
            }
            if !publishers.iter().any(|q| q.id == p.publisher) {
                return bad(format!(
                    "plugin `{}` names an unknown publisher `{}`",
                    p.name,
                    p.publisher.escape_debug()
                ));
            }
            if p.versions.len() > MAX_REGISTRY_VERSIONS {
                return bad(format!(
                    "plugin `{}` lists more than {MAX_REGISTRY_VERSIONS} versions",
                    p.name
                ));
            }
            let mut versions: Vec<RegistryVersion> = Vec::with_capacity(p.versions.len());
            for v in p.versions {
                let parsed = semver::Version::parse(&v.version).or_else(|_| {
                    bad(format!(
                        "plugin `{}`: version `{}` is not semver",
                        p.name,
                        v.version.escape_debug()
                    ))
                })?;
                if versions
                    .iter()
                    .any(|w| semver::Version::parse(&w.version).ok().as_ref() == Some(&parsed))
                {
                    return bad(format!(
                        "plugin `{}`: version {} is listed twice",
                        p.name, v.version
                    ));
                }
                if v.signature.len() > MAX_SIGNATURE_BYTES {
                    return bad(format!(
                        "plugin `{}`: the signature is larger than {MAX_SIGNATURE_BYTES} bytes",
                        p.name
                    ));
                }
                versions.push(RegistryVersion {
                    world: text("world", v.world, MAX_WORLD_CHARS)?,
                    published_at: v
                        .published_at
                        .as_deref()
                        .map(|t| time("published_at", t))
                        .transpose()?,
                    manifest: file("manifest", v.manifest, MAX_REGISTRY_MANIFEST_BYTES)?,
                    component: file("component", v.component, MAX_REGISTRY_COMPONENT_BYTES)?,
                    signature: v.signature,
                    yanked: v
                        .yanked
                        .map(|y| text("yanked", y, MAX_REASON_CHARS))
                        .transpose()?,
                    version: parsed.to_string(),
                });
            }
            plugins.push(RegistryPlugin {
                description: text("description", p.description, MAX_DESCRIPTION_CHARS)?,
                homepage: homepage("plugin homepage", p.homepage)?,
                license: p
                    .license
                    .map(|l| text("license", l, MAX_LICENSE_CHARS))
                    .transpose()?,
                name: p.name,
                publisher: p.publisher,
                versions,
            });
        }
        Ok(RegistryIndex {
            name: text("name", raw.name, MAX_NAME_CHARS)?,
            serial: raw.serial,
            generated_at: time("generated_at", &raw.generated_at)?,
            publishers,
            plugins,
        })
    }

    /// The publisher a current (not revoked) key belongs to.
    pub fn publisher_of_key(&self, id: &KeyId) -> Option<&RegistryPublisher> {
        self.publishers
            .iter()
            .find(|p| p.keys.iter().any(|k| k.key_id == *id))
    }

    /// The revocation of a key, with its publisher.
    pub fn revocation(&self, id: &KeyId) -> Option<(&RegistryPublisher, &RevokedKey)> {
        self.publishers.iter().find_map(|p| {
            p.revoked_keys
                .iter()
                .find(|r| r.key.key_id == *id)
                .map(|r| (p, r))
        })
    }

    pub fn publisher(&self, id: &str) -> Option<&RegistryPublisher> {
        self.publishers.iter().find(|p| p.id == id)
    }

    pub fn plugin(&self, name: &str) -> Option<&RegistryPlugin> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// The newest version of `name` for `world`, skipping yanked ones unless asked.
    pub fn newest(
        &self,
        name: &str,
        world: &str,
        include_yanked: bool,
    ) -> Option<&RegistryVersion> {
        self.plugin(name)?
            .versions
            .iter()
            .filter(|v| v.world == world && (include_yanked || v.yanked.is_none()))
            .max_by(|a, b| cmp_versions(&a.version, &b.version))
    }

    /// One version of `name`, by its semver text.
    pub fn version(&self, name: &str, version: &str) -> Option<&RegistryVersion> {
        let want = semver::Version::parse(version).ok()?;
        self.plugin(name)?
            .versions
            .iter()
            .find(|v| semver::Version::parse(&v.version).ok().as_ref() == Some(&want))
    }

    /// The plugins whose name, description or publisher name contains `q`
    /// (case-insensitive), at most `limit`, by name. A blank `q` matches every plugin.
    pub fn search(&self, q: &str, limit: usize) -> Vec<&RegistryPlugin> {
        let q = q.trim().to_lowercase();
        let mut out: Vec<&RegistryPlugin> = self
            .plugins
            .iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_lowercase().contains(&q)
                    || p.description.to_lowercase().contains(&q)
                    || self
                        .publisher(&p.publisher)
                        .is_some_and(|pb| pb.name.to_lowercase().contains(&q))
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.truncate(limit);
        out
    }
}

/// Whether `a` is a newer semver than `b` (false when either does not parse).
pub fn is_newer(a: &str, b: &str) -> bool {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;

    pub(crate) fn key_b64(n: u64) -> String {
        use base64::Engine;
        let mut bin = b"Ed".to_vec();
        bin.extend_from_slice(&n.to_le_bytes());
        bin.extend_from_slice(&[7u8; 32]);
        base64::engine::general_purpose::STANDARD.encode(bin)
    }

    fn version(v: &str) -> Value {
        json!({
            "version": v,
            "world": "ferrite:plugin/plugin@0.1.0",
            "published_at": "2026-11-20T10:00:00Z",
            "manifest": {"url": format!("seo/{v}/ferrite-plugin.toml"), "sha256": "a".repeat(64), "bytes": 100},
            "component": {"url": format!("https://github.com/x/releases/download/v{v}/seo.wasm"), "sha256": "b".repeat(64), "bytes": 1000},
            "signature": "untrusted comment: x\nRUQ\ntrusted comment: t\nX\n",
            "yanked": null
        })
    }

    pub(crate) fn sample() -> Value {
        json!({
            "schema": 1,
            "name": "FerriteCMS plugins",
            "serial": 3,
            "generated_at": "2026-11-20T10:00:00Z",
            "publishers": [{
                "id": "ferritecms.dev",
                "name": "FerriteCMS (Softberries)",
                "homepage": "https://softberries.github.io/ferritecms/",
                "keys": [key_b64(1)],
                "revoked_keys": [{"key": key_b64(2), "revoked_at": "2026-11-19T00:00:00Z", "reason": "rotated", "compromised": false}]
            }],
            "plugins": [{
                "name": "seo",
                "publisher": "ferritecms.dev",
                "description": "Meta tags, Open Graph and JSON-LD",
                "homepage": "https://softberries.github.io/ferritecms/guides/plugins/",
                "license": "AGPL-3.0-only",
                "versions": [version("0.3.0"), version("0.2.0")]
            }]
        })
    }

    fn parse(v: &Value) -> Result<RegistryIndex, RegistryError> {
        RegistryIndex::parse(v.to_string().as_bytes())
    }

    fn refused(v: Value, needle: &str) {
        let err = parse(&v).unwrap_err();
        assert!(err.0.contains(needle), "{err} (wanted {needle})");
    }

    #[test]
    fn sample_parses() {
        let i = parse(&sample()).unwrap();
        assert_eq!(i.serial, 3);
        assert_eq!(i.plugins[0].versions.len(), 2);
        let k1 = PublisherKey::from_base64(&key_b64(1)).unwrap().key_id;
        let k2 = PublisherKey::from_base64(&key_b64(2)).unwrap().key_id;
        assert_eq!(i.publisher_of_key(&k1).unwrap().id, "ferritecms.dev");
        assert!(i.publisher_of_key(&k2).is_none());
        assert_eq!(i.revocation(&k2).unwrap().1.reason, "rotated");
        assert_eq!(i.version("seo", "0.2.0").unwrap().version, "0.2.0");
    }

    #[test]
    fn schema_2_refused() {
        let mut v = sample();
        v["schema"] = json!(2);
        refused(v, "schema 1");
        refused(json!({"name": "x"}), "schema 1");
    }

    #[test]
    fn unknown_members_ignored() {
        let mut v = sample();
        v["future"] = json!({"x": 1});
        v["plugins"][0]["stars"] = json!(5);
        v["plugins"][0]["versions"][0]["extra"] = json!(true);
        assert!(parse(&v).is_ok());
    }

    #[test]
    fn duplicate_plugin_refused() {
        let mut v = sample();
        let p = v["plugins"][0].clone();
        v["plugins"].as_array_mut().unwrap().push(p);
        refused(v, "listed twice");
    }

    #[test]
    fn duplicate_version_refused() {
        let mut v = sample();
        v["plugins"][0]["versions"][1] = version("0.3.0");
        refused(v, "listed twice");
    }

    #[test]
    fn bad_semver_refused() {
        let mut v = sample();
        v["plugins"][0]["versions"][0]["version"] = json!("1.0");
        refused(v, "not semver");
    }

    #[test]
    fn unknown_publisher_refused() {
        let mut v = sample();
        v["plugins"][0]["publisher"] = json!("acme.dev");
        refused(v, "unknown publisher");
        let mut v = sample();
        v["publishers"][0]["id"] = json!("Bad_Id");
        refused(v, "not valid");
    }

    #[test]
    fn key_under_two_publishers_refused() {
        let mut v = sample();
        let mut p = v["publishers"][0].clone();
        p["id"] = json!("other.dev");
        p["revoked_keys"] = json!([]);
        v["publishers"].as_array_mut().unwrap().push(p);
        refused(v, "listed twice");
        // A current key also listed as revoked.
        let mut v = sample();
        v["publishers"][0]["revoked_keys"][0]["key"] = json!(key_b64(1));
        refused(v, "listed twice");
    }

    #[test]
    fn http_url_refused() {
        let mut v = sample();
        v["plugins"][0]["versions"][0]["component"]["url"] = json!("http://example.dev/x.wasm");
        refused(v, "https");
        let mut v = sample();
        v["plugins"][0]["versions"][0]["component"]["url"] = json!("file:///etc/passwd");
        refused(v, "https");
        let mut v = sample();
        v["plugins"][0]["versions"][0]["component"]["url"] = json!("https://u:p@example.dev/x");
        refused(v, "credentials");
    }

    #[test]
    fn dotdot_relative_refused() {
        for url in ["../x.wasm", "a/../../x.wasm", "a/./x.wasm", "a//x.wasm"] {
            let mut v = sample();
            v["plugins"][0]["versions"][0]["manifest"]["url"] = json!(url);
            refused(v, "segment");
        }
    }

    #[test]
    fn leading_slash_and_backslash_refused() {
        let mut v = sample();
        v["plugins"][0]["versions"][0]["manifest"]["url"] = json!("/etc/passwd");
        refused(v, "starts with /");
        let mut v = sample();
        v["plugins"][0]["versions"][0]["manifest"]["url"] = json!("a\\..\\b");
        refused(v, "backslash");
    }

    #[test]
    fn uppercase_sha_refused() {
        let mut v = sample();
        v["plugins"][0]["versions"][0]["manifest"]["sha256"] = json!("A".repeat(64));
        refused(v, "lowercase hex");
        let mut v = sample();
        v["plugins"][0]["versions"][0]["manifest"]["sha256"] = json!("a".repeat(63));
        refused(v, "lowercase hex");
    }

    #[test]
    fn component_over_32mib_refused() {
        let mut v = sample();
        v["plugins"][0]["versions"][0]["component"]["bytes"] = json!(32 * 1024 * 1024 + 1);
        refused(v, "bytes");
        let mut v = sample();
        v["plugins"][0]["versions"][0]["manifest"]["bytes"] = json!(64 * 1024 + 1);
        refused(v, "bytes");
    }

    #[test]
    fn too_many_plugins_refused() {
        let mut v = sample();
        let base = v["plugins"][0].clone();
        let list = v["plugins"].as_array_mut().unwrap();
        list.clear();
        for i in 0..=MAX_REGISTRY_PLUGINS {
            let mut p = base.clone();
            p["name"] = json!(format!("p{i}"));
            p["versions"] = json!([]);
            list.push(p);
        }
        refused(v, "more than 1000 plugins");
        let mut v = sample();
        let vs: Vec<Value> = (0..=MAX_REGISTRY_VERSIONS)
            .map(|i| version(&format!("0.0.{i}")))
            .collect();
        v["plugins"][0]["versions"] = json!(vs);
        refused(v, "more than 100 versions");
    }

    #[test]
    fn long_description_refused() {
        let mut v = sample();
        v["plugins"][0]["description"] = json!("x".repeat(501));
        refused(v, "longer than 500");
        let mut v = sample();
        v["publishers"][0]["name"] = json!("a\u{1b}[31mred");
        refused(v, "control");
    }

    #[test]
    fn oversize_index_refused() {
        let big = vec![b' '; MAX_INDEX_BYTES as usize + 1];
        let err = RegistryIndex::parse(&big).unwrap_err();
        assert!(err.0.contains("larger than"), "{err}");
        assert!(RegistryIndex::parse(b"not json").is_err());
        let mut v = sample();
        v["serial"] = json!(0);
        refused(v, "serial");
    }

    #[test]
    fn newest_skips_yanked_and_other_worlds() {
        let mut v = sample();
        v["plugins"][0]["versions"]
            .as_array_mut()
            .unwrap()
            .push(version("0.10.0"));
        v["plugins"][0]["versions"][2]["yanked"] = json!("broken");
        let mut other = version("0.9.0");
        other["world"] = json!("ferrite:plugin/plugin@0.2.0");
        v["plugins"][0]["versions"]
            .as_array_mut()
            .unwrap()
            .push(other);
        let i = parse(&v).unwrap();
        let w = "ferrite:plugin/plugin@0.1.0";
        assert_eq!(i.newest("seo", w, false).unwrap().version, "0.3.0");
        assert_eq!(i.newest("seo", w, true).unwrap().version, "0.10.0");
        assert_eq!(
            i.newest("seo", "ferrite:plugin/plugin@0.2.0", false)
                .unwrap()
                .version,
            "0.9.0"
        );
        assert!(i.newest("nope", w, true).is_none());
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.3.0", "0.3.0"));
    }

    #[test]
    fn search_matches_name_description_publisher_case_insensitively() {
        let mut v = sample();
        let mut p = v["plugins"][0].clone();
        p["name"] = json!("analytics");
        p["description"] = json!("Page views");
        v["plugins"].as_array_mut().unwrap().push(p);
        let i = parse(&v).unwrap();
        let names = |q: &str| {
            i.search(q, 50)
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("SEO"), ["seo"]);
        assert_eq!(names("open graph"), ["seo"]);
        assert_eq!(names("softberries"), ["analytics", "seo"]);
        assert_eq!(names(""), ["analytics", "seo"]);
        assert!(names("zzz").is_empty());
        assert_eq!(i.search("", 1).len(), 1);
    }
}
