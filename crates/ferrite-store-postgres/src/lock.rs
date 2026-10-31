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
/// "not acquired" (`Ok(None)`). It is also the margin added to a `lock_wait`'s own
/// `wait` as a safety net around the connect and the `set_config` call.
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

    /// Waits up to `wait` for lock `name` on one connection (P19): `SET lock_timeout`,
    /// then a blocking `pg_advisory_lock`, so the server wakes the waiter the moment the
    /// holder lets go, with no polling and no further connections. SQLSTATE `55P03`
    /// (lock_not_available) means the wait ran out: `Ok(None)`. After the grant,
    /// `lock_timeout` goes back to 0 so later statements on the connection are not cut
    /// short. A cancelled wait drops the connection, so a lock the server grants late is
    /// freed when the socket closes (the server notices a closed socket only when it next
    /// writes to it, so the abandoned backend can stay queued until `lock_timeout` fires
    /// or the lock is granted: bounded by `wait`). The caller has validated the name; `wait` is
    /// non-zero (0 would disable `lock_timeout`).
    pub(crate) async fn lock_wait(
        &self,
        name: &str,
        wait: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        Ok(self
            .wait_guard(name, wait)
            .await?
            .map(|guard| Box::new(PgLockGuard { guard }) as Box<dyn LockGuard>))
    }

    /// [`lock_wait`](Self::lock_wait) up to the guard.
    async fn wait_guard(
        &self,
        name: &str,
        wait: Duration,
    ) -> Result<Option<PgAdvisoryLockGuard<PgConnection>>, StoreError> {
        let opts = self.options(name);
        let lock = PgAdvisoryLock::new(lock_key(self.schema.as_deref(), name));
        let ms = wait.as_millis().clamp(1, i32::MAX as u128);
        let attempt = async {
            let mut conn = PgConnection::connect_with(&opts)
                .await
                .map_err(map_db_err)?;
            // `statement_timeout` off too: a role or database default shorter than
            // `wait` would otherwise cancel the blocking `pg_advisory_lock` (57014).
            sqlx::query(
                "SELECT set_config('lock_timeout', $1, false), \
                 set_config('statement_timeout', '0', false)",
            )
            .bind(format!("{ms}ms"))
            .execute(&mut conn)
            .await
            .map_err(map_db_err)?;
            let mut guard = match lock.acquire(conn).await {
                Ok(guard) => guard,
                Err(e) if is_lock_not_available(&e) => return Ok(None),
                Err(e) => return Err(map_db_err(e)),
            };
            sqlx::query("SET lock_timeout = 0")
                .execute(AsMut::<PgConnection>::as_mut(&mut guard))
                .await
                .map_err(map_db_err)?;
            Ok(Some(guard))
        };
        // Safety net only: `lock_timeout` ends the wait itself. Dropping the attempt
        // closes its connection.
        let budget = wait.saturating_add(LOCK_ATTEMPT_TIMEOUT);
        let Ok(result) = tokio::time::timeout(budget, attempt).await else {
            tracing::warn!(lock = name, ?wait, "lock wait timed out");
            return Ok(None);
        };
        result
    }
}

/// SQLSTATE 55P03: `lock_timeout` expired.
fn is_lock_not_available(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "55P03")
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

    /// After a granted wait the connection has `lock_timeout = 0` (reset) and
    /// `statement_timeout = 0`, even when the server default says otherwise.
    #[tokio::test]
    async fn lock_wait_leaves_timeouts_off() {
        let Ok(url) = std::env::var("FERRITE_TEST_PG_URL") else {
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("FERRITE_TEST_PG_URL must be set in CI");
            }
            eprintln!("SKIP: Postgres test skipped (FERRITE_TEST_PG_URL is not set)");
            return;
        };
        let base: PgConnectOptions = url.parse().unwrap();
        let base = base.options([("statement_timeout", "50")]);
        let locks = Locks::new(base, "t_timeouts", Some("t_timeouts_schema"));
        let mut guard = locks
            .wait_guard("w.timeouts", Duration::from_secs(5))
            .await
            .unwrap()
            .expect("free lock");
        let (lock_timeout, statement_timeout): (String, String) = sqlx::query_as(
            "SELECT current_setting('lock_timeout'), current_setting('statement_timeout')",
        )
        .fetch_one(AsMut::<PgConnection>::as_mut(&mut guard))
        .await
        .unwrap();
        assert_eq!(lock_timeout, "0");
        assert_eq!(statement_timeout, "0");
        let conn = guard.release_now().await.unwrap();
        conn.close().await.unwrap();
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
