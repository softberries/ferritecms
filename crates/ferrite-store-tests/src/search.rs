//! Contract tests for full-text search (FTS5 on SQLite, tsvector on Postgres).

use ferrite_domain::{ContentId, ContentStatus, MAX_PAGE, SearchQuery};
use ferrite_store::{SearchRepo, Store, StoreError};

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

/// One page of `q` over every kind and status, as ids.
async fn find(search: &dyn SearchRepo, q: &str, limit: u32) -> Result<Vec<ContentId>, StoreError> {
    let query = SearchQuery {
        q: q.into(),
        kinds: Vec::new(),
        statuses: Vec::new(),
        readable_by: None,
        limit,
        cursor: None,
    };
    Ok(search.search(&query).await?.items)
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

    assert_eq!(find(search, "rustacean", 10).await.unwrap(), [a, b]);
    // Case-insensitive, and every term must match.
    assert_eq!(find(search, "RUSTACEAN", 10).await.unwrap(), [a, b]);
    assert_eq!(find(search, "rustacean weekly", 10).await.unwrap(), [b]);
    assert_eq!(find(search, "nothing", 10).await.unwrap().len(), 2);
    assert_eq!(find(search, "rustacean", 1).await.unwrap(), [a]);
    assert!(find(search, "absent", 10).await.unwrap().is_empty());
}

pub async fn search_reindex_replaces(db: &TestDb) {
    let store = db.store();
    let id = posts(store, 1).await[0];
    let search = store.read().search();
    search.index(id, "old title", "old body").await.unwrap();
    search.index(id, "new title", "fresh body").await.unwrap();
    assert_eq!(find(search, "title", 10).await.unwrap(), [id]);
    assert_eq!(find(search, "fresh", 10).await.unwrap(), [id]);
    assert!(find(search, "old", 10).await.unwrap().is_empty());

    // Inside a transaction: visible to the transaction, gone after rollback.
    let tx = store.begin().await.unwrap();
    tx.search().index(id, "draft", "draft").await.unwrap();
    assert_eq!(find(tx.search(), "draft", 10).await.unwrap(), [id]);
    tx.rollback().await.unwrap();
    assert!(find(search, "draft", 10).await.unwrap().is_empty());
    assert_eq!(find(search, "fresh", 10).await.unwrap(), [id]);
}

pub async fn search_remove_and_cascade(db: &TestDb) {
    let store = db.store();
    let ids = posts(store, 2).await;
    let search = store.read().search();
    search.index(ids[0], "alpha", "shared").await.unwrap();
    search.index(ids[1], "beta", "shared").await.unwrap();

    search.remove(ids[0]).await.unwrap();
    assert!(find(search, "alpha", 10).await.unwrap().is_empty());
    assert_eq!(find(search, "shared", 10).await.unwrap(), [ids[1]]);
    // Removing an unindexed item is a no-op.
    search.remove(ids[0]).await.unwrap();

    store.read().content().delete(ids[1]).await.unwrap();
    assert!(find(search, "shared", 10).await.unwrap().is_empty());
    assert!(find(search, "beta", 10).await.unwrap().is_empty());

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
    assert_eq!(find(search, "keeper", 10).await.unwrap(), [parent.id]);

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
        let got = find(search, q, 10);
        assert!(got.await.is_ok(), "query {q:?} failed");
    }
    assert!(find(search, "", 10).await.unwrap().is_empty());
    // The index is intact afterwards.
    assert_eq!(find(search, "spot", 10).await.unwrap(), [id]);
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
            find(search, "x", limit).await,
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            find(search, "", limit).await,
            Err(StoreError::Invalid(_))
        ));
    }
    search.index(id, "t", "b").await.unwrap();
    assert_eq!(find(search, "t", MAX_PAGE).await.unwrap(), [id]);
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
        let mut got = find(search, &q, 50).await.unwrap();
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

/// Inserts a post-like item of `kind` in `status` and returns its id.
async fn item(
    store: &dyn Store,
    author: &ferrite_domain::User,
    slug: &str,
    kind: &str,
    status: ContentStatus,
) -> ContentId {
    let mut new = new_post(author, slug);
    new.kind = kind.into();
    new.status = status;
    store.read().content().insert(&new).await.unwrap().id
}

fn query(q: &str, kinds: &[&str], statuses: &[ContentStatus], limit: u32) -> SearchQuery {
    SearchQuery {
        q: q.into(),
        kinds: kinds.iter().map(|k| (*k).into()).collect(),
        statuses: statuses.to_vec(),
        readable_by: None,
        limit,
        cursor: None,
    }
}

/// Walks every page of `q` and returns the pages' ids.
async fn pages(search: &dyn SearchRepo, mut q: SearchQuery) -> Vec<Vec<ContentId>> {
    let mut out = Vec::new();
    loop {
        let page = search.search(&q).await.unwrap();
        assert!(!page.items.is_empty() || page.next_cursor.is_none());
        out.push(page.items);
        match page.next_cursor {
            Some(cursor) => q.cursor = Some(cursor),
            None => return out,
        }
        assert!(out.len() < 50, "cursor does not advance");
    }
}

pub async fn search_filters_status_kind_and_pages(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let search = store.read().search();
    let mut published = Vec::new();
    // Seven published posts with different document lengths, so ranks differ and tie.
    for i in 0..7 {
        let id = item(
            store,
            &author,
            &format!("pub-{i}"),
            "post",
            ContentStatus::Published,
        )
        .await;
        let body = "filler ".repeat(i % 4 * 5);
        search.index(id, "needle", &body).await.unwrap();
        published.push(id);
    }
    let draft = item(store, &author, "draft", "post", ContentStatus::Draft).await;
    let trash = item(store, &author, "trash", "post", ContentStatus::Trash).await;
    let page_item = item(store, &author, "a-page", "page", ContentStatus::Published).await;
    let private_page = item(store, &author, "p-page", "page", ContentStatus::Private).await;
    for id in [draft, trash, page_item, private_page] {
        search.index(id, "needle", "").await.unwrap();
    }
    // Not matching at all.
    let other = item(store, &author, "other", "post", ContentStatus::Published).await;
    search.index(other, "unrelated", "").await.unwrap();

    let sorted = |mut v: Vec<ContentId>| {
        v.sort();
        v
    };
    let flat = |pages: Vec<Vec<ContentId>>| pages.into_iter().flatten().collect::<Vec<_>>();

    // Published posts, 3 per page: 3 + 3 + 1, no duplicate, none omitted, and the page
    // walk equals the one-page result.
    let q = query("needle", &["post"], &[ContentStatus::Published], 3);
    let walked = pages(search, q.clone()).await;
    assert_eq!(
        walked.iter().map(Vec::len).collect::<Vec<_>>(),
        [3, 3, 1],
        "{walked:?}"
    );
    let all = flat(walked);
    assert_eq!(sorted(all.clone()), sorted(published.clone()));
    let whole = search
        .search(&SearchQuery { limit: 200, ..q })
        .await
        .unwrap();
    assert_eq!(whole.items, all);
    assert!(whole.next_cursor.is_none());

    // A page of exactly `limit` hits has no next cursor.
    let q7 = query("needle", &["post"], &[ContentStatus::Published], 7);
    let page = search.search(&q7).await.unwrap();
    assert_eq!((page.items.len(), page.next_cursor), (7, None));

    // Kind and status filters combine; empty means all.
    let set = |kinds: &[&str], statuses: &[ContentStatus]| {
        let q = query("needle", kinds, statuses, 200);
        async move { sorted(search.search(&q).await.unwrap().items) }
    };
    let mut expect = published.clone();
    expect.push(page_item);
    assert_eq!(
        set(&[], &[ContentStatus::Published]).await,
        sorted(expect.clone())
    );
    assert_eq!(
        set(&["page"], &[]).await,
        sorted(vec![page_item, private_page])
    );
    assert_eq!(
        set(
            &["page"],
            &[ContentStatus::Published, ContentStatus::Private]
        )
        .await,
        sorted(vec![page_item, private_page])
    );
    assert_eq!(
        set(
            &["post", "page"],
            &[ContentStatus::Draft, ContentStatus::Trash]
        )
        .await,
        sorted(vec![draft, trash])
    );
    assert_eq!(set(&["nope"], &[]).await, Vec::<ContentId>::new());
    let mut everything = expect;
    everything.extend([draft, trash, private_page]);
    assert_eq!(set(&[], &[]).await, sorted(everything));

    // No terms: an empty page without a statement.
    let empty = search.search(&query("  -- ", &[], &[], 10)).await.unwrap();
    assert!(empty.items.is_empty() && empty.next_cursor.is_none());

    // A cursor in any other shape is Invalid.
    for bad in ["garbage!", "aGVsbG8"] {
        let mut q = query("needle", &[], &[], 10);
        q.cursor = Some(bad.into());
        assert!(matches!(
            search.search(&q).await,
            Err(StoreError::Invalid(_))
        ));
    }
    let mut q = query("needle", &[], &[], 10);
    q.cursor = Some(ferrite_store::encode_text_cursor(
        "c:title",
        "x",
        uuid::Uuid::now_v7(),
    ));
    assert!(matches!(
        search.search(&q).await,
        Err(StoreError::Invalid(_))
    ));
    // Limits.
    for limit in [0, MAX_PAGE + 1] {
        assert!(matches!(
            search.search(&query("needle", &[], &[], limit)).await,
            Err(StoreError::Invalid(_))
        ));
    }
}

/// Equal documents have equal ranks, so they come out by id descending, one per page too.
pub async fn search_ties_break_by_id_descending(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let search = store.read().search();
    let mut ids = Vec::new();
    for i in 0..5 {
        let id = item(
            store,
            &author,
            &format!("tie-{i}"),
            "post",
            ContentStatus::Published,
        )
        .await;
        search.index(id, "same title", "same body").await.unwrap();
        ids.push(id);
    }
    ids.sort_by(|a, b| b.cmp(a));
    let walked = pages(search, query("same", &[], &[], 1)).await;
    assert_eq!(walked.concat(), ids);
    assert_eq!(walked.len(), 5);
}

/// Cursors carry the kind of listing: a content cursor is `Invalid` for search and a
/// search cursor is `Invalid` for every content order.
pub async fn cursor_kinds_are_not_interchangeable(db: &TestDb) {
    use ferrite_domain::{ContentOrder, ContentQuery};

    let store = db.store();
    let repos = store.read();
    let id = uuid::Uuid::now_v7();

    let mut q = query("needle", &[], &[], 10);
    q.cursor = Some(ferrite_store::encode_text_cursor("c:title", "x", id));
    assert!(matches!(
        repos.search().search(&q).await,
        Err(StoreError::Invalid(_))
    ));

    let search_cursor = ferrite_store::encode_rank_cursor(1.5, id);
    for order in [
        ContentOrder::PublishedDesc,
        ContentOrder::UpdatedDesc,
        ContentOrder::MenuOrderAsc,
        ContentOrder::TitleAsc,
    ] {
        let r = repos
            .content()
            .list(&ContentQuery {
                order,
                cursor: Some(search_cursor.clone()),
                ..ContentQuery::default()
            })
            .await;
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{order:?}: {r:?}");
    }
}

/// `readable_by` in search: published items of anyone plus the user's own items.
pub async fn search_readable_by(db: &TestDb) {
    let store = db.store();
    let me = fixture_user(store).await;
    let other = fixture_user(store).await;
    let mut ids = Vec::new();
    for (author, slug, status) in [
        (&me, "wombat-mine", ContentStatus::Draft),
        (&other, "wombat-live", ContentStatus::Published),
        (&other, "wombat-draft", ContentStatus::Draft),
        (&other, "wombat-private", ContentStatus::Private),
    ] {
        let mut n = new_post(author, slug);
        n.status = status;
        let item = store.read().content().insert(&n).await.unwrap();
        store
            .read()
            .search()
            .index(item.id, "wombat", slug)
            .await
            .unwrap();
        ids.push(item.id);
    }
    let query = |readable_by| SearchQuery {
        q: "wombat".into(),
        kinds: Vec::new(),
        statuses: Vec::new(),
        readable_by,
        limit: 50,
        cursor: None,
    };
    let mut got = store
        .read()
        .search()
        .search(&query(Some(me.id)))
        .await
        .unwrap()
        .items;
    got.sort();
    let mut want = vec![ids[0], ids[1]];
    want.sort();
    assert_eq!(got, want);
    let all = store
        .read()
        .search()
        .search(&query(None))
        .await
        .unwrap()
        .items;
    assert_eq!(all.len(), 4);
    // Paging stays exact under the filter.
    let mut q = query(Some(me.id));
    q.limit = 1;
    let first = store.read().search().search(&q).await.unwrap();
    assert_eq!(first.items.len(), 1);
    q.cursor = first.next_cursor;
    let second = store.read().search().search(&q).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    assert_ne!(first.items, second.items);
}
