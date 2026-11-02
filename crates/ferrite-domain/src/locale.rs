//! Locale tags and the site's locale list (design 13 §2.1–§2.2).
//!
//! A locale is a BCP 47 tag of the subset `language[-Script][-REGION]`: a language of
//! 2–3 ASCII letters, an optional script of 4 letters and an optional region of 2
//! letters or 3 digits. [`parse_tag`] reads it case-insensitively (`_` as `-`) and
//! returns the canonical case (`pt-BR`, `zh-Hant-TW`); extensions, variants and private
//! use are refused. The URL prefix of a locale is its tag lowercased ([`prefix`]).
//!
//! [`SiteLocales`] is the `site.locales` option: the default locale first, then up to
//! [`MAX_LOCALES`]` - 1` others, distinct by prefix.

use serde_json::{Value, json};

/// The option holding the site's locales (a JSON array, default first).
pub const LOCALES_OPTION: &str = "site.locales";
/// The option holding the per-locale site title and tagline.
pub const LOCALE_TEXTS_OPTION: &str = "site.locale_texts";
/// Most locales a site may list (D18).
pub const MAX_LOCALES: usize = 20;
/// Longest locale label, in characters.
pub const MAX_LABEL: usize = 50;
/// Longest tag accepted, in bytes.
pub const MAX_TAG_BYTES: usize = 35;

/// Why a tag or a locale list was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocaleError {
    #[error("{0:?} is not a locale tag (language[-Script][-REGION], e.g. de or pt-BR)")]
    Tag(String),
    #[error("{0}")]
    List(String),
}

/// `s` as a canonical tag of the strict subset, or [`LocaleError::Tag`].
pub fn parse_tag(s: &str) -> Result<String, LocaleError> {
    let bad = || LocaleError::Tag(s.chars().take(40).collect());
    if s.is_empty() || s.len() > MAX_TAG_BYTES || !s.is_ascii() {
        return Err(bad());
    }
    let normalized = s.replace('_', "-");
    let mut parts = normalized.split('-');
    let lang = parts.next().ok_or_else(bad)?;
    if !(2..=3).contains(&lang.len()) || !lang.bytes().all(|c| c.is_ascii_alphabetic()) {
        return Err(bad());
    }
    let mut out = lang.to_ascii_lowercase();
    let mut seen_script = false;
    let mut seen_region = false;
    for part in parts {
        let alpha = part.bytes().all(|c| c.is_ascii_alphabetic());
        let digit = part.bytes().all(|c| c.is_ascii_digit());
        if part.len() == 4 && alpha && !seen_script && !seen_region {
            seen_script = true;
            out.push('-');
            out.push_str(&part[..1].to_ascii_uppercase());
            out.push_str(&part[1..].to_ascii_lowercase());
        } else if ((part.len() == 2 && alpha) || (part.len() == 3 && digit)) && !seen_region {
            seen_region = true;
            out.push('-');
            out.push_str(&part.to_ascii_uppercase());
        } else {
            return Err(bad());
        }
    }
    Ok(out)
}

/// `locale` as a BCP 47 tag for `<html lang>`, leniently: a POSIX-style
/// `en_GB.UTF-8@euro` becomes `en-GB`; any character outside `[A-Za-z0-9-]` is dropped,
/// and an empty result is `en`. (The rule `<html lang>` always had.)
pub fn bcp47_loose(locale: &str) -> String {
    let base = locale.split(['.', '@']).next().unwrap_or_default();
    let tag: String = base
        .chars()
        .map(|c| if c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let tag = tag.trim_matches('-');
    if tag.is_empty() {
        "en".to_owned()
    } else {
        tag.to_owned()
    }
}

/// The URL prefix of `tag`: the tag lowercased (`pt-BR` → `pt-br`).
pub fn prefix(tag: &str) -> String {
    tag.to_ascii_lowercase()
}

/// The language subtag of `tag` (`pt-BR` → `pt`).
pub fn language(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

/// Native names of common languages, by language subtag.
const NATIVE: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("ar", "العربية"),
    ("az", "Azərbaycan"),
    ("be", "Беларуская"),
    ("bg", "Български"),
    ("bn", "বাংলা"),
    ("bs", "Bosanski"),
    ("ca", "Català"),
    ("cs", "Čeština"),
    ("cy", "Cymraeg"),
    ("da", "Dansk"),
    ("de", "Deutsch"),
    ("el", "Ελληνικά"),
    ("en", "English"),
    ("eo", "Esperanto"),
    ("es", "Español"),
    ("et", "Eesti"),
    ("eu", "Euskara"),
    ("fa", "فارسی"),
    ("fi", "Suomi"),
    ("fr", "Français"),
    ("ga", "Gaeilge"),
    ("gl", "Galego"),
    ("he", "עברית"),
    ("hi", "हिन्दी"),
    ("hr", "Hrvatski"),
    ("hu", "Magyar"),
    ("hy", "Հայերեն"),
    ("id", "Bahasa Indonesia"),
    ("is", "Íslenska"),
    ("it", "Italiano"),
    ("ja", "日本語"),
    ("ka", "ქართული"),
    ("kk", "Қазақ"),
    ("km", "ខ្មែរ"),
    ("ko", "한국어"),
    ("lt", "Lietuvių"),
    ("lv", "Latviešu"),
    ("mk", "Македонски"),
    ("ms", "Bahasa Melayu"),
    ("mt", "Malti"),
    ("nb", "Norsk bokmål"),
    ("nl", "Nederlands"),
    ("nn", "Norsk nynorsk"),
    ("no", "Norsk"),
    ("pl", "Polski"),
    ("pt", "Português"),
    ("ro", "Română"),
    ("ru", "Русский"),
    ("sk", "Slovenčina"),
    ("sl", "Slovenščina"),
    ("sq", "Shqip"),
    ("sr", "Српски"),
    ("sv", "Svenska"),
    ("sw", "Kiswahili"),
    ("ta", "தமிழ்"),
    ("th", "ไทย"),
    ("tr", "Türkçe"),
    ("uk", "Українська"),
    ("ur", "اردو"),
    ("uz", "Oʻzbek"),
    ("vi", "Tiếng Việt"),
    ("zh", "中文"),
];

/// The native name of `tag`'s language (`de` → `Deutsch`), if the built-in table has it.
pub fn native_name(tag: &str) -> Option<&'static str> {
    let lang = language(tag).to_ascii_lowercase();
    NATIVE.iter().find(|(l, _)| *l == lang).map(|(_, n)| *n)
}

/// Whether `c` may not appear in a label: control and bidi formatting characters.
fn forbidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}')
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// One locale of the site: its tag and the label the switcher shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleDef {
    pub tag: String,
    pub label: String,
}

impl LocaleDef {
    /// `tag` with its default label (the native name, else the tag).
    pub fn new(tag: impl Into<String>) -> Self {
        let tag = tag.into();
        let label = native_name(&tag).map_or_else(|| tag.clone(), str::to_owned);
        Self { tag, label }
    }

    /// The URL prefix of this locale.
    pub fn prefix(&self) -> String {
        prefix(&self.tag)
    }
}

/// The site's locales (`site.locales`): the default first, then the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteLocales {
    pub default: LocaleDef,
    pub others: Vec<LocaleDef>,
}

impl SiteLocales {
    /// A one-locale site in `tag` (taken as given: a single default may be a loose tag).
    pub fn single(tag: impl Into<String>) -> Self {
        Self {
            default: LocaleDef::new(tag),
            others: Vec::new(),
        }
    }

    /// The list a `site.locales` value describes: 1–[`MAX_LOCALES`] entries of `{ "tag",
    /// "label"? }`, tags strict (a single entry's tag may be loose, as `settle` writes
    /// it from an old `site.locale`), distinct by prefix, labels 1–[`MAX_LABEL`]
    /// characters without control or bidi characters (absent or empty: the default).
    pub fn from_option(value: &Value) -> Result<Self, LocaleError> {
        let list = |m: &str| LocaleError::List(m.to_owned());
        let entries = value
            .as_array()
            .ok_or_else(|| list("site.locales must be a list of {tag, label}"))?;
        if entries.is_empty() {
            return Err(list("site.locales needs at least one locale"));
        }
        if entries.len() > MAX_LOCALES {
            return Err(LocaleError::List(format!(
                "site.locales holds at most {MAX_LOCALES} locales"
            )));
        }
        let mut defs: Vec<LocaleDef> = Vec::with_capacity(entries.len());
        for entry in entries {
            let obj = entry
                .as_object()
                .ok_or_else(|| list("each locale must be an object {tag, label}"))?;
            if obj.keys().any(|k| k != "tag" && k != "label") {
                return Err(list("a locale has only the fields tag and label"));
            }
            let raw = obj
                .get("tag")
                .and_then(Value::as_str)
                .ok_or_else(|| list("each locale needs a tag"))?;
            let tag = if entries.len() == 1 {
                parse_tag(raw).or_else(|e| loose_single(raw).ok_or(e))?
            } else {
                parse_tag(raw)?
            };
            let mut def = LocaleDef::new(tag);
            match obj.get("label") {
                None | Some(Value::Null) => {}
                Some(Value::String(l)) if l.trim().is_empty() => {}
                Some(Value::String(l)) => {
                    if l.chars().count() > MAX_LABEL {
                        return Err(LocaleError::List(format!(
                            "a locale label has at most {MAX_LABEL} characters"
                        )));
                    }
                    if l.chars().any(forbidden) {
                        return Err(list(
                            "a locale label may not hold control or bidi characters",
                        ));
                    }
                    def.label = l.trim().to_owned();
                }
                Some(_) => return Err(list("a locale label must be text")),
            }
            if defs.iter().any(|d| d.prefix() == def.prefix()) {
                return Err(LocaleError::List(format!(
                    "the locale {} is listed twice",
                    def.tag
                )));
            }
            defs.push(def);
        }
        let mut defs = defs.into_iter();
        let default = defs.next().ok_or_else(|| list("site.locales is empty"))?;
        Ok(Self {
            default,
            others: defs.collect(),
        })
    }

    /// The `site.locales` value of this list.
    pub fn to_option(&self) -> Value {
        Value::Array(
            self.all()
                .map(|d| json!({ "tag": d.tag, "label": d.label }))
                .collect(),
        )
    }

    /// Whether the site has more than one locale.
    pub fn is_multilingual(&self) -> bool {
        !self.others.is_empty()
    }

    /// Every locale, the default first.
    pub fn all(&self) -> impl Iterator<Item = &LocaleDef> {
        std::iter::once(&self.default).chain(self.others.iter())
    }

    /// The locale with exactly `tag`.
    pub fn get(&self, tag: &str) -> Option<&LocaleDef> {
        self.all().find(|d| d.tag == tag)
    }

    /// Whether `tag` is one of the site's locales (exact, canonical case).
    pub fn contains(&self, tag: &str) -> bool {
        self.get(tag).is_some()
    }

    /// The non-default locale whose prefix is `segment` (ASCII case-insensitive).
    pub fn by_prefix(&self, segment: &str) -> Option<&LocaleDef> {
        self.others
            .iter()
            .find(|d| d.prefix().eq_ignore_ascii_case(segment))
    }

    /// The URL prefix of `tag`: `None` for the default locale and for a tag not listed.
    pub fn prefix_of(&self, tag: &str) -> Option<String> {
        self.others
            .iter()
            .find(|d| d.tag == tag)
            .map(LocaleDef::prefix)
    }

    /// The prefixes of the non-default locales, in site order.
    pub fn prefixes(&self) -> Vec<String> {
        self.others.iter().map(LocaleDef::prefix).collect()
    }
}

/// A loose single default tag as `settle` may have written it: ASCII alphanumerics and
/// `-`, not empty, at most [`MAX_TAG_BYTES`] bytes.
fn loose_single(raw: &str) -> Option<String> {
    let ok = !raw.is_empty()
        && raw.len() <= MAX_TAG_BYTES
        && raw.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && !raw.starts_with('-')
        && !raw.ends_with('-');
    ok.then(|| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_case() {
        for (raw, tag) in [
            ("PT_br", "pt-BR"),
            ("zh-hant-tw", "zh-Hant-TW"),
            ("es-419", "es-419"),
            ("DE", "de"),
            ("sr-latn", "sr-Latn"),
            ("fil", "fil"),
        ] {
            assert_eq!(parse_tag(raw).as_deref(), Ok(tag), "{raw}");
        }
    }

    #[test]
    fn refuses_extensions_variants_private_use_and_junk() {
        for raw in [
            "en-US-x-a",
            "de-1996",
            "\"><x",
            "",
            &"a".repeat(36),
            "e",
            "engl",
            "en--US",
            "en-",
            "en-US-GB",
            "en-Latn-Latn",
            "en-US-Latn",
            "dé",
            "en-u-ca-gregory",
        ] {
            assert!(parse_tag(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn loose_matches_old_bcp47_table() {
        for (raw, tag) in [
            ("en", "en"),
            ("en_GB", "en-GB"),
            ("pl_PL.UTF-8", "pl-PL"),
            ("de_DE@euro", "de-DE"),
            ("pt-BR", "pt-BR"),
            ("\"><x", "x"),
            ("", "en"),
            ("_", "en"),
        ] {
            assert_eq!(bcp47_loose(raw), tag, "{raw:?}");
        }
    }

    #[test]
    fn prefix_is_lowercase() {
        assert_eq!(prefix("pt-BR"), "pt-br");
        assert_eq!(prefix("zh-Hant-TW"), "zh-hant-tw");
        assert_eq!(language("pt-BR"), "pt");
        assert_eq!(native_name("de-AT"), Some("Deutsch"));
        assert_eq!(native_name("xx"), None);
    }

    #[test]
    fn site_locales_roundtrip_and_caps() {
        let v = json!([{"tag": "en"}, {"tag": "de", "label": "Deutsch (DE)"}, {"tag": "pt_br", "label": ""}, {"tag": "xx"}]);
        let l = SiteLocales::from_option(&v).unwrap();
        assert_eq!(l.default.tag, "en");
        assert_eq!(l.default.label, "English");
        assert_eq!(l.others[0].label, "Deutsch (DE)");
        assert_eq!(l.others[1].tag, "pt-BR");
        assert_eq!(l.others[1].label, "Português");
        assert_eq!(l.others[2].label, "xx");
        assert!(l.is_multilingual());
        assert_eq!(SiteLocales::from_option(&l.to_option()).unwrap(), l);

        let many: Vec<Value> = (0..21)
            .map(|i| json!({"tag": format!("a{}", (b'a' + i as u8) as char)}))
            .collect();
        assert!(SiteLocales::from_option(&Value::Array(many)).is_err());
        let twenty: Vec<Value> = (0..20)
            .map(|i| json!({"tag": format!("a{}", (b'a' + i as u8) as char)}))
            .collect();
        assert!(SiteLocales::from_option(&Value::Array(twenty)).is_ok());
        assert!(SiteLocales::from_option(&json!([{"tag": "de"}, {"tag": "DE"}])).is_err());
        assert!(
            SiteLocales::from_option(&json!([{"tag": "en"}, {"tag": "de", "label": "a\u{202E}b"}]))
                .is_err()
        );
        assert!(
            SiteLocales::from_option(
                &json!([{"tag": "en"}, {"tag": "de", "label": "x".repeat(51)}])
            )
            .is_err()
        );
        assert!(SiteLocales::from_option(&json!([])).is_err());
        assert!(SiteLocales::from_option(&json!({"tag": "en"})).is_err());
        assert!(SiteLocales::from_option(&json!([{"tag": "en", "x": 1}])).is_err());
        // A single loose default stays as written; with others it must be strict.
        let single = SiteLocales::from_option(&json!([{"tag": "en-US-x-a"}])).unwrap();
        assert_eq!(single.default.tag, "en-US-x-a");
        assert!(!single.is_multilingual());
        assert!(SiteLocales::from_option(&json!([{"tag": "en-US-x-a"}, {"tag": "de"}])).is_err());
    }

    #[test]
    fn by_prefix_is_ascii_case_insensitive() {
        let l = SiteLocales::from_option(&json!([{"tag": "en"}, {"tag": "pt-BR"}])).unwrap();
        assert_eq!(l.by_prefix("PT-br").map(|d| d.tag.as_str()), Some("pt-BR"));
        assert_eq!(l.by_prefix("en"), None, "the default has no prefix");
        assert_eq!(l.prefix_of("pt-BR").as_deref(), Some("pt-br"));
        assert_eq!(l.prefix_of("en"), None);
        assert_eq!(l.prefix_of("fr"), None);
        assert!(l.contains("pt-BR") && !l.contains("pt-br"));
        assert_eq!(l.prefixes(), vec!["pt-br".to_owned()]);
    }
}
