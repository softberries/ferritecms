//! Content type, content item and meta contract tests.

use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentQuery, ContentStatus, ContentType, ParentFilter,
    Revision, RevisionId, Timestamp, User,
};
use ferrite_store::{Store, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

/// (slug, kind, status, author, published_at micros, menu_order, title)
type Spec<'a> = (
    &'a str,
    &'a str,
    ContentStatus,
    &'a User,
    Option<i64>,
    i32,
    &'a str,
);

fn micros(m: i64) -> Timestamp {
    Timestamp::from_micros(m).unwrap()
}

async fn insert(store: &dyn Store, new: &ferrite_domain::NewContent) -> ContentItem {
    store.read().content().insert(new).await.unwrap()
}

/// Collects every page of `query` (limit set by the caller), returning slugs per page.
async fn pages(store: &dyn Store, mut query: ContentQuery) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    loop {
        let page = store.read().content().list(&query).await.unwrap();
        out.push(page.items.iter().map(|i| i.slug.clone()).collect());
        match page.next_cursor {
            Some(c) => query.cursor = Some(c),
            None => return out,
        }
        assert!(out.len() < 50, "pagination does not terminate");
    }
}

pub async fn types_seeded_and_upsert(db: &TestDb) {
    let types = db.store().read().types();
    let all = types.list().await.unwrap();
    let names: Vec<_> = all.iter().map(|t| t.name.as_str()).collect();
    for built_in in ["post", "page", "media"] {
        assert!(names.contains(&built_in), "{built_in} missing: {names:?}");
        let t = types.get(built_in).await.unwrap().unwrap();
        assert!(t.built_in);
        assert!(matches!(
            types.delete(built_in).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert!(types.get("page").await.unwrap().unwrap().hierarchical);
    assert!(types.get("nope").await.unwrap().is_none());

    let mut custom = ContentType {
        name: "event".into(),
        label: "Events".into(),
        hierarchical: false,
        has_archive: true,
        fields: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        built_in: false,
        created_at: micros(1_700_000_000_000_000),
    };
    types.upsert(&custom).await.unwrap();
    assert_eq!(types.get("event").await.unwrap().unwrap(), custom);

    custom.label = "Happenings".into();
    custom.hierarchical = true;
    custom.fields = json!({"x": 1});
    types.upsert(&custom).await.unwrap();
    assert_eq!(types.get("event").await.unwrap().unwrap(), custom);
    assert_eq!(types.list().await.unwrap().len(), all.len() + 1);

    assert!(matches!(
        types
            .upsert(&ContentType {
                fields: json!({"a\u{0}": 1}),
                ..custom.clone()
            })
            .await,
        Err(StoreError::Invalid(_))
    ));

    types.delete("event").await.unwrap();
    assert!(types.get("event").await.unwrap().is_none());
    types.delete("event").await.unwrap();
}

pub async fn content_insert_get_roundtrip(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut new = new_post(&author, "hello-ünïcode");
    new.title = "Zażółć gęślą jaźń ☃ 日本語".into();
    new.excerpt = "short".into();
    new.body = json!([
        {"type": "group", "children": [
            {"type": "paragraph", "text": "nested", "marks": [1, 2.5, null, true]},
            {"type": "list", "items": [{"type": "item", "children": []}]}
        ]}
    ]);
    new.body_html = Some("<p>nested</p>".into());
    new.fields = json!({"city": "Kraków", "n": 3});
    new.menu_order = -7;
    new.scheduled_at = Some(micros(1_800_000_000_123_456));
    assert!(new.published_at.is_none());

    let before = Timestamp::now();
    let inserted = insert(store, &new).await;
    assert_eq!(inserted.version, 1);
    assert!(inserted.created_at >= before);
    assert_eq!(inserted.created_at, inserted.updated_at);

    let got = store.read().content().get(new.id).await.unwrap().unwrap();
    assert_eq!(got, inserted);
    assert_eq!(got.id, new.id);
    assert_eq!(got.kind, new.kind);
    assert_eq!(got.status, new.status);
    assert_eq!(got.slug, new.slug);
    assert_eq!(got.parent_id, None);
    assert_eq!(got.author_id, author.id);
    assert_eq!(got.title, new.title);
    assert_eq!(got.excerpt, new.excerpt);
    assert_eq!(got.body, new.body);
    assert_eq!(got.body_html, new.body_html);
    assert_eq!(got.fields, new.fields);
    assert_eq!(got.menu_order, -7);
    assert_eq!(got.published_at, None);
    assert_eq!(got.scheduled_at, new.scheduled_at);

    assert!(
        store
            .read()
            .content()
            .get(ContentId::new())
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn slug_unique_per_type_and_parent(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let first = insert(store, &new_post(&author, "same")).await;

    // Same slug, same type, both root: rejected (Review Focus 3).
    let err = store
        .read()
        .content()
        .insert(&new_post(&author, "same"))
        .await
        .unwrap_err();
    match err {
        StoreError::UniqueViolation { constraint } => {
            assert_eq!(constraint, "content_items_slug_uq");
        }
        other => panic!("expected UniqueViolation, got {other:?}"),
    }

    // Same slug, different type: fine.
    let mut page = new_post(&author, "same");
    page.kind = "page".into();
    let page = insert(store, &page).await;

    // Same slug under different parents: fine; under the same parent: rejected.
    let mut a = new_post(&author, "child");
    a.kind = "page".into();
    a.parent_id = Some(page.id);
    insert(store, &a).await;
    let mut b = new_post(&author, "child");
    b.kind = "page".into();
    b.parent_id = Some(first.id);
    insert(store, &b).await;
    let mut c = a.clone();
    c.id = ContentId::new();
    assert!(matches!(
        store.read().content().insert(&c).await,
        Err(StoreError::UniqueViolation { .. })
    ));

    // Updating into an occupied slug is a unique violation too.
    let other = insert(store, &new_post(&author, "other")).await;
    let mut moved = other.clone();
    moved.slug = "same".into();
    assert!(matches!(
        store.read().content().update(&moved).await,
        Err(StoreError::UniqueViolation { .. })
    ));
}

pub async fn unknown_references_are_foreign_keys(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut bad_type = new_post(&author, "x");
    bad_type.kind = "nonexistent".into();
    assert!(matches!(
        store.read().content().insert(&bad_type).await,
        Err(StoreError::ForeignKey { .. })
    ));
    let mut bad_parent = new_post(&author, "y");
    bad_parent.parent_id = Some(ContentId::new());
    assert!(matches!(
        store.read().content().insert(&bad_parent).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert!(matches!(
        store
            .read()
            .meta()
            .set(ContentId::new(), "k", &json!(1))
            .await,
        Err(StoreError::ForeignKey { .. })
    ));
}

pub async fn get_by_slug_root_and_child(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut root = new_post(&author, "about");
    root.kind = "page".into();
    let root = insert(store, &root).await;
    let mut child = new_post(&author, "team");
    child.kind = "page".into();
    child.parent_id = Some(root.id);
    let child = insert(store, &child).await;
    let content = store.read().content();

    let got = content
        .get_by_slug("page", None, "about")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, root.id);
    let got = content
        .get_by_slug("page", Some(root.id), "team")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, child.id);
    assert!(
        content
            .get_by_slug("page", None, "team")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        content
            .get_by_slug("page", Some(root.id), "about")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        content
            .get_by_slug("post", None, "about")
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn optimistic_lock_race(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = insert(store, &new_post(&author, "race")).await;
    let content = store.read().content();
    let mut a = content.get(item.id).await.unwrap().unwrap();
    let mut b = content.get(item.id).await.unwrap().unwrap();
    a.title = "from a".into();
    b.title = "from b".into();

    let (ra, rb) = tokio::join!(content.update(&a), content.update(&b));
    let results = [ra, rb];
    let oks: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert_eq!(oks.len(), 1, "{results:?}");
    assert_eq!(oks[0].version, 2);
    assert!(oks[0].updated_at >= item.updated_at);
    let losers: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(
        matches!(losers[0], StoreError::Conflict { current_version: 2 }),
        "{losers:?}"
    );

    // The stale writer retrying with its old copy still conflicts at version 2.
    let losing_copy = if results[0].is_err() { &a } else { &b };
    assert!(matches!(
        content.update(losing_copy).await,
        Err(StoreError::Conflict { current_version: 2 })
    ));
    // The winner's copy advances.
    let stored = content.get(item.id).await.unwrap().unwrap();
    assert_eq!(stored.version, 2);
    assert_eq!(stored.title, oks[0].title);
    assert_eq!(stored.created_at, item.created_at);
    let next = content.update(&stored).await.unwrap();
    assert_eq!(next.version, 3);
}

pub async fn update_missing_is_not_found(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut item = insert(store, &new_post(&author, "gone")).await;
    store.read().content().delete(item.id).await.unwrap();
    item.title = "x".into();
    assert!(matches!(
        store.read().content().update(&item).await,
        Err(StoreError::NotFound)
    ));
}

pub async fn list_filters_and_orders(db: &TestDb) {
    let store = db.store();
    let a1 = fixture_user(store).await;
    let a2 = fixture_user(store).await;

    // (slug, kind, status, author, parent, menu_order, title, published_at micros)
    let mut specs: Vec<Spec> = vec![
        (
            "p1",
            "post",
            ContentStatus::Published,
            &a1,
            Some(1_000),
            3,
            "Banana",
        ),
        (
            "p2",
            "post",
            ContentStatus::Published,
            &a2,
            Some(3_000),
            1,
            "apple",
        ),
        ("p3", "post", ContentStatus::Draft, &a1, None, 2, "Cherry"),
        (
            "p4",
            "post",
            ContentStatus::Published,
            &a1,
            Some(2_000),
            1,
            "Äpfel",
        ),
        (
            "g1",
            "page",
            ContentStatus::Published,
            &a1,
            Some(500),
            0,
            "About",
        ),
    ];
    specs.sort_by_key(|s| s.0);
    let mut ids = std::collections::HashMap::new();
    for (slug, kind, status, author, published, menu_order, title) in &specs {
        let mut n = new_post(author, slug);
        n.kind = (*kind).into();
        n.status = *status;
        n.menu_order = *menu_order;
        n.title = (*title).into();
        n.published_at = published.map(micros);
        ids.insert(*slug, insert(store, &n).await.id);
    }
    // A child of g1.
    let mut child = new_post(&a2, "g1-child");
    child.kind = "page".into();
    child.parent_id = Some(ids["g1"]);
    child.status = ContentStatus::Draft;
    child.title = "Child".into();
    insert(store, &child).await;

    let list = |q: ContentQuery| async move {
        store
            .read()
            .content()
            .list(&q)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.slug)
            .collect::<Vec<_>>()
    };
    let base = ContentQuery {
        limit: 50,
        ..Default::default()
    };

    // kind + status filters.
    let q = |f: &dyn Fn(&mut ContentQuery)| {
        let mut q = base.clone();
        f(&mut q);
        q
    };
    assert_eq!(list(q(&|q| q.kind = Some("page".into()))).await.len(), 2);
    let published_posts = list(q(&|q| {
        q.kind = Some("post".into());
        q.statuses = vec![ContentStatus::Published];
    }))
    .await;
    assert_eq!(published_posts, ["p2", "p4", "p1"]); // PublishedDesc
    let multi = list(q(&|q| {
        q.kind = Some("post".into());
        q.statuses = vec![ContentStatus::Draft, ContentStatus::Published];
    }))
    .await;
    assert_eq!(multi, ["p2", "p4", "p1", "p3"]); // NULL published_at last
    assert!(
        list(q(&|q| q.statuses = vec![ContentStatus::Trash]))
            .await
            .is_empty()
    );

    // Parent filters.
    let roots = list(q(&|q| {
        q.kind = Some("page".into());
        q.parent = ParentFilter::Root;
    }))
    .await;
    assert_eq!(roots, ["g1"]);
    let of = list(q(&|q| q.parent = ParentFilter::Of(ids["g1"]))).await;
    assert_eq!(of, ["g1-child"]);

    // Author filter.
    let by_a2 = list(q(&|q| q.author = Some(a2.id))).await;
    assert_eq!(by_a2.len(), 2);
    assert!(by_a2.contains(&"p2".to_string()) && by_a2.contains(&"g1-child".to_string()));

    // Orders (posts only).
    let posts = |order| {
        q(&move |q| {
            q.kind = Some("post".into());
            q.order = order;
        })
    };
    assert_eq!(
        list(posts(ContentOrder::MenuOrderAsc)).await,
        // menu_order asc, id asc (UUIDv7 insertion order p1<p2<p3<p4 within the same value)
        ["p2", "p4", "p3", "p1"]
    );
    // BINARY title ordering: uppercase before lowercase, non-ASCII last.
    assert_eq!(
        list(posts(ContentOrder::TitleAsc)).await,
        ["p1", "p3", "p2", "p4"]
    );
    let updated = list(posts(ContentOrder::UpdatedDesc)).await;
    assert_eq!(updated.len(), 4);
    assert_eq!(updated[0], "p4"); // most recently inserted

    // Invalid limits and cursors.
    for limit in [0, 201] {
        let r = store
            .read()
            .content()
            .list(&ContentQuery {
                limit,
                ..Default::default()
            })
            .await;
        assert!(matches!(r, Err(StoreError::Invalid(_))), "limit {limit}");
    }
    let r = store
        .read()
        .content()
        .list(&ContentQuery {
            cursor: Some("%%%".into()),
            ..Default::default()
        })
        .await;
    assert!(matches!(r, Err(StoreError::Invalid(_))));
    let r = store
        .read()
        .content()
        .list(&ContentQuery {
            limit: 200,
            ..Default::default()
        })
        .await;
    assert!(r.is_ok());
}

/// 7 items, limit 3: every order yields three pages with no duplicate or omission.
pub async fn list_pagination_covers_every_order(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    // Duplicate sort keys (menu_order, published_at) exercise the id tie-break.
    let keys = [
        (5, 100),
        (5, 100),
        (1, 300),
        (1, 300),
        (3, 200),
        (9, 100),
        (0, 400),
    ];
    let mut expected = Vec::new();
    for (i, (menu_order, published)) in keys.iter().enumerate() {
        let slug = format!("s{i}");
        let mut n = new_post(&author, &slug);
        n.menu_order = *menu_order;
        n.published_at = Some(micros(*published));
        n.title = format!("t{}", i % 3);
        insert(store, &n).await;
        expected.push(slug);
    }
    for order in [
        ContentOrder::PublishedDesc,
        ContentOrder::UpdatedDesc,
        ContentOrder::MenuOrderAsc,
        ContentOrder::TitleAsc,
    ] {
        let got = pages(
            store,
            ContentQuery {
                limit: 3,
                order,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            got.iter().map(Vec::len).collect::<Vec<_>>(),
            [3, 3, 1],
            "{order:?}"
        );
        let mut flat: Vec<_> = got.into_iter().flatten().collect();
        flat.sort();
        assert_eq!(flat, expected, "{order:?}");
    }
    // Page boundaries must agree with the unpaginated order.
    let all = pages(
        store,
        ContentQuery {
            limit: 50,
            order: ContentOrder::MenuOrderAsc,
            ..Default::default()
        },
    )
    .await;
    let paged: Vec<_> = pages(
        store,
        ContentQuery {
            limit: 2,
            order: ContentOrder::MenuOrderAsc,
            ..Default::default()
        },
    )
    .await
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(all.concat(), paged);
}

/// Items without `published_at` sort last and must be neither lost nor duplicated across
/// page boundaries, including a boundary that falls inside the NULL group.
pub async fn list_pagination_with_null_published(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    for i in 0..3 {
        let mut n = new_post(&author, &format!("dated{i}"));
        n.published_at = Some(micros(1_000 + i));
        insert(store, &n).await;
    }
    for i in 0..4 {
        insert(store, &new_post(&author, &format!("undated{i}"))).await;
    }
    for limit in [1, 2, 3, 4, 5, 7] {
        let got: Vec<_> = pages(
            store,
            ContentQuery {
                limit,
                order: ContentOrder::PublishedDesc,
                ..Default::default()
            },
        )
        .await
        .into_iter()
        .flatten()
        .collect();
        assert_eq!(got.len(), 7, "limit {limit}: {got:?}");
        let mut uniq = got.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 7, "duplicates with limit {limit}: {got:?}");
        assert_eq!(&got[..3], ["dated2", "dated1", "dated0"], "limit {limit}");
        // UUIDv7 ids ascend with insertion, so id DESC reverses it inside the NULL group.
        assert_eq!(
            &got[3..],
            ["undated3", "undated2", "undated1", "undated0"],
            "limit {limit}"
        );
    }
}

pub async fn due_scheduled_returns_only_due(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mk = |slug: &str, status, at: Option<i64>| {
        let mut n = new_post(&author, slug);
        n.status = status;
        n.scheduled_at = at.map(micros);
        n
    };
    let due_old = insert(store, &mk("due-old", ContentStatus::Scheduled, Some(1_000))).await;
    let due_new = insert(store, &mk("due-new", ContentStatus::Scheduled, Some(2_000))).await;
    insert(store, &mk("future", ContentStatus::Scheduled, Some(9_000))).await;
    insert(store, &mk("draft", ContentStatus::Draft, Some(500))).await;
    insert(store, &mk("published", ContentStatus::Published, Some(500))).await;
    insert(store, &mk("no-time", ContentStatus::Scheduled, None)).await;

    let content = store.read().content();
    let due = content.due_scheduled(micros(2_000), 10).await.unwrap();
    let ids: Vec<_> = due.iter().map(|i| i.id).collect();
    assert_eq!(ids, [due_old.id, due_new.id]);
    assert_eq!(
        content.due_scheduled(micros(2_000), 1).await.unwrap().len(),
        1
    );
    assert!(
        content
            .due_scheduled(micros(999), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        content.due_scheduled(micros(2_000), 0).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn meta_crud(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = insert(store, &new_post(&author, "with-meta")).await;
    let other = insert(store, &new_post(&author, "other-meta")).await;
    let meta = store.read().meta();

    assert!(meta.get(item.id, "k").await.unwrap().is_none());
    assert!(meta.list(item.id).await.unwrap().is_empty());

    let nested = json!({"a": [1, 2.5, null, "ü"], "b": {"c": true}});
    meta.set(item.id, "seo", &nested).await.unwrap();
    meta.set(item.id, "b-key", &json!(1)).await.unwrap();
    meta.set(item.id, "A-key", &json!("x")).await.unwrap();
    meta.set(other.id, "seo", &json!("other")).await.unwrap();
    assert_eq!(meta.get(item.id, "seo").await.unwrap().unwrap(), nested);

    // Replace.
    meta.set(item.id, "b-key", &json!(2)).await.unwrap();
    assert_eq!(meta.get(item.id, "b-key").await.unwrap(), Some(json!(2)));

    // Listed by key in binary order, scoped to the item.
    let listed = meta.list(item.id).await.unwrap();
    assert_eq!(
        listed,
        [
            ("A-key".to_string(), json!("x")),
            ("b-key".to_string(), json!(2)),
            ("seo".to_string(), nested),
        ]
    );

    meta.delete(item.id, "seo").await.unwrap();
    meta.delete(item.id, "seo").await.unwrap();
    assert!(meta.get(item.id, "seo").await.unwrap().is_none());
    assert_eq!(meta.list(item.id).await.unwrap().len(), 2);
    assert_eq!(
        meta.get(other.id, "seo").await.unwrap(),
        Some(json!("other"))
    );

    assert!(matches!(
        meta.set(item.id, "bad", &json!({"k": "a\u{0}b"})).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn delete_cascades_meta(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = insert(store, &new_post(&author, "cascade")).await;
    store
        .read()
        .meta()
        .set(item.id, "k", &json!(1))
        .await
        .unwrap();

    store.read().content().delete(item.id).await.unwrap();
    assert!(store.read().content().get(item.id).await.unwrap().is_none());
    assert!(store.read().meta().list(item.id).await.unwrap().is_empty());
    // Deleting again is a no-op.
    store.read().content().delete(item.id).await.unwrap();
}

/// R9: deleting an item that still has children is rejected (RESTRICT), not cascaded.
pub async fn delete_with_children_is_foreign_key(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut parent = new_post(&author, "parent");
    parent.kind = "page".into();
    let parent = insert(store, &parent).await;
    let mut child = new_post(&author, "kid");
    child.kind = "page".into();
    child.parent_id = Some(parent.id);
    let child = insert(store, &child).await;

    assert!(matches!(
        store.read().content().delete(parent.id).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert!(
        store
            .read()
            .content()
            .get(parent.id)
            .await
            .unwrap()
            .is_some()
    );

    store.read().content().delete(child.id).await.unwrap();
    store.read().content().delete(parent.id).await.unwrap();
}

pub async fn content_rejects_nul_json(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut bad = new_post(&author, "nul-body");
    bad.body = json!([{"text": "a\u{0}"}]);
    assert!(matches!(
        store.read().content().insert(&bad).await,
        Err(StoreError::Invalid(_))
    ));
    let mut bad = new_post(&author, "nul-fields");
    bad.fields = json!({"k\u{0}": 1});
    assert!(matches!(
        store.read().content().insert(&bad).await,
        Err(StoreError::Invalid(_))
    ));
    let mut ok = insert(store, &new_post(&author, "fine")).await;
    ok.body = json!("a\u{0}");
    assert!(matches!(
        store.read().content().update(&ok).await,
        Err(StoreError::Invalid(_))
    ));
}

/// TitleAsc is byte order (`COLLATE "C"` on Postgres): uppercase before lowercase, then
/// non-ASCII. Pages of 2 must reproduce the full unpaginated order on both backends.
pub async fn list_pagination_title_asc_mixed_case_unicode(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let titles = ["Banana", "apple", "Äpfel", "Cherry", "banana"];
    for (i, title) in titles.iter().enumerate() {
        let mut n = new_post(&author, &format!("t{i}"));
        n.title = (*title).into();
        insert(store, &n).await;
    }
    let mut query = ContentQuery {
        limit: 2,
        order: ContentOrder::TitleAsc,
        ..Default::default()
    };
    let mut got = Vec::new();
    let mut page_sizes = Vec::new();
    loop {
        let page = store.read().content().list(&query).await.unwrap();
        page_sizes.push(page.items.len());
        got.extend(page.items.into_iter().map(|i| i.title));
        match page.next_cursor {
            Some(c) => query.cursor = Some(c),
            None => break,
        }
        assert!(page_sizes.len() < 10, "pagination does not terminate");
    }
    assert_eq!(page_sizes, [2, 2, 1]);
    assert_eq!(got, ["Banana", "Cherry", "apple", "banana", "Äpfel"]);
}

fn revision(item: &ContentItem, created_micros: i64, title: &str) -> Revision {
    Revision {
        id: RevisionId::new(),
        item_id: item.id,
        author_id: item.author_id,
        created_at: micros(created_micros),
        title: title.into(),
        excerpt: "ex".into(),
        body: json!([{"type": "paragraph", "text": title}]),
        fields: json!({"n": 1}),
        meta: json!({"k": ["v", 2.5, null]}),
        note: None,
    }
}

pub async fn revisions_newest_first_and_prune(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = insert(store, &new_post(&author, "revisioned")).await;
    let other = insert(store, &new_post(&author, "other")).await;
    let revs = store.read().revisions();

    let mut all = Vec::new();
    for i in 0..60_i64 {
        let mut r = revision(&item, 1_000 + i, &format!("rev {i}"));
        r.note = (i == 59).then(|| "last".to_owned());
        revs.insert(&r).await.unwrap();
        all.push(r);
    }
    revs.insert(&revision(&other, 5_000, "unrelated"))
        .await
        .unwrap();

    let top = revs.list(item.id, 10).await.unwrap();
    assert_eq!(top.len(), 10);
    assert_eq!(top, all.iter().rev().take(10).cloned().collect::<Vec<_>>());
    assert_eq!(revs.get(all[59].id).await.unwrap().unwrap(), all[59]);
    assert!(revs.get(RevisionId::new()).await.unwrap().is_none());

    // Equal created_at: id DESC breaks the tie (UUIDv7 ids are increasing).
    let tie_a = revision(&other, 6_000, "tie a");
    let tie_b = revision(&other, 6_000, "tie b");
    revs.insert(&tie_a).await.unwrap();
    revs.insert(&tie_b).await.unwrap();
    let listed = revs.list(other.id, 2).await.unwrap();
    assert_eq!(listed[0].id, tie_b.id);
    assert_eq!(listed[1].id, tie_a.id);

    assert_eq!(revs.prune(item.id, 50).await.unwrap(), 10);
    let left = revs.list(item.id, 200).await.unwrap();
    assert_eq!(left.len(), 50);
    assert_eq!(left[0], all[59]);
    assert_eq!(left[49], all[10]);
    assert!(revs.get(all[9].id).await.unwrap().is_none());
    assert_eq!(revs.prune(item.id, 50).await.unwrap(), 0);
    assert_eq!(revs.list(other.id, 200).await.unwrap().len(), 3);

    // Revisions go with their item.
    store.read().content().delete(item.id).await.unwrap();
    assert!(revs.list(item.id, 10).await.unwrap().is_empty());

    let mut bad = revision(&other, 7_000, "nul");
    bad.meta = json!({"k": "a\u{0}"});
    assert!(matches!(
        revs.insert(&bad).await,
        Err(StoreError::Invalid(_))
    ));
    let mut bad = revision(&other, 7_000, "nul");
    bad.body = json!("a\u{0}");
    assert!(matches!(
        revs.insert(&bad).await,
        Err(StoreError::Invalid(_))
    ));
}
