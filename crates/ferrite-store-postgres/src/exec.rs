//! Where repository statements run: a pool (autocommit) or an open transaction.
//!
//! **Poisoning (R18).** Any driver error from a statement run inside a `Tx` poisons the
//! level it ran on: every later call through that level returns
//! [`StoreError::Aborted`] and its `commit` rolls back and returns `Err(Aborted)`.
//! Postgres aborts a transaction after any statement error and SQLite may roll one back
//! on its own (e.g. `SQLITE_FULL`), so treating every database error as fatal is the only
//! rule both engines can honour. Validation errors raised before any SQL is sent do not
//! poison. Failures inside a savepoint (`atomic!`, or a nested `Tx` from
//! `begin_nested`) are contained: the savepoint is rolled back and only that level is
//! affected, unless rolling back to the savepoint itself fails, which poisons the parent.

use std::sync::{Arc, PoisonError};

use ferrite_store::StoreError;
use sqlx::{AssertSqlSafe, PgPool, Postgres, Transaction};
use tokio::sync::{Mutex, MutexGuard};

/// The transaction shared by every level and repository of one `Tx`.
pub(crate) struct Shared {
    /// `None` once the transaction is committed or rolled back.
    conn: Mutex<Option<Transaction<'static, Postgres>>>,
    /// Open levels, outermost first. A std mutex so `Drop` can update it synchronously;
    /// it is never held across an `.await`.
    levels: std::sync::Mutex<Levels>,
}

#[derive(Default)]
struct Levels {
    open: Vec<Level>,
    next_id: u64,
    /// Level 0 was abandoned, committed or rolled back: the whole transaction is over and
    /// every nested handle still alive reports `Aborted`.
    dead: bool,
}

struct Level {
    id: u64,
    poisoned: bool,
    /// Dropped without commit/rollback; rolled back before the next statement.
    abandoned: bool,
}

/// One level of a transaction: level 0 is the transaction itself, level `n > 0` the
/// savepoint `ferrite_sp_<n>` opened by `begin_nested`.
#[derive(Clone)]
pub(crate) struct TxHandle {
    shared: Arc<Shared>,
    level: usize,
    id: u64,
}

pub(crate) enum Exec {
    Pool(PgPool),
    Tx(TxHandle),
}

type ConnGuard<'a> = MutexGuard<'a, Option<Transaction<'static, Postgres>>>;

impl Shared {
    fn levels(&self) -> std::sync::MutexGuard<'_, Levels> {
        self.levels.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TxHandle {
    /// Level 0 of a freshly begun transaction.
    pub(crate) fn new(tx: Transaction<'static, Postgres>) -> Self {
        let shared = Arc::new(Shared {
            conn: Mutex::new(Some(tx)),
            levels: std::sync::Mutex::new(Levels {
                open: vec![Level {
                    id: 0,
                    poisoned: false,
                    abandoned: false,
                }],
                next_id: 1,
                dead: false,
            }),
        });
        Self {
            shared,
            level: 0,
            id: 0,
        }
    }

    /// Locks the connection for a statement on this level: rolls back abandoned nested
    /// levels first, then checks this level is the innermost open one and not poisoned.
    pub(crate) async fn enter(&self) -> Result<ConnGuard<'_>, StoreError> {
        let mut guard = self.shared.conn.lock().await;
        self.release_if_dead(&mut guard);
        self.settle(&mut guard).await;
        self.check()?;
        if guard.is_none() {
            return Err(finished());
        }
        Ok(guard)
    }

    /// Marks this level poisoned (a statement on it failed in the database).
    pub(crate) fn poison(&self) {
        let mut levels = self.shared.levels();
        if let Some(level) = levels.open.get_mut(self.level)
            && level.id == self.id
        {
            level.poisoned = true;
        }
    }

    /// Opens a savepoint one level deeper.
    pub(crate) async fn nested(&self) -> Result<TxHandle, StoreError> {
        let mut guard = self.enter().await?;
        let depth = self.level + 1;
        let Some(tx) = guard.as_mut() else {
            return Err(finished());
        };
        if let Err(e) = savepoint_sql(tx, &format!("SAVEPOINT ferrite_sp_{depth}")).await {
            self.poison();
            return Err(e);
        }
        let mut levels = self.shared.levels();
        let id = levels.next_id;
        levels.next_id += 1;
        levels.open.push(Level {
            id,
            poisoned: false,
            abandoned: false,
        });
        Ok(TxHandle {
            shared: self.shared.clone(),
            level: depth,
            id,
        })
    }

    /// Commits (`commit = true`) or rolls back this level. A poisoned level always rolls
    /// back; committing it returns `Err(Aborted)`.
    pub(crate) async fn finish(&self, commit: bool) -> Result<(), StoreError> {
        let mut guard = self.shared.conn.lock().await;
        self.release_if_dead(&mut guard);
        self.settle(&mut guard).await;
        let (poisoned, nested_open) = match self.check() {
            Ok(()) => (false, false),
            Err(StoreError::Aborted) => (true, false),
            // Level 0 ends the whole transaction even with a nested level alive.
            Err(StoreError::NestedOpen) if self.level == 0 => (false, true),
            Err(e) => return Err(e),
        };
        if poisoned && guard.is_none() {
            // The transaction already ended (level 0 is gone): nothing left to undo.
            return if commit {
                Err(StoreError::Aborted)
            } else {
                Ok(())
            };
        }
        let result = if self.level == 0 {
            let tx = guard.take().ok_or_else(finished)?;
            {
                let mut levels = self.shared.levels();
                levels.open.clear();
                levels.dead = true;
            }
            if commit && !poisoned && !nested_open {
                tx.commit().await.map_err(map_db_err)
            } else {
                tx.rollback().await.map_err(map_db_err)
            }
        } else {
            let tx = guard.as_mut().ok_or_else(finished)?;
            let name = format!("ferrite_sp_{}", self.level);
            let done = if commit && !poisoned {
                savepoint_sql(tx, &format!("RELEASE SAVEPOINT {name}")).await
            } else {
                rollback_to(tx, &name).await
            };
            let mut levels = self.shared.levels();
            levels.open.truncate(self.level);
            if done.is_err()
                && let Some(parent) = levels.open.last_mut()
            {
                parent.poisoned = true;
            }
            done
        };
        // A poisoned level's rollback may fail because the engine already ended the
        // transaction; either way the work is gone.
        match (poisoned, commit) {
            (true, true) => Err(StoreError::Aborted),
            (true, false) => Ok(()),
            // Committing level 0 with a nested level alive rolls everything back.
            (false, true) if nested_open => result.and(Err(StoreError::NestedOpen)),
            (false, _) => result,
        }
    }

    /// Called when the owning `Tx` is dropped. A nested level is rolled back lazily,
    /// before the next statement. Level 0 ends the whole transaction: nested handles
    /// still alive get `Aborted`, and the transaction (hence the connection) is released
    /// now unless a statement holds it, in which case the next call on any handle, or the
    /// last handle dropping, releases it.
    pub(crate) fn abandon(&self) {
        if self.level == 0 {
            self.shared.levels().dead = true;
            if let Ok(mut guard) = self.shared.conn.try_lock() {
                drop(guard.take());
            }
            return;
        }
        let mut levels = self.shared.levels();
        if let Some(level) = levels.open.get_mut(self.level)
            && level.id == self.id
        {
            level.abandoned = true;
        }
    }

    fn check(&self) -> Result<(), StoreError> {
        let levels = self.shared.levels();
        if levels.dead {
            return Err(StoreError::Aborted);
        }
        match levels.open.get(self.level) {
            Some(level) if level.id == self.id => {
                if levels.open.len() > self.level + 1 {
                    Err(StoreError::NestedOpen)
                } else if level.poisoned {
                    Err(StoreError::Aborted)
                } else {
                    Ok(())
                }
            }
            _ => Err(finished()),
        }
    }

    /// Drops (hence rolls back) the transaction once level 0 is over.
    fn release_if_dead(&self, guard: &mut ConnGuard<'_>) {
        if self.shared.levels().dead {
            drop(guard.take());
        }
    }

    /// Rolls back abandoned nested levels from the innermost outwards.
    async fn settle(&self, guard: &mut ConnGuard<'_>) {
        loop {
            let depth = {
                let levels = self.shared.levels();
                match levels.open.last() {
                    Some(level) if level.abandoned && levels.open.len() > 1 => {
                        levels.open.len() - 1
                    }
                    _ => return,
                }
            };
            let failed = match guard.as_mut() {
                Some(tx) => rollback_to(tx, &format!("ferrite_sp_{depth}"))
                    .await
                    .is_err(),
                None => false,
            };
            let mut levels = self.shared.levels();
            levels.open.truncate(depth);
            if failed && let Some(parent) = levels.open.last_mut() {
                parent.poisoned = true;
            }
        }
    }
}

async fn savepoint_sql(
    tx: &mut Transaction<'static, Postgres>,
    sql: &str,
) -> Result<(), StoreError> {
    sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(map_db_err)
}

/// Undoes everything since savepoint `name` and removes it.
async fn rollback_to(
    tx: &mut Transaction<'static, Postgres>,
    name: &str,
) -> Result<(), StoreError> {
    savepoint_sql(tx, &format!("ROLLBACK TO SAVEPOINT {name}")).await?;
    savepoint_sql(tx, &format!("RELEASE SAVEPOINT {name}")).await
}

/// Maps a driver error: SQLSTATE 23505 becomes `UniqueViolation` carrying the
/// constraint name (named identically in both 0001 migrations), 23503 `ForeignKey` with
/// an empty name (SQLite cannot name it, so neither backend does), and a pool acquire
/// timeout becomes `Busy`.
pub(crate) fn map_db_err(e: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(db) = &e {
        match db.code().as_deref() {
            Some("23505") => {
                return StoreError::UniqueViolation {
                    constraint: db.constraint().unwrap_or_default().to_owned(),
                };
            }
            Some("23503") => {
                tracing::debug!(constraint = db.constraint(), "foreign key violated");
                return StoreError::ForeignKey {
                    constraint: String::new(),
                };
            }
            _ => {}
        }
    }
    if matches!(e, sqlx::Error::PoolTimedOut) {
        return StoreError::Busy;
    }
    StoreError::Database(e.to_string())
}

pub(crate) fn finished() -> StoreError {
    StoreError::Invalid("transaction already finished".into())
}

/// `run!(&self.exec, |e| sqlx::query(..).bind(..).fetch_one(e))`: evaluates the query
/// future with `e` bound to the pool or to the transaction's connection, awaits it and
/// maps the error to [`StoreError`]. Inside a `Tx` a failed statement poisons the level.
macro_rules! run {
    ($exec:expr, |$e:ident| $body:expr) => {
        match $exec {
            $crate::exec::Exec::Pool(pool) => {
                let $e = pool;
                $body.await.map_err($crate::exec::map_db_err)
            }
            $crate::exec::Exec::Tx(h) => match h.enter().await {
                Ok(mut guard) => match guard.as_mut() {
                    Some(t) => {
                        let $e = &mut **t;
                        let out = $body.await;
                        if out.is_err() {
                            h.poison();
                        }
                        out.map_err($crate::exec::map_db_err)
                    }
                    None => Err($crate::exec::finished()),
                },
                Err(e) => Err(e),
            },
        }
    };
}
pub(crate) use run;

/// `atomic!(&self.exec, |conn| async_body)`: runs a multi-statement body atomically.
/// On a pool it opens its own transaction and commits it; inside a caller's `Tx` it opens
/// a savepoint (nested transaction) and releases it, so a failure rolls back only the body
/// and leaves the outer transaction usable (R11, R18). `conn` is `&mut` the connection;
/// the body is an async block returning `Result<_, StoreError>`.
macro_rules! atomic {
    ($exec:expr, |$c:ident| $body:expr) => {
        match $exec {
            $crate::exec::Exec::Pool(pool) => {
                let mut tx = pool.begin().await.map_err($crate::exec::map_db_err)?;
                let out = {
                    let $c = &mut *tx;
                    $body.await
                };
                match out {
                    Ok(v) => {
                        tx.commit().await.map_err($crate::exec::map_db_err)?;
                        Ok(v)
                    }
                    Err(e) => {
                        let _ = tx.rollback().await;
                        Err(e)
                    }
                }
            }
            $crate::exec::Exec::Tx(h) => match h.enter().await {
                Ok(mut guard) => match guard.as_mut() {
                    Some(t) => match sqlx::Connection::begin(&mut **t).await {
                        Err(e) => {
                            h.poison();
                            Err($crate::exec::map_db_err(e))
                        }
                        Ok(mut sp) => {
                            let out = {
                                let $c = &mut *sp;
                                $body.await
                            };
                            match out {
                                Ok(v) => match sp.commit().await {
                                    Ok(()) => Ok(v),
                                    Err(e) => {
                                        h.poison();
                                        Err($crate::exec::map_db_err(e))
                                    }
                                },
                                Err(e) => {
                                    // If the savepoint is gone (the engine rolled the whole
                                    // transaction back), the outer level is poisoned.
                                    if sp.rollback().await.is_err() {
                                        h.poison();
                                    }
                                    Err(e)
                                }
                            }
                        }
                    },
                    None => Err($crate::exec::finished()),
                },
                Err(e) => Err(e),
            },
        }
    };
}
pub(crate) use atomic;

#[cfg(test)]
mod tests {
    use super::*;

    /// Level 0 dropped while a nested statement holds the connection lock (so `abandon`
    /// cannot release it), and again with a parked nested handle that is never called:
    /// the connection is back once the last handle is gone, nothing is persisted.
    async fn lazy_release(pool: PgPool) {
        for in_flight in [true, false] {
            let outer = TxHandle::new(pool.begin().await.unwrap());
            let nested = outer.nested().await.unwrap();
            {
                let mut guard = nested.enter().await.unwrap();
                let tx = guard.as_mut().unwrap();
                sqlx::raw_sql("CREATE TABLE IF NOT EXISTS lazy_t (a INT)")
                    .execute(&mut **tx)
                    .await
                    .unwrap();
            }
            let held = if in_flight {
                Some(nested.enter().await.unwrap())
            } else {
                None
            };
            outer.abandon();
            drop(held);
            if in_flight {
                // The next call releases the transaction and reports Aborted.
                assert!(matches!(
                    nested.enter().await.err(),
                    Some(StoreError::Aborted)
                ));
            }
            drop(nested);
            let tx = tokio::time::timeout(std::time::Duration::from_secs(6), pool.begin())
                .await
                .expect("connection must be free")
                .unwrap();
            let exists: bool = sqlx::query_scalar("SELECT to_regclass('pg_temp.lazy_t') IS NOT NULL OR to_regclass('lazy_t') IS NOT NULL")
                .fetch_one(&mut *{ tx })
                .await
                .unwrap();
            assert!(!exists, "nothing persisted");
        }
    }

    #[tokio::test]
    async fn lazy_release_after_outer_drop() {
        let Ok(url) = std::env::var("FERRITE_TEST_PG_URL") else {
            // Same rule as the contract-test harness: skipping is not allowed in CI.
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("FERRITE_TEST_PG_URL must be set in CI");
            }
            eprintln!("SKIP: Postgres test skipped (FERRITE_TEST_PG_URL is not set)");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect(&url)
            .await
            .unwrap();
        lazy_release(pool).await;
    }
}
