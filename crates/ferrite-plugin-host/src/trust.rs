//! Signature checks for plugins, releases and the registry index (M6-2, design 14 §2.3).
//! Verify only: no secret key is ever handled here.

use ferrite_domain::plugin_registry::{MAX_INDEX_BYTES, RegistryError, RegistryIndex};
use ferrite_domain::plugin_trust::{
    KeyError, KeyId, MAX_SIGNATURE_BYTES, MinisigHeader, PublisherKey,
};
use sha2::{Digest, Sha256};

/// The first line of a plugin signing payload (D2: domain separation, versioned).
pub const PAYLOAD_HEADER: &str = "ferrite-plugin-signature/1";

/// Why a signature was refused. Never "unsigned" (D6).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    #[error("the signature is malformed: {0}")]
    Malformed(String),
    #[error(
        "the signature is a legacy (not prehashed) minisign signature; sign with minisign 0.8 or newer"
    )]
    Legacy,
    #[error(
        "the signature was made with key {0}, which this site does not know; add its public key (ferrite-plugin.pub)"
    )]
    UnknownKey(KeyId),
    #[error("the signature does not match these files")]
    Mismatch,
    /// Two different public keys with the signature's key id (a key id is 8 bytes anyone
    /// can choose): refused rather than guessing which one is meant.
    #[error("two different public keys share the key id {0}; refusing to choose between them")]
    KeyConflict(KeyId),
    #[error("the signature is larger than {MAX_SIGNATURE_BYTES} bytes")]
    TooLarge,
}

impl From<KeyError> for TrustError {
    fn from(e: KeyError) -> Self {
        match e {
            KeyError::TooLarge => TrustError::TooLarge,
            KeyError::Malformed(m) => TrustError::Malformed(m),
        }
    }
}

/// The key that verified a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSigner {
    pub key: PublisherKey,
}

/// The bytes a publisher signs for a plugin (design 14 §2.1): three LF-terminated lines.
pub fn signing_payload(manifest: &[u8], component: &[u8]) -> String {
    format!(
        "{PAYLOAD_HEADER}\nmanifest {}\ncomponent {}\n",
        ferrite_domain::to_hex(&Sha256::digest(manifest)),
        ferrite_domain::to_hex(&Sha256::digest(component)),
    )
}

/// The key id a minisig names, after checking its shape (prehashed only).
pub fn signature_key_id(signature: &str) -> Result<KeyId, TrustError> {
    let header = MinisigHeader::parse(signature)?;
    if !header.prehashed {
        return Err(TrustError::Legacy);
    }
    Ok(header.key_id)
}

/// Checks a minisig over `data` with `key`. Only prehashed signatures verify.
pub fn verify_minisig(key: &PublisherKey, data: &[u8], signature: &str) -> Result<(), TrustError> {
    let id = signature_key_id(signature)?;
    if id != key.key_id {
        return Err(TrustError::UnknownKey(id));
    }
    let pk = minisign_verify::PublicKey::from_base64(&key.base64)
        .map_err(|e| TrustError::Malformed(format!("public key: {e}")))?;
    let sig = minisign_verify::Signature::decode(signature)
        .map_err(|e| TrustError::Malformed(e.to_string()))?;
    pk.verify(data, &sig, false).map_err(|e| match e {
        minisign_verify::Error::UnexpectedAlgorithm => TrustError::Legacy,
        _ => TrustError::Mismatch,
    })
}

/// Verifies a plugin's signature over its two files with the candidate whose key id the
/// signature names.
pub fn verify_bundle(
    manifest: &[u8],
    component: &[u8],
    signature: &str,
    candidates: &[PublisherKey],
) -> Result<VerifiedSigner, TrustError> {
    let id = signature_key_id(signature)?;
    let mut same_id = candidates.iter().filter(|k| k.key_id == id);
    let key = same_id.next().ok_or(TrustError::UnknownKey(id))?;
    if same_id.any(|k| k != key) {
        return Err(TrustError::KeyConflict(id));
    }
    verify_minisig(
        key,
        signing_payload(manifest, component).as_bytes(),
        signature,
    )?;
    Ok(VerifiedSigner { key: key.clone() })
}

/// Why a registry index was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    #[error("the registry index signature: {0}")]
    Signature(#[from] TrustError),
    #[error(transparent)]
    Invalid(#[from] RegistryError),
}

/// Verifies a registry index's signature with the registry key (signature first, so an
/// unsigned document is never parsed), then parses and validates it.
pub fn verify_index(
    key: &PublisherKey,
    bytes: &[u8],
    signature: &str,
) -> Result<RegistryIndex, IndexError> {
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(RegistryError(format!("larger than {MAX_INDEX_BYTES} bytes")).into());
    }
    verify_minisig(key, bytes, signature)?;
    Ok(RegistryIndex::parse(bytes)?)
}

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn verify_index_refuses_tampered_bytes() {
        let (kp, pk) = throwaway();
        let index = br#"{"schema":1,"name":"t","serial":1,"generated_at":"2026-11-20T10:00:00Z"}"#;
        let sig = sign(&kp, index);
        let i = verify_index(&pk, index, &sig).unwrap();
        assert_eq!(i.serial, 1);
        let tampered =
            br#"{"schema":1,"name":"t","serial":2,"generated_at":"2026-11-20T10:00:00Z"}"#;
        assert_eq!(
            verify_index(&pk, tampered, &sig),
            Err(IndexError::Signature(TrustError::Mismatch))
        );
        let (_, other) = throwaway();
        assert!(matches!(
            verify_index(&other, index, &sig),
            Err(IndexError::Signature(TrustError::UnknownKey(_)))
        ));
        // A signed document that is not a valid index.
        let junk = b"[]";
        let sig = sign(&kp, junk);
        assert!(matches!(
            verify_index(&pk, junk, &sig),
            Err(IndexError::Invalid(_))
        ));
    }

    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    /// A throwaway key pair and its public half as a `PublisherKey`.
    pub(crate) fn throwaway() -> (minisign::KeyPair, PublisherKey) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk = PublisherKey::parse(&kp.pk.to_box().unwrap().into_string())
            .unwrap()
            .unwrap();
        (kp, pk)
    }

    pub(crate) fn sign(kp: &minisign::KeyPair, data: &[u8]) -> String {
        minisign::sign(Some(&kp.pk), &kp.sk, data, None, None)
            .unwrap()
            .into_string()
    }

    #[test]
    fn key_id_matches_minisign_display() {
        let text = include_str!("../../ferrite/release.pub");
        let key = PublisherKey::parse(text).unwrap().unwrap();
        assert_eq!(key.key_id.to_string(), "432051207B9BA065");
    }

    #[test]
    fn payload_is_three_lf_lines() {
        let p = signing_payload(b"m", b"c");
        assert!(p.is_ascii());
        let lines: Vec<&str> = p.split_inclusive('\n').collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines
                .iter()
                .all(|l| l.ends_with('\n') && !l.ends_with(" \n"))
        );
        assert_eq!(lines[0], "ferrite-plugin-signature/1\n");
        assert_eq!(
            lines[1],
            "manifest 62c66a7a5dd70c3146618063c344e531e6d4b59e379808443ce962b3abd63c5a\n"
        );
        assert!(lines[2].starts_with("component "));
        assert!(!p.contains('\r'));
    }

    #[test]
    fn bundle_round_trip() {
        let (kp, pk) = throwaway();
        let sig = sign(&kp, signing_payload(b"manifest", b"wasm").as_bytes());
        let (_, other) = throwaway();
        let v = verify_bundle(b"manifest", b"wasm", &sig, &[other, pk.clone()]).unwrap();
        assert_eq!(v.key, pk);
    }

    #[test]
    fn bundle_other_bytes_mismatch() {
        let (kp, pk) = throwaway();
        let sig = sign(&kp, signing_payload(b"manifest", b"wasm").as_bytes());
        let keys = [pk];
        assert_eq!(
            verify_bundle(b"manifesT", b"wasm", &sig, &keys),
            Err(TrustError::Mismatch)
        );
        assert_eq!(
            verify_bundle(b"manifest", b"wasM", &sig, &keys),
            Err(TrustError::Mismatch)
        );
    }

    /// M6-2 review (critical): two candidates with one id but other bytes are refused.
    #[test]
    fn same_id_two_keys_conflict() {
        let (kp, pk) = throwaway();
        let sig = sign(&kp, signing_payload(b"m", b"c").as_bytes());
        let mut bin = STANDARD.decode(&pk.base64).unwrap();
        bin[20] ^= 0xff;
        let forged = PublisherKey::from_base64(&STANDARD.encode(&bin)).unwrap();
        assert_eq!(
            verify_bundle(b"m", b"c", &sig, &[forged.clone(), pk.clone()]),
            Err(TrustError::KeyConflict(pk.key_id))
        );
        // The same key twice is no conflict.
        assert!(verify_bundle(b"m", b"c", &sig, &[pk.clone(), pk]).is_ok());
    }

    #[test]
    fn legacy_signature_refused() {
        let (kp, pk) = throwaway();
        let sig = sign(&kp, b"data");
        let lines: Vec<&str> = sig.lines().collect();
        let mut bin = STANDARD.decode(lines[1]).unwrap();
        bin[1] = b'd';
        let legacy = format!(
            "{}\n{}\n{}\n{}\n",
            lines[0],
            STANDARD.encode(&bin),
            lines[2],
            lines[3]
        );
        assert_eq!(
            verify_minisig(&pk, b"data", &legacy),
            Err(TrustError::Legacy)
        );
        assert_eq!(
            verify_bundle(b"m", b"c", &legacy, &[pk]),
            Err(TrustError::Legacy)
        );
    }

    #[test]
    fn unknown_key_names_its_id() {
        let (kp, pk) = throwaway();
        let (_, other) = throwaway();
        let sig = sign(&kp, signing_payload(b"m", b"c").as_bytes());
        assert_eq!(
            verify_bundle(b"m", b"c", &sig, &[other]),
            Err(TrustError::UnknownKey(pk.key_id))
        );
        assert_eq!(
            verify_bundle(b"m", b"c", &sig, &[]),
            Err(TrustError::UnknownKey(pk.key_id))
        );
    }

    #[test]
    fn oversize_signature_refused() {
        let (_, pk) = throwaway();
        let big = format!("untrusted comment: {}\n", "x".repeat(MAX_SIGNATURE_BYTES));
        assert_eq!(
            verify_bundle(b"m", b"c", &big, &[pk]),
            Err(TrustError::TooLarge)
        );
    }

    #[test]
    fn garbage_signature_malformed() {
        let (_, pk) = throwaway();
        for text in [
            "",
            "garbage",
            "untrusted comment: x\nnot-base64!\n",
            "a\nb\nc\nd\n",
        ] {
            assert!(
                matches!(
                    verify_bundle(b"m", b"c", text, std::slice::from_ref(&pk)),
                    Err(TrustError::Malformed(_))
                ),
                "{text:?}"
            );
        }
        // Valid first lines, broken trusted comment.
        let (kp, pk) = throwaway();
        let sig = sign(&kp, signing_payload(b"m", b"c").as_bytes());
        let broken: String = sig.lines().take(2).map(|l| format!("{l}\n")).collect();
        assert!(matches!(
            verify_bundle(b"m", b"c", &broken, &[pk]),
            Err(TrustError::Malformed(_))
        ));
    }

    #[test]
    fn tampered_trusted_comment_mismatch() {
        let (kp, pk) = throwaway();
        let sig = sign(&kp, b"data");
        let tampered = sig.replacen("\ntrusted comment: ", "\ntrusted comment: x", 1);
        assert_eq!(
            verify_minisig(&pk, b"data", &tampered),
            Err(TrustError::Mismatch)
        );
    }
}
