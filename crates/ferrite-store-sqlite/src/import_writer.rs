//! The row loader's statements (M4-1 P4): one transaction on the writer connection,
//! held until the load commits or is dropped.

use async_trait::async_trait;
use ferrite_store::{
    InsertBatch, LoadTarget, StoreError,
    catalogue::{Cell, ColType, TableSpec},
    delete_sql, insert_sql,
};
use sqlx::{AssertSqlSafe, Sqlite, Transaction, query::Query, sqlite::SqliteArguments};

use crate::{codec::json_to_db, exec::map_db_err};

/// SQLite's default `SQLITE_MAX_VARIABLE_NUMBER` (since 3.32).
const MAX_PARAMS: usize = 32_766;

pub(crate) struct SqliteLoadTarget {
    tx: Transaction<'static, Sqlite>,
}

impl SqliteLoadTarget {
    pub(crate) fn new(tx: Transaction<'static, Sqlite>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl LoadTarget for SqliteLoadTarget {
    fn max_params(&self) -> usize {
        MAX_PARAMS
    }

    async fn insert(
        &mut self,
        table: &'static TableSpec,
        batch: InsertBatch,
        upsert: bool,
    ) -> Result<(), StoreError> {
        let sql = insert_sql(table, &batch.columns, batch.rows.len(), upsert, |_| {
            "?".to_owned()
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
        let mut q = sqlx::query(AssertSqlSafe(delete_sql(table, keys, |_| "?".to_owned())));
        for k in keys.unwrap_or_default() {
            q = q.bind(*k);
        }
        q.execute(&mut *self.tx).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn commit(self) -> Result<(), StoreError> {
        self.tx.commit().await.map_err(map_db_err)
    }
}

/// Binds a decoded value of a `col` column in SQLite's physical form (02 §5).
fn bind_cell(
    q: Query<'_, Sqlite, SqliteArguments>,
    col: ColType,
    cell: Cell,
) -> Result<Query<'_, Sqlite, SqliteArguments>, StoreError> {
    Ok(match (col, cell) {
        (_, Cell::Null) => q.bind(None::<i64>),
        (ColType::Id, Cell::Id(id)) => q.bind(id),
        (ColType::Timestamp, Cell::Micros(n)) | (ColType::Int, Cell::Int(n)) => q.bind(n),
        (ColType::Json, Cell::Json(v)) => q.bind(json_to_db(&v)?),
        (ColType::Bool, Cell::Bool(b)) => q.bind(b),
        (ColType::Bytes, Cell::Bytes(b)) => q.bind(b),
        (ColType::Text, Cell::Text(s)) => q.bind(s),
        (col, cell) => {
            return Err(StoreError::Invalid(format!(
                "a {cell:?} value for a {col:?} column"
            )));
        }
    })
}
