//! Where repository statements run: a pool (autocommit) or an open transaction.

use std::sync::Arc;

use ferrite_store::StoreError;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::sync::Mutex;

/// A transaction shared by the repositories of one `Tx`; `None` once finished.
pub(crate) type SharedTx = Arc<Mutex<Option<Transaction<'static, Sqlite>>>>;

pub(crate) enum Exec {
    Pool(SqlitePool),
    Tx(SharedTx),
}

/// Maps a driver error: unique and foreign-key violations become typed errors (SQLite
/// extended codes 2067/1555 and 787); the constraint is the text after
/// "constraint failed: " (the column list for a unique violation).
pub(crate) fn map_db_err(e: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(db) = &e {
        let message = db.message();
        let constraint = || {
            message
                .split_once("constraint failed: ")
                .map_or(message, |(_, rest)| rest)
                .to_owned()
        };
        match db.code().as_deref() {
            Some("2067" | "1555") => {
                return StoreError::UniqueViolation {
                    constraint: constraint(),
                };
            }
            Some("787") => {
                return StoreError::ForeignKey {
                    constraint: constraint(),
                };
            }
            _ => {}
        }
    }
    StoreError::Database(e.to_string())
}

pub(crate) fn finished() -> StoreError {
    StoreError::Invalid("transaction already finished".into())
}

/// `run!(&self.exec, |e| sqlx::query(..).bind(..).fetch_one(e))`: evaluates the query
/// future with `e` bound to the pool or to the transaction's connection, awaits it and
/// maps the error to [`StoreError`].
macro_rules! run {
    ($exec:expr, |$e:ident| $body:expr) => {
        match $exec {
            $crate::exec::Exec::Pool(pool) => {
                let $e = pool;
                $body.await.map_err($crate::exec::map_db_err)
            }
            $crate::exec::Exec::Tx(tx) => match tx.lock().await.as_mut() {
                Some(t) => {
                    let $e = &mut **t;
                    $body.await.map_err($crate::exec::map_db_err)
                }
                None => Err($crate::exec::finished()),
            },
        }
    };
}
pub(crate) use run;
