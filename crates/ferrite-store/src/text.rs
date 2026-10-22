//! Backend parity checks for text columns.

use crate::StoreError;

/// Rejects `\0` in caller text (PostgreSQL `text` cannot store it, SQLite can), so both
/// backends accept exactly the same strings. Call it for every text bound in a write.
pub fn validate_text(field: &str, value: &str) -> Result<(), StoreError> {
    if value.contains('\0') {
        Err(StoreError::Invalid(format!("{field} contains NUL")))
    } else {
        Ok(())
    }
}

/// The unique lookup key of an e-mail address: ASCII-lowercased in Rust so both backends
/// fold identically (no database collation or locale involved; `É` and `é` stay distinct).
pub fn email_key(email: &str) -> String {
    email.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_is_rejected_and_email_folds_ascii_only() {
        assert!(validate_text("f", "ok ünï").is_ok());
        assert!(matches!(
            validate_text("slug", "a\0b"),
            Err(StoreError::Invalid(m)) if m == "slug contains NUL"
        ));
        assert_eq!(email_key("USER@Example.COM"), "user@example.com");
        assert_ne!(email_key("É@x.com"), email_key("é@x.com"));
    }
}
