//! Backend parity checks for JSON values (PostgreSQL `jsonb` rejects `\u0000`).

use serde_json::Value;

use crate::StoreError;

/// Rejects any string or object key containing NUL, so SQLite and PostgreSQL accept
/// exactly the same documents. Call it from every repo method that writes JSON.
pub fn validate_json(value: &Value) -> Result<(), StoreError> {
    let nul = || StoreError::Invalid("json contains NUL".into());
    match value {
        Value::String(s) if s.contains('\0') => Err(nul()),
        Value::Array(items) => items.iter().try_for_each(validate_json),
        Value::Object(map) => map.iter().try_for_each(|(k, v)| {
            if k.contains('\0') {
                Err(nul())
            } else {
                validate_json(v)
            }
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn rejects_nul_in_strings_and_keys_at_any_depth() {
        assert!(validate_json(&json!({"a": [1, {"b": "ok"}], "c": null})).is_ok());
        assert!(validate_json(&json!("a\0b")).is_err());
        assert!(validate_json(&json!({"a": ["x", {"b": "\0"}]})).is_err());
        assert!(validate_json(&json!({"k\0": 1})).is_err());
    }
}
