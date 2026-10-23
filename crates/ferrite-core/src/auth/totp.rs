//! TOTP parameters (P9): SHA-1, 6 digits, 30 s steps, skew 1, 20-byte secrets, through
//! totp-rs's `Builder` (it checks the RFC rules; issuers and accounts must not contain
//! `:`).

use ferrite_domain::Timestamp;
use totp_rs::{Builder, Totp};

use crate::CoreError;

/// Secret length in bytes (160 bits, the RFC 4226 recommendation).
pub(crate) const SECRET_LEN: usize = 20;
/// The issuer shown in authenticator apps when no site title is set.
pub(crate) const DEFAULT_ISSUER: &str = "FerriteCMS";

/// The issuer for a site title: `:` removed (totp-rs refuses it), trimmed, and
/// [`DEFAULT_ISSUER`] when nothing is left (F11).
pub(crate) fn issuer_name(title: &str) -> String {
    let cleaned: String = title.chars().filter(|&c| c != ':').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        DEFAULT_ISSUER.to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// A TOTP for `secret`; `issuer` and `account` only matter for the `otpauth://` URL.
pub(crate) fn build(secret: &[u8], issuer: &str, account: &str) -> Result<Totp, CoreError> {
    Builder::new()
        .with_secret(secret.to_vec())
        .with_issuer(Some(issuer))
        .with_account_name(account)
        .build()
        .map_err(|e| {
            tracing::error!(error = %e, "TOTP parameters refused");
            CoreError::Invalid("TOTP parameters refused".into())
        })
}

/// The step `code` matches at `now` (± one step), or `None`. totp-rs compares codes in
/// constant time.
pub(crate) fn check(totp: &Totp, code: &str, now: Timestamp) -> Option<i64> {
    let secs = u64::try_from(now.as_micros().div_euclid(1_000_000)).ok()?;
    let step = totp.check(code.trim(), secs)?;
    i64::try_from(step).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn issuer_rules() {
        assert_eq!(issuer_name("My: Blog"), "My Blog");
        assert_eq!(issuer_name("  Plain  "), "Plain");
        assert_eq!(issuer_name(" : "), DEFAULT_ISSUER);
        assert_eq!(issuer_name(""), DEFAULT_ISSUER);
        build(&[7; SECRET_LEN], &issuer_name("a:b:c"), "alice").unwrap();
    }

    #[test]
    fn check_returns_the_step() {
        let totp = build(&[1; SECRET_LEN], DEFAULT_ISSUER, "alice").unwrap();
        let now = Timestamp::from_micros(1_700_000_000_000_000).unwrap();
        let code = totp.generate(1_700_000_000).to_string();
        assert_eq!(check(&totp, &code, now), Some(1_700_000_000 / 30));
        assert_eq!(
            check(&totp, &format!(" {code} "), now),
            Some(1_700_000_000 / 30)
        );
        // One step of skew either way, not two.
        let later = Timestamp::from_micros(1_700_000_030_000_000).unwrap();
        assert!(check(&totp, &code, later).is_some());
        let much_later = Timestamp::from_micros(1_700_000_090_000_000).unwrap();
        assert_eq!(check(&totp, &code, much_later), None);
        assert_eq!(check(&totp, "12345", now), None);
        assert_eq!(check(&totp, "abcdef", now), None);
    }
}
