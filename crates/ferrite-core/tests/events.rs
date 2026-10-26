//! The exact `invalidate` key set of each write path, on both backends (R9/I5). The
//! renderer-side half of the contract is asserted in `site_reader.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeSet, sync::Arc};

use ferrite_core::{
    Actor, ContentDraft, ContentService, OptionService, PermalinkRules, RedirectService,
    SiteReader, TaxonomyService,
    events::{ALL_KEY, archive_key, option_key, post_key, redirect_key, term_key},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, Media, NewUser, Role, Term,
    TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

type Keys = BTreeSet<String>;

async fn admin(store: &dyn Store) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, Role::Admin)
}

fn service(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

fn draft(kind: &str, title: &str, parent: Option<ContentId>, terms: &[TermId]) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: parent,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: terms.to_vec(),
        seo: None,
    }
}

async fn term(store: &dyn Store, taxonomy: &str, slug: &str) -> TermId {
    let id = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id,
            taxonomy: taxonomy.into(),
            slug: slug.into(),
            name: slug.into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    id
}

async fn seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

/// The union of the keys of every event after `after`; all of them must be `invalidate`.
async fn keys_since(store: &dyn Store, after: i64) -> Keys {
    let events = store.read().events().since(after, 1000).await.unwrap();
    for e in &events {
        assert_eq!(e.kind, "invalidate", "{e:?}");
    }
    events.into_iter().flat_map(|e| e.keys).collect()
}

fn set<const N: usize>(keys: [String; N]) -> Keys {
    keys.into_iter().collect()
}

fn redirect_pair(path: &str) -> [String; 2] {
    let other = match path.strip_suffix('/') {
        Some(s) => s.to_owned(),
        None => format!("{path}/"),
    };
    [redirect_key(path), redirect_key(&other)]
}

pub async fn update_emits_union_of_terms(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let a = admin(store).await;
    let (t1, t2, t3) = (
        term(store, "category", "one").await,
        term(store, "category", "two").await,
        term(store, "category", "three").await,
    );
    let before = seq(store).await;
    let it = svc
        .create(&a, draft("post", "P", None, &[t1, t2]))
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        set([
            post_key(it.id),
            archive_key("post"),
            term_key(t1),
            term_key(t2)
        ])
    );

    // t1 removed, t2 kept, t3 added: old and new terms all appear.
    let before = seq(store).await;
    svc.update(&a, it.id, it.version, draft("post", "P2", None, &[t2, t3]))
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        set([
            post_key(it.id),
            archive_key("post"),
            term_key(t1),
            term_key(t2),
            term_key(t3),
        ])
    );
}

pub async fn update_emits_both_parents(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let a = admin(store).await;
    let p1 = svc
        .create(&a, draft("page", "P1", None, &[]))
        .await
        .unwrap();
    let p2 = svc
        .create(&a, draft("page", "P2", None, &[]))
        .await
        .unwrap();

    let before = seq(store).await;
    let child = svc
        .create(&a, draft("page", "C", Some(p1.id), &[]))
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        set([post_key(child.id), archive_key("page"), post_key(p1.id)])
    );

    // Same parent: still named (the parent page lists its children).
    let before = seq(store).await;
    let child = svc
        .update(
            &a,
            child.id,
            child.version,
            draft("page", "C2", Some(p1.id), &[]),
        )
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        set([post_key(child.id), archive_key("page"), post_key(p1.id)])
    );

    // Changed parent: old and new.
    let before = seq(store).await;
    let child = svc
        .update(
            &a,
            child.id,
            child.version,
            draft("page", "C2", Some(p2.id), &[]),
        )
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        set([
            post_key(child.id),
            archive_key("page"),
            post_key(p1.id),
            post_key(p2.id)
        ])
    );

    // Parent removed: the old one is still named.
    let before = seq(store).await;
    svc.update(&a, child.id, child.version, draft("page", "C2", None, &[]))
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        set([post_key(child.id), archive_key("page"), post_key(p2.id)])
    );
}

pub async fn update_emits_moved_descendants_and_redirects(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let a = admin(store).await;
    let mk = |title: &str, parent| draft("page", title, parent, &[]);
    let top = svc.create(&a, mk("Top", None)).await.unwrap();
    let mid = svc.create(&a, mk("Mid", Some(top.id))).await.unwrap();
    let leaf = svc.create(&a, mk("Leaf", Some(mid.id))).await.unwrap();
    let draft_child = svc.create(&a, mk("Hidden", Some(top.id))).await.unwrap();
    let top = svc.publish(&a, top.id, top.version).await.unwrap();
    svc.publish(&a, mid.id, mid.version).await.unwrap();
    svc.publish(&a, leaf.id, leaf.version).await.unwrap();

    let before = seq(store).await;
    let mut d = mk("Top", None);
    d.slug = Some("renamed".into());
    svc.update(&a, top.id, top.version, d).await.unwrap();
    let mut want = set([
        post_key(top.id),
        post_key(mid.id),
        post_key(leaf.id),
        archive_key("page"),
    ]);
    // Redirects recorded for the moved published pages: top, mid, leaf (old paths).
    for old in ["/top/", "/top/mid/", "/top/mid/leaf/"] {
        want.extend(redirect_pair(old));
    }
    let got = keys_since(store, before).await;
    assert_eq!(got, want);
    // The unpublished descendant did not move a published URL: not named.
    assert!(!got.contains(&post_key(draft_child.id)));
}

pub async fn lifecycle_emits_item_archive_terms_parent(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let a = admin(store).await;
    let t = term(store, "category", "life").await;
    let parent = svc
        .create(&a, draft("page", "Parent", None, &[]))
        .await
        .unwrap();
    let item = svc
        .create(&a, draft("page", "Child", Some(parent.id), &[t]))
        .await
        .unwrap();
    let want = set([
        post_key(item.id),
        archive_key("page"),
        term_key(t),
        post_key(parent.id),
    ]);

    let mut cur = item;
    macro_rules! step {
        ($call:expr) => {{
            let before = seq(store).await;
            cur = $call.await.unwrap();
            assert_eq!(keys_since(store, before).await, want, "{:?}", cur.status);
        }};
    }
    step!(svc.submit(&a, cur.id, cur.version));
    step!(svc.publish(&a, cur.id, cur.version));
    step!(svc.unpublish(&a, cur.id, cur.version));
    let later = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap();
    step!(svc.schedule(&a, cur.id, cur.version, later));
    step!(svc.trash(&a, cur.id, cur.version));
    step!(svc.restore(&a, cur.id, cur.version));

    // publish_due: the schedule is in the future, so ask for a later "now".
    let cur2 = svc.schedule(&a, cur.id, cur.version, later).await.unwrap();
    let before = seq(store).await;
    let far = Timestamp::from_micros(later.as_micros() + 1).unwrap();
    assert_eq!(svc.publish_due(far).await.unwrap(), 1);
    assert_eq!(keys_since(store, before).await, want);

    // delete: terms are read before the links go with the row.
    let cur3 = svc.get(cur2.id).await.unwrap();
    assert_eq!(cur3.status, ContentStatus::Published);
    let trashed = svc.trash(&a, cur3.id, cur3.version).await.unwrap();
    let before = seq(store).await;
    svc.delete(&a, trashed.id, trashed.version).await.unwrap();
    assert_eq!(keys_since(store, before).await, want);
}

pub async fn publish_over_a_redirect_emits_its_keys(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let redirects = RedirectService::new(db.shared_store());
    let a = admin(store).await;
    redirects
        .upsert(&a, "/foo/", "/elsewhere/", 301)
        .await
        .unwrap();
    let mut d = draft("post", "Foo", None, &[]);
    d.slug = Some("foo".into());
    let it = svc.create(&a, d).await.unwrap();
    let before = seq(store).await;
    svc.publish(&a, it.id, it.version).await.unwrap();
    let mut want = set([post_key(it.id), archive_key("post")]);
    want.extend(redirect_pair("/foo/"));
    assert_eq!(keys_since(store, before).await, want);
}

pub async fn redirect_service_emits_source_keys(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let a = admin(store).await;

    let before = seq(store).await;
    let r = svc.upsert(&a, "/old", "/new", 301).await.unwrap();
    assert_eq!(
        keys_since(store, before).await,
        redirect_pair("/old").into_iter().collect()
    );

    let before = seq(store).await;
    svc.upsert(&a, "/dir/", "https://example.com/", 302)
        .await
        .unwrap();
    assert_eq!(
        keys_since(store, before).await,
        redirect_pair("/dir/").into_iter().collect()
    );

    let before = seq(store).await;
    svc.delete(&a, r.id).await.unwrap();
    assert_eq!(
        keys_since(store, before).await,
        redirect_pair("/old").into_iter().collect()
    );

    // A missing redirect is a no-op: no event.
    let before = seq(store).await;
    svc.delete(&a, r.id).await.unwrap();
    assert_eq!(seq(store).await, before);

    // A rejected upsert emits nothing.
    assert!(svc.upsert(&a, "/same", "/same", 301).await.is_err());
    assert_eq!(seq(store).await, before);
}

pub async fn redirect_update_and_import_emit_source_keys(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let a = admin(store).await;
    let r = svc.upsert(&a, "/from", "/to", 301).await.unwrap();

    // A moved source: both the old and the new source (with variants).
    let before = seq(store).await;
    svc.update(&a, r.id, "/moved/", "/to", 301).await.unwrap();
    let mut want: Keys = redirect_pair("/from").into_iter().collect();
    want.extend(redirect_pair("/moved/"));
    assert_eq!(keys_since(store, before).await, want);

    // Same source: only its keys.
    let before = seq(store).await;
    svc.update(&a, r.id, "/moved/", "/else", 302).await.unwrap();
    assert_eq!(
        keys_since(store, before).await,
        redirect_pair("/moved/").into_iter().collect()
    );

    // An import: every row's source.
    let before = seq(store).await;
    svc.import(
        &a,
        &[
            ("/i1".to_owned(), "/x".to_owned(), 301),
            ("/moved/".to_owned(), "/y".to_owned(), 308),
        ],
    )
    .await
    .unwrap();
    let mut want: Keys = redirect_pair("/i1").into_iter().collect();
    want.extend(redirect_pair("/moved/"));
    assert_eq!(keys_since(store, before).await, want);

    // A rejected update or import emits nothing.
    let before = seq(store).await;
    assert!(svc.update(&a, r.id, "/i1", "/x", 301).await.is_err());
    assert!(
        svc.import(&a, &[("/z".to_owned(), "/z".to_owned(), 301)])
            .await
            .is_err()
    );
    assert_eq!(seq(store).await, before);
}

pub async fn term_writes_emit_term_and_archive_keys(db: &TestDb) {
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let a = admin(store).await;

    let before = seq(store).await;
    let t = svc
        .create_term(&a, "category", "Tech", None, None, "")
        .await
        .unwrap();
    let want = set([term_key(t.id), archive_key("post")]);
    assert_eq!(keys_since(store, before).await, want);

    // Linked items need no keys of their own: their pages depend on `term:<id>`.
    let it = service(db)
        .create(&a, draft("post", "Linked", None, &[t.id]))
        .await
        .unwrap();
    let before = seq(store).await;
    svc.update_term(&a, t.id, "Technology", None, None, "x")
        .await
        .unwrap();
    assert_eq!(keys_since(store, before).await, want);
    assert!(!keys_since(store, before).await.contains(&post_key(it.id)));

    let before = seq(store).await;
    svc.delete_term(&a, t.id).await.unwrap();
    assert_eq!(keys_since(store, before).await, want);
}

pub async fn permalink_options_flush_everything(db: &TestDb) {
    let store = db.store();
    let a = admin(store).await;
    let svc = OptionService::new(db.shared_store());

    let kinds = |after: i64| async move {
        let ev = store.read().events().since(after, 100).await.unwrap();
        ev.into_iter()
            .map(|e| (e.kind, e.keys.into_iter().collect::<Keys>()))
            .collect::<Vec<_>>()
    };
    let before = seq(store).await;
    svc.set(&a, "permalink.post", &json!("/%year%/%slug%/"), true)
        .await
        .unwrap();
    assert_eq!(
        kinds(before).await,
        [
            (
                "options.changed".to_owned(),
                set([option_key("permalink.post")])
            ),
            ("invalidate".to_owned(), set([ALL_KEY.to_owned()])),
        ]
    );
    let before = seq(store).await;
    svc.delete(&a, "permalink.post").await.unwrap();
    assert_eq!(
        kinds(before).await,
        [
            (
                "options.changed".to_owned(),
                set([option_key("permalink.post")])
            ),
            ("invalidate".to_owned(), set([ALL_KEY.to_owned()])),
        ]
    );
    // Other options do not.
    let before = seq(store).await;
    svc.set(&a, "reading.per_page", &json!(5), true)
        .await
        .unwrap();
    assert_eq!(
        kinds(before).await,
        [(
            "options.changed".to_owned(),
            set([option_key("reading.per_page")])
        )]
    );
}

pub async fn publishing_a_referenced_draft_is_a_dependency(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let a = admin(store).await;
    let reader = SiteReader::new(
        db.shared_store(),
        Arc::new(PermalinkRules::default()),
        Arc::new(|h: &ArtifactHash, n: &str| format!("/_media/{h}/{n}")),
        Arc::new(Timestamp::now),
    );
    // A draft media item with a media row, referenced as the featured image.
    let pic = svc
        .create(&a, draft("media", "pic", None, &[]))
        .await
        .unwrap();
    let hash = ArtifactHash::parse(&format!("{:0>64}", "abc")).unwrap();
    let tx = store.begin().await.unwrap();
    tx.artifacts()
        .insert_if_absent(&Artifact {
            hash: hash.clone(),
            kind: ArtifactKind::Media,
            bytes: 10,
            mime: "image/png".into(),
            created_at: Timestamp::now(),
        })
        .await
        .unwrap();
    tx.media()
        .insert(&Media {
            item_id: pic.id,
            artifact_hash: hash,
            mime: "image/png".into(),
            bytes: 10,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "pic.png".into(),
            variants: json!({}),
            sanitized: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut d = draft("post", "Has pic", None, &[]);
    d.slug = Some("has-pic".into());
    d.fields = json!({"featured_image": pic.id.to_string()});
    let post = svc.create(&a, d).await.unwrap();
    svc.publish(&a, post.id, post.version).await.unwrap();

    let got = reader
        .single("post", &["has-pic".to_owned()])
        .await
        .unwrap();
    let page = got.value.unwrap();
    assert!(page.featured.is_none(), "draft media is not shown");
    assert!(got.deps.contains(&post_key(pic.id)), "hidden ref recorded");

    // Publishing the draft emits the key the page recorded.
    let before = seq(store).await;
    svc.publish(&a, pic.id, pic.version).await.unwrap();
    assert!(keys_since(store, before).await.contains(&post_key(pic.id)));
    let got = reader
        .single("post", &["has-pic".to_owned()])
        .await
        .unwrap();
    assert!(got.value.unwrap().featured.is_some());
}

backend_tests!(
    update_emits_union_of_terms,
    update_emits_both_parents,
    update_emits_moved_descendants_and_redirects,
    lifecycle_emits_item_archive_terms_parent,
    publish_over_a_redirect_emits_its_keys,
    redirect_service_emits_source_keys,
    redirect_update_and_import_emit_source_keys,
    term_writes_emit_term_and_archive_keys,
    permalink_options_flush_everything,
    publishing_a_referenced_draft_is_a_dependency,
);
