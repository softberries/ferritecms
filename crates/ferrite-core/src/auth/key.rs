//! The site key (P4): 32 random bytes kept outside the database, from which every
//! purpose gets its own HMAC-derived subkey.

use std::fmt;
use std::net::IpAddr;

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::CoreError;

/// Subkey label for [`ip_hash`].
pub const IP_HASH_LABEL: &str = "ferrite:ip-hash:v1";
/// Subkey label for [`AuthService::plugin_ip_hash`](super::AuthService::plugin_ip_hash).
pub const PLUGIN_IP_LABEL: &str = "ferrite:plugin-ip:v1";
/// Subkey label for [`AuthService::comment_ip_hash`](super::AuthService::comment_ip_hash).
pub const COMMENT_IP_LABEL: &str = "ferrite:comment-ip:v1";
/// Subkey label for [`seal`](super::seal) / [`open`](super::open).
pub const SEAL_LABEL: &str = "ferrite:seal:v1";

const KEY_LEN: usize = 32;
/// SHA-256 block size: HMAC zero-pads shorter keys to it (RFC 2104 §2).
const HMAC_BLOCK: usize = 64;

/// The site's master key. `Debug` is redacted; there is deliberately no serde, so it
/// cannot end up in a config dump, a log line or an API response.
#[derive(Clone)]
pub struct SiteKey([u8; KEY_LEN]);

impl fmt::Debug for SiteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SiteKey([redacted])")
    }
}

impl SiteKey {
    /// Exactly 64 hex characters (either case).
    pub fn from_hex(s: &str) -> Result<Self, CoreError> {
        let invalid = || CoreError::Invalid("site key must be 64 hex characters (32 bytes)".into());
        let bytes = s.as_bytes();
        if bytes.len() != KEY_LEN * 2 {
            return Err(invalid());
        }
        let mut key = [0u8; KEY_LEN];
        let (pairs, _) = bytes.as_chunks::<2>();
        for (out, [hi, lo]) in key.iter_mut().zip(pairs) {
            let hi = hex_value(*hi).ok_or_else(invalid)?;
            let lo = hex_value(*lo).ok_or_else(invalid)?;
            *out = (hi << 4) | lo;
        }
        Ok(Self(key))
    }

    /// A new random key from the operating system's generator.
    pub fn generate() -> Result<Self, CoreError> {
        random_bytes().map(Self)
    }

    /// 64 lowercase hex characters.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// HMAC-SHA256(master key, `label`): one independent key per purpose.
    pub fn subkey(&self, label: &str) -> [u8; 32] {
        hmac_sha256(&self.0, label.as_bytes())
    }
}

/// `N` bytes from the operating system's generator (`getrandom`).
pub fn random_bytes<const N: usize>() -> Result<[u8; N], CoreError> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|_| CoreError::Random)?;
    Ok(buf)
}

/// A keyed, non-reversible id for a client address (rate-limit buckets, sessions): the
/// first 32 hex characters of HMAC-SHA256 under the [`IP_HASH_LABEL`] subkey, over the
/// canonical text form (an IPv4-mapped IPv6 address hashes like its IPv4 address).
pub fn ip_hash(key: &SiteKey, ip: IpAddr) -> String {
    let sub = key.subkey(IP_HASH_LABEL);
    let mac = hmac_sha256(&sub, ip.to_canonical().to_string().as_bytes());
    hex(&mac[..16])
}

/// The first 32 hex characters of HMAC-SHA256 under the `label` subkey over `msg`: a
/// keyed, non-reversible, bounded id (login rate-limit buckets, F7).
pub(crate) fn keyed_id(key: &SiteKey, label: &str, msg: &[u8]) -> String {
    let mac = hmac_sha256(&key.subkey(label), msg);
    hex(&mac[..16])
}

/// HMAC-SHA256 with a 32-byte key.
fn hmac_sha256(key: &[u8; KEY_LEN], msg: &[u8]) -> [u8; 32] {
    let mut mac = keyed_mac(key);
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

/// An HMAC-SHA256 instance keyed with `key` of any length. The key is prepared as RFC 2104
/// §2 says (hashed when longer than the block, then zero-padded to it), so the
/// infallible `KeyInit::new` applies and the result equals `new_from_slice(key)`.
pub(crate) fn keyed_mac(key: &[u8]) -> Hmac<Sha256> {
    let mut block = [0u8; HMAC_BLOCK];
    if key.len() > HMAC_BLOCK {
        let digest: [u8; 32] = Sha256::digest(key).into();
        block[..digest.len()].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    <Hmac<Sha256> as KeyInit>::new(&block.into())
}

/// Lowercase hex SHA-256 of `bytes` (session and token ids in the database).
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lowercase hex (the workspace's one encoder).
pub(crate) use ferrite_domain::to_hex as hex;

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn site_key_hex_rules() {
        let key = SiteKey::from_hex(HEX).unwrap();
        assert_eq!(key.to_hex(), HEX);
        // Uppercase is accepted and normalised to lowercase.
        let upper = SiteKey::from_hex(&HEX.to_uppercase()).unwrap();
        assert_eq!(upper.to_hex(), HEX);
        for bad in [
            &HEX[..63],
            &format!("{HEX}0"),
            &format!("{}g", &HEX[..63]),
            &format!("{}é", &HEX[..62]),
            &format!(" {}", &HEX[..63]),
            "",
        ] {
            let err = SiteKey::from_hex(bad).unwrap_err();
            assert!(
                matches!(&err, CoreError::Invalid(m) if m == "site key must be 64 hex characters (32 bytes)"),
                "{bad:?}: {err:?}"
            );
        }
        // Debug never shows the key.
        let dbg = format!("{key:?}");
        assert!(!dbg.contains("0102") && !dbg.contains(HEX), "{dbg}");
        // Generated keys are random and roundtrip.
        let a = SiteKey::generate().unwrap();
        let b = SiteKey::generate().unwrap();
        assert_ne!(a.to_hex(), b.to_hex());
        assert_eq!(SiteKey::from_hex(&a.to_hex()).unwrap().to_hex(), a.to_hex());
        assert_eq!(a.to_hex().len(), 64);
    }

    #[test]
    fn subkeys_differ_by_label() {
        let key = SiteKey::from_hex(HEX).unwrap();
        assert_ne!(key.subkey(IP_HASH_LABEL), key.subkey(SEAL_LABEL));
        assert_ne!(key.subkey(IP_HASH_LABEL), key.subkey(PLUGIN_IP_LABEL));
        assert_ne!(key.subkey(IP_HASH_LABEL), key.subkey(COMMENT_IP_LABEL));
        assert_ne!(key.subkey(PLUGIN_IP_LABEL), key.subkey(COMMENT_IP_LABEL));
        assert_eq!(key.subkey(SEAL_LABEL), key.subkey(SEAL_LABEL));
        assert_ne!(key.subkey(SEAL_LABEL), [0u8; 32]);
        let other = SiteKey::generate().unwrap();
        assert_ne!(key.subkey(SEAL_LABEL), other.subkey(SEAL_LABEL));
    }

    /// The zero-padded infallible key setup equals HMAC with the 32-byte key (RFC 2104).
    #[test]
    fn padded_hmac_matches_new_from_slice() {
        use hmac::{KeyInit, Mac};
        let key = SiteKey::from_hex(HEX).unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(&key.0).unwrap();
        mac.update(SEAL_LABEL.as_bytes());
        let want: [u8; 32] = mac.finalize().into_bytes().into();
        assert_eq!(key.subkey(SEAL_LABEL), want);
    }

    #[test]
    fn keyed_mac_matches_new_from_slice_for_any_length() {
        use hmac::{KeyInit, Mac};
        for len in [0, 1, 32, 63, 64, 65, 200] {
            let key: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let mut want = Hmac::<Sha256>::new_from_slice(&key).unwrap();
            want.update(b"msg");
            let mut got = keyed_mac(&key);
            got.update(b"msg");
            assert_eq!(
                got.finalize().into_bytes(),
                want.finalize().into_bytes(),
                "{len}"
            );
        }
        assert_eq!(sha256_hex(b"abc").len(), 64);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn ip_hash_is_stable_and_keyed() {
        let key = SiteKey::from_hex(HEX).unwrap();
        let v4 = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
        let h = ip_hash(&key, v4);
        assert_eq!(h.len(), 32);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{h}"
        );
        assert_eq!(h, ip_hash(&key, v4));
        // ::ffff:203.0.113.7 is the same client as 203.0.113.7.
        let mapped = IpAddr::V6(Ipv4Addr::new(203, 0, 113, 7).to_ipv6_mapped());
        assert_eq!(ip_hash(&key, mapped), h);
        assert_ne!(ip_hash(&key, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8))), h);
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
        assert_ne!(ip_hash(&key, v6), h);
        let other = SiteKey::generate().unwrap();
        assert_ne!(ip_hash(&other, v4), h);
    }
}
