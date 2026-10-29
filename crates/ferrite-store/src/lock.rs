//! Coarse named locks (design 04 §7, rulings P1, P13): held across nodes on Postgres
//! (session advisory locks), in process on SQLite.

use std::time::Duration;

use async_trait::async_trait;

use crate::{Store, StoreError};

/// Held by `ferrite migrate` and by `serve` while it migrates at startup.
pub const LOCK_MIGRATE: &str = "migrate";
/// Held while a theme or plugin is installed or activated.
pub const LOCK_EXTENSIONS: &str = "extensions";
/// Held while a content type or taxonomy is registered, changed or removed (M2a P18).
pub const LOCK_TYPES: &str = "types";
/// Reserved for the WordPress import (M3): one at a time per site.
pub const LOCK_IMPORT: &str = "import";

/// How often the default [`Store::lock_wait`] retries [`Store::try_lock`] while the lock is taken.
pub const LOCK_POLL: Duration = Duration::from_millis(250);

/// A held lock. Dropping it also releases it (Postgres: the dedicated connection
/// closes, so the server releases it; a crashed process releases it the same way).
/// Prefer `release().await` over drop: it frees the lock before returning and reports
/// errors, while a drop leaves the release to the server noticing the closed connection.
///
/// **Not a fence.** On Postgres the holder is not told when it loses the lock: if the
/// lock connection is terminated (`pg_terminate_backend`, a failover, a proxy or pooler
/// that closes idle connections), the server frees the lock and another process can
/// take it while the first one still runs and believes it holds it. A lock serialises
/// cooperating processes in the normal case only. Work that must stay exclusive while
/// the lock is lost needs its own check in the data it writes (a version or token
/// column, a short `Tx`), not the lock; today `migrate` relies on sqlx's own advisory
/// lock and per-migration transactions, and `extensions` writes are one short `Tx`. The
/// M3 `import` lock, held for a long time, must not rely on it for exclusion.
/// `Sync` too: an import batch holds one, and the futures of its `&self` methods must
/// be `Send` for drivers on a multi-threaded runtime.
#[async_trait]
pub trait LockGuard: Send + Sync {
    /// Releases the lock now and reports a failure to do so cleanly (the lock is gone
    /// either way once the guard is consumed).
    async fn release(self: Box<Self>) -> Result<(), StoreError>;
}

/// Validates a lock name: 1 to 64 characters from `[a-z0-9._-]`, else `Invalid`.
pub fn check_lock_name(name: &str) -> Result<(), StoreError> {
    let ok = (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!(
            "lock name {name:?} must be 1 to 64 characters from [a-z0-9._-]"
        )))
    }
}

/// Takes the lock `name`, waiting up to `wait` ([`Store::lock_wait`]); then
/// [`StoreError::Busy`]. Postgres waits on one connection with `lock_timeout`; the
/// default (SQLite) retries [`Store::try_lock`] every [`LOCK_POLL`]. The first attempt
/// is immediate. Cancel-safe: dropping the future gives up the wait and leaks no lock.
pub async fn lock(
    store: &dyn Store,
    name: &str,
    wait: Duration,
) -> Result<Box<dyn LockGuard>, StoreError> {
    store.lock_wait(name, wait).await?.ok_or(StoreError::Busy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_names() {
        for ok in [
            LOCK_MIGRATE,
            LOCK_EXTENSIONS,
            LOCK_IMPORT,
            LOCK_TYPES,
            "a.b_c-9",
            &"x".repeat(64),
        ] {
            check_lock_name(ok).unwrap();
        }
        for bad in ["", "Bad Name", "UPPER", "a/b", "é", &"x".repeat(65)] {
            assert!(
                matches!(check_lock_name(bad), Err(StoreError::Invalid(_))),
                "{bad:?}"
            );
        }
    }
}
