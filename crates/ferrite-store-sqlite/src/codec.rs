//! SQLite physical encodings (02 §5): timestamp = unix microseconds `INTEGER`,
//! JSON = `TEXT`. Booleans bind and decode natively as 0/1.

use ferrite_domain::{OptionEntry, Timestamp};
use ferrite_store::StoreError;
use sqlx::{Row, sqlite::SqliteRow};

use crate::exec::db_err;

pub(crate) fn json_to_db(value: &serde_json::Value) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Invalid(e.to_string()))
}

fn json_from_db(text: &str) -> Result<serde_json::Value, StoreError> {
    serde_json::from_str(text).map_err(|e| StoreError::Database(format!("corrupt JSON: {e}")))
}

/// Maps a `SELECT key, value, autoload, updated_at FROM options` row.
pub(crate) fn option_entry(row: &SqliteRow) -> Result<OptionEntry, StoreError> {
    let value: String = row.try_get("value").map_err(db_err)?;
    let micros: i64 = row.try_get("updated_at").map_err(db_err)?;
    Ok(OptionEntry {
        key: row.try_get("key").map_err(db_err)?,
        value: json_from_db(&value)?,
        autoload: row.try_get("autoload").map_err(db_err)?,
        updated_at: Timestamp::from_micros(micros)?,
    })
}
