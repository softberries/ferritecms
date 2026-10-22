//! Contract tests for full-text search (FTS5 on SQLite, tsvector on Postgres).

use ferrite_domain::{ContentId, MAX_PAGE};
use ferrite_store::{Store, StoreError};

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

/// Inserts `n` posts and returns their ids.
async fn posts(store: &dyn Store, n: usize) -> Vec<ContentId> {
    let author = fixture_user(store).await;
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let item = store
            .read()
            .content()
            .insert(&new_post(&author, &format!("post-{i}")))
            .await
            .unwrap();
        ids.push(item.id);
    }
    ids
}

pub async fn search_finds_title_and_body(db: &TestDb) {
    let store = db.store();
    let ids = posts(store, 3).await;
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let search = store.read().search();
    search
        .index(a, "Ferrite rustacean", "nothing to see here")
        .await
        .unwrap();
    search
        // A short body: only the title weight can rank A above B.
        .index(b, "Weekly notes", "rustacean")
        .await
        .unwrap();
    search
        .index(c, "Unrelated", "nothing relevant at all")
        .await
        .unwrap();

    assert_eq!(search.query("rustacean", 10).await.unwrap(), [a, b]);
    // Case-insensitive, and every term must match.
    assert_eq!(search.query("RUSTACEAN", 10).await.unwrap(), [a, b]);
    assert_eq!(search.query("rustacean weekly", 10).await.unwrap(), [b]);
    assert_eq!(search.query("nothing", 10).await.unwrap().len(), 2);
    assert_eq!(search.query("rustacean", 1).await.unwrap(), [a]);
    assert!(search.query("absent", 10).await.unwrap().is_empty());
}

pub async fn search_reindex_replaces(db: &TestDb) {
    let store = db.store();
    let id = posts(store, 1).await[0];
    let search = store.read().search();
    search.index(id, "old title", "old body").await.unwrap();
    search.index(id, "new title", "fresh body").await.unwrap();
    assert_eq!(search.query("title", 10).await.unwrap(), [id]);
    assert_eq!(search.query("fresh", 10).await.unwrap(), [id]);
    assert!(search.query("old", 10).await.unwrap().is_empty());

    // Inside a transaction: visible to the transaction, gone after rollback.
    let tx = store.begin().await.unwrap();
    tx.search().index(id, "draft", "draft").await.unwrap();
    assert_eq!(tx.search().query("draft", 10).await.unwrap(), [id]);
    tx.rollback().await.unwrap();
    assert!(search.query("draft", 10).await.unwrap().is_empty());
    assert_eq!(search.query("fresh", 10).await.unwrap(), [id]);
}

pub async fn search_remove_and_cascade(db: &TestDb) {
    let store = db.store();
    let ids = posts(store, 2).await;
    let search = store.read().search();
    search.index(ids[0], "alpha", "shared").await.unwrap();
    search.index(ids[1], "beta", "shared").await.unwrap();

    search.remove(ids[0]).await.unwrap();
    assert!(search.query("alpha", 10).await.unwrap().is_empty());
    assert_eq!(search.query("shared", 10).await.unwrap(), [ids[1]]);
    // Removing an unindexed item is a no-op.
    search.remove(ids[0]).await.unwrap();

    store.read().content().delete(ids[1]).await.unwrap();
    assert!(search.query("shared", 10).await.unwrap().is_empty());
    assert!(search.query("beta", 10).await.unwrap().is_empty());

    // A delete that fails (children RESTRICT) keeps the search row as well.
    let author = fixture_user(store).await;
    let mut parent = new_post(&author, "search-parent");
    parent.kind = "page".into();
    let parent = store.read().content().insert(&parent).await.unwrap();
    let mut child = new_post(&author, "search-child");
    child.kind = "page".into();
    child.parent_id = Some(parent.id);
    store.read().content().insert(&child).await.unwrap();
    search.index(parent.id, "keeper", "").await.unwrap();
    assert!(matches!(
        store.read().content().delete(parent.id).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert_eq!(search.query("keeper", 10).await.unwrap(), [parent.id]);

    // Like every other item reference, an unknown item is a foreign-key error.
    assert!(matches!(
        search.index(ContentId::new(), "t", "b").await,
        Err(StoreError::ForeignKey { .. })
    ));
}

pub async fn search_handles_operators_safely(db: &TestDb) {
    let store = db.store();
    let id = posts(store, 1).await[0];
    let search = store.read().search();
    search
        .index(id, "a title", "x marks the spot")
        .await
        .unwrap();
    for q in [
        "\"",
        "a OR",
        "-x",
        "(",
        "*",
        "",
        "   ",
        "NEAR(a b)",
        "title:x",
        "a AND",
        "\"unbalanced",
        "x\"y",
        "^a",
        "a & b | !c",
        "');DROP TABLE search_fts;--",
    ] {
        let got = search.query(q, 10);
        assert!(got.await.is_ok(), "query {q:?} failed");
    }
    assert!(search.query("", 10).await.unwrap().is_empty());
    // The index is intact afterwards.
    assert_eq!(search.query("spot", 10).await.unwrap(), [id]);
}

pub async fn search_rejects_nul_and_bad_limits(db: &TestDb) {
    let store = db.store();
    let id = posts(store, 1).await[0];
    let search = store.read().search();
    for r in [
        search.index(id, "t\0", "b").await,
        search.index(id, "t", "b\0").await,
    ] {
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{r:?}");
    }
    for limit in [0, MAX_PAGE + 1] {
        assert!(matches!(
            search.query("x", limit).await,
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            search.query("", limit).await,
            Err(StoreError::Invalid(_))
        ));
    }
    search.index(id, "t", "b").await.unwrap();
    assert_eq!(search.query("t", MAX_PAGE).await.unwrap(), [id]);
}

/// R16/R17: both backends match exactly the same documents (only ranking may differ), so
/// these compare sorted id sets against fixed expectations.
pub async fn search_semantics_identical(db: &TestDb) {
    let store = db.store();
    let ids = posts(store, 11).await;
    let search = store.read().search();
    let long_body = format!("{} contact user@example.com", "a".repeat(3000));
    let big_body: String = (0..5000).map(|i| format!("w{i} ")).collect();
    let docs: [(&str, &str); 11] = [
        ("Ünïcode handbook", "about ünïcode text"),
        ("Unicode basics", "plain unicode"),
        ("Dash notes", "x marks the spot"),
        ("Logic", "a or b"),
        ("Other", "nothing"),
        ("हिन्दी भाषा", "Hindi text"),
        ("İstanbul", "city"),
        ("Cafe\u{301} menu", "nfd input"),
        ("Fullwidth ｆｏｏ", "squared x²"),
        ("Long", &long_body),
        // 1 + 5000 distinct words: only the first 4096 distinct terms are indexed.
        ("big", &big_body),
    ];
    for (id, (title, body)) in ids.iter().zip(docs) {
        search.index(*id, title, body).await.unwrap();
    }
    let found = |q: String| async move {
        let mut got = search.query(&q, 50).await.unwrap();
        got.sort();
        got
    };
    let set = |idx: &[usize]| {
        let mut v: Vec<ContentId> = idx.iter().map(|&i| ids[i]).collect();
        v.sort();
        v
    };
    let cases: Vec<(String, Vec<usize>)> = vec![
        // Prefix matching.
        ("uni".into(), vec![1]),
        ("hand".into(), vec![0]),
        // Accent-sensitive, case-insensitive.
        ("unicode".into(), vec![1]),
        ("ünïcode".into(), vec![0]),
        ("ÜNÏ".into(), vec![0]),
        // Combining-mark scripts and dotted capital I stay whole on both backends.
        ("हिन्दी".into(), vec![5]),
        ("हिन".into(), vec![5]),
        ("भाषा".into(), vec![5]),
        ("İstanbul".into(), vec![6]),
        ("i\u{307}stanbul".into(), vec![6]),
        ("istanbul".into(), vec![]),
        // NFC and NFD input are the same term.
        ("café".into(), vec![7]),
        ("cafe\u{301}".into(), vec![7]),
        // NFKC: fullwidth and superscript fold.
        ("foo".into(), vec![8]),
        ("ｆｏｏ".into(), vec![8]),
        ("x²".into(), vec![8]),
        ("x2".into(), vec![8]),
        // No operators: punctuation separates literal terms, all terms must match.
        ("-x".into(), vec![2, 8]),
        ("\"x\"".into(), vec![2, 8]),
        ("a OR b".into(), vec![3]),
        ("unicode OR nothing".into(), vec![]),
        ("x -marks".into(), vec![2]),
        ("(spot)*".into(), vec![2]),
        ("title:x".into(), vec![]),
        ("- * ( )".into(), vec![]),
        // A lone combining mark is no term: empty without a statement.
        ("\u{301}".into(), vec![]),
        // Over-long terms are truncated the same way on both sides.
        ("a".repeat(3000), vec![9]),
        ("aaaa".into(), vec![9]),
        // An e-mail address is split like any other text.
        ("example".into(), vec![9]),
        ("user example com".into(), vec![9]),
        // The 4096-term cap: w4094 is the last indexed term.
        ("w4094".into(), vec![10]),
        ("w4095".into(), vec![]),
        // 40 terms: only the first 32 (all in doc 10) count.
        (
            (0..32)
                .map(|i| format!("w{i}"))
                .chain((0..8).map(|i| format!("zz{i}")))
                .collect::<Vec<_>>()
                .join(" "),
            vec![10],
        ),
    ];
    for (q, want) in cases {
        let label: String = q.chars().take(40).collect();
        assert_eq!(found(q).await, set(&want), "query {label:?}");
    }
}
