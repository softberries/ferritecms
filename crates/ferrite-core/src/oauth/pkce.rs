//! PKCE (RFC 7636), `S256` only (design 12 §3.2): the client sends
//! `BASE64URL(SHA-256(verifier))` with the authorization request and the verifier with
//! the token request.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// The length of an `S256` challenge: 32 bytes as base64url without padding.
const CHALLENGE_LEN: usize = 43;

/// Whether `v` is a valid verifier: 43 to 128 characters of the unreserved set
/// (`A-Z a-z 0-9 - . _ ~`, RFC 7636 §4.1).
pub(crate) fn is_verifier(v: &str) -> bool {
    (43..=128).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// Whether `c` has the shape of an `S256` challenge (43 base64url characters).
pub(crate) fn is_challenge(c: &str) -> bool {
    c.len() == CHALLENGE_LEN
        && c.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// `BASE64URL(SHA-256(ASCII(verifier)))`.
pub(crate) fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether `verifier` is valid and hashes to `challenge`.
pub(crate) fn verify(verifier: &str, challenge: &str) -> bool {
    is_verifier(verifier) && s256(verifier) == challenge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_s256_matches_rfc7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(s256(verifier), challenge);
        assert!(is_challenge(challenge));
        assert!(verify(verifier, challenge));
        assert!(!verify(
            verifier,
            &s256("another-verifier-another-verifier-another-v")
        ));
    }

    #[test]
    fn verifier_shape() {
        assert!(is_verifier(&"a".repeat(43)));
        assert!(is_verifier(&"~._-".repeat(32)));
        assert!(!is_verifier(&"a".repeat(42)));
        assert!(!is_verifier(&"a".repeat(129)));
        assert!(!is_verifier(&format!("{}+", "a".repeat(42))));
        assert!(!is_verifier(&format!("{}=", "a".repeat(42))));
        // A short verifier never verifies, even against its own hash.
        let short = "a".repeat(42);
        assert!(!verify(&short, &s256(&short)));
        assert!(!is_challenge(&"a".repeat(44)));
        assert!(!is_challenge(&format!("{}+", "a".repeat(42))));
    }
}
