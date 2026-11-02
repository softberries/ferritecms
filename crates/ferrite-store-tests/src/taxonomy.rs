//! Taxonomy, term and term-link contract tests.

use ferrite_domain::{ContentQuery, ContentType, Taxonomy, Term, TermId, Timestamp};
use ferrite_store::{Store, StoreError};

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn term(tax: &str, slug: &str, name: &str, parent: Option<TermId>) -> Term {
    Term {
        id: TermId::new(),
        taxonomy: tax.into(),
        slug: slug.into(),
        name: name.into(),
        description: format!("about {slug}"),
        parent_id: parent,
        locale: "en".into(),
        translation_group: TermId::new(),
    }
}

async fn insert_terms(store: &dyn Store, terms: &[&Term]) {
    for t in terms {
        store.read().terms().insert(t).await.unwrap();
    }
}

pub async fn taxonomies_seeded_and_upsert(db: &TestDb) {
    let taxes = db.store().read().taxonomies();
    let all = taxes.list().await.unwrap();
    let names: Vec<_> = all.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["category", "tag"]);
    let category = taxes.get("category").await.unwrap().unwrap();
    assert!(category.hierarchical && category.built_in);
    assert_eq!(category.content_types, ["post"]);
    assert!(!taxes.get("tag").await.unwrap().unwrap().hierarchical);
    assert!(taxes.get("nope").await.unwrap().is_none());

    let mut genre = Taxonomy {
        name: "genre".into(),
        label: "Genres".into(),
        hierarchical: true,
        content_types: vec!["post".into(), "page".into()],
        built_in: false,
    };
    taxes.upsert(&genre).await.unwrap();
    assert_eq!(taxes.get("genre").await.unwrap().unwrap(), genre);

    genre.label = "Kinds".into();
    genre.hierarchical = false;
    genre.content_types = vec!["event".into()];
    taxes.upsert(&genre).await.unwrap();
    assert_eq!(taxes.get("genre").await.unwrap().unwrap(), genre);
    assert_eq!(taxes.list().await.unwrap().len(), 3);

    // built_in is fixed at first insert.
    let mut cat = category.clone();
    cat.label = "Topics".into();
    cat.built_in = false;
    taxes.upsert(&cat).await.unwrap();
    let after = taxes.get("category").await.unwrap().unwrap();
    assert_eq!(after.label, "Topics");
    assert!(after.built_in);

    genre.content_types = vec!["a\u{0}".into()];
    assert!(matches!(
        taxes.upsert(&genre).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn type_and_taxonomy_insert(db: &TestDb) {
    let store = db.store();
    let ty = ContentType {
        name: "event".into(),
        label: "Events".into(),
        hierarchical: true,
        has_archive: true,
        fields: serde_json::json!({"fields": []}),
        built_in: false,
        created_at: Timestamp::from_micros(1_700_000_000_000_000).unwrap(),
    };
    store.read().types().insert(&ty).await.unwrap();
    assert_eq!(
        store.read().types().get("event").await.unwrap().unwrap(),
        ty
    );
    // A taken name is a unique violation and the stored row is unchanged.
    let mut other = ty.clone();
    other.label = "Other".into();
    other.hierarchical = false;
    for name in ["event", "post"] {
        other.name = name.into();
        assert!(matches!(
            store.read().types().insert(&other).await,
            Err(StoreError::UniqueViolation { constraint }) if constraint == "content_types_pkey"
        ));
    }
    assert_eq!(
        store.read().types().get("event").await.unwrap().unwrap(),
        ty
    );
    let mut nul = ty.clone();
    nul.name = "a\u{0}".into();
    assert!(matches!(
        store.read().types().insert(&nul).await,
        Err(StoreError::Invalid(_))
    ));

    let genre = Taxonomy {
        name: "genre".into(),
        label: "Genres".into(),
        hierarchical: false,
        content_types: vec!["event".into()],
        built_in: false,
    };
    store.read().taxonomies().insert(&genre).await.unwrap();
    assert_eq!(
        store
            .read()
            .taxonomies()
            .get("genre")
            .await
            .unwrap()
            .unwrap(),
        genre
    );
    let mut dup = genre.clone();
    dup.label = "Kinds".into();
    for name in ["genre", "category"] {
        dup.name = name.into();
        assert!(matches!(
            store.read().taxonomies().insert(&dup).await,
            Err(StoreError::UniqueViolation { constraint }) if constraint == "taxonomies_pkey"
        ));
    }
    assert_eq!(
        store
            .read()
            .taxonomies()
            .get("genre")
            .await
            .unwrap()
            .unwrap(),
        genre
    );
    // In a transaction the violation poisons it (R18); the store stays usable.
    let tx = store.begin().await.unwrap();
    assert!(tx.taxonomies().insert(&genre).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(store.read().taxonomies().list().await.unwrap().len(), 3);
}

pub async fn taxonomy_delete(db: &TestDb) {
    let store = db.store();
    let taxes = store.read().taxonomies();
    let genre = Taxonomy {
        name: "genre".into(),
        label: "Genres".into(),
        hierarchical: false,
        content_types: vec!["post".into()],
        built_in: false,
    };
    taxes.upsert(&genre).await.unwrap();
    // Missing: a no-op.
    taxes.delete("nope").await.unwrap();
    // Built in: Invalid, the row stays.
    assert!(matches!(
        taxes.delete("category").await,
        Err(StoreError::Invalid(m)) if m.contains("built-in")
    ));
    assert!(taxes.get("category").await.unwrap().is_some());
    // With terms: ForeignKey, the row stays.
    let t = term("genre", "jazz", "Jazz", None);
    insert_terms(store, &[&t]).await;
    assert!(matches!(
        taxes.delete("genre").await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert!(taxes.get("genre").await.unwrap().is_some());
    // Without: gone.
    store.read().terms().delete(t.id).await.unwrap();
    taxes.delete("genre").await.unwrap();
    assert!(taxes.get("genre").await.unwrap().is_none());
    assert_eq!(taxes.list().await.unwrap().len(), 2);
    // Inside a transaction, rolled back: still there.
    taxes.upsert(&genre).await.unwrap();
    let tx = store.begin().await.unwrap();
    tx.taxonomies().delete("genre").await.unwrap();
    assert!(tx.taxonomies().get("genre").await.unwrap().is_none());
    tx.rollback().await.unwrap();
    assert!(taxes.get("genre").await.unwrap().is_some());
    // NUL in the name: Invalid.
    assert!(matches!(
        taxes.delete("a\u{0}").await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn terms_crud_and_hierarchy(db: &TestDb) {
    let store = db.store();
    let terms = store.read().terms();
    let parent = term("category", "news", "News", None);
    let child = term("category", "local", "Local", Some(parent.id));
    let other = term("category", "art", "art", None);
    insert_terms(store, &[&parent, &child, &other]).await;

    assert_eq!(terms.get(parent.id).await.unwrap().unwrap(), parent);
    assert_eq!(terms.get(child.id).await.unwrap().unwrap(), child);
    assert!(terms.get(TermId::new()).await.unwrap().is_none());
    assert_eq!(
        terms
            .get_by_slug("category", None, "local")
            .await
            .unwrap()
            .unwrap(),
        child
    );
    assert!(
        terms
            .get_by_slug("tag", None, "local")
            .await
            .unwrap()
            .is_none()
    );

    // Ordered by name, byte order: "Local" < "News" < "art".
    let listed = terms.list("category", None).await.unwrap();
    assert_eq!(listed, [child.clone(), parent.clone(), other.clone()]);
    assert!(terms.list("tag", None).await.unwrap().is_empty());

    // Slug is unique per taxonomy, but may repeat across taxonomies.
    let dup = term("category", "news", "Dup", None);
    assert!(matches!(
        terms.insert(&dup).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    let same_slug_tag = term("tag", "news", "News", None);
    terms.insert(&same_slug_tag).await.unwrap();
    assert!(matches!(
        terms
            .update(&Term {
                slug: "art".into(),
                ..child.clone()
            })
            .await,
        Err(StoreError::UniqueViolation { .. })
    ));

    // Unknown taxonomy or parent is a foreign-key error.
    assert!(matches!(
        terms.insert(&term("nope", "x", "X", None)).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert!(matches!(
        terms
            .insert(&term("category", "orphan", "Orphan", Some(TermId::new())))
            .await,
        Err(StoreError::ForeignKey { .. })
    ));

    let mut renamed = child.clone();
    renamed.name = "Zeta".into();
    renamed.description = "changed".into();
    renamed.parent_id = None;
    terms.update(&renamed).await.unwrap();
    assert_eq!(terms.get(child.id).await.unwrap().unwrap(), renamed);
    assert!(matches!(
        terms
            .update(&term("category", "ghost", "Ghost", None))
            .await,
        Err(StoreError::NotFound)
    ));
}

/// R9: a term that still has child terms cannot be deleted (RESTRICT).
pub async fn deleting_term_with_children_is_foreign_key(db: &TestDb) {
    let store = db.store();
    let terms = store.read().terms();
    let parent = term("category", "p", "P", None);
    let child = term("category", "c", "C", Some(parent.id));
    insert_terms(store, &[&parent, &child]).await;

    assert!(matches!(
        terms.delete(parent.id).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert!(terms.get(parent.id).await.unwrap().is_some());
    assert_eq!(
        terms.get(child.id).await.unwrap().unwrap().parent_id,
        Some(parent.id)
    );

    terms.delete(child.id).await.unwrap();
    terms.delete(parent.id).await.unwrap();
    assert!(terms.get(parent.id).await.unwrap().is_none());
    terms.delete(parent.id).await.unwrap();
}

pub async fn term_links_replace_and_order(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "tagged"))
        .await
        .unwrap();
    let a = term("tag", "a", "Alpha", None);
    let b = term("tag", "b", "Beta", None);
    let c = term("tag", "c", "Gamma", None);
    insert_terms(store, &[&a, &b, &c]).await;
    let terms = store.read().terms();

    assert!(terms.for_item(item.id).await.unwrap().is_empty());
    terms.set_for_item(item.id, &[b.id, a.id]).await.unwrap();
    assert_eq!(
        terms.for_item(item.id).await.unwrap(),
        [b.clone(), a.clone()]
    );
    terms.set_for_item(item.id, &[c.id]).await.unwrap();
    assert_eq!(
        terms.for_item(item.id).await.unwrap(),
        std::slice::from_ref(&c)
    );

    // A failed replacement (unknown term) leaves the previous links untouched.
    assert!(matches!(
        terms.set_for_item(item.id, &[a.id, TermId::new()]).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert_eq!(
        terms.for_item(item.id).await.unwrap(),
        std::slice::from_ref(&c)
    );

    // Inside a transaction the replacement is part of it.
    let tx = store.begin().await.unwrap();
    tx.terms()
        .set_for_item(item.id, &[a.id, c.id])
        .await
        .unwrap();
    assert_eq!(
        tx.terms().for_item(item.id).await.unwrap(),
        [a.clone(), c.clone()]
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        terms.for_item(item.id).await.unwrap(),
        std::slice::from_ref(&c)
    );
    let tx = store.begin().await.unwrap();
    tx.terms()
        .set_for_item(item.id, &[c.id, b.id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        terms.for_item(item.id).await.unwrap(),
        [c.clone(), b.clone()]
    );

    terms.set_for_item(item.id, &[]).await.unwrap();
    assert!(terms.for_item(item.id).await.unwrap().is_empty());
}

/// R11: a failing `set_for_item` inside a caller's `Tx` keeps the previous links and
/// leaves the outer transaction usable.
pub async fn set_for_item_failure_inside_tx_keeps_links_and_tx(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "tx-links"))
        .await
        .unwrap();
    let a = term("tag", "a", "A", None);
    let b = term("tag", "b", "B", None);
    insert_terms(store, &[&a, &b]).await;
    store
        .read()
        .terms()
        .set_for_item(item.id, &[a.id])
        .await
        .unwrap();

    let tx = store.begin().await.unwrap();
    tx.options()
        .set("before", &serde_json::json!(1), false)
        .await
        .unwrap();
    assert!(matches!(
        tx.terms()
            .set_for_item(item.id, &[b.id, TermId::new()])
            .await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert_eq!(
        tx.terms().for_item(item.id).await.unwrap(),
        std::slice::from_ref(&a)
    );
    tx.options()
        .set("after", &serde_json::json!(2), false)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        store.read().terms().for_item(item.id).await.unwrap(),
        std::slice::from_ref(&a)
    );
    let options = store.read().options();
    assert!(options.get("before").await.unwrap().is_some());
    assert!(options.get("after").await.unwrap().is_some());
}

pub async fn deleting_term_removes_links(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "linked"))
        .await
        .unwrap();
    let a = term("tag", "a", "A", None);
    let b = term("tag", "b", "B", None);
    insert_terms(store, &[&a, &b]).await;
    let terms = store.read().terms();
    terms.set_for_item(item.id, &[a.id, b.id]).await.unwrap();

    terms.delete(a.id).await.unwrap();
    assert_eq!(
        terms.for_item(item.id).await.unwrap(),
        std::slice::from_ref(&b)
    );

    // Deleting the item removes its links, not the terms.
    store.read().content().delete(item.id).await.unwrap();
    assert!(terms.get(b.id).await.unwrap().is_some());
    assert!(terms.for_item(item.id).await.unwrap().is_empty());
}

pub async fn content_list_by_term(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let a = term("tag", "a", "A", None);
    let b = term("tag", "b", "B", None);
    insert_terms(store, &[&a, &b]).await;
    let mut ids = Vec::new();
    for slug in ["p1", "p2", "p3"] {
        let item = store
            .read()
            .content()
            .insert(&new_post(&author, slug))
            .await
            .unwrap();
        ids.push(item.id);
    }
    store
        .read()
        .terms()
        .set_for_item(ids[0], &[a.id])
        .await
        .unwrap();
    store
        .read()
        .terms()
        .set_for_item(ids[1], &[a.id, b.id])
        .await
        .unwrap();

    let slugs = |term: TermId| {
        let store = db.store();
        async move {
            let page = store
                .read()
                .content()
                .list(&ContentQuery {
                    term: Some(term),
                    ..Default::default()
                })
                .await
                .unwrap();
            let mut slugs: Vec<_> = page.items.into_iter().map(|i| i.slug).collect();
            slugs.sort();
            slugs
        }
    };
    assert_eq!(slugs(a.id).await, ["p1", "p2"]);
    assert_eq!(slugs(b.id).await, ["p2"]);
    assert!(slugs(TermId::new()).await.is_empty());
}
