//! Standard Webhooks signatures (design 18 §5.1): `v1,<base64(HMAC-SHA256(key,
//! "<id>.<timestamp>.<body>"))>`, standard base64 with padding.

use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The `v1` signature of one message.
pub fn signature(key: &[u8], id: &str, ts: i64, body: &[u8]) -> String {
    // HMAC takes a key of any length; `new_from_slice` cannot fail for it.
    let mut mac = match Hmac::<Sha256>::new_from_slice(key) {
        Ok(mac) => mac,
        Err(_) => return String::new(),
    };
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
}

/// The `webhook-signature` header: one signature per key, current first, separated by
/// a space (two during a rotation).
pub fn header(keys: &[&[u8]], id: &str, ts: i64, body: &[u8]) -> String {
    keys.iter()
        .map(|k| signature(k, id, ts, body))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::webhooks::secret;

    /// The known answer of the Standard Webhooks libraries' tests (design 18 §5.1).
    #[test]
    fn signature_known_answer() {
        let key = secret::parse("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw").unwrap();
        assert_eq!(
            signature(
                &key,
                "msg_p5jXN8AQM9LWM0D4loKWxJek",
                1614265330,
                br#"{"test": 2432232314}"#
            ),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
    }

    #[test]
    fn rotation_header_has_two_signatures() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let h = header(&[&a, &b], "id", 5, b"{}");
        let parts: Vec<&str> = h.split(' ').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], signature(&a, "id", 5, b"{}"));
        assert_eq!(parts[1], signature(&b, "id", 5, b"{}"));
        assert_ne!(parts[0], parts[1]);
        assert_eq!(header(&[&a], "id", 5, b"{}"), parts[0]);
    }
}
