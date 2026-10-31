//! Site options (capability `options.read`).

use crate::bindings::ferrite::plugin::options as wit_options;

/// The option `key` when its value is a JSON string, decoded. `None` when the option is
/// not readable by this plugin, is unset, or is any other JSON type (number, bool, object).
pub fn get(key: &str) -> Option<String> {
    wit_options::get(key).and_then(|json| crate::json::string(&json))
}
