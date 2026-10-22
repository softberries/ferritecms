//! Site-level contract tests: options, schema_meta and transaction semantics.

use std::{sync::Arc, time::Duration};

use ferrite_store::StoreError;
use serde_json::json;

use crate::TestDb;

pub async fn options_crud(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let opts = tx.options();
    assert!(opts.get("blogname").await.unwrap().is_none());

    let nested = json!({"name": "Ünïcode ☃ 日本語", "n": [1, 2.5, null, true], "o": {"k": "v"}});
    opts.set("blogname", &nested, true).await.unwrap();
    opts.set("secret", &json!("hidden"), false).await.unwrap();
    tx.commit().await.unwrap();

    let got = store
        .read()
        .options()
        .get("blogname")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.key, "blogname");
    assert_eq!(got.value, nested);
    assert!(got.autoload);

    let auto = store.read().options().autoloaded().await.unwrap();
    assert_eq!(auto.len(), 1);
    assert_eq!(auto[0].key, "blogname");

    let tx = store.begin().await.unwrap();
    tx.options()
        .set("blogname", &json!(42), false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let got2 = store
        .read()
        .options()
        .get("blogname")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got2.value, json!(42));
    assert!(!got2.autoload);
    assert!(got2.updated_at > got.updated_at);
    assert!(
        store
            .read()
            .options()
            .autoloaded()
            .await
            .unwrap()
            .is_empty()
    );

    let tx = store.begin().await.unwrap();
    tx.options().delete("blogname").await.unwrap();
    tx.options().delete("missing").await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        store
            .read()
            .options()
            .get("blogname")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read()
            .options()
            .get("secret")
            .await
            .unwrap()
            .is_some()
    );
}

pub async fn tx_commit_visible(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("k", &json!(1), false).await.unwrap();
    // The transaction sees its own write.
    assert!(tx.options().get("k").await.unwrap().is_some());
    tx.commit().await.unwrap();
    assert!(store.read().options().get("k").await.unwrap().is_some());
}

pub async fn tx_rollback_invisible(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("k", &json!(1), false).await.unwrap();
    assert!(tx.options().get("k").await.unwrap().is_some());
    tx.rollback().await.unwrap();
    assert!(store.read().options().get("k").await.unwrap().is_none());
}

pub async fn tx_dropped_is_rolled_back(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("dropped", &json!(1), false).await.unwrap();
    drop(tx);

    // The writer connection must be released: begin again must not hang.
    let tx = tokio::time::timeout(Duration::from_secs(2), store.begin())
        .await
        .expect("begin() after a dropped Tx timed out")
        .unwrap();
    assert!(tx.options().get("dropped").await.unwrap().is_none());
    tx.options().set("after", &json!(2), false).await.unwrap();
    tx.commit().await.unwrap();

    assert!(
        store
            .read()
            .options()
            .get("dropped")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.read().options().get("after").await.unwrap().is_some());
}

pub async fn read_sees_committed_after_commit(db: &TestDb) {
    let store = db.store();
    // Warm the reader connections first, then commit through the writer.
    assert!(store.read().options().get("warm").await.unwrap().is_none());
    let tx = store.begin().await.unwrap();
    tx.options().set("warm", &json!("up"), false).await.unwrap();
    tx.commit().await.unwrap();
    let got = store.read().options().get("warm").await.unwrap().unwrap();
    assert_eq!(got.value, json!("up"));

    // Autocommit writes are visible to subsequent reads too.
    store
        .read()
        .options()
        .set("auto", &json!(1), true)
        .await
        .unwrap();
    assert!(store.read().options().get("auto").await.unwrap().is_some());
}

pub async fn concurrent_writers(db: Arc<TestDb>) {
    const TASKS: usize = 8;
    const ITERATIONS: usize = 50;
    let handles: Vec<_> = (0..TASKS)
        .map(|t| {
            let db = db.clone();
            tokio::spawn(async move {
                for i in 0..ITERATIONS {
                    let store = db.store();
                    let tx = store.begin().await.unwrap();
                    tx.options()
                        .set(&format!("w{t}-{i}"), &json!(i), false)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }
    for t in 0..TASKS {
        for i in 0..ITERATIONS {
            let key = format!("w{t}-{i}");
            assert!(
                db.store()
                    .read()
                    .options()
                    .get(&key)
                    .await
                    .unwrap()
                    .is_some(),
                "{key} missing"
            );
        }
    }
}

pub async fn options_reject_nul(db: &TestDb) {
    let store = db.store();
    for bad in [
        json!("a\u{0}b"),
        json!({"k": ["x", {"deep": "\u{0}"}]}),
        json!({"key\u{0}": 1}),
    ] {
        let tx = store.begin().await.unwrap();
        let err = tx.options().set("nul", &bad, false).await.unwrap_err();
        assert!(
            matches!(err, StoreError::Invalid(ref m) if m == "json contains NUL"),
            "{err:?}"
        );
        tx.rollback().await.unwrap();
    }
    assert!(store.read().options().get("nul").await.unwrap().is_none());
}

pub async fn options_number_roundtrip(db: &TestDb) {
    let store = db.store();
    let value = json!({"max": u64::MAX, "big": 1e300, "neg": i64::MIN, "f": 0.1});
    let tx = store.begin().await.unwrap();
    tx.options().set("nums", &value, false).await.unwrap();
    tx.commit().await.unwrap();
    let got = store.read().options().get("nums").await.unwrap().unwrap();
    assert_eq!(got.value, value);
}

pub async fn autoloaded_order_is_binary(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    for k in ["B", "a", "\u{e9}", "Z"] {
        tx.options().set(k, &json!(1), true).await.unwrap();
    }
    tx.commit().await.unwrap();
    let keys: Vec<_> = store
        .read()
        .options()
        .autoloaded()
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.key)
        .collect();
    assert_eq!(keys, ["B", "Z", "a", "\u{e9}"]);
}

pub async fn schema_meta_roundtrip(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let meta = tx.schema_meta();
    assert_eq!(meta.get("test_key").await.unwrap(), None);
    meta.set("test_key", "one").await.unwrap();
    assert_eq!(meta.get("test_key").await.unwrap().as_deref(), Some("one"));
    meta.set("test_key", "two").await.unwrap();
    assert_eq!(meta.get("test_key").await.unwrap().as_deref(), Some("two"));
    tx.commit().await.unwrap();
    let read = store.read().schema_meta().get("test_key").await.unwrap();
    assert_eq!(read.as_deref(), Some("two"));
}
