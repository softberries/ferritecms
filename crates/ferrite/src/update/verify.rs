//! Release verification (M4-2 P26): a minisign signature over `SHA256SUMS`, made with the
//! release key embedded from `crates/ferrite/release.pub`, then the asset's sha256 from
//! that file.

use anyhow::{Context, Result, anyhow, bail};
use ferrite_domain::plugin_trust::PublisherKey;
use ferrite_plugin_host::trust::{TrustError, verify_minisig};

/// The embedded release key file: minisign's two lines since the release gate (key id
/// 432051207B9BA065); before it, a comment-only placeholder.
const RELEASE_PUB: &str = include_str!("../../release.pub");

/// The release key this binary trusts; `None` for a build without one (a placeholder file).
/// Under `test-seams`, `FERRITE_TEST_RELEASE_PUBKEY` replaces the embedded file.
pub(super) fn release_key() -> Result<Option<PublisherKey>> {
    #[cfg(feature = "test-seams")]
    if let Ok(text) = std::env::var("FERRITE_TEST_RELEASE_PUBKEY") {
        return parse_key(&text);
    }
    parse_key(RELEASE_PUB)
}

/// Parses a minisign public key file (`untrusted comment: …` + base64 line) or a bare
/// base64 key. Blank lines and lines starting with `#` are ignored; nothing else left
/// means no key.
fn parse_key(text: &str) -> Result<Option<PublisherKey>> {
    PublisherKey::parse(text).map_err(|e| anyhow!("the embedded release key is malformed: {e}"))
}

/// Checks `signature` (a `.minisig` file) over `sums` with `key`. Only prehashed
/// signatures (minisign's default since 0.8) are accepted.
pub(super) fn check_signature(key: &PublisherKey, sums: &[u8], signature: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(signature)
        .map_err(|_| anyhow!("the SHA256SUMS signature is not valid minisign text"))?;
    verify_minisig(key, sums, text).map_err(|e| match e {
        TrustError::Mismatch | TrustError::UnknownKey(_) => {
            anyhow!("the SHA256SUMS signature does not verify with the release key: {e}")
        }
        other => anyhow!("the SHA256SUMS signature is malformed: {other}"),
    })
}

/// The sha256 (lowercase hex) that `sums` (`sha256sum` output) lists for `name`.
pub(super) fn listed_sha256(sums: &[u8], name: &str) -> Result<String> {
    let text = std::str::from_utf8(sums).context("SHA256SUMS is not UTF-8")?;
    for line in text.lines() {
        let Some((hash, file)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        // `sha256sum` writes "<hash>  <name>", or "<hash> *<name>" in binary mode.
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        if file == name {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("SHA256SUMS has a malformed hash for {name}");
            }
            return Ok(hash.to_ascii_lowercase());
        }
    }
    bail!("SHA256SUMS does not list {name}")
}

/// Lowercase hex.
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The placeholder's text, as the repository held it before the release key was added.
    const PLACEHOLDER: &str = "\
# FerriteCMS release public key (minisign), embedded by `ferrite self-update` (M4-2 P26).
# This is a placeholder and holds no key: a binary built from it refuses to self-update.
";

    /// The embedded file holds the release key (added by the release gate).
    #[test]
    fn embedded_key_file_holds_the_release_key() {
        assert!(parse_key(RELEASE_PUB).unwrap().is_some());
    }

    #[test]
    fn placeholder_and_comments_mean_no_key() {
        assert!(parse_key(PLACEHOLDER).unwrap().is_none());
        assert!(parse_key("").unwrap().is_none());
        assert!(parse_key("# only a comment\n\n").unwrap().is_none());
    }

    #[test]
    fn signature_round_trip() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key_file = kp.pk.to_box().unwrap().into_string();
        let key = parse_key(&key_file).unwrap().unwrap();
        // The bare base64 line works too.
        assert!(parse_key(&kp.pk.to_base64()).unwrap().is_some());
        let sums = b"0000  a.tar.gz\n";
        let sig = minisign::sign(Some(&kp.pk), &kp.sk, &sums[..], None, None)
            .unwrap()
            .into_string();
        check_signature(&key, sums, sig.as_bytes()).unwrap();
        let err = check_signature(&key, b"1111  a.tar.gz\n", sig.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("does not verify"), "{err}");
        assert!(check_signature(&key, sums, b"garbage").is_err());
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let other_key = parse_key(&other.pk.to_box().unwrap().into_string())
            .unwrap()
            .unwrap();
        assert!(check_signature(&other_key, sums, sig.as_bytes()).is_err());
    }

    #[test]
    fn malformed_keys_are_errors() {
        assert!(parse_key("not base64 at all").is_err());
        assert!(parse_key("a\nb\nc").is_err());
    }

    #[test]
    fn listed_sha256_finds_both_formats() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  one.tar.gz\n{b} *two.tar.gz\n");
        assert_eq!(listed_sha256(sums.as_bytes(), "one.tar.gz").unwrap(), a);
        assert_eq!(
            listed_sha256(sums.as_bytes(), "two.tar.gz").unwrap(),
            "b".repeat(64)
        );
        let err = listed_sha256(sums.as_bytes(), "three.tar.gz").unwrap_err();
        assert!(err.to_string().contains("does not list"), "{err}");
        let bad = "xyz  one.tar.gz\n";
        assert!(listed_sha256(bad.as_bytes(), "one.tar.gz").is_err());
    }
}
