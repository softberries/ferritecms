//! Where repository statements run: a pool (autocommit) or an open transaction.

use std::sync::Arc;

use ferrite_store::StoreError;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::Mutex;

/// A transaction shared by the repositories of one `Tx`; `None` once finished.
pub(crate) type SharedTx = Arc<Mutex<Option<Transaction<'static, Postgres>>>>;

pub(crate) enum Exec {
    Pool(PgPool),
    Tx(SharedTx),
}

/// Maps a driver error: SQLSTATE 23505 and 23503 become typed errors carrying the
/// violated constraint (or unique index) name.
pub(crate) fn map_db_err(e: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(db) = &e {
        let constraint = || db.constraint().unwrap_or_default().to_owned();
        match db.code().as_deref() {
            Some("23505") => {
                return StoreError::UniqueViolation {
                    constraint: constraint(),
                };
            }
            Some("23503") => {
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
