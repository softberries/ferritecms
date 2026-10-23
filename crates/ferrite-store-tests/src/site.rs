//! Site-level contract tests: options, schema_meta and transaction semantics.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{Redirect, RedirectId, RedirectOrigin, Timestamp};
use ferrite_store::StoreError;
use serde_json::json;

use crate::TestDb;

pub async fn options_crud(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let opts = tx.options();
    assert!(opts.get("blogname").await.unwrap().is_none());

    let nested = json!({"name": "Ünïcode ☃ 日本語", "n": [1, 2.5, null, true], "o": {"k": "v"}});
    opts.set("blogname", &nested, true).await.unwrap();
    opts.set("secret", &json!("hidden"), false).await.unwrap();
    tx.commit().await.unwrap();

    let got = store
        .read()
        .options()
        .get("blogname")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.key, "blogname");
    assert_eq!(got.value, nested);
    assert!(got.autoload);

    let auto = store.read().options().autoloaded().await.unwrap();
    assert_eq!(auto.len(), 1);
    assert_eq!(auto[0].key, "blogname");

    tokio::time::sleep(Duration::from_millis(5)).await;
    let tx = store.begin().await.unwrap();
    tx.options()
        .set("blogname", &json!(42), false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let got2 = store
        .read()
        .options()
        .get("blogname")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got2.value, json!(42));
    assert!(!got2.autoload);
    assert!(got2.updated_at > got.updated_at);
    assert!(
        store
            .read()
            .options()
            .autoloaded()
            .await
            .unwrap()
            .is_empty()
    );

    let tx = store.begin().await.unwrap();
    tx.options().delete("blogname").await.unwrap();
    tx.options().delete("missing").await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        store
            .read()
            .options()
            .get("blogname")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read()
            .options()
            .get("secret")
            .await
            .unwrap()
            .is_some()
    );
}

pub async fn tx_commit_visible(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("k", &json!(1), false).await.unwrap();
    // The transaction sees its own write.
    assert!(tx.options().get("k").await.unwrap().is_some());
    tx.commit().await.unwrap();
    assert!(store.read().options().get("k").await.unwrap().is_some());
}

pub async fn tx_rollback_invisible(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("k", &json!(1), false).await.unwrap();
    assert!(tx.options().get("k").await.unwrap().is_some());
    tx.rollback().await.unwrap();
    assert!(store.read().options().get("k").await.unwrap().is_none());
}

pub async fn tx_dropped_is_rolled_back(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("dropped", &json!(1), false).await.unwrap();
    drop(tx);

    // The writer connection must be released: begin again must not hang.
    let tx = tokio::time::timeout(Duration::from_secs(2), store.begin())
        .await
        .expect("begin() after a dropped Tx timed out")
        .unwrap();
    assert!(tx.options().get("dropped").await.unwrap().is_none());
    tx.options().set("after", &json!(2), false).await.unwrap();
    tx.commit().await.unwrap();

    assert!(
        store
            .read()
            .options()
            .get("dropped")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.read().options().get("after").await.unwrap().is_some());
}

pub async fn read_sees_committed_after_commit(db: &TestDb) {
    let store = db.store();
    // Warm the reader connections first, then commit through the writer.
    assert!(store.read().options().get("warm").await.unwrap().is_none());
    let tx = store.begin().await.unwrap();
    tx.options().set("warm", &json!("up"), false).await.unwrap();
    tx.commit().await.unwrap();
    let got = store.read().options().get("warm").await.unwrap().unwrap();
    assert_eq!(got.value, json!("up"));

    // Autocommit writes are visible to subsequent reads too.
    store
        .read()
        .options()
        .set("auto", &json!(1), true)
        .await
        .unwrap();
    assert!(store.read().options().get("auto").await.unwrap().is_some());
}

pub async fn concurrent_writers(db: Arc<TestDb>) {
    const TASKS: usize = 8;
    const ITERATIONS: usize = 50;
    let handles: Vec<_> = (0..TASKS)
        .map(|t| {
            let db = db.clone();
            tokio::spawn(async move {
                for i in 0..ITERATIONS {
                    let store = db.store();
                    let tx = store.begin().await.unwrap();
                    tx.options()
                        .set(&format!("w{t}-{i}"), &json!(i), false)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }
    for t in 0..TASKS {
        for i in 0..ITERATIONS {
            let key = format!("w{t}-{i}");
            assert!(
                db.store()
                    .read()
                    .options()
                    .get(&key)
                    .await
                    .unwrap()
                    .is_some(),
                "{key} missing"
            );
        }
    }
}

pub async fn options_reject_nul(db: &TestDb) {
    let store = db.store();
    for bad in [
        json!("a\u{0}b"),
        json!({"k": ["x", {"deep": "\u{0}"}]}),
        json!({"key\u{0}": 1}),
    ] {
        let tx = store.begin().await.unwrap();
        let err = tx.options().set("nul", &bad, false).await.unwrap_err();
        assert!(
            matches!(err, StoreError::Invalid(ref m) if m == "json contains NUL"),
            "{err:?}"
        );
        tx.rollback().await.unwrap();
    }
    assert!(store.read().options().get("nul").await.unwrap().is_none());
}

pub async fn options_number_roundtrip(db: &TestDb) {
    let store = db.store();
    let value = json!({"max": u64::MAX, "big": 1e300, "neg": i64::MIN, "f": 0.1});
    let tx = store.begin().await.unwrap();
    tx.options().set("nums", &value, false).await.unwrap();
    tx.commit().await.unwrap();
    let got = store.read().options().get("nums").await.unwrap().unwrap();
    assert_eq!(got.value, value);
}

pub async fn autoloaded_order_is_binary(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    for k in ["B", "a", "\u{e9}", "Z"] {
        tx.options().set(k, &json!(1), true).await.unwrap();
    }
    tx.commit().await.unwrap();
    let keys: Vec<_> = store
        .read()
        .options()
        .autoloaded()
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.key)
        .collect();
    assert_eq!(keys, ["B", "Z", "a", "\u{e9}"]);
}

pub async fn schema_meta_roundtrip(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let meta = tx.schema_meta();
    assert_eq!(meta.get("test_key").await.unwrap(), None);
    meta.set("test_key", "one").await.unwrap();
    assert_eq!(meta.get("test_key").await.unwrap().as_deref(), Some("one"));
    meta.set("test_key", "two").await.unwrap();
    assert_eq!(meta.get("test_key").await.unwrap().as_deref(), Some("two"));
    tx.commit().await.unwrap();
    let read = store.read().schema_meta().get("test_key").await.unwrap();
    assert_eq!(read.as_deref(), Some("two"));
}

fn redirect(source: &str, target: &str, code: u16) -> Redirect {
    Redirect {
        id: RedirectId::new(),
        source_path: source.into(),
        target: target.into(),
        code,
        hits: 0,
        origin: RedirectOrigin::Manual,
        created_at: Timestamp::now(),
        last_hit_at: None,
    }
}

pub async fn redirects_upsert_get_hit(db: &TestDb) {
    let redirects = db.store().read().redirects();
    let first = redirect("/old", "/new", 301);
    redirects.upsert(&first).await.unwrap();
    assert_eq!(
        redirects.get_by_source("/old").await.unwrap().unwrap(),
        first
    );
    assert!(redirects.get_by_source("/missing").await.unwrap().is_none());
    // Source matching is exact and case-sensitive.
    assert!(redirects.get_by_source("/OLD").await.unwrap().is_none());
    let upper = redirect("/OLD", "/upper", 302);
    redirects.upsert(&upper).await.unwrap();
    assert_eq!(
        redirects.get_by_source("/OLD").await.unwrap().unwrap(),
        upper
    );

    redirects.hit(first.id).await.unwrap();
    redirects.hit(first.id).await.unwrap();
    assert_eq!(
        redirects.get_by_source("/old").await.unwrap().unwrap().hits,
        2
    );

    // Upsert by source updates target, code and origin; id, hits and created_at stay.
    let mut again = redirect("/old", "/newer", 410);
    again.origin = RedirectOrigin::Import;
    redirects.upsert(&again).await.unwrap();
    let got = redirects.get_by_source("/old").await.unwrap().unwrap();
    assert_eq!(got.id, first.id);
    assert_eq!(got.hits, 2);
    assert_eq!(got.created_at, first.created_at);
    assert_eq!((got.target.as_str(), got.code), ("/newer", 410));
    assert_eq!(got.origin, RedirectOrigin::Import);

    // Every allowed code is accepted; anything else is Invalid and changes nothing.
    for code in [301, 302, 307, 308, 410] {
        redirects
            .upsert(&redirect("/old", "/x", code))
            .await
            .unwrap();
    }
    for bad in [0, 200, 299, 303, 404] {
        assert!(matches!(
            redirects.upsert(&redirect("/old", "/y", bad)).await,
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            redirects.upsert(&redirect("/fresh", "/y", bad)).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(
        redirects
            .get_by_source("/old")
            .await
            .unwrap()
            .unwrap()
            .target,
        "/x"
    );
    assert!(redirects.get_by_source("/fresh").await.unwrap().is_none());

    assert!(matches!(
        redirects.hit(RedirectId::new()).await,
        Err(StoreError::NotFound)
    ));
    assert_eq!(
        redirects.get(first.id).await.unwrap().unwrap().source_path,
        "/old"
    );
    assert!(redirects.get(RedirectId::new()).await.unwrap().is_none());
    redirects.delete(first.id).await.unwrap();
    assert!(redirects.get(first.id).await.unwrap().is_none());
    assert!(redirects.get_by_source("/old").await.unwrap().is_none());
    redirects.delete(first.id).await.unwrap();
}

pub async fn redirects_list_paginates_in_binary_order(db: &TestDb) {
    let redirects = db.store().read().redirects();
    // Binary order: "/B" < "/a" < "/a/b" < "/é".
    for source in ["/a/b", "/é", "/B", "/a"] {
        redirects
            .upsert(&redirect(source, "/t", 301))
            .await
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = redirects.list(3, cursor.as_deref()).await.unwrap();
        pages += 1;
        seen.extend(page.items.iter().map(|r| r.source_path.clone()));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, ["/B", "/a", "/a/b", "/é"]);
    assert_eq!(pages, 2);
    let all = redirects.list(4, None).await.unwrap();
    assert_eq!(all.items.len(), 4);
    assert!(all.next_cursor.is_none());
    assert!(matches!(
        redirects.list(0, None).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        redirects.list(201, None).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        redirects.list(2, Some("???")).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn redirects_list_by_target(db: &TestDb) {
    let store = db.store();
    let redirects = store.read().redirects();
    // Binary source order; the target match is exact and case-sensitive.
    for (source, target) in [
        ("/z", "/t"),
        ("/é", "/t"),
        ("/B", "/t"),
        ("/a", "/t"),
        ("/other", "/T"),
        ("/third", "/t/"),
    ] {
        redirects
            .upsert(&redirect(source, target, 301))
            .await
            .unwrap();
    }
    let got: Vec<String> = redirects
        .list_by_target("/t")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.source_path)
        .collect();
    assert_eq!(got, ["/B", "/a", "/z", "/é"]);
    assert!(redirects.list_by_target("/none").await.unwrap().is_empty());
    assert!(matches!(
        redirects.list_by_target("/t\0").await,
        Err(StoreError::Invalid(ref m)) if m.contains("NUL")
    ));

    // Capped at 1000 rows, still in binary source order; works inside a Tx too.
    let tx = store.begin().await.unwrap();
    for i in 0..1_001 {
        tx.redirects()
            .upsert(&redirect(&format!("/many/{i:04}"), "/many-target", 301))
            .await
            .unwrap();
    }
    let in_tx = tx.redirects().list_by_target("/many-target").await.unwrap();
    assert_eq!(in_tx.len(), 1_000);
    tx.commit().await.unwrap();
    let got = redirects.list_by_target("/many-target").await.unwrap();
    assert_eq!(got.len(), 1_000);
    assert_eq!(got[0].source_path, "/many/0000");
    assert_eq!(got[999].source_path, "/many/0999");
}

/// R13: caller text containing NUL is rejected identically on both backends (one
/// representative write per repo family).
pub async fn text_writes_reject_nul(db: &TestDb) {
    use ferrite_domain::{
        ContentType, Menu, MenuItem, MenuItemId, MenuItemKind, NewUser, Role, Taxonomy, Term,
        TermId, UserId, UserStatus,
    };
    use serde_json::json;

    use crate::fixtures::{fixture_user, new_post};

    fn invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) {
        assert!(
            matches!(r, Err(StoreError::Invalid(ref m)) if m.contains("NUL")),
            "{r:?}"
        );
    }
    let store = db.store();
    let repos = store.read();
    let author = fixture_user(store).await;

    invalid(repos.options().set("k\0", &json!(1), false).await);
    invalid(repos.schema_meta().set("k", "v\0").await);
    invalid(repos.redirects().upsert(&redirect("/a\0", "/b", 301)).await);
    invalid(repos.redirects().upsert(&redirect("/a", "/b\0", 301)).await);

    let mut post = new_post(&author, "ok");
    post.title = "t\0".into();
    invalid(repos.content().insert(&post).await);
    let mut post = new_post(&author, "ok");
    post.body_html = Some("<p>\0</p>".into());
    invalid(repos.content().insert(&post).await);
    let mut post = new_post(&author, "sl\0ug");
    post.title = "ok".into();
    invalid(repos.content().insert(&post).await);
    let stored = repos
        .content()
        .insert(&new_post(&author, "fine"))
        .await
        .unwrap();
    invalid(repos.meta().set(stored.id, "k\0", &json!(1)).await);
    let mut changed = stored.clone();
    changed.excerpt = "e\0".into();
    invalid(repos.content().update(&changed).await);

    let ty = ContentType {
        name: "n\0".into(),
        label: "L".into(),
        hierarchical: false,
        has_archive: false,
        fields: json!([]),
        built_in: false,
        created_at: Timestamp::now(),
    };
    invalid(repos.types().upsert(&ty).await);

    invalid(
        repos
            .taxonomies()
            .upsert(&Taxonomy {
                name: "x".into(),
                label: "L\0".into(),
                hierarchical: false,
                content_types: vec![],
                built_in: false,
            })
            .await,
    );
    invalid(
        repos
            .terms()
            .insert(&Term {
                id: TermId::new(),
                taxonomy: "tag".into(),
                slug: "s\0".into(),
                name: "n".into(),
                description: String::new(),
                parent_id: None,
            })
            .await,
    );

    invalid(
        repos
            .menus()
            .put(&Menu {
                name: "m".into(),
                label: "L".into(),
                items: vec![MenuItem {
                    id: MenuItemId::new(),
                    parent_id: None,
                    position: 0,
                    kind: MenuItemKind::Url,
                    target_id: None,
                    url: Some("/u\0".into()),
                    label: "l".into(),
                }],
            })
            .await,
    );

    invalid(
        repos
            .users()
            .insert(&NewUser {
                id: UserId::new(),
                login: "lo\0gin".into(),
                email: "nul@example.com".into(),
                display_name: "D".into(),
                password_hash: None,
                role: Role::Editor,
                status: UserStatus::Active,
                must_reset: false,
            })
            .await,
    );
    let mut renamed = author.clone();
    renamed.display_name = "d\0".into();
    invalid(repos.users().update(&renamed).await);
    let mut hashed = author.clone();
    hashed.password_hash = Some(ferrite_domain::Secret::new("$argon2\0"));
    invalid(repos.users().update(&hashed).await);
    invalid(
        repos
            .users()
            .set_totp(author.id, Some(&ferrite_domain::Secret::new("JBSW\0")))
            .await,
    );
    let mut new_user = NewUser {
        id: UserId::new(),
        login: "hash-nul".into(),
        email: "hash-nul@example.com".into(),
        display_name: "D".into(),
        password_hash: Some(ferrite_domain::Secret::new("h\0")),
        role: Role::Editor,
        status: UserStatus::Active,
        must_reset: false,
    };
    invalid(repos.users().insert(&new_user).await);
    new_user.password_hash = None;
    repos.users().insert(&new_user).await.unwrap();
    assert!(repos.menus().get("m").await.unwrap().is_none());
}

/// R15: lookups, deletes and cursors that bind caller text reject NUL with `Invalid` on
/// both backends (Postgres would otherwise fail in the driver, SQLite would match nothing).
pub async fn text_lookups_reject_nul(db: &TestDb) {
    use ferrite_domain::{ContentId, ContentOrder, ContentQuery, SearchQuery};
    use ferrite_store::{REDIRECT_CURSOR_TAG, USER_CURSOR_TAG, encode_text_cursor};

    use crate::fixtures::{fixture_user, new_post};

    fn invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) {
        assert!(
            matches!(r, Err(StoreError::Invalid(ref m)) if m.contains("NUL")),
            "{r:?}"
        );
    }
    let store = db.store();
    let repos = store.read();
    let author = fixture_user(store).await;
    let item = repos
        .content()
        .insert(&new_post(&author, "nul-lookups"))
        .await
        .unwrap();
    const BAD: &str = "a\0b";

    // Content family.
    invalid(repos.types().get(BAD).await);
    invalid(repos.types().delete(BAD).await);
    invalid(repos.content().get_by_slug(BAD, None, "x").await);
    invalid(repos.content().get_by_slug("post", None, BAD).await);
    invalid(
        repos
            .content()
            .list(&ContentQuery {
                kind: Some(BAD.into()),
                ..ContentQuery::default()
            })
            .await,
    );
    invalid(
        repos
            .content()
            .list(&ContentQuery {
                order: ContentOrder::TitleAsc,
                cursor: Some(encode_text_cursor("c:title", BAD, uuid::Uuid::now_v7())),
                ..ContentQuery::default()
            })
            .await,
    );
    invalid(repos.meta().get(item.id, BAD).await);
    invalid(repos.meta().delete(item.id, BAD).await);
    // Taxonomies and terms.
    invalid(repos.taxonomies().get(BAD).await);
    invalid(repos.terms().get_by_slug(BAD, "x").await);
    invalid(repos.terms().get_by_slug("tag", BAD).await);
    invalid(repos.terms().list(BAD).await);
    // Menus.
    invalid(repos.menus().get(BAD).await);
    invalid(repos.menus().delete(BAD).await);
    // Site.
    invalid(repos.options().get(BAD).await);
    invalid(repos.options().delete(BAD).await);
    invalid(repos.schema_meta().get(BAD).await);
    invalid(repos.schema_meta().set(BAD, "v").await);
    invalid(repos.redirects().get_by_source(BAD).await);
    invalid(
        repos
            .redirects()
            .list(
                10,
                Some(&encode_text_cursor(
                    REDIRECT_CURSOR_TAG,
                    BAD,
                    uuid::Uuid::now_v7(),
                )),
            )
            .await,
    );
    // Users.
    invalid(repos.users().get_by_login(BAD).await);
    invalid(repos.users().get_by_email(BAD).await);
    invalid(
        repos
            .users()
            .list(
                10,
                Some(&encode_text_cursor(
                    USER_CURSOR_TAG,
                    BAD,
                    uuid::Uuid::now_v7(),
                )),
            )
            .await,
    );
    // Plugins, plugin KV and themes.
    invalid(repos.plugins().get(BAD).await);
    invalid(repos.plugins().delete(BAD).await);
    invalid(repos.plugin_kv().get(BAD, "k").await);
    invalid(repos.plugin_kv().get("p", BAD).await);
    invalid(repos.plugin_kv().list_keys(BAD, "", 10).await);
    invalid(repos.plugin_kv().list_keys("p", BAD, 10).await);
    invalid(repos.plugin_kv().usage(BAD).await);
    invalid(repos.themes().get(BAD).await);
    invalid(repos.themes().activate(BAD).await);
    // Search.
    let search = |q: &str, kind: &str, cursor: Option<String>| SearchQuery {
        q: q.into(),
        kinds: vec![kind.into()],
        statuses: Vec::new(),
        readable_by: None,
        limit: 10,
        cursor,
    };
    invalid(repos.search().search(&search(BAD, "post", None)).await);
    invalid(repos.search().search(&search("x", BAD, None)).await);
    // Batch loaders.
    invalid(repos.meta().get_many(&[ContentId::new()], &[BAD]).await);
}
