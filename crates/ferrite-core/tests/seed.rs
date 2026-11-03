//! `DemoBatch` (M4-1 P20): one transaction for the demo's writes, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, CoreError, PermalinkRules, events::INVALIDATE, seed::DemoBatch,
};
use ferrite_domain::{ContentQuery, Menu, MenuItem, MenuItemId, MenuItemKind, NewUser, Role};
use ferrite_domain::{UserId, UserStatus};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

fn new_admin() -> NewUser {
    NewUser {
        id: UserId::new(),
        login: "admin".into(),
        email: "admin@example.test".into(),
        display_name: "Admin".into(),
        password_hash: None,
        role: Role::Admin,
        status: UserStatus::Active,
        must_reset: true,
    }
}

fn post(title: &str) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

/// Writes one of everything through `batch`.
async fn write_all(batch: &DemoBatch) {
    let admin = batch.user(&new_admin()).await.unwrap();
    let actor = Actor::new(admin.id, admin.role);
    batch.term("category", "news", "News").await.unwrap();
    batch.publish(&actor, post("Hello"), None).await.unwrap();
    batch
        .menu(&Menu {
            name: "primary".into(),
            label: "Primary".into(),
            items: vec![MenuItem {
                id: MenuItemId::new(),
                parent_id: None,
                position: 0,
                kind: MenuItemKind::Url,
                target_id: None,
                url: Some("/".into()),
                label: "Home".into(),
            }],
            locale: "en".into(),
            translation_of: None,
        })
        .await
        .unwrap();
    batch
        .option(&actor, "site.title", &json!("Demo"), true)
        .await
        .unwrap();
}

async fn begin(db: &TestDb) -> DemoBatch {
    DemoBatch::begin(db.shared_store(), Arc::new(PermalinkRules::default()))
        .await
        .unwrap()
}

async fn event_count(store: &dyn Store) -> usize {
    store.read().events().since(0, 1000).await.unwrap().len()
}

/// A rolled-back batch leaves no row and no event.
pub async fn rollback_leaves_nothing(db: &TestDb) {
    let store = db.store();
    let events = event_count(store).await;
    let batch = begin(db).await;
    write_all(&batch).await;
    batch.rollback().await.unwrap();

    let read = store.read();
    assert!(read.users().get_by_login("admin").await.unwrap().is_none());
    assert!(
        read.terms()
            .get_by_slug("category", None, "news")
            .await
            .unwrap()
            .is_none()
    );
    assert!(read.menus().get("primary").await.unwrap().is_none());
    assert!(read.options().get("site.title").await.unwrap().is_none());
    assert_eq!(
        read.content()
            .count(&ContentQuery::default())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        event_count(store).await,
        events,
        "a rolled-back batch emitted events"
    );
}

/// A committed batch writes its rows and the events the services emit.
pub async fn commit_writes_rows_and_events(db: &TestDb) {
    let store = db.store();
    let events = event_count(store).await;
    let batch = begin(db).await;
    write_all(&batch).await;
    batch.commit().await.unwrap();

    let read = store.read();
    assert!(read.users().get_by_login("admin").await.unwrap().is_some());
    assert!(
        read.terms()
            .get_by_slug("category", None, "news")
            .await
            .unwrap()
            .is_some()
    );
    assert!(read.menus().get("primary").await.unwrap().is_some());
    assert!(read.options().get("site.title").await.unwrap().is_some());
    let published = ContentQuery {
        kind: Some("post".into()),
        ..Default::default()
    };
    assert_eq!(read.content().count(&published).await.unwrap(), 1);

    let all = read.events().since(0, 1000).await.unwrap();
    let new = &all[events..];
    let keys: Vec<&String> = new
        .iter()
        .filter(|e| e.kind == INVALIDATE)
        .flat_map(|e| &e.keys)
        .collect();
    assert!(
        keys.iter().any(|k| k.starts_with("menu:primary")),
        "{keys:?}"
    );
    assert!(keys.iter().any(|k| k.starts_with("post:")), "{keys:?}");
    assert!(
        new.iter()
            .any(|e| e.keys.iter().any(|k| k.contains("site.title"))),
        "options.changed: {new:?}"
    );
}

/// `permalink.*` options need the `types` lock, which a batch does not hold.
pub async fn option_refuses_permalink_keys(db: &TestDb) {
    let batch = begin(db).await;
    let admin = batch.user(&new_admin()).await.unwrap();
    let actor = Actor::new(admin.id, admin.role);
    let err = batch
        .option(&actor, "permalink.post", &json!("/%postname%/"), true)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
    batch.rollback().await.unwrap();
}

backend_tests!(
    rollback_leaves_nothing,
    commit_writes_rows_and_events,
    option_refuses_permalink_keys,
);
