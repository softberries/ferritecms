//! The row loader's statements (M4-1 P4): one transaction on a dedicated connection
//! (`<application_name>:import`, no timeouts), closed by the commit or when dropped.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_store::{
    InsertBatch, LoadTarget, StoreError,
    catalogue::{Cell, ColType, TableSpec},
    delete_sql, insert_sql,
};
use sqlx::{AssertSqlSafe, PgPool, Postgres, Transaction, postgres::PgArguments, query::Query};
use uuid::Uuid;

use crate::exec::map_db_err;

/// Postgres's bind-parameter limit (the protocol's 16-bit count).
const MAX_PARAMS: usize = 65_535;

pub(crate) struct PgLoadTarget {
    tx: Transaction<'static, Postgres>,
    /// The loader's own one-connection pool, closed by `commit`.
    pool: PgPool,
}

impl PgLoadTarget {
    /// Begins the transaction on `pool` (from `PgStore::import_pool`).
    pub(crate) async fn open(pool: PgPool) -> Result<Self, StoreError> {
        match pool.begin().await {
            Ok(tx) => Ok(Self { tx, pool }),
            Err(e) => {
                pool.close().await;
                Err(map_db_err(e))
            }
        }
    }
}

#[async_trait]
impl LoadTarget for PgLoadTarget {
    fn max_params(&self) -> usize {
        MAX_PARAMS
    }

    async fn insert(
        &mut self,
        table: &'static TableSpec,
        batch: InsertBatch,
        upsert: bool,
    ) -> Result<(), StoreError> {
        let sql = insert_sql(table, &batch.columns, batch.rows.len(), upsert, |n| {
            format!("${n}")
        });
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for row in batch.rows {
            for (cell, (_, col)) in row.into_iter().zip(&batch.columns) {
                q = bind_cell(q, *col, cell)?;
            }
        }
        q.execute(&mut *self.tx).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn delete(
        &mut self,
        table: &'static TableSpec,
        keys: Option<&'static [&'static str]>,
    ) -> Result<(), StoreError> {
        let sql = delete_sql(table, keys, |n| format!("${n}"));
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for k in keys.unwrap_or_default() {
            q = q.bind(*k);
        }
        q.execute(&mut *self.tx).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn commit(self) -> Result<(), StoreError> {
        let Self { tx, pool } = self;
        let done = tx.commit().await.map_err(map_db_err);
        pool.close().await;
        done
    }
}

/// Binds a decoded value of a `col` column in Postgres's physical form (02 §5). A `NULL`
/// is bound with the column's type, so the server never has to guess it.
fn bind_cell(
    q: Query<'_, Postgres, PgArguments>,
    col: ColType,
    cell: Cell,
) -> Result<Query<'_, Postgres, PgArguments>, StoreError> {
    Ok(match (col, cell) {
        (ColType::Id, Cell::Null) => q.bind(None::<Uuid>),
        (ColType::Timestamp, Cell::Null) => q.bind(None::<DateTime<Utc>>),
        (ColType::Json, Cell::Null) => q.bind(None::<serde_json::Value>),
        (ColType::Bool, Cell::Null) => q.bind(None::<bool>),
        (ColType::Bytes, Cell::Null) => q.bind(None::<Vec<u8>>),
        (ColType::Text, Cell::Null) => q.bind(None::<String>),
        (ColType::Int, Cell::Null) => q.bind(None::<i64>),
        (ColType::Id, Cell::Id(id)) => q.bind(id),
        (ColType::Timestamp, Cell::Micros(n)) => q.bind(
            DateTime::<Utc>::from_timestamp_micros(n)
                .ok_or_else(|| StoreError::Invalid(format!("timestamp out of range: {n}")))?,
        ),
        (ColType::Json, Cell::Json(v)) => q.bind(v),
        (ColType::Bool, Cell::Bool(b)) => q.bind(b),
        (ColType::Bytes, Cell::Bytes(b)) => q.bind(b),
        (ColType::Text, Cell::Text(s)) => q.bind(s),
        (ColType::Int, Cell::Int(n)) => q.bind(n),
        (col, cell) => {
            return Err(StoreError::Invalid(format!(
                "a {cell:?} value for a {col:?} column"
            )));
        }
    })
}
