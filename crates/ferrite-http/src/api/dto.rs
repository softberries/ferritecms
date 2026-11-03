//! Request and response bodies of the API, one module per resource (P12). They derive
//! `Serialize`/`Deserialize` and `utoipa::ToSchema`; the domain types stay without
//! utoipa (M1a R3). Bodies that carry passwords, codes or tokens derive no `Debug`;
//! every password field is a [`Password`].

use std::fmt;

use ferrite_domain::Secret;
use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

pub mod auth;
pub mod change_sets;
pub mod comments;
pub mod content;
pub mod extensions;
pub mod import;
pub mod media;
pub mod oauth;
pub mod site;
pub mod system;
pub mod types;
pub mod users;
pub mod webhooks;

/// A password in a request body (M2a-2b P24): deserialised through a `String` that is
/// wiped when the value is dropped, and redacted in `Debug`. In the OpenAPI document a
/// password field is a `string` with `format: password` (`#[schema(value_type = String,
/// format = Password)]` on the field). Not wiped: serde_json's copy of the request body
/// and its scratch buffer for escaped strings.
pub struct Password(Zeroizing<String>);

impl Password {
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The password as a [`Secret`] (also wiped on drop), moving the bytes, not copying.
    pub fn into_secret(mut self) -> Secret {
        Secret::new(std::mem::take(&mut *self.0))
    }
}

impl<'de> Deserialize<'de> for Password {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Self(Zeroizing::new(s)))
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_debug_is_redacted() {
        let pw: Password = serde_json::from_str("\"correct horse battery\"").unwrap();
        assert_eq!(pw.expose(), "correct horse battery");
        assert_eq!(format!("{pw:?}"), "Password([redacted])");
        let secret = pw.into_secret();
        assert_eq!(secret.expose(), "correct horse battery");
        assert!(!format!("{secret:?}").contains("horse"));
        assert!(serde_json::from_str::<Password>("1").is_err());
    }
}
