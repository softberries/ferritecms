//! ContentService create/update on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, PermalinkInput, PermalinkRules, Permalinks,
    RedirectService,
    events::{archive_key, post_key, term_key},
};
use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, NewUser, Redirect, RedirectId, RedirectOrigin,
    RedirectQuery, Role, SearchQuery, Term, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::{Store, StoreError};
use ferrite_store_tests::{
    CountingStore, TestDb, backend_tests, backend_tests_spawning, fixtures::new_post,
};
use serde_json::json;

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, role)
}

/// The default rules: posts at `/%slug%/`.
fn permalink() -> Arc<PermalinkRules> {
    Arc::new(PermalinkRules::default())
}

fn service(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), permalink())
}

fn draft(kind: &str, title: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": format!("Body of {title} with zebra words")}]}
        ]}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

async fn term(store: &dyn Store, slug: &str) -> TermId {
    let id = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id,
            taxonomy: "category".into(),
            slug: slug.into(),
            name: slug.into(),
            description: String::new(),
            parent_id: None,
            locale: "en".into(),
            translation_group: id,
        })
        .await
        .unwrap();
    id
}

async fn search(store: &dyn Store, q: &str) -> Vec<ContentId> {
    store
        .read()
        .search()
        .search(&SearchQuery {
            q: q.into(),
            kinds: Vec::new(),
            statuses: Vec::new(),
            readable_by: None,
            limit: 50,
            cursor: None,
            locale: None,
        })
        .await
        .unwrap()
        .items
}

async fn revision_count(store: &dyn Store, id: ContentId) -> usize {
    store.read().revisions().list(id, 100).await.unwrap().len()
}

async fn max_seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

async fn last_event_keys(store: &dyn Store) -> Vec<String> {
    let events = store.read().events().since(0, 1000).await.unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "invalidate");
    // Without the translation group key (M6-1; checked in `content_locales.rs`).
    last.keys
        .iter()
        .filter(|k| !k.starts_with("group:"))
        .cloned()
        .collect()
}

/// Sets an item's status directly (lifecycle transitions are Task 7).
async fn force_status(store: &dyn Store, id: ContentId, status: ContentStatus) -> ContentItem {
    let mut item = store.read().content().get(id).await.unwrap().unwrap();
    item.status = status;
    store.read().content().update(&item).await.unwrap()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

async fn redirect_target(store: &dyn Store, source: &str) -> Option<String> {
    store
        .read()
        .redirects()
        .get_by_source(source)
        .await
        .unwrap()
        .map(|r| r.target)
}

pub async fn create_update_roundtrip(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let (t1, t2, t3) = (
        term(store, "one").await,
        term(store, "two").await,
        term(store, "three").await,
    );

    let mut d = draft("post", "Hello World");
    d.terms = vec![t1, t2, t1];
    let created = svc.create(&admin, d).await.unwrap();
    assert_eq!(created.status, ContentStatus::Draft);
    assert_eq!(created.slug, "hello-world");
    assert_eq!(created.author_id, admin.user);
    assert_eq!(created.version, 1);
    assert_eq!(created.body_html, None);
    assert_eq!(created.excerpt, "Body of Hello World with zebra words");
    assert_eq!(svc.get(created.id).await.unwrap(), created);
    assert_eq!(revision_count(store, created.id).await, 1);
    assert_eq!(search(store, "zebra").await, [created.id]);
    let terms: Vec<TermId> = store
        .read()
        .terms()
        .for_item(created.id)
        .await
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(terms, [t1, t2]);
    assert_eq!(
        sorted(last_event_keys(store).await),
        sorted(vec![
            post_key(created.id),
            archive_key("post"),
            term_key(t1),
            term_key(t2),
        ])
    );
    let rev = &store.read().revisions().list(created.id, 1).await.unwrap()[0];
    assert_eq!(rev.title, "Hello World");
    assert_eq!(rev.author_id, admin.user);

    // Update: new title, explicit excerpt, terms t2 kept, t1 removed, t3 added.
    let mut d = draft("post", "Goodbye Moon");
    d.excerpt = Some("custom".into());
    d.terms = vec![t2, t3];
    d.body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "giraffe"}]}]});
    let updated = svc.update(&admin, created.id, 1, d).await.unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.title, "Goodbye Moon");
    assert_eq!(updated.excerpt, "custom");
    // No slug given: the slug stays (WordPress keeps the slug once set).
    assert_eq!(updated.slug, "hello-world");
    assert_eq!(updated.status, ContentStatus::Draft);
    assert_eq!(updated.author_id, admin.user);
    assert_eq!(svc.get(created.id).await.unwrap(), updated);
    assert_eq!(revision_count(store, created.id).await, 2);
    assert!(search(store, "zebra").await.is_empty());
    assert_eq!(search(store, "giraffe").await, [created.id]);
    assert_eq!(search(store, "goodbye").await, [created.id]);
    assert_eq!(
        sorted(last_event_keys(store).await),
        sorted(vec![
            post_key(created.id),
            archive_key("post"),
            term_key(t1),
            term_key(t2),
            term_key(t3),
        ])
    );

    // A caller slug is validated and normalized; a taken one gets `-N`.
    let mut d = draft("post", "Other");
    d.slug = Some("  Hello-World ".into());
    let other = svc.create(&admin, d).await.unwrap();
    assert_eq!(other.slug, "hello-world-2");
    let mut d = draft("post", "Other");
    d.slug = Some("bad/slug".into());
    assert!(matches!(
        svc.update(&admin, other.id, 1, d).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    // Renaming to its own slug keeps it (no `-N` against itself).
    let mut d = draft("post", "Other");
    d.slug = Some("hello-world-2".into());
    let same = svc.update(&admin, other.id, 1, d).await.unwrap();
    assert_eq!(same.slug, "hello-world-2");

    assert!(matches!(
        svc.update(&admin, ContentId::new(), 1, draft("post", "x"))
            .await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        svc.get(ContentId::new()).await,
        Err(CoreError::NotFound)
    ));
}

pub async fn explicit_empty_excerpt_is_kept(db: &TestDb) {
    let svc = service(db);
    let admin = actor(db.store(), Role::Admin).await;
    let mut d = draft("post", "Quiet");
    d.excerpt = Some(String::new());
    let created = svc.create(&admin, d.clone()).await.unwrap();
    assert_eq!(created.excerpt, "");
    // `None` still derives one from the body.
    d.excerpt = None;
    let updated = svc
        .update(&admin, created.id, created.version, d)
        .await
        .unwrap();
    assert_eq!(updated.excerpt, "Body of Quiet with zebra words");
    let mut d = draft("post", "Quiet");
    d.excerpt = Some(String::new());
    let again = svc
        .update(&admin, created.id, updated.version, d)
        .await
        .unwrap();
    assert_eq!(again.excerpt, "");
}

pub async fn revisions_pruned_to_50(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let mut item = svc.create(&admin, draft("post", "Many")).await.unwrap();
    for i in 0..51 {
        item = svc
            .update(
                &admin,
                item.id,
                item.version,
                draft("post", &format!("Many {i}")),
            )
            .await
            .unwrap();
    }
    assert_eq!(revision_count(store, item.id).await, 50);
    let newest = &store.read().revisions().list(item.id, 1).await.unwrap()[0];
    assert_eq!(newest.title, "Many 50");
}

pub async fn parent_change_and_validation(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let p1 = svc.create(&admin, draft("page", "P1")).await.unwrap();
    let p2 = svc.create(&admin, draft("page", "P2")).await.unwrap();

    let mut d = draft("page", "Child");
    d.parent_id = Some(p1.id);
    let child = svc.create(&admin, d).await.unwrap();
    assert_eq!(child.parent_id, Some(p1.id));
    let keys = last_event_keys(store).await;
    assert!(keys.contains(&post_key(p1.id)), "{keys:?}");

    let mut d = draft("page", "Child");
    d.parent_id = Some(p2.id);
    let moved = svc.update(&admin, child.id, 1, d).await.unwrap();
    assert_eq!(moved.parent_id, Some(p2.id));
    assert_eq!(
        sorted(last_event_keys(store).await),
        sorted(vec![
            post_key(child.id),
            archive_key("page"),
            post_key(p1.id),
            post_key(p2.id),
        ])
    );

    // Same slug under a different parent is free.
    let mut d = draft("page", "Child");
    d.parent_id = Some(p1.id);
    let twin = svc.create(&admin, d).await.unwrap();
    assert_eq!(twin.slug, "child");

    let seq = max_seq(store).await;
    // Unknown parent, self-parent and a cycle are Invalid.
    let mut d = draft("page", "X");
    d.parent_id = Some(ContentId::new());
    assert!(matches!(
        svc.create(&admin, d).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    let mut d = draft("page", "P2");
    d.parent_id = Some(p2.id);
    assert!(matches!(
        svc.update(&admin, p2.id, 1, d).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    let mut d = draft("page", "P2");
    d.parent_id = Some(child.id);
    assert!(matches!(
        svc.update(&admin, p2.id, 1, d).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    // Changing the type is Invalid; an unknown term is Invalid.
    assert!(matches!(
        svc.update(&admin, p2.id, 1, draft("post", "P2")).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    let mut d = draft("post", "T");
    d.terms = vec![TermId::new()];
    assert!(matches!(
        svc.create(&admin, d).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    assert_eq!(max_seq(store).await, seq);
}

pub async fn unknown_type_is_invalid(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let seq = max_seq(store).await;
    assert!(matches!(
        svc.create(&admin, draft("nope", "Hello")).await,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    ));
    assert_eq!(max_seq(store).await, seq);
    assert!(search(store, "hello").await.is_empty());
}

pub async fn version_conflict(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let item = svc.create(&admin, draft("post", "V")).await.unwrap();
    let v2 = svc
        .update(&admin, item.id, 1, draft("post", "V2"))
        .await
        .unwrap();
    let seq = max_seq(store).await;
    assert!(matches!(
        svc.update(&admin, item.id, 1, draft("post", "Stale")).await,
        Err(CoreError::Conflict { current_version: 2 })
    ));
    assert_eq!(svc.get(item.id).await.unwrap(), v2);
    assert_eq!(revision_count(store, item.id).await, 2);
    assert_eq!(max_seq(store).await, seq);
}

/// Review Focus 2.
pub async fn contributor_cannot_escalate(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let contributor = actor(store, Role::Contributor).await;
    let author = actor(store, Role::Author).await;
    let editor = actor(store, Role::Editor).await;

    let own = svc
        .create(&contributor, draft("post", "Mine"))
        .await
        .unwrap();
    assert_eq!(own.author_id, contributor.user);
    // Own draft: allowed; status cannot change through update.
    let own = svc
        .update(&contributor, own.id, own.version, draft("post", "Mine 2"))
        .await
        .unwrap();
    assert_eq!(own.status, ContentStatus::Draft);
    // Own pending: allowed, stays pending.
    let pending = force_status(store, own.id, ContentStatus::Pending).await;
    let pending = svc
        .update(
            &contributor,
            own.id,
            pending.version,
            draft("post", "Mine 3"),
        )
        .await
        .unwrap();
    assert_eq!(pending.status, ContentStatus::Pending);

    // Own published: forbidden, nothing written.
    let published = force_status(store, own.id, ContentStatus::Published).await;
    let seq = max_seq(store).await;
    let revs = revision_count(store, own.id).await;
    assert!(matches!(
        svc.update(
            &contributor,
            own.id,
            published.version,
            draft("post", "Hacked")
        )
        .await,
        Err(CoreError::Forbidden("edit_own"))
    ));
    assert_eq!(svc.get(own.id).await.unwrap(), published);
    assert_eq!(revision_count(store, own.id).await, revs);
    assert_eq!(max_seq(store).await, seq);

    // Someone else's draft: forbidden for contributors and authors.
    let theirs = svc.create(&author, draft("post", "Theirs")).await.unwrap();
    let seq = max_seq(store).await;
    for who in [&contributor, &author] {
        let other = actor(store, who.role).await;
        assert!(matches!(
            svc.update(&other, theirs.id, 1, draft("post", "Hacked"))
                .await,
            Err(CoreError::Forbidden("edit_others"))
        ));
    }
    assert_eq!(svc.get(theirs.id).await.unwrap(), theirs);
    assert_eq!(max_seq(store).await, seq);

    // Authors edit their own published items; editors edit anyone's.
    let theirs_pub = force_status(store, theirs.id, ContentStatus::Published).await;
    let theirs_pub = svc
        .update(
            &author,
            theirs.id,
            theirs_pub.version,
            draft("post", "Theirs 2"),
        )
        .await
        .unwrap();
    let edited = svc
        .update(
            &editor,
            theirs.id,
            theirs_pub.version,
            draft("post", "Edited"),
        )
        .await
        .unwrap();
    assert_eq!(edited.author_id, author.user);
    assert_eq!(edited.status, ContentStatus::Published);
}

/// Review Focus 3.
pub async fn slug_change_chain(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let mut d = draft("post", "A");
    d.slug = Some("a".into());
    let item = svc.create(&admin, d).await.unwrap();

    // Draft slug changes create no redirects.
    let mut d = draft("post", "A");
    d.slug = Some("draft-only".into());
    let item = svc.update(&admin, item.id, item.version, d).await.unwrap();
    let mut d = draft("post", "A");
    d.slug = Some("a".into());
    let item = svc.update(&admin, item.id, item.version, d).await.unwrap();
    assert_eq!(redirect_target(store, "/draft-only/").await, None);
    assert_eq!(redirect_target(store, "/a/").await, None);

    let mut item = force_status(store, item.id, ContentStatus::Published).await;
    // A manual redirect elsewhere pointing at the old path is re-targeted too.
    store
        .read()
        .redirects()
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: "/legacy".into(),
            target: "/a/".into(),
            code: 302,
            hits: 0,
            origin: RedirectOrigin::Manual,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();

    for slug in ["b", "c"] {
        let mut d = draft("post", "A");
        d.slug = Some(slug.into());
        item = svc.update(&admin, item.id, item.version, d).await.unwrap();
    }
    assert_eq!(redirect_target(store, "/a/").await.as_deref(), Some("/c/"));
    assert_eq!(redirect_target(store, "/b/").await.as_deref(), Some("/c/"));
    let legacy = store
        .read()
        .redirects()
        .get_by_source("/legacy")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((legacy.target.as_str(), legacy.code), ("/c/", 302));
    assert_eq!(legacy.origin, RedirectOrigin::Manual);
    let a = store
        .read()
        .redirects()
        .get_by_source("/a/")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((a.code, a.origin), (301, RedirectOrigin::SlugChange));

    // Back to `a`: the self-referencing redirect is deleted, the rest point at `a`.
    let mut d = draft("post", "A");
    d.slug = Some("a".into());
    item = svc.update(&admin, item.id, item.version, d).await.unwrap();
    assert_eq!(item.slug, "a");
    assert_eq!(redirect_target(store, "/a/").await, None);
    assert_eq!(redirect_target(store, "/b/").await.as_deref(), Some("/a/"));
    assert_eq!(redirect_target(store, "/c/").await.as_deref(), Some("/a/"));
    assert_eq!(
        redirect_target(store, "/legacy").await.as_deref(),
        Some("/a/")
    );

    // An unchanged permalink touches no redirects.
    let before = store
        .read()
        .redirects()
        .list(&RedirectQuery {
            limit: 200,
            ..RedirectQuery::default()
        })
        .await
        .unwrap()
        .items;
    let mut d = draft("post", "Retitled");
    d.slug = Some("a".into());
    svc.update(&admin, item.id, item.version, d).await.unwrap();
    assert_eq!(
        store
            .read()
            .redirects()
            .list(&RedirectQuery {
                limit: 200,
                ..RedirectQuery::default()
            })
            .await
            .unwrap()
            .items,
        before
    );
}

/// Review Focus 1: parallel creates with the same title all succeed with distinct slugs.
pub async fn parallel_same_title(db: Arc<TestDb>) {
    let store = db.shared_store();
    let svc = Arc::new(ContentService::new(store.clone(), permalink()));
    let admin = actor(store.as_ref(), Role::Admin).await;
    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let svc = svc.clone();
            tokio::spawn(async move { svc.create(&admin, draft("post", "Hello World")).await })
        })
        .collect();
    let mut slugs = Vec::new();
    for t in tasks {
        slugs.push(t.await.unwrap().unwrap().slug);
    }
    assert_eq!(
        sorted(slugs),
        [
            "hello-world",
            "hello-world-2",
            "hello-world-3",
            "hello-world-4"
        ]
    );
}

/// Review Focus 1, deterministic: a competing transaction holds `race` uncommitted while
/// the service allocates the same slug. On Postgres the service's insert waits on the
/// unique index and fails once the rival commits, so it must retry; on SQLite the
/// service waits for the writer and then sees the committed row.
pub async fn slug_retry_after_unique_violation(db: Arc<TestDb>) {
    let store = db.shared_store();
    let svc = Arc::new(ContentService::new(store.clone(), permalink()));
    let admin = actor(store.as_ref(), Role::Admin).await;
    let author = store.read().users().get(admin.user).await.unwrap().unwrap();

    let rival = store.begin().await.unwrap();
    rival
        .content()
        .insert(&new_post(&author, "race"))
        .await
        .unwrap();
    let task = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.create(&admin, draft("post", "Race")).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    rival.commit().await.unwrap();
    let item = task.await.unwrap().unwrap();
    assert_eq!(item.slug, "race-2");
}

pub async fn non_slug_store_errors_surface(db: &TestDb) {
    // A store error other than a slug race surfaces unchanged (unknown author: FK).
    let store = db.store();
    let svc = service(db);
    let ghost = Actor::new(UserId::new(), Role::Admin);
    assert!(matches!(
        svc.create(&ghost, draft("post", "Ghost")).await,
        Err(CoreError::Store(StoreError::ForeignKey { .. }))
    ));
    assert!(search(store, "ghost").await.is_empty());
}

fn invalid(r: Result<impl std::fmt::Debug, CoreError>) -> String {
    match r {
        Err(CoreError::Invalid(m) | CoreError::InvalidField { message: m, .. }) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

async fn manual_redirect(store: &dyn Store, source: &str, target: &str) {
    store
        .read()
        .redirects()
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: source.into(),
            target: target.into(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::Manual,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();
}

/// I1: parents only for hierarchical types, of the same type, not trashed; checked on
/// create and when an update changes the parent.
pub async fn parent_rules(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let post = svc.create(&admin, draft("post", "Post")).await.unwrap();
    let other_post = svc.create(&admin, draft("post", "Other")).await.unwrap();
    let page = svc.create(&admin, draft("page", "Page")).await.unwrap();
    let trashed = svc.create(&admin, draft("page", "Gone")).await.unwrap();
    force_status(store, trashed.id, ContentStatus::Trash).await;
    let seq = max_seq(store).await;

    // Non-hierarchical type: no parent at all.
    let mut d = draft("post", "Child");
    d.parent_id = Some(post.id);
    invalid(svc.create(&admin, d.clone()).await);
    d.title = "Other".into();
    invalid(svc.update(&admin, other_post.id, 1, d).await);
    // Hierarchical type: the parent must have the same type ...
    let mut d = draft("page", "Child");
    d.parent_id = Some(post.id);
    invalid(svc.create(&admin, d.clone()).await);
    invalid(svc.update(&admin, page.id, 1, d).await);
    // ... and must not be trashed.
    let mut d = draft("page", "Child");
    d.parent_id = Some(trashed.id);
    invalid(svc.create(&admin, d.clone()).await);
    invalid(svc.update(&admin, page.id, 1, d).await);
    assert_eq!(max_seq(store).await, seq);

    let mut d = draft("page", "Child");
    d.parent_id = Some(page.id);
    let child = svc.create(&admin, d.clone()).await.unwrap();
    assert_eq!(child.parent_id, Some(page.id));
    // Checked on parent change only: a parent trashed later does not block edits.
    force_status(store, page.id, ContentStatus::Trash).await;
    d.title = "Child 2".into();
    svc.update(&admin, child.id, child.version, d)
        .await
        .unwrap();
}

/// I2: publishing an item removes redirects that still claim its URL (both slash
/// variants), on `publish` and on `publish_due`.
pub async fn publish_reclaims_redirected_path(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let redirects = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let old = svc.create(&admin, draft("post", "Hello")).await.unwrap();
    let old = svc.publish(&admin, old.id, old.version).await.unwrap();
    let mut d = draft("post", "Hello");
    d.slug = Some("hello-world".into());
    svc.update(&admin, old.id, old.version, d).await.unwrap();
    assert_eq!(
        redirect_target(store, "/hello/").await.as_deref(),
        Some("/hello-world/")
    );
    manual_redirect(store, "/hello", "/elsewhere").await;

    let new = svc.create(&admin, draft("post", "Hello")).await.unwrap();
    assert_eq!(new.slug, "hello");
    // Still a draft: the redirects stay.
    assert!(redirects.resolve("/hello/").await.unwrap().is_some());
    svc.publish(&admin, new.id, new.version).await.unwrap();
    assert_eq!(redirects.resolve("/hello/").await.unwrap(), None);
    assert_eq!(redirects.resolve("/hello").await.unwrap(), None);

    // publish_due does the same.
    let later = svc.create(&admin, draft("post", "Later")).await.unwrap();
    manual_redirect(store, "/later/", "/x").await;
    manual_redirect(store, "/later", "/y").await;
    let mut it = store.read().content().get(later.id).await.unwrap().unwrap();
    it.status = ContentStatus::Scheduled;
    it.scheduled_at =
        Some(Timestamp::from_micros(Timestamp::now().as_micros() - 1_000_000).unwrap());
    store.read().content().update(&it).await.unwrap();
    assert_eq!(svc.publish_due(Timestamp::now()).await.unwrap(), 1);
    assert_eq!(redirects.resolve("/later/").await.unwrap(), None);
    assert_eq!(redirects.resolve("/later").await.unwrap(), None);
}

/// M1: a manual `/b -> /a/` plus a move of `/a/` to `/b/` must leave no redirect from
/// `/b` or `/b/` (it would loop through the slash toggle).
pub async fn move_onto_slash_variant_leaves_no_loop(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let redirects = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let mut d = draft("post", "A");
    d.slug = Some("a".into());
    let item = svc.create(&admin, d).await.unwrap();
    let item = svc.publish(&admin, item.id, item.version).await.unwrap();
    redirects.upsert(&admin, "/b", "/a/", 301).await.unwrap();

    let mut d = draft("post", "A");
    d.slug = Some("b".into());
    svc.update(&admin, item.id, item.version, d).await.unwrap();
    assert_eq!(redirect_target(store, "/b").await, None);
    assert_eq!(redirect_target(store, "/b/").await, None);
    assert_eq!(redirects.resolve("/b/").await.unwrap(), None);
    assert_eq!(redirect_target(store, "/a/").await.as_deref(), Some("/b/"));
}

/// M4: `.`/`..` are never slugs; reserved words are rejected for top-level items and
/// generated slugs step around them.
pub async fn reserved_and_dot_slugs(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    for s in [
        "admin",
        "API",
        "sitemap.xml",
        "_p",
        "healthz",
        "_preview",
        "_Preview",
    ] {
        let mut d = draft("post", "X");
        d.slug = Some(s.into());
        assert_eq!(invalid(svc.create(&admin, d).await), "reserved slug", "{s}");
    }
    for s in [".", ".."] {
        let mut d = draft("post", "X");
        d.slug = Some(s.into());
        invalid(svc.create(&admin, d).await);
    }
    assert_eq!(
        svc.create(&admin, draft("post", "Admin"))
            .await
            .unwrap()
            .slug,
        "admin-2"
    );
    assert_eq!(
        svc.create(&admin, draft("post", "Feed"))
            .await
            .unwrap()
            .slug,
        "feed-2"
    );

    // Under a parent the words are fine.
    let parent = svc.create(&admin, draft("page", "Parent")).await.unwrap();
    let mut d = draft("page", "Admin");
    d.parent_id = Some(parent.id);
    let child = svc.create(&admin, d).await.unwrap();
    assert_eq!(child.slug, "admin");
    // Moving it to the top level: an explicit reserved slug is Invalid, a kept one
    // steps around it.
    let mut d = draft("page", "Admin");
    d.slug = Some("admin".into());
    assert_eq!(
        invalid(svc.update(&admin, child.id, child.version, d).await),
        "reserved slug"
    );
    let moved = svc
        .update(&admin, child.id, child.version, draft("page", "Admin"))
        .await
        .unwrap();
    // `admin-2` is the post above: posts and top-level pages share the root namespace.
    assert_eq!((moved.parent_id, moved.slug.as_str()), (None, "admin-3"));
    let mut d = draft("page", "Admin");
    d.slug = Some("metrics".into());
    assert_eq!(
        invalid(svc.update(&admin, moved.id, moved.version, d).await),
        "reserved slug"
    );
}

/// I4: a trashed item must be restored before it is edited (no re-indexing of trash).
pub async fn trashed_item_cannot_be_updated(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let item = svc.create(&admin, draft("post", "Bin")).await.unwrap();
    let item = svc.trash(&admin, item.id, item.version).await.unwrap();
    assert!(search(store, "zebra").await.is_empty());
    let (seq, revs) = (max_seq(store).await, revision_count(store, item.id).await);
    assert_eq!(
        invalid(
            svc.update(&admin, item.id, item.version, draft("post", "Bin 2"))
                .await
        ),
        "restore first"
    );
    assert_eq!(svc.get(item.id).await.unwrap(), item);
    assert!(search(store, "zebra").await.is_empty());
    assert_eq!(max_seq(store).await, seq);
    assert_eq!(revision_count(store, item.id).await, revs);
}

/// Rules that change under a running write: the first `path` call answers with the
/// default rules (`/%slug%/`), every later one with `/moved/%slug%/`. `pin` returns a
/// snapshot of the default rules, as `SwappablePermalinks` would before a swap.
struct SwapsMidWrite {
    calls: std::sync::atomic::AtomicUsize,
    before: PermalinkRules,
    after: PermalinkRules,
}

impl Permalinks for SwapsMidWrite {
    fn path(&self, input: &PermalinkInput) -> String {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            self.before.path(input)
        } else {
            self.after.path(input)
        }
    }

    fn pin(&self) -> Arc<dyn Permalinks> {
        Arc::new(self.before.clone())
    }

    fn root_namespace_peers(&self, kind: &str) -> Vec<String> {
        self.before.root_namespace_peers(kind)
    }
}

pub async fn update_pins_permalinks_once(db: &TestDb) {
    // R10: a write computes the old and the new path from one pinned rule set, so a
    // swap during the write cannot invent a move.
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let setup = service(db);
    let item = setup.create(&admin, draft("post", "Pinned")).await.unwrap();
    let item = setup.publish(&admin, item.id, item.version).await.unwrap();

    let swapping = Arc::new(SwapsMidWrite {
        calls: Default::default(),
        before: PermalinkRules::default(),
        after: PermalinkRules::from_options(Some("/moved/%slug%/"), None).unwrap(),
    });
    let svc = ContentService::new(db.shared_store(), swapping.clone());
    let mut d = draft("post", "Pinned, retitled");
    d.slug = Some(item.slug.clone());
    let saved = svc.update(&admin, item.id, item.version, d).await.unwrap();
    assert_eq!(saved.title, "Pinned, retitled");
    assert_eq!(saved.slug, item.slug);
    let redirects = store
        .read()
        .redirects()
        .list(&RedirectQuery {
            limit: 100,
            ..RedirectQuery::default()
        })
        .await
        .unwrap();
    assert!(redirects.items.is_empty(), "{:?}", redirects.items);
    assert_eq!(
        swapping.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the write used the unpinned rules"
    );
}

/// A child may not take `page` (`/{path}/page/{n}/` is a listing page): a caller slug
/// is refused, a generated one steps around it. At the top level it is a route word.
pub async fn child_slug_page_is_reserved(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let parent = svc.create(&admin, draft("page", "Parent")).await.unwrap();
    for s in ["page", "Page"] {
        let mut d = draft("page", "Child");
        d.parent_id = Some(parent.id);
        d.slug = Some(s.into());
        assert_eq!(invalid(svc.create(&admin, d).await), "reserved slug", "{s}");
    }
    let mut d = draft("page", "Page");
    d.parent_id = Some(parent.id);
    let child = svc.create(&admin, d).await.unwrap();
    assert_eq!(child.slug, "page-2");
    // Renaming a child onto it is refused too; other words stay fine below the top.
    let mut d = draft("page", "Page");
    d.parent_id = Some(parent.id);
    d.slug = Some("page".into());
    assert_eq!(
        invalid(svc.update(&admin, child.id, child.version, d).await),
        "reserved slug"
    );
    let mut d = draft("page", "Feed");
    d.parent_id = Some(parent.id);
    d.slug = Some("feed".into());
    assert_eq!(svc.create(&admin, d).await.unwrap().slug, "feed");
    // Top level: `page` is a router word.
    let mut d = draft("page", "Page");
    d.slug = Some("page".into());
    assert_eq!(invalid(svc.create(&admin, d).await), "reserved slug");
}

/// An update that keeps slug and parent never renames, even a slug a later rule
/// reserves (rows written before the rule, or through the store).
pub async fn legacy_reserved_slug_survives_update(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let author = store.read().users().get(admin.user).await.unwrap().unwrap();
    let legacy = store
        .read()
        .content()
        .insert(&new_post(&author, "feed"))
        .await
        .unwrap();
    let saved = svc
        .update(&admin, legacy.id, legacy.version, draft("post", "Retitled"))
        .await
        .unwrap();
    assert_eq!(
        (saved.title.as_str(), saved.slug.as_str()),
        ("Retitled", "feed")
    );
    // Naming the kept slug explicitly is fine as well.
    let mut d = draft("post", "Retitled again");
    d.slug = Some("feed".into());
    let saved = svc
        .update(&admin, saved.id, saved.version, d)
        .await
        .unwrap();
    assert_eq!(saved.slug, "feed");

    // A legacy child slugged `page` keeps it under the same parent.
    let parent = svc.create(&admin, draft("page", "Parent")).await.unwrap();
    let mut row = new_post(&author, "page");
    row.kind = "page".into();
    row.parent_id = Some(parent.id);
    let child = store.read().content().insert(&row).await.unwrap();
    let mut d = draft("page", "Child retitled");
    d.parent_id = Some(parent.id);
    let saved = svc
        .update(&admin, child.id, child.version, d)
        .await
        .unwrap();
    assert_eq!(saved.slug, "page");
    // Moving it elsewhere applies the rules again.
    let other = svc.create(&admin, draft("page", "Other")).await.unwrap();
    let mut d = draft("page", "Child moved");
    d.parent_id = Some(other.id);
    let moved = svc
        .update(&admin, saved.id, saved.version, d)
        .await
        .unwrap();
    assert_eq!(moved.slug, "page-2");
}

/// The kind check and the root namespace share one `types().list()` per write
/// (create goes through `create_in`; transactions are counted too).
pub async fn top_level_write_lists_types_once(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let counting = Arc::new(CountingStore::new(db.shared_store()));
    let svc = ContentService::new(counting.clone(), permalink());

    counting.reset();
    let item = svc.create(&admin, draft("post", "Counted")).await.unwrap();
    assert_eq!(
        (counting.type_lists(), counting.type_reads()),
        (1, 1),
        "create"
    );

    counting.reset();
    let mut d = draft("post", "Counted");
    d.slug = Some("renamed".into());
    let item = svc.update(&admin, item.id, item.version, d).await.unwrap();
    assert_eq!(item.slug, "renamed");
    assert_eq!(
        (counting.type_lists(), counting.type_reads()),
        (1, 1),
        "update with a new slug"
    );

    counting.reset();
    let item = svc
        .update(&admin, item.id, item.version, draft("post", "Kept"))
        .await
        .unwrap();
    assert_eq!(item.slug, "renamed");
    assert_eq!(
        (counting.type_lists(), counting.type_reads()),
        (1, 1),
        "update keeping the slug"
    );

    let parent = svc.create(&admin, draft("page", "Parent")).await.unwrap();
    counting.reset();
    let mut d = draft("page", "Child");
    d.parent_id = Some(parent.id);
    svc.create(&admin, d).await.unwrap();
    assert_eq!(
        (counting.type_lists(), counting.type_reads()),
        (1, 1),
        "child create"
    );
}

backend_tests!(
    update_pins_permalinks_once,
    create_update_roundtrip,
    explicit_empty_excerpt_is_kept,
    revisions_pruned_to_50,
    parent_change_and_validation,
    unknown_type_is_invalid,
    version_conflict,
    contributor_cannot_escalate,
    slug_change_chain,
    non_slug_store_errors_surface,
    parent_rules,
    publish_reclaims_redirected_path,
    move_onto_slash_variant_leaves_no_loop,
    reserved_and_dot_slugs,
    trashed_item_cannot_be_updated,
    child_slug_page_is_reserved,
    legacy_reserved_slug_survives_update,
    top_level_write_lists_types_once,
);

backend_tests_spawning!(parallel_same_title, slug_retry_after_unique_violation);
