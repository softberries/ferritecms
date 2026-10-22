//! Permalinks through ContentService on both backends: each structure's path,
//! descendant redirects, category-change redirects and percent-encoding.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{Actor, ContentDraft, ContentService, PermalinkRules};
use ferrite_domain::{
    ContentId, ContentStatus, NewContent, NewUser, Role, Term, TermId, Timestamp, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
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
    Actor { user: id, role }
}

fn service(db: &TestDb, rules: PermalinkRules) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(rules))
}

fn draft(kind: &str, slug: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: slug.into(),
        slug: Some(slug.into()),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
    }
}

fn child(kind: &str, slug: &str, parent: ContentId) -> ContentDraft {
    let mut d = draft(kind, slug);
    d.parent_id = Some(parent);
    d
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

async fn redirect_target(store: &dyn Store, source: &str) -> Option<String> {
    store
        .read()
        .redirects()
        .get_by_source(source)
        .await
        .unwrap()
        .map(|r| r.target)
}

async fn redirect_count(store: &dyn Store) -> usize {
    let mut n = 0;
    let mut cursor: Option<String> = None;
    loop {
        let page = store
            .read()
            .redirects()
            .list(200, cursor.as_deref())
            .await
            .unwrap();
        n += page.items.len();
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => return n,
        }
    }
}

async fn create_published(svc: &ContentService, admin: &Actor, d: ContentDraft) -> ContentId {
    let item = svc.create(admin, d).await.unwrap();
    svc.publish(admin, item.id, item.version).await.unwrap().id
}

async fn edit(svc: &ContentService, admin: &Actor, id: ContentId, d: ContentDraft) {
    let item = svc.get(id).await.unwrap();
    svc.update(admin, id, item.version, d).await.unwrap();
}

/// Default rules: posts `/%slug%/`, pages `/%parent_path%/%slug%/`, other types
/// `/{type}/%slug%/`; date and category structures for posts.
pub async fn structure_paths(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = service(db, PermalinkRules::default());
    let news = term(store, "category", "news").await;
    let sport = term(store, "category", "sport").await;
    let tag = term(store, "tag", "aaa").await;

    let mut d = draft("post", "hello");
    // The primary term is the first *category* by position; tags do not count.
    d.terms = vec![tag, sport, news];
    let post = create_published(&svc, &admin, d).await;
    let bare = svc.create(&admin, draft("post", "bare")).await.unwrap();
    let top = create_published(&svc, &admin, draft("page", "about")).await;
    let mid = create_published(&svc, &admin, child("page", "team", top)).await;
    let leaf = create_published(&svc, &admin, child("page", "lead", mid)).await;
    let media = svc.create(&admin, draft("media", "logo")).await.unwrap();

    assert_eq!(svc.permalink(post).await.unwrap(), "/hello/");
    assert_eq!(svc.permalink(top).await.unwrap(), "/about/");
    assert_eq!(svc.permalink(mid).await.unwrap(), "/about/team/");
    assert_eq!(svc.permalink(leaf).await.unwrap(), "/about/team/lead/");
    assert_eq!(svc.permalink(media.id).await.unwrap(), "/media/logo/");
    assert!(matches!(
        svc.permalink(ContentId::new()).await,
        Err(ferrite_core::CoreError::NotFound)
    ));

    let published = svc.get(post).await.unwrap().published_at.unwrap();
    let dt = published.as_datetime();
    let dated = service(
        db,
        PermalinkRules::from_options(Some("/%year%/%month%/%slug%/"), None).unwrap(),
    );
    assert_eq!(
        dated.permalink(post).await.unwrap(),
        format!("/{}/{}/hello/", dt.format("%Y"), dt.format("%m"))
    );
    // Unpublished items use created_at.
    let created = bare.created_at.as_datetime();
    assert_eq!(
        dated.permalink(bare.id).await.unwrap(),
        format!("/{}/{}/bare/", created.format("%Y"), created.format("%m"))
    );

    let by_cat = service(
        db,
        PermalinkRules::from_options(Some("/%category%/%slug%/"), Some("/p/%slug%/")).unwrap(),
    );
    assert_eq!(by_cat.permalink(post).await.unwrap(), "/sport/hello/");
    assert_eq!(
        by_cat.permalink(bare.id).await.unwrap(),
        "/uncategorized/bare/"
    );
    assert_eq!(by_cat.permalink(leaf).await.unwrap(), "/p/lead/");
}

/// Unicode slugs are percent-encoded per segment (UTF-8, uppercase hex).
pub async fn unicode_slugs_are_percent_encoded(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = service(db, PermalinkRules::default());
    let top = create_published(&svc, &admin, draft("page", "zażółć")).await;
    let sub = create_published(&svc, &admin, child("page", "日本", top)).await;
    let amp = create_published(&svc, &admin, draft("post", "a&b+c.d_e~f")).await;
    assert_eq!(
        svc.permalink(top).await.unwrap(),
        "/za%C5%BC%C3%B3%C5%82%C4%87/"
    );
    assert_eq!(
        svc.permalink(sub).await.unwrap(),
        "/za%C5%BC%C3%B3%C5%82%C4%87/%E6%97%A5%E6%9C%AC/"
    );
    assert_eq!(svc.permalink(amp).await.unwrap(), "/a%26b%2Bc.d_e~f/");

    // Redirects store the encoded paths.
    let mut d = draft("page", "źdźbło");
    d.title = "x".into();
    edit(&svc, &admin, top, d).await;
    assert_eq!(
        redirect_target(store, "/za%C5%BC%C3%B3%C5%82%C4%87/%E6%97%A5%E6%9C%AC/")
            .await
            .as_deref(),
        Some("/%C5%BAd%C5%BAb%C5%82o/%E6%97%A5%E6%9C%AC/")
    );
}

/// Renaming or moving a published page records a redirect for every published
/// descendant; drafts (and what is below them) get none, and unchanged paths none.
pub async fn descendant_redirects(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = service(db, PermalinkRules::default());
    let a = create_published(&svc, &admin, draft("page", "a")).await;
    let b = create_published(&svc, &admin, child("page", "b", a)).await;
    let c = create_published(&svc, &admin, child("page", "c", b)).await;
    let d = svc.create(&admin, child("page", "d", a)).await.unwrap();
    let other = create_published(&svc, &admin, draft("page", "other")).await;

    edit(&svc, &admin, a, draft("page", "a2")).await;
    assert_eq!(redirect_target(store, "/a/").await.as_deref(), Some("/a2/"));
    assert_eq!(
        redirect_target(store, "/a/b/").await.as_deref(),
        Some("/a2/b/")
    );
    assert_eq!(
        redirect_target(store, "/a/b/c/").await.as_deref(),
        Some("/a2/b/c/")
    );
    assert_eq!(redirect_target(store, "/a/d/").await, None);
    assert_eq!(redirect_count(store).await, 3);
    assert_eq!(svc.permalink(c).await.unwrap(), "/a2/b/c/");
    assert_eq!(svc.permalink(d.id).await.unwrap(), "/a2/d/");

    // Moving `b` under `other`: `b` and `c` move, earlier redirects are re-targeted.
    edit(&svc, &admin, b, child("page", "b", other)).await;
    assert_eq!(
        redirect_target(store, "/a2/b/c/").await.as_deref(),
        Some("/other/b/c/")
    );
    assert_eq!(
        redirect_target(store, "/a/b/c/").await.as_deref(),
        Some("/other/b/c/")
    );
    assert_eq!(
        redirect_target(store, "/a2/b/").await.as_deref(),
        Some("/other/b/")
    );
    assert_eq!(redirect_count(store).await, 5);

    // A title-only edit of a page with descendants records nothing.
    let mut same = child("page", "b", other);
    same.title = "Retitled".into();
    edit(&svc, &admin, b, same).await;
    assert_eq!(redirect_count(store).await, 5);

    // A draft parent's rename records nothing, not even for its published children.
    let draft_parent = svc.create(&admin, draft("page", "dp")).await.unwrap();
    let _kid = create_published(&svc, &admin, child("page", "kid", draft_parent.id)).await;
    edit(&svc, &admin, draft_parent.id, draft("page", "dp2")).await;
    assert_eq!(redirect_count(store).await, 5);
}

/// At most 1000 descendants are walked per move.
pub async fn descendant_walk_is_capped(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = service(db, PermalinkRules::default());
    let root = create_published(&svc, &admin, draft("page", "root")).await;
    let tx = store.begin().await.unwrap();
    for i in 0..1005 {
        tx.content()
            .insert(&NewContent {
                id: ContentId::new(),
                kind: "page".into(),
                status: ContentStatus::Published,
                slug: format!("k{i}"),
                parent_id: Some(root),
                author_id: admin.user,
                title: format!("k{i}"),
                excerpt: String::new(),
                body: json!({}),
                body_html: None,
                fields: json!({}),
                menu_order: 0,
                published_at: Some(Timestamp::now()),
                scheduled_at: None,
            })
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    edit(&svc, &admin, root, draft("page", "root2")).await;
    // The root itself plus 1000 descendants.
    assert_eq!(redirect_count(store).await, 1001);
}

/// Under `/%category%/%slug%/` a change of a published post's primary category moves
/// it; other term changes and the default structure record nothing.
pub async fn category_change_redirect(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = service(
        db,
        PermalinkRules::from_options(Some("/%category%/%slug%/"), None).unwrap(),
    );
    let news = term(store, "category", "news").await;
    let sport = term(store, "category", "sport").await;
    let tag = term(store, "tag", "t").await;

    let mut d = draft("post", "p");
    d.terms = vec![news];
    let post = create_published(&svc, &admin, d).await;
    assert_eq!(svc.permalink(post).await.unwrap(), "/news/p/");

    // Adding a tag or a second category keeps the primary term.
    let mut d = draft("post", "p");
    d.terms = vec![tag, news, sport];
    edit(&svc, &admin, post, d).await;
    assert_eq!(redirect_count(store).await, 0);

    let mut d = draft("post", "p");
    d.terms = vec![sport, news];
    edit(&svc, &admin, post, d).await;
    assert_eq!(
        redirect_target(store, "/news/p/").await.as_deref(),
        Some("/sport/p/")
    );
    // No category at all: uncategorized.
    edit(&svc, &admin, post, draft("post", "p")).await;
    assert_eq!(
        redirect_target(store, "/sport/p/").await.as_deref(),
        Some("/uncategorized/p/")
    );
    assert_eq!(
        redirect_target(store, "/news/p/").await.as_deref(),
        Some("/uncategorized/p/")
    );

    // Default structure: a category change is not a move.
    let plain = service(db, PermalinkRules::default());
    let mut d = draft("post", "q");
    d.terms = vec![news];
    let q = create_published(&plain, &admin, d).await;
    let before = redirect_count(store).await;
    let mut d = draft("post", "q");
    d.terms = vec![sport];
    edit(&plain, &admin, q, d).await;
    assert_eq!(redirect_count(store).await, before);
}

backend_tests!(
    structure_paths,
    unicode_slugs_are_percent_encoded,
    descendant_redirects,
    descendant_walk_is_capped,
    category_change_redirect,
);
