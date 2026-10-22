//! `theme.toml` (06 §1).
//!
//! Setting types mirror the plugin manifest's (`ferrite-plugin-host` `SettingKind`)
//! minus `secret`: every theme setting value is visible to templates, so a secret
//! has no place there. The kinds are redeclared here rather than imported so this
//! crate does not pull in wasmtime.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Parsed and validated `theme.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeManifest {
    pub name: String,
    pub version: semver::Version,
    pub author: Option<String>,
    pub license: Option<String>,
    pub description: Option<String>,
    /// Menu locations: name -> label.
    pub menus: BTreeMap<String, String>,
    /// Image variant sizes the theme wants (03 §4).
    pub images: BTreeMap<String, ImageSize>,
    /// Declarative settings, rendered by the admin.
    pub settings: BTreeMap<String, SettingSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSize {
    pub width: u32,
    pub height: Option<u32>,
    #[serde(default)]
    pub crop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    Text,
    Textarea,
    Bool,
    Number,
    Select,
    /// `#rgb` or `#rrggbb`.
    Color,
    /// A media item id (UUID string); no default.
    Media,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    pub kind: SettingKind,
    /// Defaults to the key.
    pub label: String,
    pub default: Option<serde_json::Value>,
    /// Choices; non-empty for `select`, empty otherwise.
    pub options: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    theme: RawMeta,
    #[serde(default)]
    menus: BTreeMap<String, String>,
    #[serde(default)]
    images: BTreeMap<String, ImageSize>,
    #[serde(default)]
    settings: BTreeMap<String, RawSetting>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    name: String,
    version: semver::Version,
    author: Option<String>,
    license: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSetting {
    #[serde(rename = "type")]
    kind: SettingKind,
    label: Option<String>,
    default: Option<toml::Value>,
    #[serde(default)]
    options: Vec<String>,
}

const MAX_NAME: usize = 64;

/// Parses `theme.toml`; every problem found is returned.
pub(crate) fn parse(src: &str) -> Result<ThemeManifest, Vec<String>> {
    let raw: Raw = toml::from_str(src).map_err(|e| vec![format!("theme.toml: {e}")])?;
    let mut problems = Vec::new();
    if !is_name(&raw.theme.name, false) {
        problems.push(format!(
            "theme.toml: theme.name {:?} must be 1-{MAX_NAME} chars of [a-z0-9-]",
            raw.theme.name
        ));
    }
    for name in raw.menus.keys() {
        if !is_name(name, false) {
            problems.push(format!("theme.toml: menu name {name:?} must be [a-z0-9-]"));
        }
    }
    for name in raw.images.keys() {
        if !is_name(name, true) {
            problems.push(format!(
                "theme.toml: image size name {name:?} must be [a-z0-9_-]"
            ));
        }
    }
    let mut settings = BTreeMap::new();
    for (key, s) in raw.settings {
        let path = format!("theme.toml: settings.{key}");
        if !is_name(&key, true) {
            problems.push(format!("{path}: key must be [a-z0-9_-]"));
            continue;
        }
        let default = match s.default.map(serde_json::to_value).transpose() {
            Ok(d) => d,
            Err(e) => {
                problems.push(format!("{path}.default: {e}"));
                continue;
            }
        };
        if s.kind == SettingKind::Select && s.options.is_empty() {
            problems.push(format!("{path}.options: `select` needs a non-empty list"));
            continue;
        }
        if s.kind != SettingKind::Select && !s.options.is_empty() {
            problems.push(format!("{path}.options: only `select` takes options"));
            continue;
        }
        if s.kind == SettingKind::Media && default.is_some() {
            problems.push(format!("{path}.default: `media` settings have no default"));
            continue;
        }
        if let Some(d) = &default
            && !valid_value(s.kind, &s.options, d)
        {
            problems.push(format!("{path}.default: not a valid {:?} value", s.kind));
            continue;
        }
        settings.insert(
            key.clone(),
            SettingSpec {
                kind: s.kind,
                label: s.label.unwrap_or(key),
                default,
                options: s.options,
            },
        );
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(ThemeManifest {
        name: raw.theme.name,
        version: raw.theme.version,
        author: raw.theme.author,
        license: raw.theme.license,
        description: raw.theme.description,
        menus: raw.menus,
        images: raw.images,
        settings,
    })
}

impl ThemeManifest {
    /// The value of every declared setting: the stored value when it is valid for the
    /// setting's type, else the default, else the type's empty value (`""`, `false`,
    /// `0`, the first option, `null` for media). Undeclared stored keys are dropped.
    pub fn settings_values(
        &self,
        stored: &serde_json::Map<String, serde_json::Value>,
    ) -> BTreeMap<String, serde_json::Value> {
        self.settings
            .iter()
            .map(|(key, spec)| {
                let value = stored
                    .get(key)
                    .filter(|v| valid_value(spec.kind, &spec.options, v))
                    .or(spec.default.as_ref())
                    .cloned()
                    .unwrap_or_else(|| empty_value(spec));
                (key.clone(), value)
            })
            .collect()
    }
}

fn empty_value(spec: &SettingSpec) -> serde_json::Value {
    use serde_json::Value;
    match spec.kind {
        SettingKind::Text | SettingKind::Textarea | SettingKind::Color => Value::from(""),
        SettingKind::Bool => Value::from(false),
        SettingKind::Number => Value::from(0),
        SettingKind::Select => spec
            .options
            .first()
            .map_or(Value::Null, |o| Value::from(o.as_str())),
        SettingKind::Media => Value::Null,
    }
}

fn valid_value(kind: SettingKind, options: &[String], v: &serde_json::Value) -> bool {
    match kind {
        SettingKind::Text | SettingKind::Textarea => v.is_string(),
        SettingKind::Bool => v.is_boolean(),
        SettingKind::Number => v.is_number(),
        SettingKind::Select => v.as_str().is_some_and(|s| options.iter().any(|o| o == s)),
        SettingKind::Color => v.as_str().is_some_and(is_color),
        SettingKind::Media => v
            .as_str()
            .is_some_and(|s| s.parse::<ferrite_domain::ContentId>().is_ok()),
    }
}

/// `#rgb` or `#rrggbb`: strict, because colours land in CSS where HTML escaping does
/// not help.
fn is_color(s: &str) -> bool {
    s.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_name(s: &str, underscore: bool) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || (underscore && b == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r##"
[theme]
name = "blog"
version = "0.1.0"
author = "FerriteCMS"
license = "MIT"

[menus]
primary = "Primary navigation"

[images]
card = { width = 640, height = 360, crop = true }

[settings]
accent_color = { type = "color", default = "#c2410c", label = "Accent colour" }
show_author = { type = "bool", default = true }
layout = { type = "select", options = ["wide", "narrow"], default = "wide" }
logo = { type = "media", label = "Logo" }
"##;

    #[test]
    fn parses_the_design_example() {
        let m = parse(OK).unwrap();
        assert_eq!(m.name, "blog");
        assert_eq!(m.version, semver::Version::new(0, 1, 0));
        assert_eq!(m.menus["primary"], "Primary navigation");
        assert_eq!(
            m.images["card"],
            ImageSize {
                width: 640,
                height: Some(360),
                crop: true
            }
        );
        assert_eq!(m.settings["show_author"].label, "show_author");
        assert_eq!(m.settings["accent_color"].kind, SettingKind::Color);
    }

    #[test]
    fn settings_values_fall_back_to_defaults() {
        let m = parse(OK).unwrap();
        let stored = serde_json::json!({
            "accent_color": "red; background: url(x)",
            "show_author": false,
            "layout": "nope",
            "logo": "not-a-uuid",
            "undeclared": 1,
        });
        let v = m.settings_values(stored.as_object().unwrap());
        assert_eq!(v["accent_color"], "#c2410c");
        assert_eq!(v["show_author"], false);
        assert_eq!(v["layout"], "wide");
        assert_eq!(v["logo"], serde_json::Value::Null);
        assert!(!v.contains_key("undeclared"));
    }

    #[test]
    fn rejects_bad_manifests() {
        for (src, needle) in [
            ("[theme]\nname = \"x\"\n", "version"),
            ("[theme]\nname = \"X\"\nversion = \"0.1.0\"\n", "theme.name"),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\nextra = 1\n",
                "unknown field",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[other]\n",
                "unknown field",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[settings]\na = { type = \"secret\" }\n",
                "unknown variant",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[settings]\na = { type = \"bool\", default = \"yes\" }\n",
                "settings.a.default",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[settings]\na = { type = \"select\" }\n",
                "settings.a.options",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[settings]\na = { type = \"color\", default = \"red\" }\n",
                "settings.a.default",
            ),
        ] {
            let err = parse(src).unwrap_err().join("\n");
            assert!(err.contains(needle), "{src:?}: {err}");
        }
    }
}
