//! The snapshot row reader (M4-1 P3): one read transaction on a reader connection.
//! In WAL mode it pins one snapshot without blocking the writer (it only keeps the WAL
//! from being checkpointed past that snapshot until [`ExportReader::finish`]).

use async_trait::async_trait;
use ferrite_store::{
    ExportReader, StoreError,
    catalogue::{Cell, ColType, Export, Row, TableSpec, check_exported, encode_cell, key_cells},
    check_export_limit,
};
use sqlx::{
    AssertSqlSafe, Row as _, Sqlite, SqlitePool, Transaction,
    query::Query,
    sqlite::{SqliteArguments, SqliteRow},
};
use uuid::Uuid;

use crate::{codec::json_from_db, exec::map_db_err};

pub(crate) struct SqliteExportReader {
    tx: Transaction<'static, Sqlite>,
}

impl SqliteExportReader {
    /// Begins the read transaction on `reader` and takes its snapshot at once (a
    /// deferred transaction would otherwise take it at the first page).
    pub(crate) async fn open(reader: &SqlitePool) -> Result<Self, StoreError> {
        let mut tx = reader.begin().await.map_err(map_db_err)?;
        sqlx::query("SELECT count(*) FROM schema_meta")
            .fetch_one(&mut *tx)
            .await
            .map_err(map_db_err)?;
        Ok(Self { tx })
    }
}

#[async_trait]
impl ExportReader for SqliteExportReader {
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
            q = bind_cell(q, cell);
        }
        if let Export::OnlyKeys(values) = table.export {
            for v in values {
                q = q.bind(*v);
            }
        }
        let rows = q
            .bind(i64::from(limit))
            .fetch_all(&mut *self.tx)
            .await
            .map_err(map_db_err)?;
        rows.iter().map(|r| decode_row(table, r)).collect()
    }

    async fn finish(self: Box<Self>) -> Result<(), StoreError> {
        self.tx.rollback().await.map_err(map_db_err)
    }
}

/// `SELECT` of one page: every column, keyset by `key`, the `OnlyKeys` filter.
fn page_sql(table: &TableSpec, after: bool) -> String {
    let cols = table
        .columns
        .iter()
        .map(|(c, _)| quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let key = table
        .key
        .iter()
        .map(|k| quote(k))
        .collect::<Vec<_>>()
        .join(", ");
    let mut conds = Vec::new();
    if after {
        let marks = vec!["?"; table.key.len()].join(", ");
        conds.push(format!("({key}) > ({marks})"));
    }
    if let Export::OnlyKeys(values) = table.export {
        let marks = vec!["?"; values.len()].join(", ");
        conds.push(format!("{key} IN ({marks})"));
    }
    let filter = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    format!(
        "SELECT {cols} FROM {}{filter} ORDER BY {key} LIMIT ?",
        quote(table.name)
    )
}

fn quote(ident: &str) -> String {
    format!("\"{ident}\"")
}

/// Binds a decoded value in SQLite's physical form (02 §5).
fn bind_cell(
    q: Query<'_, Sqlite, SqliteArguments>,
    cell: Cell,
) -> Query<'_, Sqlite, SqliteArguments> {
    match cell {
        Cell::Null => q.bind(None::<i64>),
        Cell::Id(id) => q.bind(id),
        Cell::Micros(n) | Cell::Int(n) => q.bind(n),
        Cell::Json(v) => q.bind(v.to_string()),
        Cell::Bool(b) => q.bind(b),
        Cell::Bytes(b) => q.bind(b),
        Cell::Text(s) => q.bind(s),
    }
}

/// Decodes one row by its catalogue column types, then encodes it neutrally.
fn decode_row(table: &TableSpec, row: &SqliteRow) -> Result<Row, StoreError> {
    let mut out = Row::new();
    for (i, (name, col)) in table.columns.iter().enumerate() {
        let cell = match col {
            ColType::Id => row.try_get::<Option<Uuid>, _>(i).map(|v| v.map(Cell::Id)),
            ColType::Timestamp => row
                .try_get::<Option<i64>, _>(i)
                .map(|v| v.map(Cell::Micros)),
            ColType::Int => row.try_get::<Option<i64>, _>(i).map(|v| v.map(Cell::Int)),
            ColType::Bool => row.try_get::<Option<bool>, _>(i).map(|v| v.map(Cell::Bool)),
            ColType::Bytes => row
                .try_get::<Option<Vec<u8>>, _>(i)
                .map(|v| v.map(Cell::Bytes)),
            ColType::Text => row
                .try_get::<Option<String>, _>(i)
                .map(|v| v.map(Cell::Text)),
            ColType::Json => match row.try_get::<Option<String>, _>(i) {
                Ok(Some(text)) => Ok(Some(Cell::Json(json_from_db(&text)?))),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            },
        }
        .map_err(|e| StoreError::Database(format!("{}.{name}: {e}", table.name)))?
        .unwrap_or(Cell::Null);
        out.insert((*name).to_owned(), encode_cell(cell)?);
    }
    Ok(out)
}
