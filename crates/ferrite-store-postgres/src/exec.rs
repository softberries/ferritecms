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

pub(crate) fn db_err(e: sqlx::Error) -> StoreError {
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
                $body.await.map_err($crate::exec::db_err)
            }
            $crate::exec::Exec::Tx(tx) => match tx.lock().await.as_mut() {
                Some(t) => {
                    let $e = &mut **t;
                    $body.await.map_err($crate::exec::db_err)
                }
                None => Err($crate::exec::finished()),
            },
        }
    };
}
pub(crate) use run;
