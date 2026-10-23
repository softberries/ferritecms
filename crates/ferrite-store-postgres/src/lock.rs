//! Named locks on Postgres (04 §7, P13): a session-level advisory lock on a dedicated
//! connection per held lock. Closing that connection releases the lock for certain,
//! including when the process crashes; a pooled connection could carry it back into the
//! pool. Advisory locks are database-wide, so the key includes the schema.

use async_trait::async_trait;
use ferrite_store::{LockGuard, StoreError};
use sqlx::{
    Connection, Either, PgConnection,
    postgres::{PgAdvisoryLock, PgAdvisoryLockGuard, PgConnectOptions},
};

use crate::exec::map_db_err;

/// The key string of lock `name` in `schema` (empty without one); sqlx derives the
/// `i64` advisory key from it with HKDF.
pub(crate) fn lock_key(schema: Option<&str>, name: &str) -> String {
    format!("ferrite:{}:{name}", schema.unwrap_or_default())
}

/// How a store opens its lock connections.
pub(crate) struct Locks {
    /// Connection options without session settings (`try_acquire` is one quick
    /// statement outside any transaction, and the connection then sits idle).
    connect: PgConnectOptions,
    application_name: String,
    schema: Option<String>,
}

impl Locks {
    pub(crate) fn new(
        connect: PgConnectOptions,
        application_name: &str,
        schema: Option<&str>,
    ) -> Self {
        Self {
            connect,
            application_name: application_name.to_owned(),
            schema: schema.map(str::to_owned),
        }
    }

    /// Opens a connection named `<app>:lock:<name>` and tries the lock on it; the
    /// caller has validated the name. Not taken: the connection is closed again.
    pub(crate) async fn try_lock(
        &self,
        name: &str,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        let opts = self
            .connect
            .clone()
            .application_name(&format!("{}:lock:{name}", self.application_name));
        let conn = PgConnection::connect_with(&opts)
            .await
            .map_err(map_db_err)?;
        let lock = PgAdvisoryLock::new(lock_key(self.schema.as_deref(), name));
        match lock.try_acquire(conn).await.map_err(map_db_err)? {
            Either::Left(guard) => Ok(Some(Box::new(PgLockGuard { guard }))),
            Either::Right(conn) => {
                conn.close().await.map_err(map_db_err)?;
                Ok(None)
            }
        }
    }
}

/// Owns the lock's connection. Dropped: sqlx queues the unlock and the connection is
/// dropped, so the server ends the session and releases the lock either way.
struct PgLockGuard {
    guard: PgAdvisoryLockGuard<PgConnection>,
}

#[async_trait]
impl LockGuard for PgLockGuard {
    /// Unlocks, then closes the connection. If the connection is already gone (the
    /// session was terminated), the lock went with it and this reports the error.
    async fn release(self: Box<Self>) -> Result<(), StoreError> {
        let conn = self.guard.release_now().await.map_err(map_db_err)?;
        conn.close().await.map_err(map_db_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_scope_includes_schema() {
        let a = lock_key(Some("a"), "migrate");
        let b = lock_key(Some("b"), "migrate");
        assert_eq!(a, "ferrite:a:migrate");
        assert_eq!(lock_key(None, "migrate"), "ferrite::migrate");
        assert_ne!(a, b);
        assert_ne!(
            PgAdvisoryLock::new(&a).key(),
            PgAdvisoryLock::new(&b).key(),
            "the derived advisory keys differ too"
        );
        assert_ne!(lock_key(Some("a"), "import"), a);
    }
}
