//! The settings allowlist (P19), `OptionService::set_many`, `permalink.*` validation on
//! every option write, and the redirect update and CSV-import rules (F24, F25), on both
//! backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, ImportReport, OptionService, PermalinkRules,
    RedirectService, RowError, Scopes,
    events::{ALL_KEY, OPTIONS_CHANGED, option_key, redirect_key},
    settings::{self, SETTINGS},
};
use ferrite_domain::{NewUser, RedirectOrigin, Role, UserId, UserStatus};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

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

/// `(kind, keys)` of every event after `after`.
async fn events_since(store: &dyn Store, after: i64) -> Vec<(String, Vec<String>)> {
    store
        .read()
        .events()
        .since(after, 1000)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.keys))
        .collect()
}

fn change(key: &str, value: Value) -> (String, Option<Value>) {
    (key.to_owned(), Some(value))
}

/// The message of an `Invalid` or `InvalidField` error (with the field, if any).
fn invalid<T: std::fmt::Debug>(r: Result<T, CoreError>) -> (Option<&'static str>, String) {
    match r {
        Err(CoreError::Invalid(m)) => (None, m),
        Err(CoreError::InvalidField { field, message }) => (Some(field), message),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn draft(kind: &str, title: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
    }
}

pub async fn settings_allowlist_and_validation(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = OptionService::new(db.shared_store());

    // The allowlist, in order.
    let keys: Vec<&str> = SETTINGS.iter().map(|d| d.key).collect();
    assert_eq!(
        keys,
        [
            "site.title",
            "site.description",
            "site.locale",
            "reading.per_page",
            "reading.front_page",
            "reading.discourage_indexing",
            "permalink.post",
            "permalink.page",
        ]
    );

    // Pure validation: types and bounds.
    let ok = [
        ("site.title", json!("")),
        ("site.title", json!("x".repeat(200))),
        ("site.description", json!("y".repeat(1000))),
        ("site.description", json!("two\nlines")),
        ("site.locale", json!("en")),
        ("site.locale", json!("x".repeat(35))),
        ("site.locale", json!("pt-BR")),
        ("reading.per_page", json!(1)),
        ("reading.per_page", json!(100)),
        ("reading.front_page", json!(null)),
        ("reading.front_page", json!(UserId::new().to_string())),
        ("reading.discourage_indexing", json!(true)),
        ("permalink.post", json!("/%year%/%slug%/")),
        ("permalink.page", json!("/%parent_path%/%slug%/")),
    ];
    for (key, value) in ok {
        assert!(settings::validate(key, &value).is_ok(), "{key} = {value}");
    }
    let bad = [
        ("site.title", json!(1)),
        ("site.title", json!("x".repeat(201))),
        ("site.title", json!("a\u{7}b")),
        ("site.description", json!("y".repeat(1001))),
        ("site.description", json!(["x"])),
        ("site.locale", json!("")),
        ("site.locale", json!("x".repeat(36))),
        ("site.locale", json!("en US")),
        ("site.locale", json!(null)),
        ("reading.per_page", json!(0)),
        ("reading.per_page", json!(101)),
        ("reading.per_page", json!(2.5)),
        ("reading.per_page", json!("10")),
        ("reading.front_page", json!("not-a-uuid")),
        ("reading.front_page", json!(7)),
        ("reading.discourage_indexing", json!("yes")),
        ("reading.discourage_indexing", json!(0)),
        ("permalink.post", json!(1)),
        ("permalink.post", json!("no-slash")),
        ("permalink.page", json!("/%slug%/")),
    ];
    for (key, value) in bad {
        let (field, _) = invalid(settings::validate(key, &value));
        assert_eq!(field, Some(key), "{key} = {value}");
    }
    // Unknown keys are refused (no field: it is not an input member we know).
    let (field, m) = invalid(settings::validate("site.url", &json!("x")));
    assert_eq!(field, None);
    assert!(m.contains("site.url"), "{m}");

    // set_many: unknown key, a missing front page, a front page that is not a page.
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let post = content
        .create(&admin, draft("post", "A post"))
        .await
        .unwrap();
    let seq = max_seq(store).await;
    let (_, m) = invalid(
        svc.set_many(&admin, &[change("hacker.key", json!(1))])
            .await,
    );
    assert!(m.contains("hacker.key"), "{m}");
    let (field, m) = invalid(
        svc.set_many(
            &admin,
            &[change(
                "reading.front_page",
                json!(UserId::new().to_string()),
            )],
        )
        .await,
    );
    assert_eq!(field, Some("reading.front_page"));
    assert!(m.contains("page"), "{m}");
    let (field, _) = invalid(
        svc.set_many(
            &admin,
            &[change("reading.front_page", json!(post.id.to_string()))],
        )
        .await,
    );
    assert_eq!(field, Some("reading.front_page"));
    assert_eq!(max_seq(store).await, seq, "rejected writes emit nothing");

    // A valid batch: stored with autoload (F23), one options.changed per key.
    let page = content.create(&admin, draft("page", "Home")).await.unwrap();
    let seq = max_seq(store).await;
    svc.set_many(
        &admin,
        &[
            change("site.title", json!("New title")),
            change("reading.per_page", json!(5)),
            change("reading.front_page", json!(page.id.to_string())),
        ],
    )
    .await
    .unwrap();
    let ev = events_since(store, seq).await;
    assert_eq!(
        ev,
        [
            (OPTIONS_CHANGED.to_owned(), vec![option_key("site.title")]),
            (
                OPTIONS_CHANGED.to_owned(),
                vec![option_key("reading.per_page")]
            ),
            (
                OPTIONS_CHANGED.to_owned(),
                vec![option_key("reading.front_page")]
            ),
        ]
    );
    for key in ["site.title", "reading.per_page", "reading.front_page"] {
        let e = store.read().options().get(key).await.unwrap().unwrap();
        assert!(e.autoload, "{key}");
    }
    // An audit row for the batch.
    let audit = store.read().audit().list(10, None).await.unwrap();
    let row = audit
        .items
        .iter()
        .find(|a| a.action == "settings.update")
        .expect("audit row");
    assert_eq!(row.actor_id, Some(admin.user));
    assert_eq!(
        row.details["keys"],
        json!(["site.title", "reading.per_page", "reading.front_page"])
    );

    // read_all: every key, null for unset ones.
    let all = settings::read_all(store, &admin).await.unwrap();
    assert_eq!(all.len(), SETTINGS.len());
    assert_eq!(all["site.title"], json!("New title"));
    assert_eq!(all["reading.per_page"], json!(5));
    assert_eq!(all["site.locale"], Value::Null);

    // null deletes (back to the default).
    svc.set_many(&admin, &[("site.title".into(), None)])
        .await
        .unwrap();
    assert!(
        store
            .read()
            .options()
            .get("site.title")
            .await
            .unwrap()
            .is_none()
    );
    svc.set_many(&admin, &[change("reading.front_page", Value::Null)])
        .await
        .unwrap();
    assert!(
        store
            .read()
            .options()
            .get("reading.front_page")
            .await
            .unwrap()
            .is_none()
    );

    // Permissions: writes need manage_settings; reads need settings:read.
    for role in [Role::Editor, Role::Author, Role::Contributor] {
        let who = actor(store, role).await;
        assert!(matches!(
            svc.set_many(&who, &[change("site.title", json!("x"))])
                .await,
            Err(CoreError::Forbidden("manage_settings"))
        ));
        settings::read_all(store, &who).await.unwrap();
    }
    let narrow = actor(store, Role::Admin)
        .await
        .with_scopes(Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    assert!(matches!(
        settings::read_all(store, &narrow).await,
        Err(CoreError::Forbidden("settings:read"))
    ));
}

pub async fn permalink_option_write_is_validated(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = OptionService::new(db.shared_store());
    let seq = max_seq(store).await;

    // A first segment that is a content type.
    let (field, m) = invalid(
        svc.set(&admin, "permalink.post", &json!("/post/%slug%/"), true)
            .await,
    );
    assert_eq!(field, Some("permalink.post"));
    assert!(m.contains("is a content type"), "{m}");
    // A page structure without %parent_path%.
    let (field, m) = invalid(
        svc.set(&admin, "permalink.page", &json!("/%slug%/"), true)
            .await,
    );
    assert_eq!(field, Some("permalink.page"));
    assert!(m.contains("%parent_path%"), "{m}");
    // Not a string.
    invalid(svc.set(&admin, "permalink.post", &json!(3), true).await);
    // The same through set_many, and a taxonomy name.
    let (field, m) = invalid(
        svc.set_many(&admin, &[change("permalink.post", json!("/tag/%slug%/"))])
            .await,
    );
    assert_eq!(field, Some("permalink.post"));
    assert!(m.contains("is a taxonomy"), "{m}");
    assert!(
        store
            .read()
            .options()
            .get("permalink.post")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(max_seq(store).await, seq, "nothing emitted");

    // A valid change: options.changed and invalidate [all] (R10).
    svc.set_many(
        &admin,
        &[
            change("permalink.post", json!("/blog/%slug%/")),
            change("site.title", json!("T")),
        ],
    )
    .await
    .unwrap();
    let ev = events_since(store, seq).await;
    assert_eq!(
        ev,
        [
            (
                OPTIONS_CHANGED.to_owned(),
                vec![option_key("permalink.post")]
            ),
            (OPTIONS_CHANGED.to_owned(), vec![option_key("site.title")]),
            ("invalidate".to_owned(), vec![ALL_KEY.to_owned()]),
        ]
    );
    // The pair is validated together: rules_from sees both pending values.
    svc.set_many(
        &admin,
        &[
            change("permalink.post", json!("/%year%/%slug%/")),
            change("permalink.page", json!("/p/%parent_path%/%slug%/")),
        ],
    )
    .await
    .unwrap();
    let rules = ferrite_core::permalink::rules_from(store.read(), &[])
        .await
        .unwrap();
    assert_eq!(rules.post().as_str(), "/%year%/%slug%/");
    assert_eq!(rules.page().as_str(), "/p/%parent_path%/%slug%/");
}

pub async fn set_many_is_atomic(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = OptionService::new(db.shared_store());
    svc.set_many(&admin, &[change("site.title", json!("Before"))])
        .await
        .unwrap();
    let seq = max_seq(store).await;
    for batch in [
        vec![
            change("site.title", json!("After")),
            change("reading.per_page", json!(0)),
        ],
        vec![
            change("site.title", json!("After")),
            change("nope", json!(1)),
        ],
        vec![
            change("site.title", json!("After")),
            change("permalink.post", json!("/page/%slug%/")),
        ],
        vec![
            change("site.title", json!("After")),
            change("site.title", json!("Again")),
        ],
    ] {
        assert!(svc.set_many(&admin, &batch).await.is_err(), "{batch:?}");
    }
    assert_eq!(
        store
            .read()
            .options()
            .get("site.title")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!("Before")
    );
    assert_eq!(max_seq(store).await, seq, "no events");
    // An empty batch is a no-op.
    svc.set_many(&admin, &[]).await.unwrap();
    assert_eq!(max_seq(store).await, seq);
}

fn rows(list: &[(&str, &str, u16)]) -> Vec<(String, String, u16)> {
    list.iter()
        .map(|(s, t, c)| ((*s).to_owned(), (*t).to_owned(), *c))
        .collect()
}

fn row_errors(r: Result<ImportReport, CoreError>) -> Vec<RowError> {
    match r {
        Err(CoreError::InvalidRows(errors)) => errors,
        other => panic!("expected InvalidRows, got {other:?}"),
    }
}

pub async fn redirect_import_all_or_nothing(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = RedirectService::new(db.shared_store());
    svc.upsert(&admin, "/existing", "/a", 301).await.unwrap();
    let seq = max_seq(store).await;

    // Every row is checked; nothing is written when any fails.
    let errors = row_errors(
        svc.import(
            &admin,
            &rows(&[
                ("/ok", "/fine", 301),
                ("bad", "/x", 301),            // 1: source
                ("/self", "/self/", 301),      // 2: to itself
                ("/ok", "/again", 302),        // 3: duplicate source
                ("/code", "/x", 200),          // 4: code
                ("/a", "/existing", 301),      // 5: loop with an existing row
                ("/l1", "/l2", 301),           // 6: loop within the file
                ("/l2", "/l1", 301),           // 7
                ("/t", "javascript:x()", 301), // 8: target
                ("/ok/", "/q", 301),           // 9: duplicate (slash variant)
                ("/api/x", "/q", 301),         // 10: router-owned source
                ("/_theme/a.css", "/q", 301),  // 11: router-owned source
            ]),
        )
        .await,
    );
    let at: Vec<usize> = errors.iter().map(|e| e.row).collect();
    assert_eq!(at, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], "{errors:?}");
    assert!(errors[2].message.contains("duplicate"), "{errors:?}");
    assert!(errors[8].message.contains("duplicate"), "{errors:?}");
    assert!(errors[9].message.contains("serves itself"), "{errors:?}");
    assert!(errors[4].message.contains("loop"), "{errors:?}");
    assert!(svc.resolve("/ok").await.unwrap().is_none());
    assert_eq!(max_seq(store).await, seq, "nothing emitted");

    // A chain longer than 10 built inside the file.
    let mut long = Vec::new();
    for i in 0..12 {
        long.push((format!("/c{i}"), format!("/c{}", i + 1), 301));
    }
    let errors = row_errors(svc.import(&admin, &long).await);
    assert!(
        errors.iter().all(|e| e.message.contains("chain")),
        "{errors:?}"
    );
    assert_eq!(errors[0].row, 0);

    // Too many rows.
    let many: Vec<(String, String, u16)> = (0..10_001)
        .map(|i| (format!("/m{i}"), "/x".to_owned(), 301))
        .collect();
    let (_, m) = invalid(svc.import(&admin, &many).await);
    assert!(m.contains("10000"), "{m}");

    // A valid file: created and updated counts, origin import, one invalidate.
    let report = svc
        .import(
            &admin,
            &rows(&[
                ("/existing", "/b", 302),
                ("/n1", "https://example.com/x,y", 301),
                ("/n2", "/n3", 308),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(
        report,
        ImportReport {
            created: 2,
            updated: 1
        }
    );
    let e = svc.resolve("/existing").await.unwrap().unwrap();
    assert_eq!((e.target.as_str(), e.code), ("/b", 302));
    assert_eq!(e.origin, RedirectOrigin::Import);
    let n1 = svc.resolve("/n1").await.unwrap().unwrap();
    assert_eq!(n1.target, "https://example.com/x,y");
    let ev = events_since(store, seq).await;
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0].0, "invalidate");
    for s in ["/existing", "/existing/", "/n1", "/n1/", "/n2", "/n2/"] {
        assert!(ev[0].1.contains(&redirect_key(s)), "{s}: {ev:?}");
    }

    // Permissions.
    let editor = actor(store, Role::Editor).await;
    assert!(matches!(
        svc.import(&editor, &rows(&[("/e", "/f", 301)])).await,
        Err(CoreError::Forbidden("manage_redirects"))
    ));
}

pub async fn redirect_update_moves_source(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = RedirectService::new(db.shared_store());
    let r = svc.upsert(&admin, "/a", "/x", 301).await.unwrap();
    let other = svc.upsert(&admin, "/other", "/y", 301).await.unwrap();
    assert_eq!(svc.get(&admin, r.id).await.unwrap(), r);

    // Moving the source: same id, old source gone, events for both sources.
    let seq = max_seq(store).await;
    let moved = svc.update(&admin, r.id, "/b", "/z", 302).await.unwrap();
    assert_eq!(moved.id, r.id);
    assert_eq!(
        (
            moved.source_path.as_str(),
            moved.target.as_str(),
            moved.code
        ),
        ("/b", "/z", 302)
    );
    assert_eq!(moved.created_at, r.created_at);
    assert!(svc.resolve("/a").await.unwrap().is_none());
    assert_eq!(svc.resolve("/b").await.unwrap().unwrap().id, r.id);
    let ev = events_since(store, seq).await;
    assert_eq!(ev.len(), 1, "{ev:?}");
    for s in ["/a", "/a/", "/b", "/b/"] {
        assert!(ev[0].1.contains(&redirect_key(s)), "{s}: {ev:?}");
    }

    // Same source: target and code change in place.
    let same = svc.update(&admin, r.id, "/b", "/w", 307).await.unwrap();
    assert_eq!((same.id, same.target.as_str()), (r.id, "/w"));

    // A source another row holds (either slash variant) is refused (F24).
    let seq = max_seq(store).await;
    for taken in ["/other", "/other/"] {
        let (field, m) = invalid(svc.update(&admin, r.id, taken, "/w", 301).await);
        assert_eq!(field, Some("source"));
        assert!(m.contains("source taken"), "{m}");
    }
    assert_eq!(svc.resolve("/other").await.unwrap().unwrap(), other);
    // A loop is refused like upsert; the old row is still there.
    let (field, m) = invalid(svc.update(&admin, other.id, "/w", "/b", 301).await);
    assert_eq!(field, Some("target"));
    assert!(m.contains("loop"), "{m}");
    assert_eq!(svc.resolve("/other").await.unwrap().unwrap(), other);
    assert_eq!(max_seq(store).await, seq);

    // Missing: NotFound; permissions.
    let gone = ferrite_domain::RedirectId::new();
    assert!(matches!(
        svc.update(&admin, gone, "/q", "/r", 301).await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        svc.get(&admin, gone).await,
        Err(CoreError::NotFound)
    ));
    // Router-owned sources are refused on create and update; stored rows still resolve.
    for owned in ["/feed/", "/robots.txt", "/_media/x", "/healthz"] {
        let (field, _) = invalid(svc.upsert(&admin, owned, "/q", 301).await);
        assert_eq!(field, Some("source"), "{owned}");
        let (field, _) = invalid(svc.update(&admin, r.id, owned, "/q", 301).await);
        assert_eq!(field, Some("source"), "{owned}");
    }
    store
        .read()
        .redirects()
        .upsert(&ferrite_domain::Redirect {
            id: ferrite_domain::RedirectId::new(),
            source_path: "/feed/legacy".into(),
            target: "/feed/".into(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::Manual,
            created_at: ferrite_domain::Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();
    assert!(svc.resolve("/feed/legacy").await.unwrap().is_some());

    // Reads: the role must hold manage_redirects; a redirects:read token is enough.
    let reader = actor(store, Role::Admin)
        .await
        .with_scopes(Scopes::from_strs(&["redirects:read".to_owned()]).unwrap());
    assert_eq!(svc.get(&reader, r.id).await.unwrap().id, r.id);
    assert!(!svc.list(&reader, 10, None).await.unwrap().items.is_empty());
    assert!(matches!(
        svc.update(&reader, r.id, "/q", "/r", 301).await,
        Err(CoreError::Forbidden("redirects:write"))
    ));
    let narrow = actor(store, Role::Admin)
        .await
        .with_scopes(Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    assert!(matches!(
        svc.get(&narrow, r.id).await,
        Err(CoreError::Forbidden("redirects:read"))
    ));
    let author = actor(store, Role::Author).await;
    assert!(matches!(
        svc.get(&author, r.id).await,
        Err(CoreError::Forbidden("manage_redirects"))
    ));
    assert!(matches!(
        svc.update(&author, r.id, "/q", "/r", 301).await,
        Err(CoreError::Forbidden("manage_redirects"))
    ));
}

/// R26 (M4, M5): create and import refuse a source whose slash-toggled variant another
/// redirect holds, as update does (F24), so `/old` and `/old/` cannot go to different
/// targets; the exact source still upserts in place. Sitemap shard paths are
/// router-owned sources.
pub async fn redirect_create_refuses_slash_variant(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = RedirectService::new(db.shared_store());
    let old = svc.upsert(&admin, "/old", "/a", 301).await.unwrap();
    let trailing = svc.upsert(&admin, "/dir/", "/a", 301).await.unwrap();
    let seq = max_seq(store).await;
    for (taken, row) in [("/old/", &old), ("/dir", &trailing)] {
        let (field, m) = invalid(svc.upsert(&admin, taken, "/b", 302).await);
        assert_eq!(field, Some("source"), "{taken}");
        assert!(m.contains("source taken"), "{m}");
        assert_eq!(svc.resolve(taken).await.unwrap().unwrap(), *row);
    }
    assert_eq!(max_seq(store).await, seq, "nothing emitted");
    // The exact source changes in place.
    let same = svc.upsert(&admin, "/old", "/c", 307).await.unwrap();
    assert_eq!(
        (same.id, same.target.as_str(), same.code),
        (old.id, "/c", 307)
    );

    // Import: the same rule against the stored rows; nothing written on failure.
    let seq = max_seq(store).await;
    let errors = row_errors(
        svc.import(
            &admin,
            &rows(&[
                ("/new", "/x", 301),
                ("/old/", "/d", 301),
                ("/dir", "/d", 301),
            ]),
        )
        .await,
    );
    let at: Vec<usize> = errors.iter().map(|e| e.row).collect();
    assert_eq!(at, [1, 2], "{errors:?}");
    assert!(
        errors.iter().all(|e| e.message.contains("source taken")),
        "{errors:?}"
    );
    assert!(svc.resolve("/new").await.unwrap().is_none());
    assert_eq!(max_seq(store).await, seq);
    let report = svc
        .import(&admin, &rows(&[("/old", "/e", 301), ("/new", "/x", 301)]))
        .await
        .unwrap();
    assert_eq!((report.created, report.updated), (1, 1));
    assert_eq!(svc.resolve("/old").await.unwrap().unwrap().id, old.id);

    // Sitemap shards are served by the router (M5).
    for owned in [
        "/sitemap-post-1.xml",
        "/Sitemap-Book-12.xml",
        "/sitemap-x.xml/",
    ] {
        let (field, m) = invalid(svc.upsert(&admin, owned, "/q", 301).await);
        assert_eq!(field, Some("source"), "{owned}");
        assert!(m.contains("serves itself"), "{m}");
        let errors = row_errors(svc.import(&admin, &rows(&[(owned, "/q", 301)])).await);
        assert_eq!(errors.len(), 1, "{owned}");
    }
}

backend_tests!(
    settings_allowlist_and_validation,
    permalink_option_write_is_validated,
    set_many_is_atomic,
    redirect_import_all_or_nothing,
    redirect_update_moves_source,
    redirect_create_refuses_slash_variant,
);
