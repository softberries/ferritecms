//! Taxonomy, menu, option and redirect services on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use ferrite_core::{
    Actor, CoreError, MenuService, OptionService, RedirectService, TaxonomyService,
    events::{archive_key, menu_key, option_key, term_key},
};
use ferrite_domain::{
    ContentStatus, Menu, MenuItem, MenuItemId, MenuItemKind, NewUser, Role, Term, TermId, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests, backend_tests_spawning, fixtures::new_post};
use serde_json::json;
use uuid::Uuid;

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

async fn max_seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

/// Kind and keys of the newest event.
async fn last_event(store: &dyn Store) -> (String, Vec<String>) {
    let events = store.read().events().since(0, 1000).await.unwrap();
    let last = events.last().unwrap();
    (last.kind.clone(), last.keys.clone())
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn is_forbidden<T: std::fmt::Debug>(r: Result<T, CoreError>, cap: &str) -> bool {
    matches!(r, Err(CoreError::Forbidden(c)) if c == cap)
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, CoreError>) -> bool {
    matches!(
        r,
        Err(CoreError::Invalid(_) | CoreError::InvalidField { .. })
    )
}

// ---------------------------------------------------------------- taxonomies

pub async fn term_crud_and_events(db: &TestDb) {
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;

    let news = svc
        .create_term(&admin, "category", "  News ", None, None, "d")
        .await
        .unwrap();
    assert_eq!((news.slug.as_str(), news.name.as_str()), ("news", "News"));
    assert_eq!(
        last_event(store).await,
        (
            "invalidate".to_owned(),
            vec![term_key(news.id), archive_key("post")]
        )
    );
    // Same name again: -2; caller slug is normalised and also made unique.
    let news2 = svc
        .create_term(&admin, "category", "News", None, None, "")
        .await
        .unwrap();
    assert_eq!(news2.slug, "news-2");
    let custom = svc
        .create_term(&admin, "category", "X", Some(" NEWS ".into()), None, "")
        .await
        .unwrap();
    assert_eq!(custom.slug, "news-3");
    // The same slug in another taxonomy is free.
    let tag = svc
        .create_term(&admin, "tag", "News", None, None, "")
        .await
        .unwrap();
    assert_eq!(tag.slug, "news");

    // Invalid input.
    assert!(matches!(
        svc.create_term(&admin, "category", "  ", None, None, "")
            .await,
        Err(CoreError::InvalidField { field: "name", .. })
    ));
    assert!(is_invalid(
        svc.create_term(&admin, "category", "A", Some("a/b".into()), None, "")
            .await
    ));
    // An unknown taxonomy is NotFound (`POST /taxonomies/{taxonomy}/terms` is a 404).
    assert!(matches!(
        svc.create_term(&admin, "nope", "A", None, None, "").await,
        Err(CoreError::NotFound)
    ));

    // Update: slug kept when None, own slug is not a conflict, a taken slug gets -N.
    let kept = svc
        .update_term(&admin, news.id, "Newer", None, None, "x")
        .await
        .unwrap();
    assert_eq!(
        (
            kept.slug.as_str(),
            kept.name.as_str(),
            kept.description.as_str()
        ),
        ("news", "Newer", "x")
    );
    let same = svc
        .update_term(&admin, news.id, "Newer", Some("news".into()), None, "x")
        .await
        .unwrap();
    assert_eq!(same.slug, "news");
    let moved = svc
        .update_term(&admin, news2.id, "N", Some("news".into()), None, "")
        .await
        .unwrap();
    assert_eq!(moved.slug, "news-2"); // its own slug is not a conflict
    let other = svc
        .create_term(&admin, "category", "Other", None, None, "")
        .await
        .unwrap();
    let taken = svc
        .update_term(&admin, other.id, "Other", Some("news".into()), None, "")
        .await
        .unwrap();
    assert_eq!(taken.slug, "news-4");
    assert_eq!(
        sorted(last_event(store).await.1),
        sorted(vec![term_key(other.id), archive_key("post")])
    );
    svc.delete_term(&admin, other.id).await.unwrap();
    assert!(matches!(
        svc.update_term(&admin, TermId::new(), "N", None, None, "")
            .await,
        Err(CoreError::NotFound)
    ));

    let listed = svc.list_terms("category").await.unwrap();
    assert_eq!(listed.len(), 3);
    assert!(matches!(
        svc.list_terms("nope").await,
        Err(CoreError::NotFound)
    ));

    // Delete.
    svc.delete_term(&admin, custom.id).await.unwrap();
    assert_eq!(
        last_event(store).await.1,
        vec![term_key(custom.id), archive_key("post")]
    );
    assert!(store.read().terms().get(custom.id).await.unwrap().is_none());
    assert!(matches!(
        svc.delete_term(&admin, custom.id).await,
        Err(CoreError::NotFound)
    ));
}

pub async fn term_permissions(db: &TestDb) {
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let editor = actor(store, Role::Editor).await;
    let term = svc
        .create_term(&editor, "tag", "Ok", None, None, "")
        .await
        .unwrap();
    for role in [Role::Author, Role::Contributor] {
        let who = actor(store, role).await;
        let seq = max_seq(store).await;
        assert!(is_forbidden(
            svc.create_term(&who, "tag", "No", None, None, "").await,
            "manage_terms"
        ));
        assert!(is_forbidden(
            svc.update_term(&who, term.id, "No", None, None, "").await,
            "manage_terms"
        ));
        assert!(is_forbidden(
            svc.delete_term(&who, term.id).await,
            "manage_terms"
        ));
        assert_eq!(max_seq(store).await, seq);
    }
    assert_eq!(store.read().terms().list("tag").await.unwrap().len(), 1);
    svc.delete_term(&admin, term.id).await.unwrap();
}

pub async fn term_parent_rules(db: &TestDb) {
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let a = svc
        .create_term(&admin, "category", "A", None, None, "")
        .await
        .unwrap();
    let b = svc
        .create_term(&admin, "category", "B", None, Some(a.id), "")
        .await
        .unwrap();
    let c = svc
        .create_term(&admin, "category", "C", None, Some(b.id), "")
        .await
        .unwrap();
    assert_eq!(c.parent_id, Some(b.id));

    // Cycles: self, child, grandchild.
    for parent in [a.id, b.id, c.id] {
        assert!(is_invalid(
            svc.update_term(&admin, a.id, "A", None, Some(parent), "")
                .await
        ));
    }
    assert_eq!(
        store
            .read()
            .terms()
            .get(a.id)
            .await
            .unwrap()
            .unwrap()
            .parent_id,
        None
    );
    // Unknown parent, parent in another taxonomy: errors on `parent_id`.
    assert!(matches!(
        svc.create_term(&admin, "category", "Z", None, Some(TermId::new()), "")
            .await,
        Err(CoreError::InvalidField {
            field: "parent_id",
            ..
        })
    ));
    let tag = svc
        .create_term(&admin, "tag", "T", None, None, "")
        .await
        .unwrap();
    assert!(is_invalid(
        svc.create_term(&admin, "category", "Z", None, Some(tag.id), "")
            .await
    ));
    assert!(is_invalid(
        svc.update_term(&admin, c.id, "C", None, Some(tag.id), "")
            .await
    ));
    // Moving to a valid parent works.
    let moved = svc
        .update_term(&admin, c.id, "C", None, Some(a.id), "")
        .await
        .unwrap();
    assert_eq!(moved.parent_id, Some(a.id));

    // Delete: children block it, nothing is written.
    let seq = max_seq(store).await;
    assert!(matches!(
        svc.delete_term(&admin, a.id).await,
        Err(CoreError::Invalid(m)) if m == "has children"
    ));
    assert_eq!(max_seq(store).await, seq);
    svc.delete_term(&admin, c.id).await.unwrap();
    svc.delete_term(&admin, b.id).await.unwrap();
    svc.delete_term(&admin, a.id).await.unwrap();

    // Depth limit: a chain of 70 terms cannot get a new descendant-side parent walk.
    let mut parent = None;
    let mut leaf = TermId::new();
    for i in 0..70 {
        leaf = TermId::new();
        store
            .read()
            .terms()
            .insert(&Term {
                id: leaf,
                taxonomy: "category".into(),
                slug: format!("deep-{i}"),
                name: format!("deep-{i}"),
                description: String::new(),
                parent_id: parent,
            })
            .await
            .unwrap();
        parent = Some(leaf);
    }
    let x = svc
        .create_term(&admin, "category", "X", None, None, "")
        .await
        .unwrap();
    assert!(matches!(
        svc.update_term(&admin, x.id, "X", None, Some(leaf), "").await,
        Err(CoreError::InvalidField { field: "parent_id", message: m }) if m.contains("deeper")
    ));
}

pub async fn flat_taxonomy_rejects_parents(db: &TestDb) {
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let a = svc
        .create_term(&admin, "tag", "A", None, None, "")
        .await
        .unwrap();
    let seq = max_seq(store).await;
    assert!(is_invalid(
        svc.create_term(&admin, "tag", "B", None, Some(a.id), "")
            .await
    ));
    let b = svc
        .create_term(&admin, "tag", "B", None, None, "")
        .await
        .unwrap();
    assert!(is_invalid(
        svc.update_term(&admin, b.id, "B", None, Some(a.id), "")
            .await
    ));
    assert_eq!(
        store
            .read()
            .terms()
            .get(b.id)
            .await
            .unwrap()
            .unwrap()
            .parent_id,
        None
    );
    assert_eq!(max_seq(store).await, seq + 1); // only B's creation
}

pub async fn term_in_use_deletes_links(db: &TestDb) {
    // Deleting a term that content items use succeeds and unlinks them (cascade).
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let user = store.read().users().get(admin.user).await.unwrap().unwrap();
    let item = store
        .read()
        .content()
        .insert(&new_post(&user, "linked"))
        .await
        .unwrap();
    let t = svc
        .create_term(&admin, "tag", "L", None, None, "")
        .await
        .unwrap();
    store
        .read()
        .terms()
        .set_for_item(item.id, &[t.id])
        .await
        .unwrap();
    svc.delete_term(&admin, t.id).await.unwrap();
    assert!(
        store
            .read()
            .terms()
            .for_item(item.id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A competing transaction holds the slug uncommitted while the service allocates it.
/// On Postgres the insert waits on the unique index and fails once the rival commits, so
/// the service must retry; on SQLite it waits for the writer and sees the row.
pub async fn term_slug_retry(db: Arc<TestDb>) {
    let store = db.shared_store();
    let svc = Arc::new(TaxonomyService::new(store.clone()));
    let admin = actor(store.as_ref(), Role::Admin).await;
    let rival = store.begin().await.unwrap();
    rival
        .terms()
        .insert(&Term {
            id: TermId::new(),
            taxonomy: "tag".into(),
            slug: "race".into(),
            name: "race".into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    let task = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.create_term(&admin, "tag", "Race", None, None, "").await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    rival.commit().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap().slug, "race-2");
}

pub async fn term_parallel_same_name(db: Arc<TestDb>) {
    let store = db.shared_store();
    let svc = Arc::new(TaxonomyService::new(store.clone()));
    let admin = actor(store.as_ref(), Role::Admin).await;
    let tasks: Vec<_> = (0..3)
        .map(|_| {
            let svc = svc.clone();
            tokio::spawn(
                async move { svc.create_term(&admin, "tag", "Same", None, None, "").await },
            )
        })
        .collect();
    let mut slugs = Vec::new();
    for t in tasks {
        slugs.push(t.await.unwrap().unwrap().slug);
    }
    assert_eq!(sorted(slugs), ["same", "same-2", "same-3"]);
}

// --------------------------------------------------------------------- menus

fn url_item(url: &str) -> MenuItem {
    MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position: 0,
        kind: MenuItemKind::Url,
        target_id: None,
        url: Some(url.into()),
        label: "L".into(),
    }
}

fn target_item(kind: MenuItemKind, target: Option<Uuid>) -> MenuItem {
    MenuItem {
        kind,
        target_id: target,
        url: None,
        ..url_item("/")
    }
}

fn menu(name: &str, items: Vec<MenuItem>) -> Menu {
    Menu {
        name: name.into(),
        label: "Main".into(),
        items,
    }
}

pub async fn menu_roundtrip_and_events(db: &TestDb) {
    let store = db.store();
    let svc = MenuService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let user = store.read().users().get(admin.user).await.unwrap().unwrap();
    let post = store
        .read()
        .content()
        .insert(&new_post(&user, "target"))
        .await
        .unwrap();
    let term = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id: term,
            taxonomy: "tag".into(),
            slug: "t".into(),
            name: "t".into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();

    assert!(matches!(svc.get("main").await, Err(CoreError::NotFound)));
    let parent = url_item("https://example.com/a");
    let mut child = target_item(MenuItemKind::Item, Some(post.id.as_uuid()));
    child.parent_id = Some(parent.id);
    let m = menu(
        "main",
        vec![
            parent,
            child,
            target_item(MenuItemKind::Term, Some(term.as_uuid())),
            url_item("/local"),
        ],
    );
    svc.put(&admin, &m).await.unwrap();
    assert_eq!(
        last_event(store).await,
        ("invalidate".to_owned(), vec![menu_key("main")])
    );
    let got = svc.get("main").await.unwrap();
    assert_eq!(got.items.len(), 4);
    assert_eq!(got.label, "Main");
    assert_eq!(
        svc.list().await.unwrap(),
        [("main".to_owned(), "Main".to_owned())]
    );

    // Replace the whole tree.
    svc.put(&admin, &menu("main", vec![url_item("/only")]))
        .await
        .unwrap();
    assert_eq!(svc.get("main").await.unwrap().items.len(), 1);

    svc.delete(&admin, "main").await.unwrap();
    assert_eq!(last_event(store).await.1, vec![menu_key("main")]);
    assert!(matches!(svc.get("main").await, Err(CoreError::NotFound)));
    assert!(matches!(
        svc.delete(&admin, "main").await,
        Err(CoreError::NotFound)
    ));
}

pub async fn menu_validation(db: &TestDb) {
    let store = db.store();
    let svc = MenuService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let user = store.read().users().get(admin.user).await.unwrap().unwrap();
    let mut trashed = new_post(&user, "gone");
    trashed.status = ContentStatus::Trash;
    let trashed = store.read().content().insert(&trashed).await.unwrap();
    let seq = max_seq(store).await;

    let bad: Vec<MenuItem> = vec![
        url_item("//evil.com"),
        url_item("javascript:alert(1)"),
        url_item("ftp://x"),
        url_item("relative"),
        MenuItem {
            url: None,
            ..url_item("/")
        },
        target_item(MenuItemKind::Item, None),
        target_item(MenuItemKind::Item, Some(Uuid::now_v7())),
        target_item(MenuItemKind::Item, Some(trashed.id.as_uuid())),
        target_item(MenuItemKind::Term, None),
        target_item(MenuItemKind::Term, Some(Uuid::now_v7())),
    ];
    for item in bad {
        let desc = format!("{item:?}");
        let m = menu("main", vec![url_item("/ok"), item]);
        assert!(is_invalid(svc.put(&admin, &m).await), "{desc}");
    }
    // A bad name, and a parent outside the menu (store rule, surfaced as Invalid).
    assert!(is_invalid(svc.put(&admin, &menu("Bad Name", vec![])).await));
    let mut orphan = url_item("/x");
    orphan.parent_id = Some(MenuItemId::new());
    assert!(is_invalid(
        svc.put(&admin, &menu("main", vec![orphan])).await
    ));
    // Nothing was written.
    assert_eq!(max_seq(store).await, seq);
    assert!(matches!(svc.get("main").await, Err(CoreError::NotFound)));
}

pub async fn menu_field_rules_and_caps(db: &TestDb) {
    let store = db.store();
    let svc = MenuService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let user = store.read().users().get(admin.user).await.unwrap().unwrap();
    let post = store
        .read()
        .content()
        .insert(&new_post(&user, "m"))
        .await
        .unwrap();
    let mut bad = Vec::new();
    let mut with_target = url_item("/x");
    with_target.target_id = Some(post.id.as_uuid());
    bad.push(with_target);
    let mut item_with_url = target_item(MenuItemKind::Item, Some(post.id.as_uuid()));
    item_with_url.url = Some("/x".into());
    bad.push(item_with_url);
    let mut term_with_url = target_item(MenuItemKind::Term, Some(Uuid::now_v7()));
    term_with_url.url = Some("/x".into());
    bad.push(term_with_url);
    bad.push(url_item(&format!("/{}", "a".repeat(2048))));
    let mut long_label = url_item("/x");
    long_label.label = "l".repeat(201);
    bad.push(long_label);
    for item in bad {
        let desc = format!("{item:?}");
        assert!(
            is_invalid(svc.put(&admin, &menu("main", vec![item])).await),
            "{desc}"
        );
    }
    let mut edge = url_item(&format!("/{}", "a".repeat(2047)));
    edge.label = "l".repeat(200);
    svc.put(&admin, &menu("main", vec![edge])).await.unwrap();
}

pub async fn menu_permissions(db: &TestDb) {
    let store = db.store();
    let svc = MenuService::new(db.shared_store());
    let editor = actor(store, Role::Editor).await;
    svc.put(&editor, &menu("main", vec![])).await.unwrap();
    for role in [Role::Author, Role::Contributor] {
        let who = actor(store, role).await;
        let seq = max_seq(store).await;
        assert!(is_forbidden(
            svc.put(&who, &menu("other", vec![])).await,
            "manage_menus"
        ));
        assert!(is_forbidden(svc.delete(&who, "main").await, "manage_menus"));
        assert_eq!(max_seq(store).await, seq);
    }
    assert!(svc.get("main").await.is_ok());
}

// ------------------------------------------------------------------- options

/// The stored value of option `key`.
async fn stored_option(store: &dyn Store, key: &str) -> Option<serde_json::Value> {
    store
        .read()
        .options()
        .get(key)
        .await
        .unwrap()
        .map(|e| e.value)
}

/// `set` and `delete` write the store and emit `options.changed` (there is no option
/// cache to keep coherent since R26).
pub async fn option_set_and_delete(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = OptionService::new(db.shared_store());

    svc.set(&admin, "site", &json!({"title": "T"}), true)
        .await
        .unwrap();
    assert_eq!(
        last_event(store).await,
        ("options.changed".to_owned(), vec![option_key("site")])
    );
    assert_eq!(
        stored_option(store, "site").await,
        Some(json!({"title": "T"}))
    );
    // The autoload flag is stored as given.
    svc.set(&admin, "flip", &json!(1), false).await.unwrap();
    let e = store.read().options().get("flip").await.unwrap().unwrap();
    assert!(!e.autoload);

    svc.delete(&admin, "site").await.unwrap();
    assert!(stored_option(store, "site").await.is_none());
    assert_eq!(
        last_event(store).await,
        ("options.changed".to_owned(), vec![option_key("site")])
    );
    // Deleting a missing option is a no-op write that still emits.
    svc.delete(&admin, "never").await.unwrap();
    assert_eq!(
        last_event(store).await,
        ("options.changed".to_owned(), vec![option_key("never")])
    );
}

pub async fn option_permissions_and_failures(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = OptionService::new(db.shared_store());
    svc.set(&admin, "k", &json!("v"), true).await.unwrap();
    for role in [Role::Editor, Role::Author, Role::Contributor] {
        let who = actor(store, role).await;
        let seq = max_seq(store).await;
        assert!(is_forbidden(
            svc.set(&who, "k", &json!("hacked"), true).await,
            "manage_settings"
        ));
        assert!(is_forbidden(svc.delete(&who, "k").await, "manage_settings"));
        assert_eq!(max_seq(store).await, seq);
    }
    assert_eq!(stored_option(store, "k").await, Some(json!("v")));

    // A failed write leaves the store alone.
    let seq = max_seq(store).await;
    assert!(is_invalid(svc.set(&admin, "", &json!(1), true).await));
    assert!(is_invalid(svc.set(&admin, "a\nb", &json!(1), true).await));
    let nul = json!({"a": ["x\u{0}y"]});
    assert!(
        is_invalid(svc.set(&admin, "k", &nul, true).await),
        "store rejects NUL in json"
    );
    assert_eq!(max_seq(store).await, seq);
    assert_eq!(stored_option(store, "k").await, Some(json!("v")));
}

// ----------------------------------------------------------------- redirects

pub async fn redirect_resolve(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let r = svc.upsert(&admin, "/old", "/new", 301).await.unwrap();
    assert_eq!(
        (r.source_path.as_str(), r.target.as_str(), r.code),
        ("/old", "/new", 301)
    );
    svc.upsert(&admin, "/dir/", "https://example.com/x", 302)
        .await
        .unwrap();

    assert_eq!(svc.resolve("/old").await.unwrap().unwrap().id, r.id);
    // Trailing slash toggled, both directions.
    assert_eq!(svc.resolve("/old/").await.unwrap().unwrap().id, r.id);
    assert_eq!(
        svc.resolve("/dir").await.unwrap().unwrap().target,
        "https://example.com/x"
    );
    // Case-sensitive; no match.
    assert!(svc.resolve("/OLD").await.unwrap().is_none());
    assert!(svc.resolve("/other").await.unwrap().is_none());
    assert!(svc.resolve("/").await.unwrap().is_none());
    // Exact match wins over the toggled one. `upsert` refuses a source whose variant is
    // taken (R26), so the second variant is a row written directly (legacy data).
    assert!(is_invalid(
        svc.upsert(&admin, "/old/", "/elsewhere", 307).await
    ));
    let exact = store
        .read()
        .redirects()
        .upsert(&ferrite_domain::Redirect {
            id: ferrite_domain::RedirectId::new(),
            source_path: "/old/".into(),
            target: "/elsewhere".into(),
            code: 307,
            hits: 0,
            origin: ferrite_domain::RedirectOrigin::Manual,
            created_at: ferrite_domain::Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();
    assert_eq!(svc.resolve("/old/").await.unwrap().unwrap().id, exact.id);
    assert_eq!(svc.resolve("/old").await.unwrap().unwrap().id, r.id);
    // "/" only matches exactly.
    svc.upsert(&admin, "/", "/home", 301).await.unwrap();
    assert_eq!(svc.resolve("/").await.unwrap().unwrap().target, "/home");

    // Upsert on the same source changes target and code, keeps id.
    let again = svc.upsert(&admin, "/old", "/newer", 308).await.unwrap();
    assert_eq!(
        (again.id, again.target.as_str(), again.code),
        (r.id, "/newer", 308)
    );

    // hit: counted; a missing id is swallowed.
    svc.hit(r.id).await;
    svc.hit(r.id).await;
    svc.hit(ferrite_domain::RedirectId::new()).await;
    let stored = svc.resolve("/old").await.unwrap().unwrap();
    assert_eq!(stored.hits, 2);
    assert!(stored.last_hit_at.is_some());

    // list / delete.
    let page = svc.list(&admin, 100, None).await.unwrap();
    assert_eq!(page.items.len(), 4);
    svc.delete(&admin, r.id).await.unwrap();
    // Only the exact "/old/" redirect is left; "/old" reaches it by slash toggle.
    assert_eq!(svc.resolve("/old").await.unwrap().unwrap().id, exact.id);
}

pub async fn redirect_validation(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    for (s, t, code) in [
        ("/a", "/a", 301),
        ("a", "/b", 301),
        ("", "/b", 301),
        ("/a b", "/b", 301),
        ("/a", "b", 301),
        ("/a", "//evil.com", 301),
        ("/a", "javascript:alert(1)", 301),
        ("/a", "ftp://x", 301),
        ("/a", "/b", 200),
    ] {
        assert!(
            is_invalid(svc.upsert(&admin, s, t, code).await),
            "{s} -> {t} {code}"
        );
    }
    assert!(svc.list(&admin, 10, None).await.unwrap().items.is_empty());
}

pub async fn redirect_loops_and_chains(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    svc.upsert(&admin, "/b", "/c", 301).await.unwrap();
    svc.upsert(&admin, "/c", "/d", 301).await.unwrap();
    // /d -> /b closes b -> c -> d -> b; /c -> /b closes b -> c -> b.
    assert!(is_invalid(svc.upsert(&admin, "/d", "/b", 301).await));
    assert!(is_invalid(svc.upsert(&admin, "/c", "/b", 301).await));
    assert!(is_invalid(svc.upsert(&admin, "/d", "/c", 301).await));
    assert_eq!(svc.resolve("/c").await.unwrap().unwrap().target, "/d");
    assert!(svc.resolve("/d").await.unwrap().is_none());
    // A mid-chain redirect pointing elsewhere is fine; an external end stops the walk.
    svc.upsert(&admin, "/d", "https://example.com/", 301)
        .await
        .unwrap();
    svc.upsert(&admin, "/a", "/b", 301).await.unwrap();

    // Chain limit: build /h1 -> /h2 -> ... -> /h11 (10 existing hops from /h1's target).
    for i in (1..=10).rev() {
        svc.upsert(&admin, &format!("/h{i}"), &format!("/h{}", i + 1), 301)
            .await
            .unwrap();
    }
    // New /h0 -> /h1: follows /h1..=/h10 = 10 existing hops, then /h11 has none: allowed.
    svc.upsert(&admin, "/h0", "/h1", 301).await.unwrap();
    // New /g -> /h0: 11 existing hops: too long.
    assert!(matches!(
        svc.upsert(&admin, "/g", "/h0", 301).await,
        Err(CoreError::InvalidField { field: "target", message: m }) if m.contains("chain")
    ));
    assert!(svc.resolve("/g").await.unwrap().is_none());
}

pub async fn redirect_slash_variants_and_caps(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    // /x -> /y/ exists; resolve("/y") would hit "/y/"'s toggled form, so /y -> /x loops.
    svc.upsert(&admin, "/x", "/y/", 301).await.unwrap();
    assert!(is_invalid(svc.upsert(&admin, "/y", "/x", 301).await));
    assert!(is_invalid(svc.upsert(&admin, "/y/", "/x", 301).await));
    assert!(is_invalid(svc.upsert(&admin, "/y", "/x/", 301).await));
    // A redirect to its own slash variant is a loop too.
    assert!(is_invalid(svc.upsert(&admin, "/z", "/z/", 301).await));
    assert!(is_invalid(svc.upsert(&admin, "/z/", "/z", 301).await));
    assert!(svc.resolve("/y").await.unwrap().is_none());

    // Chain length through variants: /h{i} -> /h{i+1}/ (found by slash toggle).
    for i in (1..=10).rev() {
        svc.upsert(&admin, &format!("/h{i}"), &format!("/h{}/", i + 1), 301)
            .await
            .unwrap();
    }
    svc.upsert(&admin, "/h0", "/h1/", 301).await.unwrap();
    assert!(matches!(
        svc.upsert(&admin, "/g", "/h0/", 301).await,
        Err(CoreError::InvalidField { field: "target", message: m }) if m.contains("chain")
    ));

    // Length caps.
    let long = format!("/{}", "a".repeat(2048));
    assert!(is_invalid(svc.upsert(&admin, &long, "/t", 301).await));
    assert!(is_invalid(svc.upsert(&admin, "/t", &long, 301).await));
    let ok = format!("/{}", "a".repeat(2047));
    svc.upsert(&admin, &ok, "/t", 301).await.unwrap();
}

pub async fn redirect_permissions(db: &TestDb) {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let r = svc.upsert(&admin, "/x", "/y", 301).await.unwrap();
    for role in [Role::Editor, Role::Author, Role::Contributor] {
        let who = actor(store, role).await;
        assert!(is_forbidden(
            svc.upsert(&who, "/p", "/q", 301).await,
            "manage_redirects"
        ));
        assert!(is_forbidden(
            svc.delete(&who, r.id).await,
            "manage_redirects"
        ));
        assert!(is_forbidden(
            svc.list(&who, 10, None).await,
            "manage_redirects"
        ));
    }
    assert!(svc.resolve("/p").await.unwrap().is_none());
    assert!(svc.resolve("/x").await.unwrap().is_some());
}

backend_tests!(
    term_crud_and_events,
    term_permissions,
    term_parent_rules,
    term_in_use_deletes_links,
    menu_roundtrip_and_events,
    menu_validation,
    menu_permissions,
    option_set_and_delete,
    option_permissions_and_failures,
    redirect_resolve,
    redirect_validation,
    redirect_loops_and_chains,
    redirect_permissions,
    redirect_slash_variants_and_caps,
    flat_taxonomy_rejects_parents,
    menu_field_rules_and_caps,
);

backend_tests_spawning!(term_slug_retry, term_parallel_same_name);
