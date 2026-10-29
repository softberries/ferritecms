//! The snapshot row reader (M4-1 P3): reads catalogue tables as neutral [`Row`]s from one
//! consistent snapshot of the whole database.

use async_trait::async_trait;

use crate::{
    StoreError,
    catalogue::{Row, TableSpec},
};

/// Most rows one [`ExportReader::page`] call returns.
pub const MAX_EXPORT_PAGE: u32 = 10_000;

/// Reads rows from one read snapshot, opened by
/// [`Store::export_reader`](crate::Store::export_reader) and held until
/// [`finish`](Self::finish) (Postgres: a `REPEATABLE READ READ ONLY` transaction on a
/// dedicated connection; SQLite: one read transaction on a reader connection, which in
/// WAL mode does not block the writer). Every page of every table sees the same data, so
/// rows written after the reader was opened are never seen.
///
/// Dropping the reader without `finish` also releases the snapshot, but asynchronously.
#[async_trait]
pub trait ExportReader: Send {
    /// Up to `limit` rows of `table` (1 to [`MAX_EXPORT_PAGE`]) in key order, starting
    /// after the row whose key `after` holds (`None`: from the first). Only the key
    /// columns of `after` are read, so the last row of the previous page is the cursor;
    /// an empty page means the table is done. `table` must be the catalogue's own entry
    /// and exported: a [`Skip`](crate::catalogue::Export::Skip) table gives `Invalid`,
    /// and an [`OnlyKeys`](crate::catalogue::Export::OnlyKeys) table gives only those
    /// rows. Values are in the neutral encoding (see [`crate::catalogue`]).
    async fn page(
        &mut self,
        table: &TableSpec,
        after: Option<&Row>,
        limit: u32,
    ) -> Result<Vec<Row>, StoreError>;

    /// Ends the snapshot and gives the connection back (Postgres: closes it).
    async fn finish(self: Box<Self>) -> Result<(), StoreError>;
}

/// Checks a page limit (1 to [`MAX_EXPORT_PAGE`]).
pub fn check_export_limit(limit: u32) -> Result<(), StoreError> {
    if limit == 0 || limit > MAX_EXPORT_PAGE {
        return Err(StoreError::Invalid(format!(
            "page limit must be 1 to {MAX_EXPORT_PAGE}, got {limit}"
        )));
    }
    Ok(())
}
