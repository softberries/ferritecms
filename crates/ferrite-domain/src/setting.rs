//! The value rules of theme and plugin settings (M2a-2 P13), in one place: the core
//! validator (`ferrite_core::ext_settings`), the theme renderer
//! (`ferrite_theme::ThemeManifest::settings_values` and its `theme.toml` defaults) and
//! the plugin manifest's defaults (`ferrite_plugin_host::Manifest::parse`) all check
//! values here, so a value one accepts the others accept too.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ContentId;

/// Most characters of a `text` value.
pub const MAX_SETTING_TEXT: usize = 1_000;
/// Most characters of a `textarea` value.
pub const MAX_SETTING_TEXTAREA: usize = 65_536;
/// Most characters of a `secret` value (at least one).
pub const MAX_SETTING_SECRET: usize = 4_096;

/// A setting's type (the manifests' `type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    Text,
    Textarea,
    Bool,
    Number,
    Select,
    /// `#rgb` or `#rrggbb`.
    Color,
    /// A media item id (UUID string).
    Media,
    /// Plugin-only; sealed at rest, write-only in the API.
    Secret,
}

impl SettingType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Textarea => "textarea",
            Self::Bool => "bool",
            Self::Number => "number",
            Self::Select => "select",
            Self::Color => "color",
            Self::Media => "media",
            Self::Secret => "secret",
        }
    }
}

/// Checks `value` for a setting of `kind` (with `options` for a `select`): `text` a
/// string of at most [`MAX_SETTING_TEXT`] characters, `textarea` at most
/// [`MAX_SETTING_TEXTAREA`], `secret` 1..=[`MAX_SETTING_SECRET`], `number` a finite JSON
/// number, `bool`, `select` one of `options`, `color` `#rgb` / `#rrggbb`, `media` a UUID
/// string; no string holds NUL. The message says what is wrong and never holds the
/// value (it may be a secret).
pub fn check_setting_value(
    kind: SettingType,
    options: &[String],
    value: &Value,
) -> Result<(), String> {
    let text = |max: usize, min: usize| -> Result<(), String> {
        let Some(s) = value.as_str() else {
            return Err(format!("must be a string ({} setting)", kind.as_str()));
        };
        if s.contains('\0') {
            return Err("must not contain NUL".to_owned());
        }
        let n = s.chars().count();
        if n < min || n > max {
            return Err(format!("must be {min}-{max} characters"));
        }
        Ok(())
    };
    match kind {
        SettingType::Text => text(MAX_SETTING_TEXT, 0),
        SettingType::Textarea => text(MAX_SETTING_TEXTAREA, 0),
        SettingType::Secret => text(MAX_SETTING_SECRET, 1),
        SettingType::Number => match value.as_f64() {
            Some(n) if n.is_finite() => Ok(()),
            _ => Err("must be a finite number".to_owned()),
        },
        SettingType::Bool if value.is_boolean() => Ok(()),
        SettingType::Bool => Err("must be true or false".to_owned()),
        SettingType::Select => match value.as_str() {
            Some(s) if options.iter().any(|o| o == s) => Ok(()),
            _ => Err(format!("must be one of: {}", options.join(", "))),
        },
        SettingType::Color => match value.as_str() {
            Some(s) if is_color(s) => Ok(()),
            _ => Err("must be a colour #rgb or #rrggbb".to_owned()),
        },
        SettingType::Media => match value.as_str().map(str::parse::<ContentId>) {
            Some(Ok(_)) => Ok(()),
            _ => Err("must be a media item id".to_owned()),
        },
    }
}

/// The value a template sees for a setting of `kind` with no stored value and no
/// default: `""` (text, textarea, color), `false`, `0`, the first option, `null`
/// (media, secret).
pub fn empty_setting_value(kind: SettingType, options: &[String]) -> Value {
    match kind {
        SettingType::Text | SettingType::Textarea | SettingType::Color => Value::from(""),
        SettingType::Bool => Value::from(false),
        SettingType::Number => Value::from(0),
        SettingType::Select => options
            .first()
            .map_or(Value::Null, |o| Value::from(o.as_str())),
        SettingType::Media | SettingType::Secret => Value::Null,
    }
}

/// `#rgb` or `#rrggbb` (strict: colours land in CSS, where HTML escaping does not help).
pub fn is_color(s: &str) -> bool {
    s.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn rules_hold_without_echoing_the_value() {
        let options = vec!["a".to_owned()];
        assert!(check_setting_value(SettingType::Select, &options, &json!("a")).is_ok());
        let e = check_setting_value(SettingType::Secret, &[], &json!("x".repeat(5_000)));
        assert!(!e.unwrap_err().contains("xxx"));
        assert!(check_setting_value(SettingType::Text, &[], &json!("é".repeat(1_000))).is_ok());
        assert!(check_setting_value(SettingType::Text, &[], &json!("é".repeat(1_001))).is_err());
        assert!(is_color("#aBc") && !is_color("#ab") && !is_color("abc"));
        assert_eq!(
            empty_setting_value(SettingType::Select, &options),
            json!("a")
        );
    }
}
