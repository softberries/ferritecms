//! Authentication (M2a-1 P4–P9, P15): the site key, per-purpose subkeys, sealed values,
//! keyed IP hashes, password hashing, and [`AuthService`] (logins, TOTP, sessions, API
//! tokens, password reset by e-mail and the DB-backed rate limits).

mod key;
mod limits;
pub mod password;
mod reset;
mod seal;
mod service;
mod totp;

pub use key::{
    COMMENT_IP_LABEL, IP_HASH_LABEL, PLUGIN_IP_LABEL, SEAL_LABEL, SiteKey, ip_hash, random_bytes,
};
pub use seal::{open, seal};
pub use service::{
    AuthService, Authenticated, Clock, IssuedSession, LoginAttempt, LoginOutcome, TotpSetup, Via,
};
