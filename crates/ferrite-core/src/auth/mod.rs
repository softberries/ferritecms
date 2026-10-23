//! Authentication building blocks (M2a-1 P4, P15): the site key, per-purpose subkeys,
//! sealed values, keyed IP hashes and password hashing.

mod key;
pub mod password;
mod seal;

pub use key::{IP_HASH_LABEL, SEAL_LABEL, SiteKey, ip_hash, random_bytes};
pub use seal::{open, seal};
