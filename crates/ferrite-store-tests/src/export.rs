//! Contract tests for the table catalogue and the snapshot row reader (M4-1 P1–P3).

use std::{collections::HashMap, time::Duration};

use ferrite_store::{
    ExportReader, StoreError,
    catalogue::{self, BACKEND_SPECIFIC, ColType, Export, Row, TABLES, TableSpec},
};
use serde_json::{Value, json};

use crate::{TestDb, fixtures::fixture_user};

/// The declared types a column of `col` may have on this backend.
fn physical_types(sqlite: bool, col: ColType) -> &'static [&'static str] {
    match (sqlite, col) {
        (true, ColType::Id | ColType::Bytes) => &["BLOB"],
        (true, ColType::Timestamp | ColType::Bool | ColType::Int) => &["INTEGER"],
        (true, ColType::Json | ColType::Text) => &["TEXT"],
        (false, ColType::Id) => &["uuid"],
        (false, ColType::Timestamp) => &["timestamp with time zone"],
        (false, ColType::Json) => &["jsonb"],
        (false, ColType::Bool) => &["boolean"],
        (false, ColType::Bytes) => &["bytea"],
        (false, ColType::Text) => &["text"],
        (false, ColType::Int) => &["integer", "bigint"],
    }
}

/// Tables of the live schema the catalogue deliberately leaves out.
fn not_catalogued(sqlite: bool, name: &str) -> bool {
    name == "_sqlx_migrations"
        || (sqlite && (name == "search_fts" || name.starts_with("search_fts_")))
}

pub async fn catalogue_matches_live_schema(db: &TestDb) {
    let sqlite = db.is_sqlite();
    let live: Vec<_> = db
        .live_tables()
        .await
        .into_iter()
        .filter(|t| !not_catalogued(sqlite, &t.name))
        .collect();
    let mut live_names: Vec<&str> = live.iter().map(|t| t.name.as_str()).collect();
    let mut names: Vec<&str> = TABLES.iter().map(|t| t.name).collect();
    live_names.sort_unstable();
    names.sort_unstable();
    assert_eq!(names, live_names, "catalogue tables vs live tables");

    for t in &live {
        let spec = catalogue::table(&t.name).unwrap();
        if BACKEND_SPECIFIC.contains(&spec.name) {
            assert_eq!(spec.export, Export::Skip, "{}", spec.name);
            for (c, ty) in spec.columns {
                let (_, live_ty) = t
                    .columns
                    .iter()
                    .find(|(n, _)| n == c)
                    .unwrap_or_else(|| panic!("{}.{c} missing", spec.name));
                assert!(
                    physical_types(sqlite, *ty).contains(&live_ty.as_str()),
                    "{}.{c}: {ty:?} vs {live_ty}",
                    spec.name
                );
            }
            continue;
        }
        let live_cols: Vec<&str> = t.columns.iter().map(|(n, _)| n.as_str()).collect();
        let cols: Vec<&str> = spec.columns.iter().map(|(n, _)| *n).collect();
        assert_eq!(cols, live_cols, "columns of {}", spec.name);
        for ((c, ty), (_, live_ty)) in spec.columns.iter().zip(&t.columns) {
            assert!(
                physical_types(sqlite, *ty).contains(&live_ty.as_str()),
                "{}.{c}: catalogue {ty:?}, live {live_ty}",
                spec.name
            );
        }
        let key: Vec<&str> = t.key.iter().map(String::as_str).collect();
        assert_eq!(spec.key, key.as_slice(), "primary key of {}", spec.name);
    }
}

pub async fn catalogue_is_fk_ordered(db: &TestDb) {
    let sqlite = db.is_sqlite();
    let pos: HashMap<&str, usize> = TABLES
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name, i))
        .collect();
    let fks = db.live_foreign_keys().await;
    assert!(!fks.is_empty());
    let mut self_refs = Vec::new();
    for (table, column, referenced) in &fks {
        if not_catalogued(sqlite, table) {
            continue;
        }
        let spec = &TABLES[pos[table.as_str()]];
        if table == referenced {
            assert_eq!(
                spec.self_ref,
                Some(column.as_str()),
                "{table}.{column} references its own table"
            );
            self_refs.push(table.as_str());
        } else {
            assert!(
                pos[referenced.as_str()] < pos[table.as_str()],
                "{table}.{column} references {referenced}, which comes later"
            );
        }
    }
    for spec in TABLES.iter().filter(|t| t.self_ref.is_some()) {
        assert!(
            self_refs.contains(&spec.name),
            "{}: self_ref without a foreign key",
            spec.name
        );
    }
}

const ID: &str = "0192f0c4-7b1a-7c3e-9a4b-0123456789ab";
/// 2026-01-02T01:02:03.000004Z.
const MICROS: i64 = 1_767_315_723_000_004;
const STAMP: &str = "2026-01-02T01:02:03.000004Z";
const NESTED: &str = r#"{"c":"x","a":[1,{"b":null}],"n":{"deep":[true,false,1.5]}}"#;

/// One row per column type, written outside the store in each backend's physical form.
async fn seed_typed_rows(db: &TestDb) {
    let sql = if db.is_sqlite() {
        format!(
            "INSERT INTO users (id, login, email, email_key, display_name, password_hash, role, \
             status, must_reset, totp_secret, created_at, last_login_at, totp_last_step) VALUES \
             (x'{hex}', 'ann', 'Ann@Example.com', 'ann@example.com', 'Ann', NULL, 'admin', \
             'active', 1, 'sealed:abc', {MICROS}, NULL, 42); \
             INSERT INTO plugin_kv (plugin, key, value, updated_at) \
             VALUES ('p', 'k', x'00ff', {MICROS}); \
             INSERT INTO options (key, value, autoload, updated_at) \
             VALUES ('nested', '{NESTED}', 0, {MICROS});",
            hex = ID.replace('-', "")
        )
    } else {
        format!(
            "INSERT INTO users (id, login, email, email_key, display_name, password_hash, role, \
             status, must_reset, totp_secret, created_at, last_login_at, totp_last_step) VALUES \
             ('{ID}', 'ann', 'Ann@Example.com', 'ann@example.com', 'Ann', NULL, 'admin', \
             'active', true, 'sealed:abc', '{STAMP}', NULL, 42); \
             INSERT INTO plugin_kv (plugin, key, value, updated_at) \
             VALUES ('p', 'k', '\\x00ff'::bytea, '{STAMP}'); \
             INSERT INTO options (key, value, autoload, updated_at) \
             VALUES ('nested', '{NESTED}', false, '{STAMP}');"
        )
    };
    db.exec_raw(&sql).await;
}

fn spec(name: &str) -> &'static TableSpec {
    catalogue::table(name).unwrap()
}

fn row(value: Value) -> Row {
    match value {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

/// Every row of `table`, `limit` per page.
async fn dump(reader: &mut dyn ExportReader, table: &TableSpec, limit: u32) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    loop {
        let page = reader.page(table, rows.last(), limit).await.unwrap();
        assert!(page.len() <= limit as usize);
        if page.is_empty() {
            return rows;
        }
        rows.extend(page);
    }
}

pub async fn export_reader_encodes_neutrally(db: &TestDb) {
    seed_typed_rows(db).await;
    let mut reader = db.store().export_reader().await.unwrap();
    let users = reader.page(spec("users"), None, 10).await.unwrap();
    assert_eq!(
        users,
        [row(json!({
            "id": ID,
            "login": "ann",
            "email": "Ann@Example.com",
            "email_key": "ann@example.com",
            "display_name": "Ann",
            "password_hash": null,
            "role": "admin",
            "status": "active",
            "must_reset": true,
            "totp_secret": "sealed:abc",
            "created_at": STAMP,
            "last_login_at": null,
            "totp_last_step": 42,
        }))]
    );
    let kv = reader.page(spec("plugin_kv"), None, 10).await.unwrap();
    assert_eq!(
        kv,
        [row(json!({
            "plugin": "p", "key": "k", "value": "AP8=", "updated_at": STAMP,
        }))]
    );
    let options = reader.page(spec("options"), None, 10).await.unwrap();
    assert_eq!(
        options,
        [row(json!({
            "key": "nested",
            "value": {"c": "x", "a": [1, {"b": null}], "n": {"deep": [true, false, 1.5]}},
            "autoload": false,
            "updated_at": STAMP,
        }))]
    );
    // The serialized row is the same bytes on both backends (keys sorted, JSON compact).
    assert_eq!(
        serde_json::to_string(&options[0]).unwrap(),
        format!(
            r#"{{"autoload":false,"key":"nested","updated_at":"{STAMP}","value":{{"a":[1,{{"b":null}}],"c":"x","n":{{"deep":[true,false,1.5]}}}}}}"#
        )
    );
    // Built-in rows from the migrations: booleans both ways, an empty JSON object.
    let types = reader.page(spec("content_types"), None, 10).await.unwrap();
    let names: Vec<&str> = types.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["media", "page", "post"]);
    assert_eq!(types[1]["hierarchical"], json!(true));
    assert_eq!(types[2]["hierarchical"], json!(false));
    assert_eq!(types[2]["fields"], json!({}));
    reader.finish().await.unwrap();
}

pub async fn export_reader_pages_by_key(db: &TestDb) {
    let store = db.store();
    for k in ["p.03", "p.01", "p.05", "p.02", "p.04"] {
        store
            .read()
            .options()
            .set(k, &json!(k), false)
            .await
            .unwrap();
    }
    for (plugin, key) in [("b", "1"), ("a", "2"), ("a", "10"), ("a", "1")] {
        store
            .read()
            .plugin_kv()
            .set(plugin, key, b"v")
            .await
            .unwrap();
    }
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(
            fixture_user(store)
                .await
                .id
                .as_uuid()
                .hyphenated()
                .to_string(),
        );
    }
    ids.sort();
    let mut reader = store.export_reader().await.unwrap();

    let options = dump(reader.as_mut(), spec("options"), 2).await;
    let keys: Vec<&str> = options.iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["p.01", "p.02", "p.03", "p.04", "p.05"]);

    let kv = dump(reader.as_mut(), spec("plugin_kv"), 1).await;
    let keys: Vec<(&str, &str)> = kv
        .iter()
        .map(|r| (r["plugin"].as_str().unwrap(), r["key"].as_str().unwrap()))
        .collect();
    assert_eq!(keys, [("a", "1"), ("a", "10"), ("a", "2"), ("b", "1")]);

    let users = dump(reader.as_mut(), spec("users"), 2).await;
    let got: Vec<&str> = users.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(got, ids);

    // `after` is read for its key only: a bare key works as the cursor.
    let after = row(json!({"key": "p.02"}));
    let page = reader.page(spec("options"), Some(&after), 2).await.unwrap();
    let keys: Vec<&str> = page.iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["p.03", "p.04"]);

    // schema_meta: only the site id (P2).
    let meta = dump(reader.as_mut(), spec("schema_meta"), 10).await;
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0]["key"], json!("site_id"));
    assert!(meta[0]["value"].as_str().is_some_and(|v| !v.is_empty()));

    // Refusals: skipped table, a spec that is not the catalogue's, bad limits, bad cursor.
    for t in TABLES.iter().filter(|t| !t.is_exported()) {
        assert!(
            matches!(reader.page(t, None, 10).await, Err(StoreError::Invalid(_))),
            "{}",
            t.name
        );
    }
    let fake = TableSpec {
        name: "options",
        columns: &[("key", ColType::Text)],
        key: &["key"],
        self_ref: None,
        export: Export::Rows,
    };
    assert!(matches!(
        reader.page(&fake, None, 10).await,
        Err(StoreError::Invalid(_))
    ));
    for limit in [0, ferrite_store::MAX_EXPORT_PAGE + 1] {
        assert!(matches!(
            reader.page(spec("options"), None, limit).await,
            Err(StoreError::Invalid(_))
        ));
    }
    let bad = row(json!({"id": "not-a-uuid"}));
    assert!(matches!(
        reader.page(spec("users"), Some(&bad), 10).await,
        Err(StoreError::Invalid(_))
    ));
    // The reader still works after refusals.
    assert_eq!(dump(reader.as_mut(), spec("options"), 10).await.len(), 5);
    reader.finish().await.unwrap();
}

pub async fn export_reader_sees_one_snapshot(db: &TestDb) {
    let store = db.store();
    store
        .read()
        .options()
        .set("before", &json!(1), false)
        .await
        .unwrap();
    let mut reader = store.export_reader().await.unwrap();
    if let Some((_, schema)) = db.pg_location() {
        assert_eq!(db.active_backends(&format!("{schema}:export")).await, 1);
    }
    // Written after the snapshot was taken (SQLite: through the writer, which the open
    // read transaction must not block).
    tokio::time::timeout(Duration::from_secs(5), async {
        store
            .read()
            .options()
            .set("after", &json!(2), false)
            .await
            .unwrap();
        store
            .read()
            .options()
            .set("before", &json!(3), false)
            .await
            .unwrap();
        let tx = store.begin().await.unwrap();
        tx.options().delete("before").await.unwrap();
        tx.options().set("before", &json!(4), true).await.unwrap();
        tx.commit().await.unwrap();
    })
    .await
    .expect("writes are not blocked by the reader");

    let rows = dump(reader.as_mut(), spec("options"), 10).await;
    let seen: Vec<(&str, &Value)> = rows
        .iter()
        .map(|r| (r["key"].as_str().unwrap(), &r["value"]))
        .collect();
    assert_eq!(seen, [("before", &json!(1))]);
    reader.finish().await.unwrap();
    if let Some((_, schema)) = db.pg_location() {
        assert_eq!(db.active_backends(&format!("{schema}:export")).await, 0);
    }

    let mut reader = store.export_reader().await.unwrap();
    let rows = dump(reader.as_mut(), spec("options"), 10).await;
    let seen: Vec<(&str, &Value)> = rows
        .iter()
        .map(|r| (r["key"].as_str().unwrap(), &r["value"]))
        .collect();
    assert_eq!(seen, [("after", &json!(2)), ("before", &json!(4))]);
    reader.finish().await.unwrap();
}
