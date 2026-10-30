//! This plugin's own settings (the `[settings]` of its manifest), decoded.
//!
//! The host hands a setting over as JSON text ([`crate::Ctx::setting`] returns it raw);
//! these read the three scalar types a manifest declares. Each is `None` when the setting
//! is unset or holds another JSON type, so a plugin applies its own default:
//!
//! ```ignore
//! let keep = ferrite_sdk::settings::number("retention_days").unwrap_or(90.0);
//! ```

use crate::bindings::ferrite::plugin::settings as wit_settings;

/// A `text`, `textarea` or `secret` setting: the JSON string, decoded and not trimmed.
pub fn text(key: &str) -> Option<String> {
    wit_settings::get(key).and_then(|json| crate::json::string(&json))
}

/// A `bool` setting.
pub fn bool(key: &str) -> Option<bool> {
    wit_settings::get(key).and_then(|json| crate::json::boolean(&json))
}

/// A `number` setting, as an `f64`. `None` also for a number too large for one.
pub fn number(key: &str) -> Option<f64> {
    wit_settings::get(key).and_then(|json| crate::json::number(&json))
}
