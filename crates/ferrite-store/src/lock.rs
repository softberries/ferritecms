//! Coarse named locks (design 04 §7, rulings P1, P13): held across nodes on Postgres
//! (session advisory locks), in process on SQLite.

use std::time::Duration;

use async_trait::async_trait;

use crate::{Store, StoreError};

/// Held by `ferrite migrate` and by `serve` while it migrates at startup.
pub const LOCK_MIGRATE: &str = "migrate";
/// Held while a theme or plugin is installed or activated.
pub const LOCK_EXTENSIONS: &str = "extensions";
/// Reserved for the WordPress import (M3): one at a time per site.
pub const LOCK_IMPORT: &str = "import";

/// How often [`lock`] retries [`Store::try_lock`] while the lock is taken.
pub const LOCK_POLL: Duration = Duration::from_millis(250);

/// A held lock. Dropping it also releases it (Postgres: the dedicated connection
/// closes, so the server releases it; a crashed process releases it the same way).
/// Prefer `release().await` over drop: it frees the lock before returning and reports
/// errors, while a drop leaves the release to the server noticing the closed connection.
#[async_trait]
pub trait LockGuard: Send {
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

/// Takes the lock `name`, retrying [`Store::try_lock`] every [`LOCK_POLL`] until `wait`
/// has elapsed; then [`StoreError::Busy`]. The first attempt is immediate, the last one
/// is at `wait`.
pub async fn lock(
    store: &dyn Store,
    name: &str,
    wait: Duration,
) -> Result<Box<dyn LockGuard>, StoreError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if let Some(guard) = store.try_lock(name).await? {
            return Ok(guard);
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(StoreError::Busy);
        }
        tokio::time::sleep(LOCK_POLL.min(deadline - now)).await;
    }
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
