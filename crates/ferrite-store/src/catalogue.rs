//! The table catalogue (M4-1 P1, P2): every table of the latest schema, in foreign-key
//! order, with its columns and a backend-neutral type per column. Export, import-site,
//! `db copy` and the scheduled cluster export read and write rows through it, so a site
//! moves between backends as plain rows.
//!
//! A [`Row`] is a JSON object keyed by column name. Values are encoded neutrally (P1):
//! [`ColType::Id`] as a hyphenated UUID string, [`ColType::Timestamp`] as RFC 3339 UTC
//! with exactly 6 fractional digits and `Z`, [`ColType::Json`] as the JSON value,
//! [`ColType::Bool`] as a JSON bool, [`ColType::Bytes`] as standard base64 with padding,
//! [`ColType::Text`] as a string, [`ColType::Int`] as an i64 number, SQL `NULL` as
//! `null`. Secrets travel as stored (hashes, sealed ciphertext).
//!
//! The contract tests `catalogue_matches_live_schema` and `catalogue_is_fk_ordered`
//! compare this list with the migrated schema of both backends, so a migration that adds
//! a table or column without updating it fails.

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::StoreError;

/// One table row in the neutral encoding, keyed by column name.
pub type Row = serde_json::Map<String, Value>;

/// The backend-neutral type of a column (02 §5 physical types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColType {
    /// SQLite 16-byte `BLOB`, Postgres `uuid`.
    Id,
    /// SQLite `INTEGER` unix microseconds, Postgres `timestamptz`.
    Timestamp,
    /// SQLite `TEXT` holding JSON, Postgres `jsonb`.
    Json,
    /// SQLite `INTEGER` 0/1, Postgres `boolean`.
    Bool,
    /// SQLite `BLOB`, Postgres `bytea`.
    Bytes,
    /// `TEXT` / `text` (including the `*_key` lowercase columns, copied as they are).
    Text,
    /// SQLite `INTEGER`, Postgres `integer` or `bigint`; always read as i64.
    Int,
}

/// Whether, and which, rows of a table are exported (P2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    /// Every row.
    Rows,
    /// Only the rows whose key (one [`ColType::Text`] column) is one of these values.
    /// `schema_meta` exports only `site_id`: the other keys (`created_by_version`,
    /// `min_binary_version`, `last_binary_version`) belong to the target's own migrations
    /// and binary. The reader applies the filter, and the loader writes such rows over the
    /// target's.
    OnlyKeys(&'static [&'static str]),
    /// Not exported: the data is transient or derived (P2). The reader refuses it.
    Skip,
}

/// One table of the catalogue.
#[derive(Debug, PartialEq, Eq)]
pub struct TableSpec {
    /// The table name, the same on every backend.
    pub name: &'static str,
    /// Every column in schema order, with its neutral type.
    pub columns: &'static [(&'static str, ColType)],
    /// The primary key; rows are paged in this order (keyset).
    pub key: &'static [&'static str],
    /// The column referencing this same table (a parent), if any. The loader holds a row
    /// whose parent is not loaded yet until that parent is inserted (R6, as built; P4's
    /// insert-`NULL`-then-set was not built), and refuses cycles and self-parents.
    pub self_ref: Option<&'static str>,
    /// The columns that may hold SQL `NULL`; every other column is `NOT NULL` (a primary
    /// key column always is). `catalogue_matches_live_schema` checks it on both backends.
    /// A `null` value of a [`ColType::Json`] column that is not listed here is the JSON
    /// value `null`, not SQL `NULL` ([`decode_column`]).
    pub nullable: &'static [&'static str],
    /// What the export takes from this table.
    pub export: Export,
}

impl TableSpec {
    /// The type of column `name`, if the table has it.
    pub fn column(&self, name: &str) -> Option<ColType> {
        self.columns
            .iter()
            .find(|(c, _)| *c == name)
            .map(|(_, t)| *t)
    }

    /// Whether column `name` may hold SQL `NULL` ([`TableSpec::nullable`]).
    pub fn is_nullable(&self, name: &str) -> bool {
        self.nullable.contains(&name)
    }

    /// Whether any rows of this table are exported.
    pub fn is_exported(&self) -> bool {
        !matches!(self.export, Export::Skip)
    }
}

/// Tables whose shape differs per backend. Only `search_docs` (02 §4: tokens plus FTS5 on
/// SQLite, a `tsvector` on Postgres); it is [`Export::Skip`] and rebuilt after a load, so
/// its entry lists just the shared `item_id`. The schema test checks that column only.
pub const BACKEND_SPECIFIC: &[&str] = &["search_docs"];

use ColType::{Bool, Bytes, Id, Int, Json, Text, Timestamp as Ts};

/// Every table of the latest schema (migrations 0001–0011), in foreign-key order: each
/// table comes after the tables it references. Not listed: SQLite's FTS5 `search_fts`
/// and its shadow tables, and sqlx's `_sqlx_migrations`.
pub static TABLES: &[TableSpec] = &[
    TableSpec {
        name: "content_types",
        columns: &[
            ("name", Text),
            ("label", Text),
            ("hierarchical", Bool),
            ("has_archive", Bool),
            ("fields", Json),
            ("built_in", Bool),
            ("created_at", Ts),
        ],
        key: &["name"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "users",
        columns: &[
            ("id", Id),
            ("login", Text),
            ("email", Text),
            ("email_key", Text),
            ("display_name", Text),
            ("password_hash", Text),
            ("role", Text),
            ("status", Text),
            ("must_reset", Bool),
            ("totp_secret", Text),
            ("created_at", Ts),
            ("last_login_at", Ts),
            ("totp_last_step", Int),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &[
            "password_hash",
            "totp_secret",
            "last_login_at",
            "totp_last_step",
        ],
        export: Export::Rows,
    },
    TableSpec {
        name: "content_items",
        columns: &[
            ("id", Id),
            ("type", Text),
            ("status", Text),
            ("slug", Text),
            ("parent_id", Id),
            ("author_id", Id),
            ("title", Text),
            ("excerpt", Text),
            ("body", Json),
            ("body_html", Text),
            ("fields", Json),
            ("menu_order", Int),
            ("published_at", Ts),
            ("scheduled_at", Ts),
            ("created_at", Ts),
            ("updated_at", Ts),
            ("version", Int),
        ],
        key: &["id"],
        self_ref: Some("parent_id"),
        nullable: &["parent_id", "body_html", "published_at", "scheduled_at"],
        export: Export::Rows,
    },
    TableSpec {
        name: "content_meta",
        columns: &[("item_id", Id), ("key", Text), ("value", Json)],
        key: &["item_id", "key"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "revisions",
        columns: &[
            ("id", Id),
            ("item_id", Id),
            ("author_id", Id),
            ("created_at", Ts),
            ("title", Text),
            ("excerpt", Text),
            ("body", Json),
            ("fields", Json),
            ("meta", Json),
            ("note", Text),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &["note"],
        export: Export::Rows,
    },
    TableSpec {
        name: "taxonomies",
        columns: &[
            ("name", Text),
            ("label", Text),
            ("hierarchical", Bool),
            ("content_types", Json),
            ("built_in", Bool),
        ],
        key: &["name"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "terms",
        columns: &[
            ("id", Id),
            ("taxonomy", Text),
            ("slug", Text),
            ("name", Text),
            ("description", Text),
            ("parent_id", Id),
        ],
        key: &["id"],
        self_ref: Some("parent_id"),
        nullable: &["parent_id"],
        export: Export::Rows,
    },
    TableSpec {
        name: "term_links",
        columns: &[("item_id", Id), ("term_id", Id), ("position", Int)],
        key: &["item_id", "term_id"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "menus",
        columns: &[("name", Text), ("label", Text)],
        key: &["name"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "menu_items",
        columns: &[
            ("id", Id),
            ("menu", Text),
            ("parent_id", Id),
            ("position", Int),
            ("kind", Text),
            ("target_id", Id),
            ("url", Text),
            ("label", Text),
        ],
        key: &["id"],
        self_ref: Some("parent_id"),
        nullable: &["parent_id", "target_id", "url"],
        export: Export::Rows,
    },
    TableSpec {
        name: "artifacts",
        columns: &[
            ("hash", Text),
            ("kind", Text),
            ("bytes", Int),
            ("mime", Text),
            ("created_at", Ts),
            ("orphaned_at", Ts),
        ],
        key: &["hash"],
        self_ref: None,
        nullable: &["orphaned_at"],
        export: Export::Rows,
    },
    TableSpec {
        name: "media",
        columns: &[
            ("item_id", Id),
            ("artifact_hash", Text),
            ("mime", Text),
            ("bytes", Int),
            ("width", Int),
            ("height", Int),
            ("alt", Text),
            ("original_name", Text),
            ("variants", Json),
            ("sanitized", Bool),
        ],
        key: &["item_id"],
        self_ref: None,
        nullable: &["width", "height"],
        export: Export::Rows,
    },
    TableSpec {
        name: "media_variants",
        columns: &[("hash", Text), ("item_id", Id), ("name", Text)],
        key: &["item_id", "name"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "comments",
        columns: &[
            ("id", Id),
            ("item_id", Id),
            ("parent_id", Id),
            ("user_id", Id),
            ("author_name", Text),
            ("author_email", Text),
            ("author_url", Text),
            ("body", Text),
            ("status", Text),
            ("ip_hash", Text),
            ("created_at", Ts),
            ("by_account", Bool),
        ],
        key: &["id"],
        self_ref: Some("parent_id"),
        nullable: &["parent_id", "user_id", "ip_hash"],
        export: Export::Rows,
    },
    TableSpec {
        name: "redirects",
        columns: &[
            ("id", Id),
            ("source_path", Text),
            ("target", Text),
            ("code", Int),
            ("hits", Int),
            ("origin", Text),
            ("created_at", Ts),
            ("last_hit_at", Ts),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &["last_hit_at"],
        export: Export::Rows,
    },
    TableSpec {
        name: "user_meta",
        columns: &[("user_id", Id), ("key", Text), ("value", Json)],
        key: &["user_id", "key"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "options",
        columns: &[
            ("key", Text),
            ("value", Json),
            ("autoload", Bool),
            ("updated_at", Ts),
        ],
        key: &["key"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "plugins",
        columns: &[
            ("name", Text),
            ("version", Text),
            ("artifact_hash", Text),
            ("manifest", Json),
            ("granted", Json),
            ("enabled", Bool),
            ("disabled_reason", Text),
            ("installed_at", Ts),
            ("updated_at", Ts),
        ],
        key: &["name"],
        self_ref: None,
        nullable: &["disabled_reason"],
        export: Export::Rows,
    },
    TableSpec {
        name: "plugin_kv",
        columns: &[
            ("plugin", Text),
            ("key", Text),
            ("value", Bytes),
            ("updated_at", Ts),
        ],
        key: &["plugin", "key"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "plugin_failures",
        columns: &[
            ("plugin", Text),
            ("hook", Text),
            ("bucket_start", Ts),
            ("count", Int),
        ],
        key: &["plugin", "hook", "bucket_start"],
        self_ref: None,
        nullable: &[],
        export: Export::Skip,
    },
    TableSpec {
        name: "themes",
        columns: &[
            ("name", Text),
            ("version", Text),
            ("artifact_hash", Text),
            ("manifest", Json),
            ("active", Bool),
            ("installed_at", Ts),
        ],
        key: &["name"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
    TableSpec {
        name: "events",
        columns: &[
            ("seq", Int),
            ("kind", Text),
            ("keys", Json),
            ("origin_node", Text),
            ("created_at", Ts),
        ],
        key: &["seq"],
        self_ref: None,
        nullable: &["origin_node"],
        export: Export::Skip,
    },
    TableSpec {
        name: "schema_meta",
        columns: &[("key", Text), ("value", Text)],
        key: &["key"],
        self_ref: None,
        nullable: &[],
        export: Export::OnlyKeys(&["site_id"]),
    },
    TableSpec {
        name: "search_docs",
        columns: &[("item_id", Id)],
        key: &["item_id"],
        self_ref: None,
        nullable: &[],
        export: Export::Skip,
    },
    TableSpec {
        name: "jobs",
        columns: &[
            ("id", Id),
            ("kind", Text),
            ("payload", Json),
            ("unique_key", Text),
            ("run_at", Ts),
            ("priority", Int),
            ("status", Text),
            ("attempts", Int),
            ("max_attempts", Int),
            ("locked_by", Text),
            ("locked_until", Ts),
            ("last_error", Text),
            ("created_at", Ts),
            ("finished_at", Ts),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &[
            "unique_key",
            "locked_by",
            "locked_until",
            "last_error",
            "finished_at",
        ],
        export: Export::Skip,
    },
    TableSpec {
        name: "nodes",
        columns: &[
            ("id", Id),
            ("name", Text),
            ("version", Text),
            ("schema_version", Int),
            ("started_at", Ts),
            ("heartbeat_at", Ts),
            ("meta", Json),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &[],
        export: Export::Skip,
    },
    TableSpec {
        name: "sessions",
        columns: &[
            ("id_hash", Text),
            ("user_id", Id),
            ("csrf_secret", Text),
            ("created_at", Ts),
            ("last_seen_at", Ts),
            ("expires_at", Ts),
            ("ip_hash", Text),
            ("user_agent", Text),
        ],
        key: &["id_hash"],
        self_ref: None,
        nullable: &["ip_hash", "user_agent"],
        export: Export::Skip,
    },
    TableSpec {
        name: "api_tokens",
        columns: &[
            ("id", Id),
            ("user_id", Id),
            ("name", Text),
            ("token_hash", Text),
            ("scopes", Json),
            ("created_at", Ts),
            ("last_used_at", Ts),
            ("expires_at", Ts),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &["last_used_at", "expires_at"],
        export: Export::Rows,
    },
    TableSpec {
        name: "rate_limits",
        columns: &[("bucket", Text), ("window_start", Ts), ("count", Int)],
        key: &["bucket"],
        self_ref: None,
        nullable: &[],
        export: Export::Skip,
    },
    TableSpec {
        name: "audit_log",
        columns: &[
            ("id", Id),
            ("at", Ts),
            ("actor_id", Id),
            ("action", Text),
            ("target_kind", Text),
            ("target_id", Text),
            ("details", Json),
            ("node", Text),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &["actor_id", "target_id", "node"],
        export: Export::Rows,
    },
    TableSpec {
        name: "idempotency_keys",
        columns: &[
            ("user_id", Id),
            ("key_hash", Text),
            ("method", Text),
            ("path", Text),
            ("status_code", Int),
            ("response", Json),
            ("created_at", Ts),
        ],
        key: &["user_id", "key_hash"],
        self_ref: None,
        nullable: &["status_code", "response"],
        export: Export::Skip,
    },
    TableSpec {
        name: "autosaves",
        columns: &[
            ("item_id", Id),
            ("user_id", Id),
            ("base_version", Int),
            ("title", Text),
            ("excerpt", Text),
            ("slug", Text),
            ("parent_id", Id),
            ("menu_order", Int),
            ("terms", Json),
            ("body", Json),
            ("fields", Json),
            ("saved_at", Ts),
            ("seo", Json),
        ],
        key: &["item_id", "user_id"],
        self_ref: None,
        nullable: &["excerpt", "slug", "parent_id", "menu_order", "terms", "seo"],
        export: Export::Rows,
    },
    TableSpec {
        name: "import_runs",
        columns: &[
            ("id", Id),
            ("source", Text),
            ("token", Text),
            ("state", Text),
            ("started_by", Id),
            ("started_at", Ts),
            ("heartbeat_at", Ts),
            ("finished_at", Ts),
            ("stats", Json),
            ("dry_run", Bool),
            ("job_id", Id),
            ("step", Text),
            ("report_hash", Text),
        ],
        key: &["id"],
        self_ref: None,
        nullable: &["finished_at", "job_id", "step", "report_hash"],
        export: Export::Rows,
    },
    TableSpec {
        name: "import_map",
        columns: &[
            ("source", Text),
            ("kind", Text),
            ("wp_id", Int),
            ("ferrite_id", Id),
            ("hash", Text),
            ("version", Int),
        ],
        key: &["source", "kind", "wp_id"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    },
];

/// The catalogue entry of table `name`.
pub fn table(name: &str) -> Option<&'static TableSpec> {
    TABLES.iter().find(|t| t.name == name)
}

/// A database value decoded by a backend, before (or after) the neutral encoding.
/// Backends read each column as the variant its [`ColType`] names and bind it back the
/// same way, so both encode and decode in one place ([`encode_cell`], [`decode_cell`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// SQL `NULL`.
    Null,
    /// [`ColType::Id`].
    Id(Uuid),
    /// [`ColType::Timestamp`] as unix microseconds.
    Micros(i64),
    /// [`ColType::Json`].
    Json(Value),
    /// [`ColType::Bool`].
    Bool(bool),
    /// [`ColType::Bytes`].
    Bytes(Vec<u8>),
    /// [`ColType::Text`].
    Text(String),
    /// [`ColType::Int`].
    Int(i64),
}

/// Encodes a decoded value neutrally (see the module docs).
pub fn encode_cell(cell: Cell) -> Result<Value, StoreError> {
    Ok(match cell {
        Cell::Null => Value::Null,
        Cell::Id(id) => Value::String(id.hyphenated().to_string()),
        Cell::Micros(micros) => Value::String(format_micros(micros)?),
        Cell::Json(v) => v,
        Cell::Bool(b) => Value::Bool(b),
        Cell::Bytes(b) => Value::String(STANDARD.encode(b)),
        Cell::Text(s) => Value::String(s),
        Cell::Int(n) => Value::from(n),
    })
}

/// Decodes a neutral value of a column of type `col`. Strict: only the exact encoding
/// [`encode_cell`] writes is accepted (a timestamp needs 6 fractional digits and `Z`).
pub fn decode_cell(col: ColType, value: &Value) -> Result<Cell, StoreError> {
    if value.is_null() {
        return Ok(Cell::Null);
    }
    let bad = || StoreError::Invalid(format!("not a valid {col:?} value: {value}"));
    Ok(match col {
        ColType::Json => Cell::Json(value.clone()),
        ColType::Bool => Cell::Bool(value.as_bool().ok_or_else(bad)?),
        ColType::Int => Cell::Int(value.as_i64().ok_or_else(bad)?),
        ColType::Text => Cell::Text(value.as_str().ok_or_else(bad)?.to_owned()),
        ColType::Id => {
            let s = value.as_str().ok_or_else(bad)?;
            let id = Uuid::try_parse(s).map_err(|_| bad())?;
            if id.hyphenated().to_string() != s {
                return Err(bad());
            }
            Cell::Id(id)
        }
        ColType::Timestamp => {
            let s = value.as_str().ok_or_else(bad)?;
            let micros = DateTime::parse_from_rfc3339(s)
                .map_err(|_| bad())?
                .with_timezone(&Utc)
                .timestamp_micros();
            if format_micros(micros)? != s {
                return Err(bad());
            }
            Cell::Micros(micros)
        }
        ColType::Bytes => Cell::Bytes(
            STANDARD
                .decode(value.as_str().ok_or_else(bad)?)
                .map_err(|_| bad())?,
        ),
    })
}

/// Decodes the neutral value of column `name` (of type `col`) of `table`. Like
/// [`decode_cell`], except that `null` in a `NOT NULL` [`ColType::Json`] column is the
/// JSON value `null` ([`Cell::Json`]): the export writes both SQL `NULL` and a stored JSON
/// `null` as `null`, and the column's nullability tells them apart.
pub fn decode_column(
    table: &TableSpec,
    name: &str,
    col: ColType,
    value: &Value,
) -> Result<Cell, StoreError> {
    if value.is_null() && col == ColType::Json && !table.is_nullable(name) {
        return Ok(Cell::Json(Value::Null));
    }
    decode_cell(col, value)
}

/// The key of `row` in `table`, decoded for binding a keyset predicate (`after`).
pub fn key_cells(table: &TableSpec, row: &Row) -> Result<Vec<Cell>, StoreError> {
    table
        .key
        .iter()
        .map(|k| {
            let col = table
                .column(k)
                .ok_or_else(|| StoreError::Invalid(format!("{}: no key column {k}", table.name)))?;
            let value = row.get(*k).ok_or_else(|| {
                StoreError::Invalid(format!("{}: `after` lacks key column {k}", table.name))
            })?;
            match decode_cell(col, value)? {
                Cell::Null => Err(StoreError::Invalid(format!(
                    "{}: key column {k} is null",
                    table.name
                ))),
                cell => Ok(cell),
            }
        })
        .collect()
}

/// Checks that `table` is the catalogue's own entry and may be read for an export.
pub fn check_exported(table: &TableSpec) -> Result<(), StoreError> {
    if self::table(table.name) != Some(table) {
        return Err(StoreError::Invalid(format!(
            "{}: not a catalogue table",
            table.name
        )));
    }
    if !table.is_exported() {
        return Err(StoreError::Invalid(format!(
            "{}: table is not exported",
            table.name
        )));
    }
    Ok(())
}

fn format_micros(micros: i64) -> Result<String, StoreError> {
    let dt = DateTime::<Utc>::from_timestamp_micros(micros)
        .ok_or_else(|| StoreError::Invalid(format!("timestamp out of range: {micros}")))?;
    Ok(dt.to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;

    #[test]
    fn names_are_unique_and_keys_are_columns() {
        let mut seen = HashSet::new();
        for t in TABLES {
            assert!(seen.insert(t.name), "{} listed twice", t.name);
            assert!(!t.key.is_empty(), "{} has no key", t.name);
            for k in t.key {
                assert!(t.column(k).is_some(), "{}: key {k} is not a column", t.name);
            }
            if let Some(p) = t.self_ref {
                assert_eq!(t.column(p), Some(ColType::Id), "{}: self_ref {p}", t.name);
            }
            for n in t.nullable {
                assert!(
                    t.column(n).is_some(),
                    "{}: nullable {n} is not a column",
                    t.name
                );
                assert!(!t.key.contains(n), "{}: key column {n} is nullable", t.name);
            }
            if let Export::OnlyKeys(_) = t.export {
                assert_eq!(t.key.len(), 1, "{}", t.name);
                assert_eq!(t.column(t.key[0]), Some(ColType::Text), "{}", t.name);
            }
        }
        assert_eq!(TABLES.len(), 34);
    }

    #[test]
    fn skipped_tables_are_the_p2_list() {
        let skipped: Vec<&str> = TABLES
            .iter()
            .filter(|t| !t.is_exported())
            .map(|t| t.name)
            .collect();
        assert_eq!(
            skipped,
            [
                "plugin_failures",
                "events",
                "search_docs",
                "jobs",
                "nodes",
                "sessions",
                "rate_limits",
                "idempotency_keys",
            ]
        );
        assert_eq!(
            table("schema_meta").map(|t| t.export),
            Some(Export::OnlyKeys(&["site_id"]))
        );
    }

    #[test]
    fn cells_encode_and_decode_neutrally() {
        let id = Uuid::parse_str("0192f0c4-7b1a-7c3e-9a4b-0123456789ab").unwrap();
        let cases = [
            (
                ColType::Id,
                Cell::Id(id),
                json!("0192f0c4-7b1a-7c3e-9a4b-0123456789ab"),
            ),
            (
                ColType::Timestamp,
                Cell::Micros(1_767_315_723_000_004),
                json!("2026-01-02T01:02:03.000004Z"),
            ),
            (
                ColType::Timestamp,
                Cell::Micros(-1),
                json!("1969-12-31T23:59:59.999999Z"),
            ),
            (
                ColType::Json,
                Cell::Json(json!({"a": [1]})),
                json!({"a": [1]}),
            ),
            (ColType::Bool, Cell::Bool(false), json!(false)),
            (ColType::Bytes, Cell::Bytes(vec![0, 255]), json!("AP8=")),
            (ColType::Text, Cell::Text("é".into()), json!("é")),
            (ColType::Int, Cell::Int(i64::MIN), json!(i64::MIN)),
            (ColType::Text, Cell::Null, Value::Null),
        ];
        for (col, cell, value) in cases {
            assert_eq!(encode_cell(cell.clone()).unwrap(), value, "{col:?}");
            assert_eq!(decode_cell(col, &value).unwrap(), cell, "{col:?}");
        }
    }

    #[test]
    fn decode_refuses_non_canonical_values() {
        for (col, value) in [
            (ColType::Timestamp, json!("2026-01-02T01:02:03Z")),
            (
                ColType::Timestamp,
                json!("2026-01-02T01:02:03.000004+00:00"),
            ),
            (ColType::Timestamp, json!("2026-01-02T01:02:03.0000041Z")),
            (ColType::Id, json!("0192F0C4-7B1A-7C3E-9A4B-0123456789AB")),
            (ColType::Id, json!("0192f0c47b1a7c3e9a4b0123456789ab")),
            (ColType::Bytes, json!("AP8")),
            (ColType::Int, json!(1.5)),
            (ColType::Bool, json!(1)),
            (ColType::Text, json!(1)),
        ] {
            assert!(
                matches!(decode_cell(col, &value), Err(StoreError::Invalid(_))),
                "{col:?} {value}"
            );
        }
    }

    #[test]
    fn json_null_is_a_value_in_a_not_null_column() {
        let meta = table("content_meta").unwrap();
        assert_eq!(
            decode_column(meta, "value", ColType::Json, &Value::Null).unwrap(),
            Cell::Json(Value::Null)
        );
        let autosaves = table("autosaves").unwrap();
        assert_eq!(
            decode_column(autosaves, "seo", ColType::Json, &Value::Null).unwrap(),
            Cell::Null
        );
        let users = table("users").unwrap();
        assert_eq!(
            decode_column(users, "totp_secret", ColType::Text, &Value::Null).unwrap(),
            Cell::Null
        );
    }

    #[test]
    fn key_cells_follow_the_key_order() {
        let t = table("plugin_kv").unwrap();
        let mut row = Row::new();
        row.insert("key".into(), json!("k"));
        row.insert("plugin".into(), json!("p"));
        assert_eq!(
            key_cells(t, &row).unwrap(),
            [Cell::Text("p".into()), Cell::Text("k".into())]
        );
        row.remove("key");
        assert!(matches!(key_cells(t, &row), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn check_exported_refuses_skipped_and_foreign_specs() {
        assert!(check_exported(table("users").unwrap()).is_ok());
        assert!(matches!(
            check_exported(table("sessions").unwrap()),
            Err(StoreError::Invalid(_))
        ));
        let fake = TableSpec {
            name: "users",
            columns: &[("id", ColType::Id)],
            key: &["id"],
            self_ref: None,
            nullable: &[],
            export: Export::Rows,
        };
        assert!(matches!(check_exported(&fake), Err(StoreError::Invalid(_))));
    }
}
