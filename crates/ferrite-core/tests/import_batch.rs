//! The import writer's run fence and batch shell (M3-1 Task 2, P5, P6, P8, P9), on both
//! backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, CoreError, NewType, PermalinkRules,
    events::{ALL_KEY, INVALIDATE, TYPES_CHANGED, term_key},
    fields::{FieldDef, FieldKind},
    import::{
        ImportBatch, ImportService, ImportTerm, ImportUser, Matched, TermMatched, WP_ID_META,
    },
};
use ferrite_domain::{
    ImportMapKind, ImportRunState, NewUser, Role, Taxonomy, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::{LOCK_IMPORT, Store};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

const SOURCE: &str = "https://wp.example";

async fn user(store: &dyn Store, login: &str, email: &str, role: Role) -> UserId {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: login.into(),
            email: email.into(),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn admin(store: &dyn Store) -> Actor {
    let id = UserId::new();
    Actor::new(
        user(
            store,
            &format!("admin-{id}"),
            &format!("{id}@example.test"),
            Role::Admin,
        )
        .await,
        Role::Admin,
    )
}

fn service(db: &TestDb) -> ImportService {
    ImportService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

/// `now - secs`.
fn ago(secs: i64) -> Timestamp {
    Timestamp::from(Timestamp::now().as_datetime() - chrono::Duration::seconds(secs))
}

/// The keys of every `invalidate` event after `seq`.
async fn invalidations_since(store: &dyn Store, seq: i64) -> Vec<Vec<String>> {
    store
        .read()
        .events()
        .since(seq, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == INVALIDATE)
        .map(|e| e.keys)
        .collect()
}

fn term(wp_id: i64, slug: &str, parent: Option<TermId>) -> ImportTerm {
    ImportTerm {
        wp_id,
        taxonomy: "category".into(),
        slug: slug.into(),
        name: format!("Name {slug}"),
        description: String::new(),
        parent,
    }
}

/// Only an actor holding `import` may start a run.
pub async fn start_requires_import_capability(db: &TestDb) {
    let store = db.store();
    let editor = Actor::new(
        user(store, "ed", "ed@example.test", Role::Editor).await,
        Role::Editor,
    );
    let err = service(db)
        .start(&editor, SOURCE, false, false)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, CoreError::Forbidden("import")), "{err:?}");
    assert_eq!(store.read().imports().running().await.unwrap(), None);
}

/// A second start is busy while the first run's heartbeat is fresh (with or without
/// takeover) and, without takeover, also once it is stale. A takeover of a stale run
/// abandons it; the old handle's batch and finish are then refused, and its finish does
/// not overwrite `abandoned`.
pub async fn second_start_is_busy_and_takeover_fences_the_first(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let a = svc.start(&admin, SOURCE, false, false).await.unwrap();
    assert_eq!(a.run.state, ImportRunState::Running);
    assert_eq!(a.run.source, SOURCE);
    assert_eq!(a.run.started_by, admin.user);

    for takeover in [false, true] {
        let err = svc
            .start(&admin, SOURCE, false, takeover)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, CoreError::Unavailable(_)), "{err:?}");
    }

    // Age A's heartbeat past the 120 s limit.
    assert!(
        store
            .read()
            .imports()
            .fence(a.run.id, &a.run.token, ago(121))
            .await
            .unwrap()
    );
    let err = svc.start(&admin, SOURCE, false, false).await.err().unwrap();
    assert!(matches!(err, CoreError::Unavailable(_)), "{err:?}");

    let b = svc.start(&admin, SOURCE, true, true).await.unwrap();
    assert_ne!(a.run.token, b.run.token);
    assert!(b.run.dry_run);
    let old = store
        .read()
        .imports()
        .get_run(a.run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.state, ImportRunState::Abandoned);

    let err = a.batch(&svc).await.err().unwrap();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    let a_id = a.run.id;
    let err = a
        .finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    let old = store.read().imports().get_run(a_id).await.unwrap().unwrap();
    assert_eq!(old.state, ImportRunState::Abandoned);

    // B's batches pass the fence; finishing B frees the slot.
    let batch = b.batch(&svc).await.unwrap();
    batch.commit().await.unwrap();
    let stats = json!({"items": 0});
    b.finish(&svc, ImportRunState::Done, stats.clone())
        .await
        .unwrap();
    assert_eq!(store.read().imports().running().await.unwrap(), None);
    let runs = store.read().imports().list_runs(10).await.unwrap();
    let done = runs
        .iter()
        .find(|r| r.state == ImportRunState::Done)
        .unwrap();
    assert_eq!(done.stats, stats);
    let c = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let err = c
        .finish(&svc, ImportRunState::Running, json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
}

/// Three terms in one batch make one `invalidate` event holding all three term keys.
pub async fn batch_emits_one_event(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let seq = store.read().events().max_seq().await.unwrap();

    let mut batch = run.batch(&svc).await.unwrap();
    let mut ids = Vec::new();
    for (wp_id, slug) in [(1, "one"), (2, "two"), (3, "three")] {
        ids.push(batch.term(term(wp_id, slug, None)).await.unwrap().id);
    }
    assert_eq!(batch.len(), 3);
    batch.commit().await.unwrap();

    let events = invalidations_since(store, seq).await;
    assert_eq!(events.len(), 1, "{events:?}");
    for id in &ids {
        assert!(events[0].contains(&term_key(*id)), "{events:?}");
    }

    // A second batch reuses the mapped terms: no new rows, no new event.
    let seq = store.read().events().max_seq().await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let again = batch.term(term(2, "two", None)).await.unwrap();
    assert_eq!((again.id, again.how), (ids[1], TermMatched::Mapped));
    let mapped = batch.lookup(ImportMapKind::Term, 2).await.unwrap().unwrap();
    assert_eq!(mapped.id, ids[1].as_uuid());
    assert_eq!(batch.len(), 0);
    batch.commit().await.unwrap();
    assert!(invalidations_since(store, seq).await.is_empty());
    assert_eq!(
        store.read().terms().list("category").await.unwrap().len(),
        3
    );
}

/// P9: e-mail first (case-insensitively), then login; otherwise a new author with no
/// password who must reset it. A WP login core refuses is slugified with `-N`.
pub async fn user_matching_order(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let by_email = user(store, "someone", "x@e.test", Role::Editor).await;
    let by_login = user(store, "wpuser", "other@e.test", Role::Editor).await;
    user(store, "john-doe", "john@e.test", Role::Editor).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();

    let out = batch
        .user(ImportUser {
            wp_id: 1,
            login: "wpuser".into(),
            email: "X@E.test".into(),
            display_name: "X".into(),
        })
        .await
        .unwrap();
    assert_eq!((out.id, out.how), (by_email, Matched::Email));

    let out = batch
        .user(ImportUser {
            wp_id: 2,
            login: "WPUser".into(),
            email: "new@e.test".into(),
            display_name: "W".into(),
        })
        .await
        .unwrap();
    assert_eq!((out.id, out.how), (by_login, Matched::Login));

    let out = batch
        .user(ImportUser {
            wp_id: 3,
            login: "fresh".into(),
            email: "fresh@e.test".into(),
            display_name: "Fresh Author".into(),
        })
        .await
        .unwrap();
    assert_eq!(out.how, Matched::Created);
    let created = out.id;

    let out = batch
        .user(ImportUser {
            wp_id: 4,
            login: "John Doe".into(),
            email: "jd@e.test".into(),
            display_name: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(out.how, Matched::Created);
    let slugged = out.id;
    let mapped = batch.lookup(ImportMapKind::User, 3).await.unwrap().unwrap();
    assert_eq!(mapped.id, created.as_uuid());
    batch.commit().await.unwrap();

    let repos = store.read();
    let fresh = repos.users().get(created).await.unwrap().unwrap();
    assert_eq!(fresh.login, "fresh");
    assert_eq!(fresh.email, "fresh@e.test");
    assert_eq!(fresh.display_name, "Fresh Author");
    assert_eq!(fresh.role, Role::Author);
    assert!(fresh.must_reset);
    assert!(fresh.password_hash.is_none());
    assert_eq!(fresh.status, UserStatus::Active);
    let jd = repos.users().get(slugged).await.unwrap().unwrap();
    assert_eq!(jd.login, "john-doe-2");
    assert_eq!(jd.display_name, "john-doe-2");
}

/// P8: a new term keeps its WP slug and parent; a WP term whose slug an existing site
/// term of the taxonomy has maps to that term (left as it is, no new term).
pub async fn term_keeps_slug_and_parent(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let existing = ferrite_domain::Term {
        id: TermId::new(),
        taxonomy: "category".into(),
        slug: "news".into(),
        name: "Site news".into(),
        description: "kept".into(),
        parent_id: None,
    };
    let tx = store.begin().await.unwrap();
    tx.terms().insert(&existing).await.unwrap();
    tx.commit().await.unwrap();
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let parent = batch.term(term(10, "world", None)).await.unwrap();
    assert_eq!(parent.how, TermMatched::Created);
    let parent = parent.id;
    let child = batch
        .term(term(11, "Local-News", Some(parent)))
        .await
        .unwrap();
    assert_eq!(child.how, TermMatched::Created);
    let news = batch.term(term(12, "News", None)).await.unwrap();
    assert_eq!((news.id, news.how), (existing.id, TermMatched::Slug));
    let mapped = batch
        .lookup(ImportMapKind::Term, 12)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapped.id, existing.id.as_uuid());
    assert_eq!(batch.len(), 2);
    batch.commit().await.unwrap();

    let terms = store.read().terms();
    let child = terms.get(child.id).await.unwrap().unwrap();
    assert_eq!(child.slug, "local-news");
    assert_eq!(child.parent_id, Some(parent));
    assert_eq!(child.name, "Name Local-News");
    assert_eq!(terms.get(parent).await.unwrap().unwrap().slug, "world");
    assert_eq!(terms.get(existing.id).await.unwrap().unwrap(), existing);
    assert_eq!(terms.list("category").await.unwrap().len(), 3);
}

/// A map row whose term now sits in another taxonomy than the WP term's is not used:
/// the term is matched again in its own taxonomy and the row replaced.
pub async fn mapped_term_of_other_taxonomy_is_not_used(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let cat = batch.term(term(40, "shared", None)).await.unwrap().id;
    let tag = batch
        .term(ImportTerm {
            taxonomy: "tag".into(),
            ..term(40, "shared", None)
        })
        .await
        .unwrap();
    assert_eq!(tag.how, TermMatched::Created);
    assert_ne!(tag.id, cat);
    let row = batch
        .lookup(ImportMapKind::Term, 40)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.id, tag.id.as_uuid());
    let back = batch.term(term(40, "shared", None)).await.unwrap();
    assert_eq!((back.id, back.how), (cat, TermMatched::Slug));
    batch.commit().await.unwrap();
}

/// A batch dropped (or rolled back) without commit writes no row, map entry or event.
pub async fn rolled_back_batch_writes_nothing(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let seq = store.read().events().max_seq().await.unwrap();
    let before = store.read().terms().list("category").await.unwrap().len();

    let mut batch = run.batch(&svc).await.unwrap();
    batch.term(term(20, "dropped", None)).await.unwrap();
    drop(batch);
    let mut batch = run.batch(&svc).await.unwrap();
    batch.term(term(21, "rolled", None)).await.unwrap();
    batch.rollback().await.unwrap();

    assert_eq!(
        store.read().terms().list("category").await.unwrap().len(),
        before
    );
    let imports = store.read().imports();
    assert_eq!(
        imports
            .map_get(SOURCE, ImportMapKind::Term, 20)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        imports
            .map_get(SOURCE, ImportMapKind::Term, 21)
            .await
            .unwrap(),
        None
    );
    assert!(
        store
            .read()
            .events()
            .since(seq, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A map row whose target is gone reads as not mapped; `remember` replaces it.
pub async fn stale_map_row_is_not_mapped(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let id = batch.term(term(30, "gone", None)).await.unwrap().id;
    batch.commit().await.unwrap();
    let tx = store.begin().await.unwrap();
    tx.terms().delete(id).await.unwrap();
    tx.commit().await.unwrap();

    let mut batch = run.batch(&svc).await.unwrap();
    assert!(
        batch
            .lookup(ImportMapKind::Term, 30)
            .await
            .unwrap()
            .is_none()
    );
    let again = batch.term(term(30, "gone", None)).await.unwrap();
    assert_eq!(again.how, TermMatched::Created);
    let again = again.id;
    assert_ne!(again, id);
    batch
        .remember(ImportMapKind::Content, 7, uuid::Uuid::now_v7(), "h", 3)
        .await
        .unwrap();
    batch.commit().await.unwrap();
    let row = store
        .read()
        .imports()
        .map_get(SOURCE, ImportMapKind::Term, 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.ferrite_id, again.as_uuid());
    // The content target does not exist: stale.
    let batch = run.batch(&svc).await.unwrap();
    assert!(
        batch
            .lookup(ImportMapKind::Content, 7)
            .await
            .unwrap()
            .is_none()
    );
}

/// Types, taxonomies and `permalink.*` options need a schema batch (it holds the
/// `types` lock). A taxonomy or type is created when absent; an existing taxonomy is
/// kept (`true` when its `hierarchical` differs); an existing type with a schema gains
/// the missing fields. The batch's single invalidate is `[all]`.
pub async fn schema_batch_registers_types_and_taxonomies(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let genre = Taxonomy {
        name: "genre".into(),
        label: "Genres".into(),
        hierarchical: false,
        content_types: vec!["post".into()],
        built_in: false,
    };
    let field = |name: &str| FieldDef {
        name: name.into(),
        label: name.into(),
        kind: FieldKind::Text,
        required: false,
        options: Vec::new(),
        fields: Vec::new(),
        types: Vec::new(),
    };
    let book = |fields: Vec<FieldDef>| NewType {
        name: "book".into(),
        label: "Books".into(),
        hierarchical: false,
        has_archive: true,
        fields: Some(fields),
    };

    let mut plain = run.batch(&svc).await.unwrap();
    let err = plain.taxonomy(genre.clone()).await.unwrap_err();
    assert!(matches!(err, CoreError::Internal(_)), "{err:?}");
    let err = plain
        .options(&[("permalink.post".into(), Some(json!("/%slug%/")))])
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Internal(_)), "{err:?}");
    plain.rollback().await.unwrap();

    let seq = store.read().events().max_seq().await.unwrap();
    let mut batch = run.schema_batch(&svc).await.unwrap();
    assert!(!batch.taxonomy(genre.clone()).await.unwrap());
    assert!(
        !batch
            .taxonomy(Taxonomy {
                label: "Other label".into(),
                ..genre.clone()
            })
            .await
            .unwrap()
    );
    assert!(
        batch
            .taxonomy(Taxonomy {
                hierarchical: true,
                ..genre.clone()
            })
            .await
            .unwrap()
    );
    batch.content_type(book(vec![field("isbn")])).await.unwrap();
    batch
        .content_type(book(vec![field("isbn"), field("pages")]))
        .await
        .unwrap();
    batch
        .options(&[
            ("site.title".into(), Some(json!("Imported"))),
            ("permalink.post".into(), Some(json!("/%year%/%slug%/"))),
        ])
        .await
        .unwrap();
    batch.commit().await.unwrap();

    let repos = store.read();
    let tax = repos.taxonomies().get("genre").await.unwrap().unwrap();
    assert!(!tax.hierarchical);
    assert_eq!(tax.label, "Genres");
    let ty = repos.types().get("book").await.unwrap().unwrap();
    let names: Vec<String> = ferrite_core::fields::schema_of(&ty.fields)
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names, ["isbn", "pages"]);
    assert_eq!(
        repos
            .options()
            .get("site.title")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!("Imported")
    );
    let events = repos.events().since(seq, 100).await.unwrap();
    let invalidations: Vec<&Vec<String>> = events
        .iter()
        .filter(|e| e.kind == INVALIDATE)
        .map(|e| &e.keys)
        .collect();
    assert_eq!(invalidations, [&vec![ALL_KEY.to_owned()]]);
    assert!(
        events
            .iter()
            .any(|e| e.kind == TYPES_CHANGED && e.keys == ["taxonomy:genre"])
    );
    assert!(
        events
            .iter()
            .any(|e| e.kind == TYPES_CHANGED && e.keys == ["type:book"])
    );
}

/// `meta` writes one value and adds the item's key to the batch's event.
pub async fn meta_is_written_with_item_key(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let author = store.read().users().get(admin.user).await.unwrap().unwrap();
    let item = ferrite_store_tests::fixtures::new_post(&author, "metapost");
    let tx = store.begin().await.unwrap();
    let saved = tx.content().insert(&item).await.unwrap();
    tx.commit().await.unwrap();
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let seq = store.read().events().max_seq().await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    for bad in ["", "seo.title", "import.other"] {
        let err = batch.meta(saved.id, bad, &json!(1)).await.unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidKey { .. }),
            "{bad}: {err:?}"
        );
    }
    batch.meta(saved.id, WP_ID_META, &json!(42)).await.unwrap();
    batch.commit().await.unwrap();
    assert_eq!(
        store
            .read()
            .meta()
            .get(saved.id, "import.wp.id")
            .await
            .unwrap(),
        Some(json!(42))
    );
    assert_eq!(
        invalidations_since(store, seq).await,
        [vec![ferrite_core::events::post_key(saved.id)]]
    );
}

/// More than 1 000 distinct keys in one batch make its one event `invalidate [all]`.
pub async fn many_keys_collapse_to_all(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let author = store.read().users().get(admin.user).await.unwrap().unwrap();
    let tx = store.begin().await.unwrap();
    let mut items = Vec::new();
    for n in 0..1001 {
        let item = ferrite_store_tests::fixtures::new_post(&author, &format!("p-{n}"));
        items.push(tx.content().insert(&item).await.unwrap().id);
    }
    tx.commit().await.unwrap();
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let seq = store.read().events().max_seq().await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    for id in &items {
        batch.meta(*id, "wp.n", &json!(1)).await.unwrap();
    }
    batch.commit().await.unwrap();
    assert_eq!(
        invalidations_since(store, seq).await,
        [vec![ALL_KEY.to_owned()]]
    );
}

/// The `import` lock is held from `start` to `finish` (Postgres: a session advisory
/// lock; SQLite: the in-process lock table, which refuses a second holder in the same
/// process just the same), and released by `finish` even when it is refused.
pub async fn import_lock_held_for_the_run(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    assert!(store.try_lock(LOCK_IMPORT).await.unwrap().is_none(), "held");
    run.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();
    let guard = store.try_lock(LOCK_IMPORT).await.unwrap().expect("free");
    guard.release().await.unwrap();

    // A finish that is refused (taken over) still releases the lock.
    let a = svc.start(&admin, SOURCE, false, false).await.unwrap();
    assert!(
        store
            .read()
            .imports()
            .fence(a.run.id, &a.run.token, ago(121))
            .await
            .unwrap()
    );
    let b = svc.start(&admin, SOURCE, false, true).await.unwrap();
    assert!(
        a.finish(&svc, ImportRunState::Done, json!({}))
            .await
            .is_err()
    );
    // B started while A held the lock, so B holds none: the lock is free now.
    let guard = store.try_lock(LOCK_IMPORT).await.unwrap().expect("free");
    guard.release().await.unwrap();
    b.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();
}

/// `heartbeat` moves the heartbeat while the run is the handle's, and is `Changed`
/// once the run was taken over.
pub async fn heartbeat_fails_after_takeover(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = service(db);
    let a = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let imports = store.read().imports();
    assert!(
        imports
            .fence(a.run.id, &a.run.token, ago(121))
            .await
            .unwrap()
    );
    a.heartbeat(&svc).await.unwrap();
    let beat = imports
        .get_run(a.run.id)
        .await
        .unwrap()
        .unwrap()
        .heartbeat_at;
    assert!(beat > ago(5), "heartbeat moved");
    // Fresh again: a takeover is refused.
    let err = svc.start(&admin, SOURCE, false, true).await.err().unwrap();
    assert!(matches!(err, CoreError::Unavailable(_)), "{err:?}");

    assert!(
        imports
            .fence(a.run.id, &a.run.token, ago(121))
            .await
            .unwrap()
    );
    let _b = svc.start(&admin, SOURCE, false, true).await.unwrap();
    let err = a.heartbeat(&svc).await.unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    assert_eq!(ImportBatch::BATCH_ITEMS, 500);
}

backend_tests!(
    start_requires_import_capability,
    second_start_is_busy_and_takeover_fences_the_first,
    batch_emits_one_event,
    user_matching_order,
    term_keeps_slug_and_parent,
    mapped_term_of_other_taxonomy_is_not_used,
    many_keys_collapse_to_all,
    import_lock_held_for_the_run,
    heartbeat_fails_after_takeover,
    rolled_back_batch_writes_nothing,
    stale_map_row_is_not_mapped,
    schema_batch_registers_types_and_taxonomies,
    meta_is_written_with_item_key,
);
