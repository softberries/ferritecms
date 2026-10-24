//! Theme and plugin settings: one schema and one validator for both (M2a-2 P13; design
//! 05 §4c, 06 §1).
//!
//! A [`SettingsSchema`] is the list of declared settings. Themes declare theirs in
//! `theme.toml` (the theme row stores the schema in `manifest.settings`, an array of
//! [`SettingDef`]), plugins in the `[settings]` table of `ferrite-plugin.toml` (the
//! plugin row stores that table as JSON; [`SettingsSchema::from_plugin_table`] reads
//! it). Values are written through [`ExtensionAdmin`](crate::ExtensionAdmin) after
//! [`validate_value`]:
//!
//! * theme `<name>`: one option `theme.<name>.settings` holding an object of the set
//!   keys; templates see every declared key ([`SettingsSchema::values`]).
//! * plugin `<name>`: one option `plugin.<name>.settings.<key>` per set key. A
//!   [`SettingType::Secret`] value is stored sealed, `{"sealed": "v1.…"}`
//!   ([`seal_secret`]), under the purpose [`secret_purpose`]`(plugin, key)`: the site
//!   key's seal with the plugin and key as associated data, so the stored value opens
//!   for that plugin and key only. Only the owning plugin's `settings.get` opens it
//!   (`ferrite-http`'s `StoreHostServices`); the API never returns it, the audit never
//!   holds it, and `options.get` never reads any `plugin.*` option.
//!
//! Secrets are plugin-only: a theme setting is visible to every template, so a theme
//! schema with a secret is refused.

use std::collections::BTreeMap;

use ferrite_domain::{
    MAX_SETTING_SECRET, MAX_SETTING_TEXT, MAX_SETTING_TEXTAREA, check_setting_value,
    empty_setting_value,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    CoreError,
    auth::{SiteKey, open, seal},
};

pub use ferrite_domain::SettingType;

/// Most characters of a `text` value.
pub const MAX_TEXT: usize = MAX_SETTING_TEXT;
/// Most characters of a `textarea` value.
pub const MAX_TEXTAREA: usize = MAX_SETTING_TEXTAREA;
/// Most characters of a `secret` value (at least one).
pub const MAX_SECRET: usize = MAX_SETTING_SECRET;

/// Prefix of a plugin secret's seal purpose; see [`secret_purpose`].
pub const SECRET_PURPOSE: &str = "plugin-setting";

/// The member of a stored secret option holding the sealed text.
const SEALED: &str = "sealed";

/// The option holding theme `name`'s settings (one object).
pub fn theme_option(name: &str) -> String {
    format!("theme.{name}.settings")
}

/// The option holding plugin `plugin`'s setting `key`.
pub fn plugin_option(plugin: &str, key: &str) -> String {
    format!("plugin.{plugin}.settings.{key}")
}

/// One declared setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingDef {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: SettingType,
    pub label: String,
    /// The declared default (never for `media` or `secret`).
    #[serde(default)]
    pub default: Option<Value>,
    /// The choices of a `select`; empty otherwise.
    #[serde(default)]
    pub options: Vec<String>,
}

/// The declared settings of a theme or plugin, by key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettingsSchema(pub Vec<SettingDef>);

/// The member of a plugin manifest's `[settings]` table, as the row stores it.
#[derive(Deserialize)]
struct TableEntry {
    #[serde(rename = "type")]
    kind: SettingType,
    label: String,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    options: Vec<String>,
}

impl SettingsSchema {
    /// The definition of `key`, if declared.
    pub fn get(&self, key: &str) -> Option<&SettingDef> {
        self.0.iter().find(|d| d.key == key)
    }

    /// The schema of a plugin row: its manifest's `[settings]` table (`None` or
    /// missing: no settings), in key order. The table passed manifest validation at
    /// install; one that does not read back is `Invalid`.
    pub fn from_plugin_table(table: Option<&Value>) -> Result<Self, CoreError> {
        let Some(table) = table else {
            return Ok(Self::default());
        };
        let entries: BTreeMap<String, TableEntry> = serde_json::from_value(table.clone())
            .map_err(|e| CoreError::Invalid(format!("the stored settings schema: {e}")))?;
        Ok(Self(
            entries
                .into_iter()
                .map(|(key, e)| SettingDef {
                    key,
                    kind: e.kind,
                    label: e.label,
                    default: e.default,
                    options: e.options,
                })
                .collect(),
        ))
    }

    /// The schema a theme row stores (`manifest.settings`, an array), or `None` when the
    /// row predates it (an M1.5 row, or an object there): the caller then reads the
    /// theme's files (pre-flight F10, F22).
    pub fn from_theme_row(manifest: &Value) -> Option<Self> {
        let settings = manifest.get("settings")?;
        if !settings.is_array() {
            return None;
        }
        serde_json::from_value(settings.clone()).ok()
    }

    /// Every declared key's value as templates see it (the semantics of
    /// `ferrite_theme::ThemeManifest::settings_values`): the stored value when it is
    /// valid for the definition, else the default, else the type's empty value (`""`,
    /// `false`, `0`, the first option, `null` for media and secrets). Undeclared stored
    /// keys are dropped.
    pub fn values(&self, stored: &Map<String, Value>) -> Map<String, Value> {
        self.0
            .iter()
            .map(|d| {
                let value = stored
                    .get(&d.key)
                    .filter(|v| is_valid(d, v))
                    .or(d.default.as_ref())
                    .cloned()
                    .unwrap_or_else(|| empty_value(d));
                (d.key.clone(), value)
            })
            .collect()
    }
}

fn empty_value(d: &SettingDef) -> Value {
    empty_setting_value(d.kind, &d.options)
}

/// Whether `value` passes [`validate_value`] for `def`.
pub fn is_valid(def: &SettingDef, value: &Value) -> bool {
    validate_value(def, value).is_ok()
}

/// Checks `value` against `def`: `text` a string of at most [`MAX_TEXT`] characters,
/// `textarea` at most [`MAX_TEXTAREA`], `secret` 1..=[`MAX_SECRET`], `number` a JSON
/// number, `bool`, `select` one of the options, `color` `#rgb` / `#rrggbb`, `media` a
/// UUID string; no string holds NUL (the rules of
/// [`ferrite_domain::check_setting_value`]). A refusal is [`CoreError::InvalidKey`] naming the
/// key; its message never holds the value.
pub fn validate_value(def: &SettingDef, value: &Value) -> Result<(), CoreError> {
    check_setting_value(def.kind, &def.options, value)
        .map_err(|message| CoreError::invalid_key(&def.key, message))
}

/// The seal purpose of plugin `plugin`'s secret setting `key`:
/// `plugin-setting:<plugin>:<key>`. Plugin names (`[a-z0-9-]`) and setting keys
/// (`[a-z0-9_-]`) hold no `:`, so no two settings share a purpose.
pub fn secret_purpose(plugin: &str, key: &str) -> String {
    format!("{SECRET_PURPOSE}:{plugin}:{key}")
}

/// The stored form of a secret: `{"sealed": "v1.…"}`.
pub fn seal_secret(
    site_key: &SiteKey,
    plugin: &str,
    key: &str,
    plaintext: &str,
) -> Result<Value, CoreError> {
    let sealed = seal(site_key, &secret_purpose(plugin, key), plaintext.as_bytes())?;
    let mut map = Map::new();
    map.insert(SEALED.to_owned(), Value::String(sealed));
    Ok(Value::Object(map))
}

/// The sealed text of a stored secret option (`{"sealed": "<text>"}` and nothing
/// else), if `stored` is one.
pub fn sealed_text(stored: &Value) -> Option<&str> {
    match stored.as_object() {
        Some(map) if map.len() == 1 => map.get(SEALED)?.as_str(),
        _ => None,
    }
}

/// Opens the sealed text of plugin `plugin`'s secret `key`. Sealed for another plugin
/// or key, under another site key, or not a sealed value: `Invalid`.
pub fn open_secret(
    site_key: &SiteKey,
    plugin: &str,
    key: &str,
    sealed: &str,
) -> Result<String, CoreError> {
    let bytes = open(site_key, &secret_purpose(plugin, key), sealed)?;
    String::from_utf8(bytes).map_err(|_| CoreError::Invalid("sealed value is not text".into()))
}

/// What a stored plugin setting means under the plugin's current definition (review
/// R11): the one rule both the plugin's `settings.get` (`ferrite-http`'s
/// `StoreHostServices`) and the admin view ([`PluginSettingsView`]) apply.
#[derive(Clone, PartialEq)]
pub struct Resolved {
    /// The value the plugin reads: for a secret its opened plaintext (a JSON string),
    /// else the stored value; the default when nothing usable is stored; `None` without
    /// one.
    pub value: Option<Value>,
    /// A value is stored but does not fit the definition (its type changed in an
    /// update, or it was written around the API), so `value` is the fallback.
    pub fell_back: bool,
}

impl std::fmt::Debug for Resolved {
    // The value may be a plaintext secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("set", &self.value.is_some())
            .field("fell_back", &self.fell_back)
            .finish()
    }
}

/// Resolves `stored` (the option `plugin.<plugin>.settings.<key>`, if set) against
/// `def`. A `secret` accepts only a sealed value that `open` opens (for this plugin and
/// key: the caller opens with [`open_secret`]) to a valid secret; a non-secret accepts
/// only an unsealed value that passes [`validate_value`] (a sealed blob is never
/// opened for it). Anything else is unset: the default (a secret has none), with
/// `fell_back` set.
pub fn resolve_plugin_value(
    def: &SettingDef,
    stored: Option<&Value>,
    open: impl FnOnce(&str) -> Option<String>,
) -> Resolved {
    let Some(stored) = stored else {
        return Resolved {
            value: def.default.clone(),
            fell_back: false,
        };
    };
    let accepted = match (def.kind, sealed_text(stored)) {
        (SettingType::Secret, Some(sealed)) => {
            open(sealed).map(Value::String).filter(|v| is_valid(def, v))
        }
        (SettingType::Secret, None) | (_, Some(_)) => None,
        (_, None) => Some(stored.clone()).filter(|v| is_valid(def, v)),
    };
    match accepted {
        Some(value) => Resolved {
            value: Some(value),
            fell_back: false,
        },
        None => Resolved {
            value: if def.kind == SettingType::Secret {
                None
            } else {
                def.default.clone()
            },
            fell_back: true,
        },
    }
}

/// A plugin's settings as the admin sees them, each key resolved by
/// [`resolve_plugin_value`] (what the plugin's `settings.get` answers): the value of
/// every non-secret key (`null` without one), for each secret only whether a usable
/// value is set, and the keys whose stored value does not fit the definition (shown at
/// their fallback). Holds no secret, so it may be logged.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSettingsView {
    pub schema: SettingsSchema,
    pub values: Map<String, Value>,
    pub secrets: BTreeMap<String, bool>,
    /// Keys with a stored value that does not fit the current definition, by key.
    pub invalid: Vec<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    #[test]
    fn plugin_table_reads_in_key_order() {
        let table = json!({
            "b": {"type": "select", "label": "B", "options": ["x"], "default": "x"},
            "a": {"type": "secret", "label": "A"},
        });
        let schema = SettingsSchema::from_plugin_table(Some(&table)).unwrap();
        let keys: Vec<&str> = schema.0.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["a", "b"]);
        assert_eq!(schema.0[0].kind, SettingType::Secret);
        assert_eq!(schema.0[1].default, Some(json!("x")));
        assert_eq!(
            SettingsSchema::from_plugin_table(None).unwrap(),
            SettingsSchema::default()
        );
        assert!(matches!(
            SettingsSchema::from_plugin_table(Some(&json!({"a": {"type": "nope"}}))),
            Err(CoreError::Invalid(_))
        ));
    }

    #[test]
    fn theme_rows_without_an_array_have_no_schema() {
        assert_eq!(SettingsSchema::from_theme_row(&json!({"name": "x"})), None);
        assert_eq!(
            SettingsSchema::from_theme_row(&json!({"settings": {}})),
            None
        );
        assert_eq!(
            SettingsSchema::from_theme_row(&json!({"settings": []})),
            Some(SettingsSchema::default())
        );
    }

    #[test]
    fn sealed_values_are_recognised_exactly() {
        let key = SiteKey::generate().unwrap();
        let stored = seal_secret(&key, "seo", "api_key", "s3cret").unwrap();
        let text = sealed_text(&stored).unwrap();
        assert!(text.starts_with("v1."));
        assert_eq!(open_secret(&key, "seo", "api_key", text).unwrap(), "s3cret");
        assert!(open_secret(&key, "seo", "other", text).is_err());
        assert_eq!(sealed_text(&json!("v1.x")), None);
        assert_eq!(sealed_text(&json!({"sealed": "v1.x", "more": 1})), None);
        assert_eq!(sealed_text(&json!({"sealed": 1})), None);
    }
}
