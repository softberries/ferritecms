//! Row decoding. PostgreSQL types (`timestamptz`, `jsonb`, `boolean`) map natively.

use chrono::{DateTime, Utc};
use ferrite_domain::OptionEntry;
use ferrite_store::StoreError;
use sqlx::{Row, postgres::PgRow, types::Json};

use crate::exec::db_err;

/// Maps a `SELECT key, value, autoload, updated_at FROM options` row.
pub(crate) fn option_entry(row: &PgRow) -> Result<OptionEntry, StoreError> {
    let value: Json<serde_json::Value> = row.try_get("value").map_err(db_err)?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(db_err)?;
    Ok(OptionEntry {
        key: row.try_get("key").map_err(db_err)?,
        value: value.0,
        autoload: row.try_get("autoload").map_err(db_err)?,
        updated_at: updated_at.into(),
    })
}
