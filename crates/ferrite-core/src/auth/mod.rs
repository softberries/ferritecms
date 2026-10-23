//! Authentication building blocks (M2a-1 P4): the site key, per-purpose subkeys, sealed
//! values and keyed IP hashes.

mod key;
mod seal;

pub use key::{IP_HASH_LABEL, SEAL_LABEL, SiteKey, ip_hash, random_bytes};
pub use seal::{open, seal};
