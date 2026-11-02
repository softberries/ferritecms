//! Contract tests for the single-transaction row loader (M4-1 P4), and the rich-site
//! fixture shared with the cross-backend round trip.

use std::time::Duration;

use ferrite_store::{
    StoreError,
    catalogue::{ColType, Export, Row, TABLES, TableSpec},
};
use serde_json::{Value, json};

use crate::{
    TestDb,
    export::{dump, row, spec, wait_gone},
    fixtures::fixture_user,
};

/// The tables of one dump or load, in catalogue order.
pub type Dump = Vec<(&'static str, Vec<Row>)>;

fn uid(n: u128) -> String {
    uuid::Uuid::from_u128(0x0192_f0c4_0000_7000_8000_0000_0000_0000 | n)
        .hyphenated()
        .to_string()
}

const T_PRE_1970: &str = "1969-07-20T20:17:40.000001Z";
const T_EDGE: &str = "1969-12-31T23:59:59.999999Z";
const T_EPOCH: &str = "1970-01-01T00:00:00.000000Z";
const T_NOW: &str = "2026-01-02T01:02:03.000004Z";

/// Ids of the rich site.
mod ids {
    pub const U1: u128 = 1;
    pub const U2: u128 = 2;
    pub const P1: u128 = 0x101;
    pub const P2: u128 = 0x102;
    pub const P3: u128 = 0x103;
    pub const P4: u128 = 0x104;
    pub const P5: u128 = 0x105;
    pub const P6: u128 = 0x106;
    pub const POST: u128 = 0x110;
    pub const MEDIA: u128 = 0x111;
    pub const T1: u128 = 0x201;
    pub const T2: u128 = 0x202;
    pub const T3: u128 = 0x203;
    pub const TAG: u128 = 0x204;
    pub const M1: u128 = 0x301;
    pub const M2: u128 = 0x302;
    pub const M3: u128 = 0x303;
    pub const C1: u128 = 0x401;
    pub const C2: u128 = 0x402;
    pub const C3: u128 = 0x403;
}

fn page_row(id: u128, slug: &str, parent: Option<u128>) -> Value {
    json!({
        "id": uid(id), "type": "page", "status": "published", "slug": slug,
        "parent_id": parent.map(uid), "author_id": uid(ids::U1), "title": format!("Page {slug}"),
        "excerpt": "", "body": [{"type": "paragraph", "text": slug}], "fields": {},
        "published_at": T_NOW, "created_at": T_PRE_1970, "updated_at": T_NOW,
    })
}

/// A site touching every exported table: TOTP state, a deep page tree whose slugs repeat
/// under different parents, nested terms, comments and menu items (every tree given
/// child before parent), media with variants, plugin kv bytes, audit rows, an import map,
/// timestamps with micros and before 1970. Columns with defaults are partly left out.
pub fn rich_site() -> Dump {
    use ids::*;
    let h = |c: char| format!("sha256:{}", c.to_string().repeat(64));
    let rows = |v: Vec<Value>| v.into_iter().map(row).collect::<Vec<_>>();
    vec![
        (
            "content_types",
            rows(vec![
                json!({"name": "page", "label": "Pages", "hierarchical": true,
                       "has_archive": false, "fields": {}, "built_in": true,
                       "created_at": T_EPOCH}),
                json!({"name": "post", "label": "Posts", "hierarchical": false,
                       "has_archive": true, "fields": {}, "built_in": true,
                       "created_at": T_EPOCH}),
                json!({"name": "media", "label": "Media", "hierarchical": false,
                       "has_archive": false, "fields": {}, "created_at": T_EPOCH}),
                json!({"name": "Book_x", "label": "Books", "hierarchical": true,
                       "has_archive": true,
                       "fields": {"isbn": {"type": "text", "required": true}},
                       "created_at": T_NOW}),
            ]),
        ),
        (
            "users",
            rows(vec![
                json!({"id": uid(U1), "login": "Ann", "email": "Ann@Example.com",
                       "email_key": "ann@example.com", "display_name": "Ann Ż",
                       "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA",
                       "role": "admin", "status": "active", "must_reset": false,
                       "totp_secret": "sealed:v1:AAECAwQFBgcICQoLDA0ODw==",
                       "created_at": T_PRE_1970, "last_login_at": T_EDGE,
                       "totp_last_step": 59_123_456_i64}),
                json!({"id": uid(U2), "login": "bob", "email": "bob@example.com",
                       "email_key": "bob@example.com", "display_name": "Bob",
                       "role": "author", "status": "active", "must_reset": true,
                       "created_at": T_NOW}),
            ]),
        ),
        (
            "content_items",
            rows(vec![
                page_row(P5, "deep", Some(P4)),
                page_row(P4, "deeper", Some(P3)),
                page_row(P3, "team", Some(P2)),
                page_row(P6, "team", None),
                page_row(P2, "team", Some(P1)),
                page_row(P1, "about", None),
                json!({"id": uid(MEDIA), "type": "media", "status": "published",
                       "slug": "photo", "parent_id": uid(POST), "author_id": uid(U2),
                       "title": "Photo", "excerpt": "", "body": [], "fields": {},
                       "created_at": T_NOW, "updated_at": T_NOW}),
                json!({"id": uid(POST), "type": "post", "status": "draft", "slug": "hello",
                       "author_id": uid(U2), "title": "Hello", "excerpt": "Hi",
                       "body": [{"type": "heading", "level": 2, "text": "ü"}],
                       "body_html": "<h2>ü</h2>",
                       "fields": {"n": 1.5, "big": 9_007_199_254_740_993_i64},
                       "menu_order": -3, "scheduled_at": T_EDGE, "created_at": T_NOW,
                       "updated_at": T_NOW, "version": 7}),
            ]),
        ),
        (
            "content_meta",
            rows(vec![
                json!({"item_id": uid(POST), "key": "_seo",
                       "value": {"title": "SEO", "noindex": false}}),
                json!({"item_id": uid(POST), "key": "count", "value": 0.1}),
                // A stored JSON `null` (a plugin's `meta_set(.., "null")`) in a NOT NULL
                // column: exported as `null`, loaded as the JSON value again.
                json!({"item_id": uid(POST), "key": "nothing", "value": null}),
            ]),
        ),
        (
            "revisions",
            rows(vec![json!({
                "id": uid(0x501), "item_id": uid(POST), "author_id": uid(U2),
                "created_at": T_NOW, "title": "Hello", "excerpt": "", "body": [],
                "fields": {}, "meta": {"a": [1, null]}, "note": "first"})]),
        ),
        (
            "taxonomies",
            rows(vec![
                json!({"name": "category", "label": "Categories", "hierarchical": true,
                       "content_types": ["post"], "built_in": true}),
                json!({"name": "post_tag", "label": "Tags", "hierarchical": false,
                       "content_types": ["post", "Book_x"]}),
            ]),
        ),
        (
            "terms",
            rows(vec![
                json!({"id": uid(T3), "taxonomy": "category", "slug": "leaf",
                       "name": "Leaf", "description": "", "parent_id": uid(T2)}),
                json!({"id": uid(T2), "taxonomy": "category", "slug": "mid", "name": "Mid",
                       "description": "", "parent_id": uid(T1)}),
                json!({"id": uid(T1), "taxonomy": "category", "slug": "root",
                       "name": "Root", "description": "top"}),
                json!({"id": uid(TAG), "taxonomy": "post_tag", "slug": "Rust_lang",
                       "name": "Rust", "description": ""}),
            ]),
        ),
        (
            "term_links",
            rows(vec![
                json!({"item_id": uid(POST), "term_id": uid(T3), "position": 0}),
                json!({"item_id": uid(POST), "term_id": uid(TAG), "position": 1}),
            ]),
        ),
        (
            "menus",
            rows(vec![json!({"name": "main", "label": "Main"})]),
        ),
        (
            "menu_items",
            rows(vec![
                json!({"id": uid(M3), "menu": "main", "parent_id": uid(M2), "position": 0,
                       "kind": "url", "url": "https://example.com/", "label": "Out"}),
                json!({"id": uid(M2), "menu": "main", "parent_id": uid(M1), "position": 0,
                       "kind": "term", "target_id": uid(T1), "label": "Root"}),
                json!({"id": uid(M1), "menu": "main", "position": 0, "kind": "item",
                       "target_id": uid(P1), "label": "About"}),
            ]),
        ),
        (
            "artifacts",
            rows(vec![
                json!({"hash": h('a'), "kind": "media", "bytes": 1234, "mime": "image/png",
                       "created_at": T_NOW}),
                json!({"hash": h('b'), "kind": "variant", "bytes": 99,
                       "mime": "image/webp", "created_at": T_NOW, "orphaned_at": T_EDGE}),
                json!({"hash": h('c'), "kind": "plugin", "bytes": 5_000_000_000_i64,
                       "mime": "application/wasm", "created_at": T_NOW}),
                json!({"hash": h('d'), "kind": "theme", "bytes": 1,
                       "mime": "application/zip", "created_at": T_NOW}),
            ]),
        ),
        (
            "media",
            rows(vec![json!({
                "item_id": uid(MEDIA), "artifact_hash": h('a'), "mime": "image/png",
                "bytes": 1234, "width": 640, "height": 480, "alt": "A photo",
                "original_name": "Photo 1.PNG",
                "variants": {"thumb": {"hash": h('b'), "width": 150, "height": 113}},
                "sanitized": true})]),
        ),
        (
            "media_variants",
            rows(vec![
                json!({"hash": h('b'), "item_id": uid(MEDIA), "name": "thumb"}),
            ]),
        ),
        (
            "comments",
            rows(vec![
                json!({"id": uid(C3), "item_id": uid(POST), "parent_id": uid(C2),
                       "author_name": "Eve", "author_email": "eve@example.com",
                       "author_url": "", "body": "third", "status": "pending",
                       "created_at": T_NOW}),
                json!({"id": uid(C2), "item_id": uid(POST), "parent_id": uid(C1),
                       "user_id": uid(U2), "author_name": "Bob",
                       "author_email": "bob@example.com", "author_url": "",
                       "body": "second", "status": "approved", "ip_hash": "ab12",
                       "created_at": T_NOW, "by_account": true}),
                json!({"id": uid(C1), "item_id": uid(POST), "author_name": "Zoë",
                       "author_email": "z@example.com", "author_url": "https://z.example",
                       "body": "first\nline", "status": "approved",
                       "created_at": T_PRE_1970}),
            ]),
        ),
        (
            "redirects",
            rows(vec![json!({
                "id": uid(0x601), "source_path": "/2020/01/hello/", "target": "/hello/",
                "code": 301, "hits": 7, "origin": "import", "created_at": T_NOW,
                "last_hit_at": T_NOW})]),
        ),
        (
            "user_meta",
            rows(vec![
                json!({"user_id": uid(U1), "key": "prefs", "value": {"theme": "dark"}}),
                json!({"user_id": uid(U1), "key": "cleared", "value": null}),
            ]),
        ),
        (
            "options",
            rows(vec![
                json!({"key": "site.title", "value": "Zażółć", "autoload": true,
                       "updated_at": T_NOW}),
                json!({"key": "nums",
                       "value": [1.0, 0.1, 0, 1e3, 9_007_199_254_740_993_i64, -7],
                       "updated_at": T_EDGE}),
            ]),
        ),
        (
            "plugins",
            rows(vec![json!({
                "name": "seo", "version": "1.2.3", "artifact_hash": h('c'),
                "manifest": {"name": "seo", "hooks": ["head"]}, "granted": {"kv": true},
                "enabled": true, "installed_at": T_NOW, "updated_at": T_NOW})]),
        ),
        (
            "plugin_kv",
            rows(vec![
                json!({"plugin": "seo", "key": "state", "value": "AAEC//4=",
                       "updated_at": T_NOW}),
                json!({"plugin": "seo", "key": "empty", "value": "",
                       "updated_at": T_PRE_1970}),
            ]),
        ),
        (
            "themes",
            rows(vec![json!({
                "name": "blog", "version": "0.1.0", "artifact_hash": h('d'),
                "manifest": {"name": "blog"}, "active": true, "installed_at": T_NOW})]),
        ),
        (
            "schema_meta",
            rows(vec![json!({"key": "site_id", "value": "site-rich"})]),
        ),
        (
            "api_tokens",
            rows(vec![json!({
                "id": uid(0x701), "user_id": uid(U1), "name": "ci",
                "token_hash": "f".repeat(64), "scopes": ["content:read", "media:write"],
                "created_at": T_NOW, "expires_at": "2099-12-31T23:59:59.999999Z"})]),
        ),
        (
            "audit_log",
            rows(vec![
                json!({"id": uid(0x801), "at": T_NOW, "actor_id": uid(U1),
                       "action": "user.login", "target_kind": "user",
                       "target_id": uid(U1), "details": {"ip": "x"}, "node": "n1"}),
                json!({"id": uid(0x802), "at": T_PRE_1970, "action": "system.boot",
                       "target_kind": "system", "details": {}}),
            ]),
        ),
        (
            "autosaves",
            rows(vec![json!({
                "item_id": uid(POST), "user_id": uid(U2), "base_version": 7,
                "title": "Hello!", "excerpt": null, "slug": "hello-2",
                "parent_id": uid(P1), "menu_order": 2, "terms": [uid(T3)], "body": [],
                "fields": {}, "saved_at": T_NOW, "seo": {"title": "x"}})]),
        ),
        (
            "import_runs",
            rows(vec![json!({
                "id": uid(0x901), "source": "wp:example", "token": "tok", "state": "done",
                "started_by": uid(U1), "started_at": T_NOW, "heartbeat_at": T_NOW,
                "finished_at": T_NOW, "stats": {"content": 2}, "dry_run": false,
                "job_id": uid(0x902), "step": "done", "report_hash": h('a')})]),
        ),
        (
            "import_map",
            rows(vec![
                json!({"source": "wp:example", "kind": "content", "wp_id": 42,
                       "ferrite_id": uid(POST), "hash": "h1", "version": 1}),
                json!({"source": "wp:example", "kind": "user", "wp_id": 1,
                       "ferrite_id": uid(U1), "hash": "h2", "version": 3}),
            ]),
        ),
    ]
}

/// Every exported table of `store`, in catalogue order, paged `limit` rows at a time.
pub async fn dump_all(store: &dyn ferrite_store::Store, limit: u32) -> Dump {
    let mut reader = store.export_reader().await.unwrap();
    let mut out = Vec::new();
    for t in TABLES.iter().filter(|t| t.is_exported()) {
        out.push((t.name, dump(reader.as_mut(), t, limit).await));
    }
    reader.finish().await.unwrap();
    out
}

/// Clears every exported table of `store`, then loads `data` (in its order), `per_call`
/// rows per [`ImportWriter::rows`](ferrite_store::ImportWriter::rows) call, and commits.
pub async fn load_all(store: &dyn ferrite_store::Store, data: &Dump, per_call: usize) {
    let mut writer = store.import_writer().await.unwrap();
    let exported: Vec<&TableSpec> = TABLES.iter().filter(|t| t.is_exported()).collect();
    writer.clear(&exported).await.unwrap();
    for (name, rows) in data {
        for chunk in rows.chunks(per_call) {
            writer
                .rows(spec(name), chunk.to_vec())
                .await
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        writer.finish_table(spec(name)).await.unwrap();
    }
    writer.commit().await.unwrap();
}

/// Rows written through the repositories (their own encodings), on top of a load.
pub async fn add_repo_rows(store: &dyn ferrite_store::Store) {
    let user = fixture_user(store).await;
    let tx = store.begin().await.unwrap();
    tx.content()
        .insert(&crate::fixtures::new_post(&user, "from-repo"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    store
        .read()
        .plugin_kv()
        .set("seo", "repo", &[0, 159, 146, 150, 255])
        .await
        .unwrap();
    store
        .read()
        .options()
        .set("repo", &json!({"z": [true, 2.25]}), true)
        .await
        .unwrap();
}

/// Asserts two dumps are equal table by table, row by row.
pub fn assert_same(left: &Dump, right: &Dump) {
    assert_eq!(
        left.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        right.iter().map(|(n, _)| *n).collect::<Vec<_>>()
    );
    for ((name, l), (_, r)) in left.iter().zip(right) {
        assert_eq!(l.len(), r.len(), "row count of {name}");
        for (a, b) in l.iter().zip(r) {
            assert_eq!(a, b, "a row of {name}");
        }
    }
}

fn rows_of<'a>(dump: &'a Dump, name: &str) -> &'a [Row] {
    &dump.iter().find(|(n, _)| *n == name).unwrap().1
}

/// A second migrated database on the same backend as `db`.
async fn fresh_like(db: &TestDb) -> TestDb {
    if db.is_sqlite() {
        TestDb::sqlite().await
    } else {
        TestDb::postgres().await.expect("postgres is configured")
    }
}

/// Every exported table (for [`ImportWriter::clear`](ferrite_store::ImportWriter::clear)).
fn exported() -> Vec<&'static TableSpec> {
    TABLES.iter().filter(|t| t.is_exported()).collect()
}

pub async fn import_writer_round_trip(db: &TestDb) {
    let source = db.store();
    load_all(source, &rich_site(), 2).await;
    add_repo_rows(source).await;
    let first = dump_all(source, 3).await;

    // What the load wrote, spot-checked against the input.
    let users = rows_of(&first, "users");
    let ann = users.iter().find(|u| u["login"] == json!("Ann")).unwrap();
    assert_eq!(ann["created_at"], json!(T_PRE_1970));
    assert_eq!(ann["last_login_at"], json!(T_EDGE));
    assert_eq!(
        ann["totp_secret"],
        json!("sealed:v1:AAECAwQFBgcICQoLDA0ODw==")
    );
    assert_eq!(ann["totp_last_step"], json!(59_123_456));
    assert_eq!(rows_of(&first, "content_items").len(), 9);
    let deep = rows_of(&first, "content_items")
        .iter()
        .find(|r| r["id"] == json!(uid(ids::P5)))
        .unwrap();
    assert_eq!(deep["parent_id"], json!(uid(ids::P4)));
    assert_eq!(
        deep["menu_order"],
        json!(0),
        "a default fills a left-out column"
    );
    let values: Vec<(&str, &str)> = rows_of(&first, "plugin_kv")
        .iter()
        .map(|r| (r["key"].as_str().unwrap(), r["value"].as_str().unwrap()))
        .collect();
    assert_eq!(
        values,
        [("empty", ""), ("repo", "AJ+Slv8="), ("state", "AAEC//4=")]
    );
    let nums = rows_of(&first, "options")
        .iter()
        .find(|r| r["key"] == json!("nums"))
        .unwrap();
    assert_eq!(
        nums["value"],
        json!([1.0, 0.1, 0, 1e3, 9_007_199_254_740_993_i64, -7])
    );
    let nothing = rows_of(&first, "content_meta")
        .iter()
        .find(|r| r["key"] == json!("nothing"))
        .expect("the JSON-null meta row is loaded");
    assert_eq!(nothing["value"], Value::Null);
    assert_eq!(
        rows_of(&first, "schema_meta"),
        [row(json!({"key": "site_id", "value": "site-rich"}))]
    );
    for (name, rows) in &first {
        if matches!(spec(name).export, Export::Rows | Export::RowsWithout(_)) {
            assert!(!rows.is_empty(), "{name} is empty in the rich site");
        }
    }

    // Into a fresh store and out again.
    let target = fresh_like(db).await;
    load_all(target.store(), &first, 3).await;
    let second = dump_all(target.store(), 1000).await;
    assert_same(&first, &second);
    target.teardown().await;
}

/// Waits until no loader connection of `db` is left (Postgres; SQLite: no-op), so a
/// rolled-back or dropped loader that leaked its connection fails the test.
async fn released(db: &TestDb) {
    if let Some((_, schema)) = db.pg_location() {
        wait_gone(db, &format!("{schema}:import")).await;
    }
}

pub async fn import_writer_rollback_leaves_nothing(db: &TestDb) {
    let store = db.store();
    let before = dump_all(store, 100).await;
    let site = rich_site();
    let (last, early) = site.split_last().unwrap();
    assert_eq!(last.0, "import_map");

    // 1. A refused row (unknown column) in the last table: nothing lands.
    let mut writer = store.import_writer().await.unwrap();
    writer.clear(&exported()).await.unwrap();
    for (name, rows) in early {
        writer.rows(spec(name), rows.clone()).await.unwrap();
        writer.finish_table(spec(name)).await.unwrap();
    }
    let mut bad = last.1.clone();
    bad[1].insert("surprise".into(), json!(1));
    assert!(matches!(
        writer.rows(spec("import_map"), bad).await,
        Err(StoreError::Invalid(m)) if m.contains("unknown column surprise")
    ));
    // Poisoned: later calls and the commit fail.
    assert!(matches!(
        writer.rows(spec("import_map"), Vec::new()).await,
        Err(StoreError::Aborted)
    ));
    assert!(matches!(writer.commit().await, Err(StoreError::Aborted)));
    released(db).await;
    assert_same(&before, &dump_all(store, 100).await);

    // 2. A row the database refuses (a NOT NULL column left out).
    let mut writer = store.import_writer().await.unwrap();
    writer.clear(&exported()).await.unwrap();
    for (name, rows) in early {
        writer.rows(spec(name), rows.clone()).await.unwrap();
    }
    let mut bad = last.1.clone();
    bad[0].remove("hash");
    assert!(writer.rows(spec("import_map"), bad).await.is_err());
    assert!(matches!(writer.commit().await, Err(StoreError::Aborted)));
    released(db).await;
    assert_same(&before, &dump_all(store, 100).await);

    // 3. Dropped without commit.
    let mut writer = store.import_writer().await.unwrap();
    writer.clear(&exported()).await.unwrap();
    writer
        .rows(spec("users"), rows_of(&site, "users").to_vec())
        .await
        .unwrap();
    if let Some((_, schema)) = db.pg_location() {
        // Only this loader's connection: the earlier ones are gone (`released`).
        assert_eq!(db.active_backends(&format!("{schema}:import")).await, 1);
    }
    drop(writer);
    released(db).await;
    assert_same(&before, &dump_all(store, 100).await);
    // The store still writes (SQLite: the writer connection came back).
    tokio::time::timeout(Duration::from_secs(5), fixture_user(store))
        .await
        .expect("the writer is free again");
}

pub async fn import_writer_self_refs_any_order(db: &TestDb) {
    use ids::*;
    let store = db.store();
    let site = rich_site();
    let pick = |name: &str| rows_of(&site, name).to_vec();
    let mut writer = store.import_writer().await.unwrap();
    // No clear: the built-in content types and taxonomies stay; only add what is new.
    writer.rows(spec("users"), pick("users")).await.unwrap();
    // The page tree one row per call, leaf first. Three pages are slugged `team` (under
    // P1, under P2, and top level), which only loads if every row is inserted with its
    // real parent.
    for item in pick("content_items")
        .into_iter()
        .filter(|r| r["type"] == json!("page"))
    {
        writer
            .rows(spec("content_items"), vec![item])
            .await
            .unwrap();
    }
    writer.finish_table(spec("content_items")).await.unwrap();
    let terms: Vec<Row> = pick("terms")
        .into_iter()
        .filter(|r| r["taxonomy"] == json!("category"))
        .collect();
    writer.rows(spec("terms"), terms).await.unwrap();
    writer.finish_table(spec("terms")).await.unwrap();
    writer.rows(spec("menus"), pick("menus")).await.unwrap();
    // The menu tree split over two calls: the root comes in the second.
    let menu = pick("menu_items");
    writer
        .rows(spec("menu_items"), menu[..2].to_vec())
        .await
        .unwrap();
    writer
        .rows(spec("menu_items"), menu[2..].to_vec())
        .await
        .unwrap();
    writer.finish_table(spec("menu_items")).await.unwrap();
    writer.commit().await.unwrap();

    let parents = |got: &Dump, name: &str| -> Vec<(String, Value)> {
        let mut v: Vec<(String, Value)> = rows_of(got, name)
            .iter()
            .map(|r| {
                (
                    r["id"].as_str().unwrap().to_owned(),
                    r[spec(name).self_ref.unwrap()].clone(),
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let expect = |pairs: &[(u128, Option<u128>)]| -> Vec<(String, Value)> {
        let mut v: Vec<(String, Value)> = pairs
            .iter()
            .map(|(id, p)| (uid(*id), json!(p.map(uid))))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let got = dump_all(store, 100).await;
    assert_eq!(
        parents(&got, "content_items"),
        expect(&[
            (P1, None),
            (P2, Some(P1)),
            (P3, Some(P2)),
            (P4, Some(P3)),
            (P5, Some(P4)),
            (P6, None),
        ])
    );
    assert_eq!(
        parents(&got, "terms"),
        expect(&[(T1, None), (T2, Some(T1)), (T3, Some(T2))])
    );
    assert_eq!(
        parents(&got, "menu_items"),
        expect(&[(M1, None), (M2, Some(M1)), (M3, Some(M2))])
    );

    // Comments: a reply chain given newest first, in one call.
    let mut writer = store.import_writer().await.unwrap();
    let post = pick("content_items")
        .into_iter()
        .find(|r| r["id"] == json!(uid(POST)))
        .unwrap();
    writer
        .rows(spec("content_items"), vec![post])
        .await
        .unwrap();
    writer
        .rows(spec("comments"), pick("comments"))
        .await
        .unwrap();
    writer.finish_table(spec("comments")).await.unwrap();
    writer.commit().await.unwrap();
    let got = dump_all(store, 100).await;
    assert_eq!(
        parents(&got, "comments"),
        expect(&[(C1, None), (C2, Some(C1)), (C3, Some(C2))])
    );

    // A parent that never comes: `finish_table` refuses, and so does `commit`.
    let orphan = row(json!({
        "id": uid(0xabc), "taxonomy": "category", "slug": "orphan", "name": "Orphan",
        "description": "", "parent_id": uid(0xdead),
    }));
    let mut writer = store.import_writer().await.unwrap();
    writer
        .rows(spec("terms"), vec![orphan.clone()])
        .await
        .unwrap();
    assert!(matches!(
        writer.finish_table(spec("terms")).await,
        Err(StoreError::Invalid(m)) if m.contains("parent")
    ));
    assert!(matches!(writer.commit().await, Err(StoreError::Aborted)));
    released(db).await;
    let mut writer = store.import_writer().await.unwrap();
    writer.rows(spec("terms"), vec![orphan]).await.unwrap();
    assert!(matches!(
        writer.commit().await,
        Err(StoreError::Invalid(m)) if m.contains("parent")
    ));
    assert_eq!(rows_of(&dump_all(store, 100).await, "terms").len(), 3);
}

pub async fn import_writer_site_id_batches_refusals(db: &TestDb) {
    let store = db.store();
    let meta = store.read().schema_meta();
    let created_by = meta.get("created_by_version").await.unwrap();
    assert!(created_by.is_some());
    let mut writer = store.import_writer().await.unwrap();
    // No clear: the site id is written over the target's.
    writer
        .rows(
            spec("schema_meta"),
            vec![row(json!({"key": "site_id", "value": "from-archive"}))],
        )
        .await
        .unwrap();
    writer.finish_table(spec("schema_meta")).await.unwrap();
    writer.commit().await.unwrap();
    assert_eq!(
        meta.get("site_id").await.unwrap().as_deref(),
        Some("from-archive")
    );
    assert_eq!(meta.get("created_by_version").await.unwrap(), created_by);

    // Clearing schema_meta removes only the site id.
    let mut writer = store.import_writer().await.unwrap();
    writer.clear(&[spec("schema_meta")]).await.unwrap();
    writer.commit().await.unwrap();
    assert_eq!(meta.get("site_id").await.unwrap(), None);
    assert_eq!(meta.get("created_by_version").await.unwrap(), created_by);

    // More rows than one statement takes: several batches, committed together.
    let bulk: Vec<Row> = (0..1201)
        .map(|i| {
            row(json!({
                "key": format!("bulk.{i:05}"), "value": {"i": i}, "autoload": i % 2 == 0,
                "updated_at": T_NOW,
            }))
        })
        .collect();
    let mut writer = store.import_writer().await.unwrap();
    writer.rows(spec("options"), bulk.clone()).await.unwrap();
    writer.finish_table(spec("options")).await.unwrap();
    writer.commit().await.unwrap();
    assert_eq!(rows_of(&dump_all(store, 10_000).await, "options"), bulk);

    // Refusals: another schema_meta key, a skipped table, values of the wrong type.
    let refusals: Vec<(&TableSpec, Row)> = vec![
        (
            spec("schema_meta"),
            row(json!({"key": "created_by_version", "value": "0.0.1"})),
        ),
        (spec("jobs"), row(json!({"id": uid(1)}))),
        (spec("options"), row(json!({"key": "k", "autoload": "yes"}))),
        (spec("users"), row(json!({"id": "not-a-uuid"}))),
    ];
    for (table, r) in refusals {
        let mut writer = store.import_writer().await.unwrap();
        assert!(
            matches!(
                writer.rows(table, vec![r]).await,
                Err(StoreError::Invalid(_))
            ),
            "{}",
            table.name
        );
    }
    // A spec that is not the catalogue's own.
    let fake = TableSpec {
        name: "options",
        columns: &[("key", ColType::Text)],
        key: &["key"],
        self_ref: None,
        nullable: &[],
        export: Export::Rows,
    };
    let mut writer = store.import_writer().await.unwrap();
    assert!(matches!(
        writer.clear(&[&fake]).await,
        Err(StoreError::Invalid(_))
    ));
    drop(writer);
    assert_eq!(meta.get("created_by_version").await.unwrap(), created_by);
}
