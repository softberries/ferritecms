//! Named locks on Postgres (04 §7, P13): a session-level advisory lock on a dedicated
//! connection per held lock. Closing that connection releases the lock for certain,
//! including when the process crashes; a pooled connection could carry it back into the
//! pool. Advisory locks are database-wide, so the key includes the schema.

use std::time::Duration;

use async_trait::async_trait;
use ferrite_store::{LockGuard, StoreError};
use sqlx::{
    Connection, Either, PgConnection,
    postgres::{PgAdvisoryLock, PgAdvisoryLockGuard, PgConnectOptions},
};

use crate::exec::map_db_err;

/// Bound on one attempt (connect + `pg_try_advisory_lock`). `Store::try_lock` does not
/// know how long its caller waits, so this is fixed; an attempt that runs out counts as
/// "not acquired" (`Ok(None)`), and `ferrite_store::lock` keeps polling until its own
/// deadline (so one slow attempt can overrun that deadline by up to this much).
pub(crate) const LOCK_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

/// Session settings of every lock connection: TCP keepalives so the server notices a
/// vanished holder within about 25 s (10 s idle + 3 × 5 s probes) and frees the lock, and
/// `idle_session_timeout` off because the connection sits idle while it holds the lock.
/// (Keepalives apply over TCP only; on a Unix socket the server ignores them.)
const LOCK_SESSION_OPTIONS: [(&str, &str); 4] = [
    ("tcp_keepalives_idle", "10"),
    ("tcp_keepalives_interval", "5"),
    ("tcp_keepalives_count", "3"),
    ("idle_session_timeout", "0"),
];

/// The key string of lock `name` in `schema` (empty without one); sqlx derives the
/// `i64` advisory key from it with HKDF.
pub(crate) fn lock_key(schema: Option<&str>, name: &str) -> String {
    format!("ferrite:{}:{name}", schema.unwrap_or_default())
}

/// How a store opens its lock connections.
pub(crate) struct Locks {
    /// Connection options without the app pool's session settings (`try_acquire` is one
    /// quick statement outside any transaction, and the connection then sits idle).
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

    /// Options of the connection for lock `name`: `application_name`
    /// `<app>:lock:<name>` plus [`LOCK_SESSION_OPTIONS`].
    fn options(&self, name: &str) -> PgConnectOptions {
        self.connect
            .clone()
            .application_name(&format!("{}:lock:{name}", self.application_name))
            .options(LOCK_SESSION_OPTIONS)
    }

    /// Opens a connection for `name` and tries the lock on it, within
    /// [`LOCK_ATTEMPT_TIMEOUT`]; the caller has validated the name. Not taken (or timed
    /// out): `Ok(None)`, and the connection is closed again (a failed close is logged,
    /// the server ends the session anyway).
    pub(crate) async fn try_lock(
        &self,
        name: &str,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        let opts = self.options(name);
        let lock = PgAdvisoryLock::new(lock_key(self.schema.as_deref(), name));
        let attempt = async {
            let conn = PgConnection::connect_with(&opts)
                .await
                .map_err(map_db_err)?;
            lock.try_acquire(conn).await.map_err(map_db_err)
        };
        // A timed-out attempt drops its connection, which also frees a lock the server
        // may have granted without the reply arriving.
        let Ok(acquired) = tokio::time::timeout(LOCK_ATTEMPT_TIMEOUT, attempt).await else {
            tracing::warn!(lock = name, timeout = ?LOCK_ATTEMPT_TIMEOUT, "lock attempt timed out");
            return Ok(None);
        };
        match acquired? {
            Either::Left(guard) => Ok(Some(Box::new(PgLockGuard { guard }))),
            Either::Right(conn) => {
                if let Err(e) = conn.close().await {
                    tracing::warn!(lock = name, error = %e, "closing a lock connection failed");
                }
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

    /// Pins sqlx's key derivation: if a sqlx upgrade changed it, nodes on the old and new
    /// binary would take different advisory locks for the same name (cross-checked with
    /// an independent HKDF-SHA256 computation).
    #[test]
    fn lock_keys_are_golden() {
        use sqlx::postgres::PgAdvisoryLockKey;
        assert_eq!(
            PgAdvisoryLock::new(lock_key(None, "migrate")).key(),
            &PgAdvisoryLockKey::BigInt(2_995_534_703_458_433_807)
        );
        assert_eq!(
            PgAdvisoryLock::new(lock_key(Some("site_a"), "migrate")).key(),
            &PgAdvisoryLockKey::BigInt(-8_529_774_564_528_246_706)
        );
    }

    /// The lock connection's session settings, as the server reports them.
    #[tokio::test]
    async fn lock_connection_settings() {
        let Ok(url) = std::env::var("FERRITE_TEST_PG_URL") else {
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("FERRITE_TEST_PG_URL must be set in CI");
            }
            eprintln!("SKIP: Postgres test skipped (FERRITE_TEST_PG_URL is not set)");
            return;
        };
        let base: PgConnectOptions = url.parse().unwrap();
        let locks = Locks::new(base, "t_settings", None);
        let mut conn = PgConnection::connect_with(&locks.options("migrate"))
            .await
            .unwrap();
        let row: (String, String, String, String, String, bool) = sqlx::query_as(
            "SELECT current_setting('application_name'), \
             current_setting('tcp_keepalives_idle'), \
             current_setting('tcp_keepalives_interval'), \
             current_setting('tcp_keepalives_count'), \
             current_setting('idle_session_timeout'), \
             inet_client_addr() IS NOT NULL",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();
        let (app, idle, interval, count, idle_session, tcp) = row;
        assert_eq!(app, "t_settings:lock:migrate");
        assert_eq!(idle_session, "0");
        // Over a Unix socket the server reports 0 for the keepalive settings.
        if tcp {
            assert_eq!(
                (idle.as_str(), interval.as_str(), count.as_str()),
                ("10", "5", "3")
            );
        } else {
            eprintln!("note: Unix-socket connection, keepalive settings not checked");
        }
    }
}
