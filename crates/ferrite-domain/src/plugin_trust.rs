//! Plugin trust (M6-2, design 14): minisign public keys and key ids, publisher identities,
//! owner bindings, trusted keys and the registry cache row. Pure: parsing and formatting
//! only; the signature crypto lives in `ferrite-plugin-host::trust`.

use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::Timestamp;

/// The largest minisig file (or multipart part) a plugin install accepts.
pub const MAX_SIGNATURE_BYTES: usize = 4096;
/// The largest publisher key file (or multipart part) a plugin install accepts.
pub const MAX_PUBLISHER_KEY_BYTES: usize = 1024;

/// The signature file beside a plugin's manifest and component.
pub const SIGNATURE_FILE: &str = "ferrite-plugin.minisig";
/// The optional publisher key file beside it (trust on first use).
pub const PUBLISHER_KEY_FILE: &str = "ferrite-plugin.pub";

/// Why a key or a signature's text could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("{0}")]
    Malformed(String),
    #[error("the text is too large")]
    TooLarge,
}

/// A minisign key number: 8 bytes, written as 16 uppercase hex digits of the
/// little-endian number, exactly as minisign prints it (`432051207B9BA065`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId(u64);

impl KeyId {
    /// The key id from the 8 key-number bytes of a key or signature.
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        KeyId(u64::from_le_bytes(bytes))
    }

    /// Parses 16 hex digits (either case).
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        let text = text.trim();
        if text.len() != 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(KeyError::Malformed(
                "a key id is 16 hex digits, as minisign prints it".into(),
            ));
        }
        u64::from_str_radix(text, 16)
            .map(KeyId)
            .map_err(|e| KeyError::Malformed(e.to_string()))
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016X}", self.0)
    }
}

impl FromStr for KeyId {
    type Err = KeyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        KeyId::parse(s)
    }
}

/// A minisign Ed25519 public key: its id and its base64 line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherKey {
    pub key_id: KeyId,
    /// The base64 line (42 decoded bytes: `Ed`, key number, 32 key bytes).
    pub base64: String,
}

impl PublisherKey {
    /// Parses a minisign public key file (`untrusted comment:` line + base64 line) or a
    /// bare base64 line. Blank lines and lines starting with `#` are ignored; nothing left
    /// is `None` (a placeholder file).
    pub fn parse(text: &str) -> Result<Option<PublisherKey>, KeyError> {
        if text.len() > MAX_PUBLISHER_KEY_BYTES {
            return Err(KeyError::TooLarge);
        }
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        let b64 = match lines.as_slice() {
            [] => return Ok(None),
            [comment, b64] if comment.starts_with("untrusted comment:") => *b64,
            [b64] => *b64,
            _ => {
                return Err(KeyError::Malformed(
                    "a minisign public key is one comment line and one base64 line".into(),
                ));
            }
        };
        Self::from_base64(b64).map(Some)
    }

    /// Parses the bare base64 line.
    pub fn from_base64(b64: &str) -> Result<PublisherKey, KeyError> {
        let bin = STANDARD
            .decode(b64.trim())
            .map_err(|_| KeyError::Malformed("the public key is not base64".into()))?;
        if bin.len() != 42 {
            return Err(KeyError::Malformed(
                "the public key has the wrong length".into(),
            ));
        }
        if &bin[0..2] != b"Ed" {
            return Err(KeyError::Malformed(
                "the public key is not an Ed25519 minisign key".into(),
            ));
        }
        let mut id = [0u8; 8];
        id.copy_from_slice(&bin[2..10]);
        Ok(PublisherKey {
            key_id: KeyId::from_bytes(id),
            base64: STANDARD.encode(&bin),
        })
    }

    /// The key as a minisign public key file.
    pub fn to_file(&self) -> String {
        format!(
            "untrusted comment: minisign public key {}\n{}\n",
            self.key_id, self.base64
        )
    }
}

/// What the first two lines of a minisig say, read without verifying anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinisigHeader {
    pub key_id: KeyId,
    /// `ED` (prehashed, minisign's default since 0.8) rather than legacy `Ed`.
    pub prehashed: bool,
}

impl MinisigHeader {
    /// Reads the key id and the algorithm of a minisig's signature line.
    pub fn parse(text: &str) -> Result<MinisigHeader, KeyError> {
        if text.len() > MAX_SIGNATURE_BYTES {
            return Err(KeyError::TooLarge);
        }
        let mut lines = text.lines();
        let (Some(comment), Some(sig)) = (lines.next(), lines.next()) else {
            return Err(KeyError::Malformed(
                "a minisign signature has four lines".into(),
            ));
        };
        if !comment.starts_with("untrusted comment:") {
            return Err(KeyError::Malformed(
                "a minisign signature starts with an untrusted comment".into(),
            ));
        }
        let bin = STANDARD
            .decode(sig.trim())
            .map_err(|_| KeyError::Malformed("the signature line is not base64".into()))?;
        if bin.len() != 74 {
            return Err(KeyError::Malformed(
                "the signature line has the wrong length".into(),
            ));
        }
        let prehashed = match &bin[0..2] {
            b"ED" => true,
            b"Ed" => false,
            _ => {
                return Err(KeyError::Malformed(
                    "the signature uses an unknown algorithm".into(),
                ));
            }
        };
        let mut id = [0u8; 8];
        id.copy_from_slice(&bin[2..10]);
        Ok(MinisigHeader {
            key_id: KeyId::from_bytes(id),
            prehashed,
        })
    }
}

/// The identity a plugin name's data belongs to (design 14 §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OwnerId {
    /// A publisher listed by the registry signed with `registry_key`.
    Registry {
        registry_key: KeyId,
        publisher: String,
    },
    /// A key trusted on its own (approved at install or added by the operator).
    Key(KeyId),
}

impl fmt::Display for OwnerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OwnerId::Registry {
                registry_key,
                publisher,
            } => write!(f, "reg:{registry_key}:{publisher}"),
            OwnerId::Key(id) => write!(f, "key:{id}"),
        }
    }
}

impl FromStr for OwnerId {
    type Err = KeyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(id) = s.strip_prefix("key:") {
            return KeyId::parse(id).map(OwnerId::Key);
        }
        if let Some(rest) = s.strip_prefix("reg:")
            && let Some((key, publisher)) = rest.split_once(':')
            && !publisher.is_empty()
        {
            return Ok(OwnerId::Registry {
                registry_key: KeyId::parse(key)?,
                publisher: publisher.to_owned(),
            });
        }
        Err(KeyError::Malformed(format!("not an owner id: {s}")))
    }
}

/// Where a trusted key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// Approved by an admin at install (trust on first use).
    Approved,
    /// Added by the operator (`ferrite plugin keys trust`).
    Operator,
    /// Listed by the registry (a row exists only once it is revoked).
    Registry,
}

impl KeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Approved => "approved",
            KeySource::Operator => "operator",
            KeySource::Registry => "registry",
        }
    }

    pub fn parse(s: &str) -> Option<KeySource> {
        match s {
            "approved" => Some(KeySource::Approved),
            "operator" => Some(KeySource::Operator),
            "registry" => Some(KeySource::Registry),
            _ => None,
        }
    }
}

/// A key's revocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub at: Timestamp,
    pub reason: String,
    /// The key was stolen or leaked: plugins signed by it are disabled.
    pub compromised: bool,
}

/// A row of `plugin_publishers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub key: PublisherKey,
    pub label: String,
    pub source: KeySource,
    pub trusted_at: Timestamp,
    pub revoked: Option<Revocation>,
}

/// How a plugin's current version arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    Upload,
    Folder,
    Registry,
}

impl InstallSource {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallSource::Upload => "upload",
            InstallSource::Folder => "folder",
            InstallSource::Registry => "registry",
        }
    }

    pub fn parse(s: &str) -> Option<InstallSource> {
        match s {
            "upload" => Some(InstallSource::Upload),
            "folder" => Some(InstallSource::Folder),
            "registry" => Some(InstallSource::Registry),
            _ => None,
        }
    }
}

/// A row of `plugin_owners`: whose data a plugin name holds and how its current version
/// arrived. Outlives the `plugins` row while the data is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOwner {
    pub plugin: String,
    /// `None`: unsigned-owned.
    pub owner: Option<OwnerId>,
    /// The key that signed the installed version (`None` when unsigned).
    pub signer_key: Option<KeyId>,
    /// That key's base64 line: a key id alone does not identify a key (M6-2 review).
    pub signer_public_key: Option<String>,
    /// The installed version's minisig text.
    pub signature: Option<String>,
    /// The manifest's `publisher` text.
    pub publisher: String,
    pub source: InstallSource,
    pub registry_url: Option<String>,
    pub bound_at: Timestamp,
    pub updated_at: Timestamp,
}

impl PluginOwner {
    /// The installed version's signing key (`None` when unsigned, or the stored key text
    /// does not parse).
    pub fn signer(&self) -> Option<PublisherKey> {
        self.signer_public_key
            .as_deref()
            .and_then(|b| PublisherKey::from_base64(b).ok())
            .filter(|k| Some(k.key_id) == self.signer_key)
    }
}

/// `[plugins] signatures` (design 14 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignaturePolicy {
    #[default]
    Any,
    Signed,
    Trusted,
}

impl SignaturePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            SignaturePolicy::Any => "any",
            SignaturePolicy::Signed => "signed",
            SignaturePolicy::Trusted => "trusted",
        }
    }

    pub fn parse(s: &str) -> Option<SignaturePolicy> {
        match s {
            "any" => Some(SignaturePolicy::Any),
            "signed" => Some(SignaturePolicy::Signed),
            "trusted" => Some(SignaturePolicy::Trusted),
            _ => None,
        }
    }
}

/// A row of `plugin_registry`: the last verified index of a registry URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCacheRow {
    pub url: String,
    pub serial: i64,
    pub index_json: String,
    pub signature: String,
    pub generated_at: Timestamp,
    pub fetched_at: Timestamp,
    pub checked_at: Timestamp,
    pub last_error: Option<String>,
    pub last_error_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_PUB: &str = "untrusted comment: minisign public key 432051207B9BA065\n\
RWRloJt7IFEgQ3QWSMCPZT0k6xJ8KEMme02/HBySQSajqZ9ld96QHy9a\n";

    #[test]
    fn key_id_matches_minisign_display() {
        let key = PublisherKey::parse(RELEASE_PUB).unwrap().unwrap();
        assert_eq!(key.key_id.to_string(), "432051207B9BA065");
        assert_eq!(KeyId::parse("432051207b9ba065").unwrap(), key.key_id);
        assert_eq!(key.to_file(), RELEASE_PUB);
    }

    #[test]
    fn bare_and_file_keys_parse() {
        let bare = PublisherKey::parse("RWRloJt7IFEgQ3QWSMCPZT0k6xJ8KEMme02/HBySQSajqZ9ld96QHy9a")
            .unwrap()
            .unwrap();
        let file = PublisherKey::parse(RELEASE_PUB).unwrap().unwrap();
        assert_eq!(bare, file);
        assert!(PublisherKey::parse("# placeholder\n\n").unwrap().is_none());
        assert!(PublisherKey::parse("").unwrap().is_none());
    }

    #[test]
    fn hostile_keys_refused() {
        // Wrong algorithm: same bytes with `Ex`.
        let mut bin = STANDARD
            .decode("RWRloJt7IFEgQ3QWSMCPZT0k6xJ8KEMme02/HBySQSajqZ9ld96QHy9a")
            .unwrap();
        bin[1] = b'x';
        assert!(PublisherKey::from_base64(&STANDARD.encode(&bin)).is_err());
        // Short and long.
        assert!(PublisherKey::from_base64(&STANDARD.encode(&bin[..41])).is_err());
        let mut long = bin.clone();
        long.push(0);
        assert!(PublisherKey::from_base64(&STANDARD.encode(&long)).is_err());
        assert!(PublisherKey::parse("not base64 at all!").is_err());
        assert!(PublisherKey::parse("a\nb\nc").is_err());
        assert_eq!(
            PublisherKey::parse(&"A".repeat(2048)),
            Err(KeyError::TooLarge)
        );
        assert!(KeyId::parse("XYZ").is_err());
        assert!(KeyId::parse("432051207B9BA06").is_err());
    }

    #[test]
    fn owner_ids_round_trip() {
        let id = KeyId::parse("432051207B9BA065").unwrap();
        for owner in [
            OwnerId::Key(id),
            OwnerId::Registry {
                registry_key: id,
                publisher: "ferritecms.dev".into(),
            },
        ] {
            assert_eq!(owner.to_string().parse::<OwnerId>().unwrap(), owner);
        }
        assert_eq!(
            OwnerId::Key(id).to_string(),
            "key:432051207B9BA065".to_string()
        );
        assert!("reg:432051207B9BA065:".parse::<OwnerId>().is_err());
        assert!("other".parse::<OwnerId>().is_err());
    }

    #[test]
    fn minisig_header_reads_key_and_algorithm() {
        let mut bin = vec![b'E', b'D'];
        bin.extend_from_slice(&0x0123_4567_89AB_CDEFu64.to_le_bytes());
        bin.extend_from_slice(&[0u8; 64]);
        let text = format!(
            "untrusted comment: x\n{}\ntrusted comment: t\n{}\n",
            STANDARD.encode(&bin),
            STANDARD.encode([0u8; 64])
        );
        let h = MinisigHeader::parse(&text).unwrap();
        assert!(h.prehashed);
        assert_eq!(h.key_id.to_string(), "0123456789ABCDEF");
        bin[1] = b'd';
        let legacy = text.replace(text.lines().nth(1).unwrap(), &STANDARD.encode(&bin));
        assert!(!MinisigHeader::parse(&legacy).unwrap().prehashed);
        assert!(MinisigHeader::parse("garbage").is_err());
        assert_eq!(
            MinisigHeader::parse(&"x".repeat(5000)),
            Err(KeyError::TooLarge)
        );
    }
}
