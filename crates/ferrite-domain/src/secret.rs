use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// A sensitive string (password hash, TOTP secret, a password on its way to be hashed).
/// Debug is redacted and there are deliberately no serde impls; callers must use
/// [`Secret::expose`]. The bytes are wiped when the value is dropped (M2a-2b P24); copies
/// a caller makes from [`expose`](Secret::expose) are the caller's to wipe.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_zeroize_clears_the_string() {
        let mut s = Secret::new("hunter2-hunter2");
        s.zeroize();
        assert_eq!(s.expose(), "");
        assert_eq!(format!("{s:?}"), "Secret([redacted])");
    }
}
