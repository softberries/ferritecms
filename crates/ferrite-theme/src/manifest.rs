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
    /// An `http://` or `https://` URL (checked by the parser).
    pub homepage: Option<String>,
    pub description: Option<String>,
    /// Menu locations: name -> label.
    pub menus: BTreeMap<String, String>,
    /// Image variant sizes the theme wants (03 §4).
    pub images: BTreeMap<String, ImageSize>,
    /// Declarative settings, rendered by the admin.
    pub settings: BTreeMap<String, SettingSpec>,
    /// Translations of the theme's own texts (`[strings.<tag>]`, 13 §5.1): locale tag
    /// -> source text -> translation; what `t(text)` looks up.
    pub strings: BTreeMap<String, BTreeMap<String, String>>,
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

impl SettingKind {
    /// The shared setting type (`ferrite_domain::SettingType`), whose value rules this
    /// crate applies.
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
        }
    }
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
    #[serde(default)]
    strings: BTreeMap<String, BTreeMap<String, String>>,
}

/// Most `[strings.<tag>]` tables a theme may have.
pub const MAX_STRING_TABLES: usize = 32;
/// Most strings per table.
pub const MAX_STRINGS: usize = 500;
/// Longest translated string, in characters.
pub const MAX_STRING_CHARS: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    name: String,
    version: semver::Version,
    author: Option<String>,
    license: Option<String>,
    homepage: Option<String>,
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
    if let Some(url) = &raw.theme.homepage
        && !ferrite_domain::is_web_url(url)
    {
        problems.push(format!(
            "theme.toml: theme.homepage {url:?} must be an http:// or https:// URL \
             (at most {} bytes, no spaces)",
            ferrite_domain::MAX_WEB_URL
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
    if raw.strings.len() > MAX_STRING_TABLES {
        problems.push(format!(
            "theme.toml: at most {MAX_STRING_TABLES} [strings.<locale>] tables"
        ));
    }
    let mut strings = BTreeMap::new();
    for (tag, table) in raw.strings {
        let path = format!("theme.toml: strings.{tag}");
        let Ok(canonical) = ferrite_domain::locale::parse_tag(&tag) else {
            problems.push(format!("{path}: not a locale tag"));
            continue;
        };
        if table.len() > MAX_STRINGS {
            problems.push(format!("{path}: at most {MAX_STRINGS} strings"));
            continue;
        }
        if table.iter().any(|(k, v)| {
            k.chars().count() > MAX_STRING_CHARS || v.chars().count() > MAX_STRING_CHARS
        }) {
            problems.push(format!(
                "{path}: a string has more than {MAX_STRING_CHARS} characters"
            ));
            continue;
        }
        strings.insert(canonical, table);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(ThemeManifest {
        strings,
        name: raw.theme.name,
        version: raw.theme.version,
        author: raw.theme.author,
        license: raw.theme.license,
        homepage: raw.theme.homepage,
        description: raw.theme.description,
        menus: raw.menus,
        images: raw.images,
        settings,
    })
}

impl ThemeManifest {
    /// The strings `t(text)` uses in `locale` (13 §5.1): its language subtag's table
    /// (`pt-BR` → `pt`) with the exact tag's entries over it. A text missing from both is
    /// printed as written.
    pub fn strings_for(&self, locale: &str) -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        let language = ferrite_domain::locale::language(locale);
        for tag in [language, locale] {
            if let Some(table) = self.strings.get(tag) {
                out.extend(table.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        out
    }

    /// The width of the widest `[images]` size, `None` when the theme declares none
    /// (or only zero widths): the largest display size of inline body images (06 §4).
    pub fn largest_image_width(&self) -> Option<u32> {
        self.images
            .values()
            .map(|s| s.width)
            .filter(|w| *w > 0)
            .max()
    }

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
    ferrite_domain::empty_setting_value(spec.kind.setting_type(), &spec.options)
}

/// The shared value rules (`ferrite_domain::check_setting_value`), the same the admin
/// API validates with: text lengths, select options, colour shape, media ids, no NUL.
fn valid_value(kind: SettingKind, options: &[String], v: &serde_json::Value) -> bool {
    ferrite_domain::check_setting_value(kind.setting_type(), options, v).is_ok()
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

    const META: &str = "[theme]\nname = \"t\"\nversion = \"0.1.0\"\n";

    #[test]
    fn t_falls_back_tag_language_text() {
        let m = parse(&format!(
            "{META}[strings.pt]\n\"Read more\" = \"Leia mais\"\n\"Home\" = \"Início\"\n\
             [strings.pt_br]\n\"Home\" = \"Página inicial\"\n"
        ))
        .unwrap();
        let br = m.strings_for("pt-BR");
        assert_eq!(br.get("Home").map(String::as_str), Some("Página inicial"));
        assert_eq!(br.get("Read more").map(String::as_str), Some("Leia mais"));
        assert!(m.strings_for("de").is_empty());
        assert!(m.strings.contains_key("pt-BR"), "tags are stored canonical");
    }

    #[test]
    fn strings_caps_refused() {
        let tables: String = (0..33)
            .map(|i| format!("[strings.a{}]\n", (b'a' + (i % 26) as u8) as char))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let many: String = (0..501).map(|i| format!("\"s{i}\" = \"x\"\n")).collect();
        for bad in [
            format!("{META}[strings.\"not a tag\"]\n\"a\" = \"b\"\n"),
            format!("{META}[strings.de]\n{many}"),
            format!("{META}[strings.de]\n\"a\" = \"{}\"\n", "x".repeat(1001)),
        ] {
            assert!(parse(&bad).is_err(), "{}", &bad[..bad.len().min(80)]);
        }
        // 26 distinct tables are fine (the 33-table case needs more tags than letters).
        assert!(parse(&format!("{META}{tables}")).is_ok());
    }

    const OK: &str = r##"
[theme]
name = "blog"
version = "0.1.0"
author = "FerriteCMS"
license = "MIT"
homepage = "https://example.com/blog"

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
        assert_eq!(m.homepage.as_deref(), Some("https://example.com/blog"));
        assert_eq!(m.menus["primary"], "Primary navigation");
        assert_eq!(
            m.images["card"],
            ImageSize {
                width: 640,
                height: Some(360),
                crop: true
            }
        );
        assert_eq!(m.largest_image_width(), Some(640));
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

    /// Review m4: templates and the admin API apply the same rules
    /// (`ferrite_domain::check_setting_value`), text lengths included.
    #[test]
    fn text_lengths_match_the_admin_rules() {
        let m = parse(
            "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[settings]\n\
             tagline = { type = \"text\", default = \"short\" }\n\
             body = { type = \"textarea\" }\n",
        )
        .unwrap();
        let stored = serde_json::json!({
            "tagline": "a".repeat(1_001),
            "body": "b\u{0}",
        });
        let v = m.settings_values(stored.as_object().unwrap());
        assert_eq!(v["tagline"], "short");
        assert_eq!(v["body"], "");
        let ok = serde_json::json!({ "tagline": "a".repeat(1_000) });
        assert_eq!(
            m.settings_values(ok.as_object().unwrap())["tagline"],
            "a".repeat(1_000)
        );
        let long = format!(
            "[theme]\nname = \"x\"\nversion = \"0.1.0\"\n[settings]\nt = {{ type = \"text\", default = \"{}\" }}\n",
            "a".repeat(1_001)
        );
        assert!(parse(&long).unwrap_err()[0].contains("settings.t.default"));
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
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\nhomepage = \"javascript:alert(1)\"\n",
                "theme.homepage",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\nhomepage = \"ftp://example.com\"\n",
                "theme.homepage",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\nhomepage = \"https://\"\n",
                "theme.homepage",
            ),
            (
                "[theme]\nname = \"x\"\nversion = \"0.1.0\"\nhomepage = \"https://a b\"\n",
                "theme.homepage",
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
