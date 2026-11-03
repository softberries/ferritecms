//! Signing secrets (design 18 §5.2): `whsec_` + standard base64 of the key bytes, sealed
//! per endpoint with purpose `webhook-secret:<id>`.

use base64::{Engine, engine::general_purpose::STANDARD};
use ferrite_domain::{Secret, WebhookId};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    CoreError,
    auth::{SiteKey, open, random_bytes, seal},
};

/// The prefix of every secret.
pub const PREFIX: &str = "whsec_";
/// Shortest and longest key a secret may carry, in bytes.
pub const MIN_KEY: usize = 24;
pub const MAX_KEY: usize = 64;

/// A fresh secret: `whsec_` + base64 of 32 random bytes.
pub fn generate() -> Result<Secret, CoreError> {
    let mut bytes: [u8; 32] = random_bytes()?;
    let s = format!("{PREFIX}{}", STANDARD.encode(bytes));
    bytes.zeroize();
    Ok(Secret::new(s))
}

/// The key bytes of a secret: `whsec_` + standard base64 of 24..=64 bytes, else
/// `InvalidField("secret")` (the message never holds the value).
pub fn parse(s: &str) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    let bad = || {
        CoreError::invalid_field(
            "secret",
            format!("must be {PREFIX} followed by base64 of {MIN_KEY} to {MAX_KEY} bytes"),
        )
    };
    let body = s.strip_prefix(PREFIX).ok_or_else(bad)?;
    let key = Zeroizing::new(STANDARD.decode(body).map_err(|_| bad())?);
    if !(MIN_KEY..=MAX_KEY).contains(&key.len()) {
        return Err(bad());
    }
    Ok(key)
}

fn purpose(endpoint: WebhookId) -> String {
    format!("webhook-secret:{endpoint}")
}

/// Seals `secret` for `endpoint`.
pub fn seal_for(key: &SiteKey, endpoint: WebhookId, secret: &Secret) -> Result<String, CoreError> {
    seal(key, &purpose(endpoint), secret.expose().as_bytes())
}

/// Opens a secret sealed for `endpoint` and returns its key bytes. A value sealed for
/// another endpoint or under another site key is `Invalid`.
pub fn open_for(
    key: &SiteKey,
    endpoint: WebhookId,
    sealed: &str,
) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    let plain = Zeroizing::new(open(key, &purpose(endpoint), sealed)?);
    let text = std::str::from_utf8(&plain)
        .map_err(|_| CoreError::Invalid("sealed value is not valid".into()))?;
    parse(text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn key(b: &str) -> SiteKey {
        SiteKey::from_hex(&b.repeat(32)).unwrap()
    }

    #[test]
    fn secret_parse_bounds() {
        let s = generate().unwrap();
        assert!(s.expose().starts_with(PREFIX));
        assert_eq!(parse(s.expose()).unwrap().len(), 32);
        let enc = |n: usize| format!("{PREFIX}{}", STANDARD.encode(vec![7u8; n]));
        assert!(parse(&enc(23)).is_err());
        assert_eq!(parse(&enc(24)).unwrap().len(), 24);
        assert_eq!(parse(&enc(64)).unwrap().len(), 64);
        assert!(parse(&enc(65)).is_err());
        for bad in [
            "",
            "whsec_",
            "whsec_!!!",
            "sk_abc",
            &STANDARD.encode([1u8; 32]),
        ] {
            let err = parse(bad).unwrap_err();
            assert!(
                matches!(
                    err,
                    CoreError::InvalidField {
                        field: "secret",
                        ..
                    }
                ),
                "{bad}"
            );
        }
        // The message never echoes the value.
        let err = parse("whsec_c2VjcmV0").unwrap_err().to_string();
        assert!(!err.contains("c2VjcmV0"), "{err}");
    }

    #[test]
    fn sealed_secret_opens_only_for_its_endpoint() {
        let k = key("ab");
        let a = WebhookId::new();
        let b = WebhookId::new();
        let s = generate().unwrap();
        let sealed = seal_for(&k, a, &s).unwrap();
        assert!(!sealed.contains(s.expose()));
        assert_eq!(
            *open_for(&k, a, &sealed).unwrap(),
            *parse(s.expose()).unwrap()
        );
        assert!(open_for(&k, b, &sealed).is_err());
        assert!(open_for(&key("cd"), a, &sealed).is_err());
    }
}
