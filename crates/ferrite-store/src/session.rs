//! The public id of a session (M2b-3 P45).

use ferrite_domain::to_hex;
use sha2::{Digest, Sha256};

/// Length of a session's public id, in hex characters.
const SESSION_PUBLIC_ID_LEN: usize = 16;

/// The id a session is shown and addressed by: the first 16 lowercase hex characters of
/// SHA-256 over its stored `id_hash` (itself the SHA-256 of the cookie value). It names
/// a session to its owner without revealing the stored key or the cookie.
pub fn session_public_id(id_hash: &str) -> String {
    let mut hex = to_hex(&Sha256::digest(id_hash.as_bytes()));
    hex.truncate(SESSION_PUBLIC_ID_LEN);
    hex
}
