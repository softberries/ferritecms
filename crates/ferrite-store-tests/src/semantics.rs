//! Cross-repository rules: missing-row semantics (M1), revision list limits (M8) and the
//! redirect row returned by upsert, including `last_hit_at` (M1, M9).

use ferrite_domain::{
    ArtifactHash, ContentId, Media, Redirect, RedirectId, RedirectOrigin, Term, TermId, Timestamp,
    UserId,
};
use ferrite_store::StoreError;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

#[track_caller]
fn assert_not_found(result: Result<(), StoreError>, what: &str) {
    assert!(
        matches!(result, Err(StoreError::NotFound)),
        "{what}: {result:?}"
    );
}

/// Updates (and `hit`, `set_enabled`, `activate`) of a missing row are `NotFound`;
/// deletes of a missing row are a successful no-op.
pub async fn missing_rows_update_not_found_delete_ok(db: &TestDb) {
    let store = db.store();
    let r = store.read();
    let author = fixture_user(store).await;

    let mut ghost = r
        .content()
        .insert(&new_post(&author, "ghost"))
        .await
        .unwrap();
    r.content().delete(ghost.id).await.unwrap();
    ghost.title = "changed".into();
    assert!(matches!(
        r.content().update(&ghost).await,
        Err(StoreError::NotFound)
    ));
    assert_not_found(
        r.terms()
            .update(&Term {
                id: TermId::new(),
                taxonomy: "tag".into(),
                slug: "x".into(),
                name: "x".into(),
                description: String::new(),
                parent_id: None,
            })
            .await,
        "terms.update",
    );
    let mut user = author.clone();
    user.id = UserId::new();
    assert_not_found(r.users().update(&user).await, "users.update");
    assert_not_found(
        r.media()
            .update(&Media {
                item_id: ContentId::new(),
                artifact_hash: ArtifactHash::parse(&"b".repeat(64)).unwrap(),
                mime: "image/png".into(),
                bytes: 1,
                width: None,
                height: None,
                alt: String::new(),
                original_name: "b.png".into(),
                variants: serde_json::json!({}),
            })
            .await,
        "media.update",
    );
    assert_not_found(
        r.plugins().set_enabled("missing", true, None).await,
        "plugins.set_enabled",
    );
    assert_not_found(r.themes().activate("missing").await, "themes.activate");
    assert_not_found(r.redirects().hit(RedirectId::new()).await, "redirects.hit");

    r.content().delete(ContentId::new()).await.unwrap();
    r.meta().delete(ContentId::new(), "k").await.unwrap();
    r.types().delete("missing").await.unwrap();
    r.terms().delete(TermId::new()).await.unwrap();
    r.menus().delete("missing").await.unwrap();
    r.options().delete("missing").await.unwrap();
    r.redirects().delete(RedirectId::new()).await.unwrap();
    r.artifacts()
        .delete(&ArtifactHash::parse(&"c".repeat(64)).unwrap())
        .await
        .unwrap();
    r.plugins().delete("missing").await.unwrap();
    r.plugin_kv().delete("missing", "k").await.unwrap();
}

/// `RevisionRepo::list` takes a page size of 1 through 200 like every other list.
pub async fn revisions_list_checks_limit(db: &TestDb) {
    let r = db.store().read();
    for bad in [0, 201] {
        let got = r.revisions().list(ContentId::new(), bad).await;
        assert!(matches!(got, Err(StoreError::Invalid(_))), "{bad}: {got:?}");
    }
    assert!(
        r.revisions()
            .list(ContentId::new(), 200)
            .await
            .unwrap()
            .is_empty()
    );
}

/// `upsert` returns the row as stored (an existing row keeps id, hits and creation
/// time), and `hit` records `last_hit_at`.
pub async fn redirects_upsert_returns_stored_and_hit_time(db: &TestDb) {
    let redirects = db.store().read().redirects();
    let first = Redirect {
        id: RedirectId::new(),
        source_path: "/a".into(),
        target: "/b".into(),
        code: 301,
        hits: 0,
        origin: RedirectOrigin::Manual,
        created_at: Timestamp::now(),
        last_hit_at: None,
    };
    let stored = redirects.upsert(&first).await.unwrap();
    assert_eq!(stored, first);

    let before = Timestamp::now();
    redirects.hit(first.id).await.unwrap();
    let got = redirects.get_by_source("/a").await.unwrap().unwrap();
    assert_eq!(got.hits, 1);
    let hit_at = got.last_hit_at.expect("last_hit_at set by hit");
    assert!(hit_at >= before, "{hit_at:?} < {before:?}");

    let replacement = Redirect {
        id: RedirectId::new(),
        target: "/c".into(),
        code: 302,
        created_at: Timestamp::now(),
        ..first.clone()
    };
    let stored = redirects.upsert(&replacement).await.unwrap();
    assert_eq!(stored.id, first.id);
    assert_eq!(stored.created_at, first.created_at);
    assert_eq!(stored.hits, 1);
    assert_eq!(stored.last_hit_at, Some(hit_at));
    assert_eq!((stored.target.as_str(), stored.code), ("/c", 302));
    assert_eq!(
        redirects.get_by_source("/a").await.unwrap().unwrap(),
        stored
    );
}

/// `close` waits for checked-out connections, then every later call fails; a second
/// `close` is a no-op.
pub async fn close_fails_later_calls(db: &TestDb) {
    let store = db.store();
    store
        .read()
        .options()
        .set("before.close", &serde_json::json!(1), false)
        .await
        .expect("write before close");
    store.close().await;
    store.close().await;
    assert!(store.read().options().get("before.close").await.is_err());
    assert!(store.begin().await.is_err());
}
