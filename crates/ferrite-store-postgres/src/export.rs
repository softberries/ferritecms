//! The snapshot row reader (M4-1 P3): a `REPEATABLE READ READ ONLY` transaction on a
//! dedicated connection (`<application_name>:export`), closed by
//! [`ExportReader::finish`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_store::{
    ExportReader, StoreError,
    catalogue::{Cell, ColType, Export, Row, TableSpec, check_exported, encode_cell, key_cells},
    check_export_limit,
};
use sqlx::{
    AssertSqlSafe, PgPool, Postgres, Row as _, Transaction,
    postgres::{PgArguments, PgRow},
    query::Query,
};
use uuid::Uuid;

use crate::exec::map_db_err;

pub(crate) struct PgExportReader {
    tx: Transaction<'static, Postgres>,
    /// The reader's own one-connection pool, closed by `finish`.
    pool: PgPool,
}

impl PgExportReader {
    /// Begins the transaction on `pool` (from `PgStore::export_pool`) and takes its
    /// snapshot at once: in `REPEATABLE READ` the first statement fixes it.
    pub(crate) async fn open(pool: PgPool) -> Result<Self, StoreError> {
        let mut tx = match pool
            .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                pool.close().await;
                return Err(map_db_err(e));
            }
        };
        if let Err(e) = sqlx::query("SELECT count(*) FROM schema_meta")
            .fetch_one(&mut *tx)
            .await
        {
            drop(tx);
            pool.close().await;
            return Err(map_db_err(e));
        }
        Ok(Self { tx, pool })
    }
}

#[async_trait]
impl ExportReader for PgExportReader {
    async fn page(
        &mut self,
        table: &TableSpec,
        after: Option<&Row>,
        limit: u32,
    ) -> Result<Vec<Row>, StoreError> {
        check_exported(table)?;
        check_export_limit(limit)?;
        let after = after.map(|row| key_cells(table, row)).transpose()?;
        let sql = page_sql(table, after.is_some());
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for cell in after.into_iter().flatten() {
            q = bind_cell(q, cell)?;
        }
        if let Export::OnlyKeys(values) = table.export {
            q = q.bind(values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>());
        }
        let rows = q
            .bind(i64::from(limit))
            .fetch_all(&mut *self.tx)
            .await
            .map_err(map_db_err)?;
        rows.iter().map(|r| decode_row(table, r)).collect()
    }

    async fn finish(self: Box<Self>) -> Result<(), StoreError> {
        let Self { tx, pool } = *self;
        let done = tx.rollback().await.map_err(map_db_err);
        pool.close().await;
        done
    }
}

/// `SELECT` of one page: every column (integers widened to `bigint`), keyset by `key`
/// (text keys compared in `"C"`, as SQLite's `BINARY`, so both backends page alike),
/// the `OnlyKeys` filter.
fn page_sql(table: &TableSpec, after: bool) -> String {
    let cols = table
        .columns
        .iter()
        .map(|(c, t)| match t {
            ColType::Int => format!("{q}::bigint AS {q}", q = quote(c)),
            _ => quote(c),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let key = table
        .key
        .iter()
        .map(|k| match table.column(k) {
            Some(ColType::Text) => format!("{} COLLATE \"C\"", quote(k)),
            _ => quote(k),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut n = 0;
    let mut mark = || {
        n += 1;
        format!("${n}")
    };
    let mut conds = Vec::new();
    if after {
        let marks = (0..table.key.len())
            .map(|_| mark())
            .collect::<Vec<_>>()
            .join(", ");
        conds.push(format!("({key}) > ({marks})"));
    }
    if let Export::OnlyKeys(_) = table.export {
        conds.push(format!("{key} = ANY({})", mark()));
    }
    if let Export::RowsWithout(column) = table.export {
        conds.push(format!("{} IS NULL", quote(column)));
    }
    let limit = mark();
    let filter = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    format!(
        "SELECT {cols} FROM {}{filter} ORDER BY {key} LIMIT {limit}",
        quote(table.name)
    )
}

fn quote(ident: &str) -> String {
    format!("\"{ident}\"")
}

/// Binds a decoded value in Postgres's physical form (02 §5).
fn bind_cell(
    q: Query<'_, Postgres, PgArguments>,
    cell: Cell,
) -> Result<Query<'_, Postgres, PgArguments>, StoreError> {
    Ok(match cell {
        Cell::Null => q.bind(None::<String>),
        Cell::Id(id) => q.bind(id),
        Cell::Micros(n) => q.bind(
            DateTime::<Utc>::from_timestamp_micros(n)
                .ok_or_else(|| StoreError::Invalid(format!("timestamp out of range: {n}")))?,
        ),
        Cell::Json(v) => q.bind(v),
        Cell::Bool(b) => q.bind(b),
        Cell::Bytes(b) => q.bind(b),
        Cell::Text(s) => q.bind(s),
        Cell::Int(n) => q.bind(n),
    })
}

/// Decodes one row by its catalogue column types, then encodes it neutrally.
fn decode_row(table: &TableSpec, row: &PgRow) -> Result<Row, StoreError> {
    let mut out = Row::new();
    for (i, (name, col)) in table.columns.iter().enumerate() {
        let cell = match col {
            ColType::Id => row.try_get::<Option<Uuid>, _>(i).map(|v| v.map(Cell::Id)),
            ColType::Timestamp => row
                .try_get::<Option<DateTime<Utc>>, _>(i)
                .map(|v| v.map(|dt| Cell::Micros(dt.timestamp_micros()))),
            ColType::Int => row.try_get::<Option<i64>, _>(i).map(|v| v.map(Cell::Int)),
            ColType::Bool => row.try_get::<Option<bool>, _>(i).map(|v| v.map(Cell::Bool)),
            ColType::Bytes => row
                .try_get::<Option<Vec<u8>>, _>(i)
                .map(|v| v.map(Cell::Bytes)),
            ColType::Text => row
                .try_get::<Option<String>, _>(i)
                .map(|v| v.map(Cell::Text)),
            ColType::Json => row
                .try_get::<Option<serde_json::Value>, _>(i)
                .map(|v| v.map(Cell::Json)),
        }
        .map_err(|e| StoreError::Database(format!("{}.{name}: {e}", table.name)))?
        .unwrap_or(Cell::Null);
        out.insert((*name).to_owned(), encode_cell(cell)?);
    }
    Ok(out)
}
