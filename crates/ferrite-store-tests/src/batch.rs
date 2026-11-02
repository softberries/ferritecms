//! Contract tests for the batch loaders: each equals the corresponding single gets.

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, Media, Term, TermId, Timestamp, UserId,
};
use ferrite_store::StoreError;
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn term(slug: &str) -> Term {
    Term {
        id: TermId::new(),
        taxonomy: "tag".into(),
        slug: slug.into(),
        name: format!("Name {slug}"),
        description: String::new(),
        parent_id: None,
        locale: "en".into(),
        translation_group: TermId::new(),
    }
}

pub async fn batch_loaders_match_single_gets(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let repos = store.read();
    let ids = |n: usize| (0..n).map(|_| ContentId::new()).collect::<Vec<_>>();

    // Items: a, b with terms, c without, plus media items.
    let mut items = Vec::new();
    for slug in ["a", "b", "c"] {
        items.push(
            repos
                .content()
                .insert(&new_post(&author, slug))
                .await
                .unwrap()
                .id,
        );
    }
    let (a, b, c) = (items[0], items[1], items[2]);
    let (t1, t2, t3) = (term("t1"), term("t2"), term("t3"));
    for t in [&t1, &t2, &t3] {
        repos.terms().insert(t).await.unwrap();
    }
    repos
        .terms()
        .set_for_item(a, &[t3.id, t1.id])
        .await
        .unwrap();
    repos
        .terms()
        .set_for_item(b, &[t2.id, t1.id, t3.id])
        .await
        .unwrap();

    // Terms: per-item position order, items without terms (and unknown ones) absent.
    let got = repos
        .terms()
        .for_items(&[a, b, c, ContentId::new(), a])
        .await
        .unwrap();
    assert_eq!(got.len(), 2);
    for id in [a, b] {
        assert_eq!(got[&id], repos.terms().for_item(id).await.unwrap());
    }
    assert_eq!(got[&a], [t3.clone(), t1.clone()]);
    assert_eq!(got[&b], [t2.clone(), t1.clone(), t3.clone()]);
    assert!(!got.contains_key(&c));
    assert!(repos.terms().for_items(&[]).await.unwrap().is_empty());
    assert!(repos.terms().for_items(&ids(200)).await.unwrap().is_empty());
    assert!(matches!(
        repos.terms().for_items(&ids(201)).await,
        Err(StoreError::Invalid(_))
    ));

    // Users: input order, missing skipped, duplicates once.
    let u1 = fixture_user(store).await;
    let u2 = fixture_user(store).await;
    let users = repos.users();
    let got = users
        .get_many(&[u2.id, UserId::new(), author.id, u2.id, u1.id])
        .await
        .unwrap();
    assert_eq!(got, [u2.clone(), author.clone(), u1.clone()]);
    for u in &got {
        assert_eq!(users.get(u.id).await.unwrap().as_ref(), Some(u));
    }
    assert!(users.get_many(&[]).await.unwrap().is_empty());
    let many = (0..201).map(|_| UserId::new()).collect::<Vec<_>>();
    assert!(matches!(
        users.get_many(&many).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(users.get_many(&many[..200]).await.unwrap().is_empty());

    // Media.
    let hash = ArtifactHash::parse(&"a".repeat(64)).unwrap();
    let art = Artifact {
        hash: hash.clone(),
        kind: ArtifactKind::Media,
        bytes: 10,
        mime: "image/png".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    assert!(repos.artifacts().insert_if_absent(&art).await.unwrap());
    let mut media_ids = Vec::new();
    for slug in ["m1", "m2"] {
        let mut new = new_post(&author, slug);
        new.kind = "media".into();
        let id = repos.content().insert(&new).await.unwrap().id;
        repos
            .media()
            .insert(&Media {
                item_id: id,
                artifact_hash: hash.clone(),
                mime: "image/png".into(),
                bytes: 10,
                width: Some(1),
                height: None,
                alt: format!("alt {slug}"),
                original_name: format!("{slug}.png"),
                variants: json!({}),
                sanitized: false,
            })
            .await
            .unwrap();
        media_ids.push(id);
    }
    let (m1, m2) = (media_ids[0], media_ids[1]);
    let got = repos.media().get_many(&[m2, a, m1, m2]).await.unwrap();
    assert_eq!(got.len(), 2);
    for m in &got {
        assert_eq!(
            repos.media().get(m.item_id).await.unwrap().as_ref(),
            Some(m)
        );
    }
    assert_eq!(got.iter().map(|m| m.item_id).collect::<Vec<_>>(), [m2, m1]);
    assert!(repos.media().get_many(&[]).await.unwrap().is_empty());
    assert!(matches!(
        repos.media().get_many(&ids(201)).await,
        Err(StoreError::Invalid(_))
    ));

    // Meta: ordered by input id order, then key bytes; only requested keys.
    let meta = repos.meta();
    meta.set(a, "seo", &json!({"t": "ü"})).await.unwrap();
    meta.set(a, "Zed", &json!(1)).await.unwrap();
    meta.set(a, "other", &json!("not asked")).await.unwrap();
    meta.set(b, "seo", &json!([1, 2.5, null])).await.unwrap();
    meta.set(b, "alpha", &json!(true)).await.unwrap();
    let got = meta
        .get_many(&[b, a, c, b], &["seo", "Zed", "alpha", "missing", "seo"])
        .await
        .unwrap();
    assert_eq!(
        got,
        [
            (b, "alpha".to_string(), json!(true)),
            (b, "seo".to_string(), json!([1, 2.5, null])),
            (a, "Zed".to_string(), json!(1)),
            (a, "seo".to_string(), json!({"t": "ü"})),
        ]
    );
    for (id, key, value) in &got {
        assert_eq!(meta.get(*id, key).await.unwrap().as_ref(), Some(value));
    }
    assert!(meta.get_many(&[], &["seo"]).await.unwrap().is_empty());
    assert!(meta.get_many(&[a], &[]).await.unwrap().is_empty());
    let keys: Vec<String> = (0..51).map(|i| format!("k{i}")).collect();
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    assert!(
        meta.get_many(&[a], &key_refs[..50])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        meta.get_many(&[a], &key_refs).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        meta.get_many(&ids(201), &["seo"]).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(meta.get_many(&ids(200), &["seo"]).await.unwrap().is_empty());
}
