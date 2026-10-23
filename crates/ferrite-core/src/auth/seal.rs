//! Sealed values (P4, P9): small secrets and stateless tokens encrypted and authenticated
//! with XChaCha20-Poly1305 under the site key's [`SEAL_LABEL`] subkey. The `purpose` is
//! the associated data, so a value sealed for one use never opens for another.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use super::key::{SEAL_LABEL, SiteKey, random_bytes};
use crate::CoreError;

const PREFIX: &str = "v1.";
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

fn cipher(key: &SiteKey) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&key.subkey(SEAL_LABEL).into())
}

/// `"v1." + base64url_nopad(nonce(24) || ciphertext || tag)` with a fresh random nonce.
pub fn seal(key: &SiteKey, purpose: &str, plaintext: &[u8]) -> Result<String, CoreError> {
    let nonce: [u8; NONCE_LEN] = random_bytes()?;
    let ciphertext = cipher(key)
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: purpose.as_bytes(),
            },
        )
        // Only plaintexts or purposes beyond the cipher's 256 GiB limit fail.
        .map_err(|_| CoreError::Invalid("value too large to seal".into()))?;
    let mut raw = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    raw.extend_from_slice(&nonce);
    raw.extend_from_slice(&ciphertext);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(raw)))
}

/// Opens a value from [`seal`] for the same `purpose`. Every failure (prefix, base64,
/// length, tag, purpose or key) is the same `Invalid("sealed value is not valid")`.
pub fn open(key: &SiteKey, purpose: &str, sealed: &str) -> Result<Vec<u8>, CoreError> {
    let invalid = || CoreError::Invalid("sealed value is not valid".into());
    let body = sealed.strip_prefix(PREFIX).ok_or_else(invalid)?;
    let raw = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
    if raw.len() < NONCE_LEN + TAG_LEN {
        return Err(invalid());
    }
    let (nonce, ciphertext) = raw.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| invalid())?;
    cipher(key)
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad: purpose.as_bytes(),
            },
        )
        .map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    use super::*;

    fn invalid(r: Result<Vec<u8>, CoreError>) -> bool {
        matches!(r, Err(CoreError::Invalid(m)) if m == "sealed value is not valid")
    }

    #[test]
    fn seal_roundtrip_and_tamper() {
        let key = SiteKey::generate().unwrap();
        let plain = b"totp secret bytes";
        let sealed = seal(&key, "totp-enroll", plain).unwrap();
        assert!(sealed.starts_with("v1."), "{sealed}");
        assert_eq!(open(&key, "totp-enroll", &sealed).unwrap(), plain);
        // A fresh nonce every time.
        let again = seal(&key, "totp-enroll", plain).unwrap();
        assert_ne!(sealed, again);
        assert_eq!(open(&key, "totp-enroll", &again).unwrap(), plain);
        // Empty plaintext works.
        let empty = seal(&key, "p", b"").unwrap();
        assert_eq!(open(&key, "p", &empty).unwrap(), b"");

        // Flipped byte anywhere in nonce, ciphertext or tag.
        let raw = URL_SAFE_NO_PAD.decode(&sealed[3..]).unwrap();
        assert_eq!(raw.len(), 24 + plain.len() + 16);
        for i in [0, 23, 24, raw.len() - 1] {
            let mut bad = raw.clone();
            bad[i] ^= 0x01;
            let bad = format!("v1.{}", URL_SAFE_NO_PAD.encode(&bad));
            assert!(invalid(open(&key, "totp-enroll", &bad)), "byte {i}");
        }
        // Other purpose, other key.
        assert!(invalid(open(&key, "login-challenge", &sealed)));
        assert!(invalid(open(
            &SiteKey::generate().unwrap(),
            "totp-enroll",
            &sealed
        )));
        // Truncated: shorter than nonce + tag, and missing the last character.
        let short = format!("v1.{}", URL_SAFE_NO_PAD.encode(&raw[..39]));
        assert!(invalid(open(&key, "totp-enroll", &short)));
        assert!(invalid(open(
            &key,
            "totp-enroll",
            &sealed[..sealed.len() - 1]
        )));
        assert!(invalid(open(&key, "totp-enroll", "v1.")));
        // Wrong prefix, bad base64.
        let v2 = format!("v2.{}", &sealed[3..]);
        assert!(invalid(open(&key, "totp-enroll", &v2)));
        assert!(invalid(open(&key, "totp-enroll", &sealed[3..])));
        assert!(invalid(open(&key, "totp-enroll", "v1.!!!!")));
        assert!(invalid(open(&key, "totp-enroll", "")));
    }
}
