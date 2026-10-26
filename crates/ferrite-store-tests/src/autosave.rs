//! Contract tests for the per-user autosave slot (`0007_autosaves`, M2b Task 3, P15;
//! its `seo` member from `0008_autosave_seo`, M2b-2 Task 1).

use ferrite_domain::{Autosave, ContentId, TermId, Timestamp, UserId};
use ferrite_store::{Store, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

/// An autosave of `item` by `user` with every optional member set.
pub fn full_autosave(item: ContentId, user: UserId) -> Autosave {
    Autosave {
        item_id: item,
        user_id: user,
        base_version: 3,
        title: "Draft title ünï".into(),
        excerpt: Some("An excerpt".into()),
        slug: Some("a-slug".into()),
        parent_id: Some(ContentId::new()),
        menu_order: Some(-4),
        terms: Some(vec![TermId::new(), TermId::new()]),
        body: json!({"type": "doc", "content": [{"type": "paragraph"}]}),
        fields: json!({"subtitle": "s", "n": 1}),
        seo: Some(json!({"title": "Search title", "description": null})),
        saved_at: Timestamp::from_micros(1_700_000_000_123_456).unwrap(),
    }
}

/// Inserts a post by a fresh user; returns the item id and the user id.
async fn post(store: &dyn Store) -> (ContentId, UserId) {
    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, &format!("p-{}", ContentId::new())))
        .await
        .unwrap();
    (item.id, author.id)
}

/// `put` inserts and replaces by (item, user), `get` reads it back exactly (the `None`
/// members included), `delete` removes only that row and says whether there was one.
/// NUL in a text or the JSON is `Invalid`; an unknown item or user is `ForeignKey`.
pub async fn autosave_upsert_get_delete(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let (item, author) = post(store).await;
    let other = fixture_user(store).await.id;
    let slot = repos.autosaves();

    assert_eq!(slot.get(item, author).await.unwrap(), None);
    assert!(!slot.delete(item, author).await.unwrap());

    let full = full_autosave(item, author);
    slot.put(&full).await.unwrap();
    assert_eq!(slot.get(item, author).await.unwrap().as_ref(), Some(&full));

    // Replaced in place, every optional member now absent.
    let bare = Autosave {
        base_version: 4,
        title: String::new(),
        excerpt: None,
        slug: None,
        parent_id: None,
        menu_order: None,
        terms: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        seo: None,
        saved_at: Timestamp::from_micros(1_700_000_001_000_000).unwrap(),
        ..full.clone()
    };
    slot.put(&bare).await.unwrap();
    assert_eq!(slot.get(item, author).await.unwrap().as_ref(), Some(&bare));
    // An empty term list is not "unchanged".
    let no_terms = Autosave {
        terms: Some(Vec::new()),
        ..bare.clone()
    };
    slot.put(&no_terms).await.unwrap();
    assert_eq!(
        slot.get(item, author).await.unwrap().unwrap().terms,
        Some(Vec::new())
    );

    // Another user's slot on the same item is its own row.
    let theirs = full_autosave(item, other);
    slot.put(&theirs).await.unwrap();
    assert_eq!(slot.get(item, other).await.unwrap().as_ref(), Some(&theirs));
    assert!(slot.delete(item, author).await.unwrap());
    assert_eq!(slot.get(item, author).await.unwrap(), None);
    assert!(!slot.delete(item, author).await.unwrap());
    assert_eq!(slot.get(item, other).await.unwrap().as_ref(), Some(&theirs));

    // Inside a Tx the write is visible to the Tx and gone after a rollback.
    let tx = store.begin().await.unwrap();
    tx.autosaves().put(&full).await.unwrap();
    assert!(tx.autosaves().get(item, author).await.unwrap().is_some());
    tx.rollback().await.unwrap();
    assert_eq!(slot.get(item, author).await.unwrap(), None);

    // NUL anywhere is refused before the database sees it.
    let invalid = |r: Result<(), StoreError>| matches!(r, Err(StoreError::Invalid(_)));
    for bad in [
        Autosave {
            title: "a\0b".into(),
            ..full.clone()
        },
        Autosave {
            excerpt: Some("a\0b".into()),
            ..full.clone()
        },
        Autosave {
            slug: Some("a\0b".into()),
            ..full.clone()
        },
        Autosave {
            body: json!({"t": "a\0b"}),
            ..full.clone()
        },
        Autosave {
            fields: json!({"a\0b": 1}),
            ..full.clone()
        },
        Autosave {
            seo: Some(json!({"title": "a\0b"})),
            ..full.clone()
        },
    ] {
        assert!(invalid(slot.put(&bad).await), "{bad:?}");
    }
    assert_eq!(slot.get(item, author).await.unwrap(), None);

    // Unknown item or user.
    for bad in [
        full_autosave(ContentId::new(), author),
        full_autosave(item, UserId::new()),
    ] {
        let err = slot.put(&bad).await.unwrap_err();
        assert!(matches!(err, StoreError::ForeignKey { .. }), "{err:?}");
    }
}

/// Deleting the item removes every slot on it; deleting a user removes that user's
/// slots and leaves the others.
pub async fn autosave_cascades_with_item_and_user(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let (item, author) = post(store).await;
    let (kept, kept_author) = post(store).await;
    let editor = fixture_user(store).await.id;
    let slot = repos.autosaves();
    for (i, u) in [
        (item, author),
        (item, editor),
        (kept, editor),
        (kept, kept_author),
    ] {
        slot.put(&full_autosave(i, u)).await.unwrap();
    }

    repos.content().delete(item).await.unwrap();
    assert_eq!(slot.get(item, author).await.unwrap(), None);
    assert_eq!(slot.get(item, editor).await.unwrap(), None);
    assert!(slot.get(kept, editor).await.unwrap().is_some());

    repos.users().delete(editor).await.unwrap();
    assert_eq!(slot.get(kept, editor).await.unwrap(), None);
    assert!(slot.get(kept, kept_author).await.unwrap().is_some());
}

/// The `seo` member (`0008_autosave_seo`) is stored as given: absent (`None`), an
/// object with `null` members, and back to absent on the next `put`.
pub async fn autosave_keeps_seo(db: &TestDb) {
    let store = db.store();
    let (item, author) = post(store).await;
    let slot = store.read().autosaves();
    let mut a = full_autosave(item, author);
    for seo in [
        Some(json!({"title": " as typed ", "description": "d"})),
        Some(json!({"title": null})),
        Some(json!({})),
        None,
    ] {
        a.seo = seo;
        slot.put(&a).await.unwrap();
        assert_eq!(slot.get(item, author).await.unwrap().as_ref(), Some(&a));
    }
    // Inside a Tx too.
    let tx = store.begin().await.unwrap();
    a.seo = Some(json!({"description": "in tx"}));
    tx.autosaves().put(&a).await.unwrap();
    assert_eq!(
        tx.autosaves().get(item, author).await.unwrap(),
        Some(a.clone())
    );
    tx.commit().await.unwrap();
    assert_eq!(slot.get(item, author).await.unwrap(), Some(a));
}
