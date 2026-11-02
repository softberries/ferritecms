//! Imported content items (M3-1 Task 3, P4, P6, P7, P8): explicit status, dates, author
//! and slug, re-run semantics and the link fix-up, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeSet, sync::Arc};

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, ExtensionService, PermalinkRules,
    content::{Robots, SeoPatch},
    events::{INVALIDATE, archive_key, post_key, term_key},
    import::{ContentOutcome, ImportContent, ImportService, WP_ID_META, Written},
    jobs::{PLUGIN_ACTION_KIND, PUBLISH_SCHEDULED_KIND, publish_scheduled_job},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, ImportMapKind, InstalledPlugin,
    JobQuery, NewUser, NodeId, Redirect, RedirectId, RedirectOrigin, Role, SearchQuery, Term,
    TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

const SOURCE: &str = "https://wp.example";

async fn user(store: &dyn Store, role: Role) -> UserId {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.test"),
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
    Actor::new(user(store, Role::Admin).await, Role::Admin)
}

fn permalinks() -> Arc<PermalinkRules> {
    Arc::new(PermalinkRules::default())
}

fn importer(db: &TestDb) -> ImportService {
    ImportService::new(db.shared_store(), permalinks())
}

fn content(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), permalinks())
}

fn micros(m: i64) -> Timestamp {
    Timestamp::from_micros(m).unwrap()
}

/// `now + secs` (negative: in the past).
fn from_now(secs: i64) -> Timestamp {
    Timestamp::from(Timestamp::now().as_datetime() + chrono::Duration::seconds(secs))
}

fn body(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
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

/// A published post with fixed dates; tests change what they need.
fn item(wp_id: i64, slug: &str, author: UserId) -> ImportContent {
    ImportContent {
        wp_id,
        id_meta: WP_ID_META,
        hash: format!("hash-{wp_id}"),
        kind: "post".into(),
        title: format!("Title {slug}"),
        slug: Some(slug.into()),
        parent: None,
        author,
        status: ContentStatus::Published,
        published_at: Some(micros(1_600_000_000_000_000)),
        created_at: micros(1_600_000_000_000_000),
        updated_at: micros(1_600_000_100_000_000),
        excerpt: None,
        body: body(&format!("Body of {slug}")),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
        meta: Vec::new(),
    }
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

async fn revisions(store: &dyn Store, id: ContentId) -> usize {
    store.read().revisions().list(id, 100).await.unwrap().len()
}

async fn jobs_of(store: &dyn Store, kind: &str) -> Vec<ferrite_domain::Job> {
    store
        .read()
        .jobs()
        .list(&JobQuery {
            status: None,
            kind: Some(kind.into()),
            limit: 200,
            cursor: None,
        })
        .await
        .unwrap()
        .items
}

/// Imports `items` in one committed batch of a fresh run.
async fn import(db: &TestDb, actor: &Actor, items: Vec<ImportContent>) -> Vec<ContentOutcome> {
    let svc = importer(db);
    let run = svc.start(actor, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let mut out = Vec::new();
    for c in items {
        out.push(batch.content(c).await.unwrap());
    }
    batch.commit().await.unwrap();
    run.finish(&svc, ferrite_domain::ImportRunState::Done, json!({}))
        .await
        .unwrap();
    out
}

/// An item written by the batch has what a service-created and published item has: one
/// revision, a search entry, the same invalidation keys and permalink shape; only the
/// dates differ (as set).
pub async fn imported_item_matches_service_item(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let cat = term(store, "news").await;
    let svc = content(db);

    let seq = store.read().events().max_seq().await.unwrap();
    let created = svc
        .create(
            &admin,
            ContentDraft {
                kind: "post".into(),
                title: "Service".into(),
                slug: Some("service".into()),
                parent_id: None,
                excerpt: None,
                body: body("servicezebra words"),
                fields: json!({}),
                menu_order: 0,
                terms: vec![cat],
                seo: None,
            },
        )
        .await
        .unwrap();
    let published = svc
        .publish(&admin, created.id, created.version)
        .await
        .unwrap();
    let service_keys: BTreeSet<String> = invalidations_since(store, seq)
        .await
        .into_iter()
        .flatten()
        .map(|k| k.replace(&created.id.to_string(), "ID"))
        .collect();

    // A redirect from the path the item will take is reclaimed, as `publish` does.
    store
        .read()
        .redirects()
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: "/imported/".into(),
            target: "/elsewhere/".into(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::Manual,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();
    let seq = store.read().events().max_seq().await.unwrap();
    let mut c = item(1, "imported", admin.user);
    c.body = body("importzebra words");
    c.terms = vec![cat];
    let out = import(db, &admin, vec![c]).await;
    assert_eq!(out[0].result, Written::Created);
    assert_eq!(out[0].slug_changed, None);
    assert!(!out[0].schedule_past);
    let id = out[0].id;
    let events = invalidations_since(store, seq).await;
    assert_eq!(events.len(), 1, "{events:?}");
    // The reclaimed redirect's keys join the batch's one event.
    for key in ["redirect:/imported/", "redirect:/imported"] {
        assert!(events[0].contains(&key.to_owned()), "{events:?}");
    }
    let import_keys: BTreeSet<String> = events[0]
        .iter()
        .filter(|k| !k.starts_with("redirect:"))
        .map(|k| k.replace(&id.to_string(), "ID"))
        .collect();
    assert_eq!(import_keys, service_keys);
    assert!(import_keys.contains(&term_key(cat)));

    let imported = svc.get(id).await.unwrap();
    assert_eq!(imported.status, published.status);
    assert_eq!(imported.kind, published.kind);
    assert_eq!(imported.excerpt, "importzebra words");
    assert_eq!(imported.published_at, Some(micros(1_600_000_000_000_000)));
    assert_eq!(imported.version, 1);
    assert_eq!(search(store, "servicezebra").await, [created.id]);
    assert_eq!(search(store, "importzebra").await, [id]);
    assert_eq!(revisions(store, created.id).await, 1);
    assert_eq!(revisions(store, id).await, 1);
    assert_eq!(svc.permalink(created.id).await.unwrap(), "/service/");
    assert_eq!(svc.permalink(id).await.unwrap(), "/imported/");
    assert_eq!(
        store
            .read()
            .redirects()
            .get_by_source("/imported/")
            .await
            .unwrap(),
        None
    );
    let terms: Vec<TermId> = store
        .read()
        .terms()
        .for_item(id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(terms, [cat]);
    // The WP id is in the item's meta (P3) and in the map with the written version.
    assert_eq!(
        store.read().meta().get(id, WP_ID_META).await.unwrap(),
        Some(json!(1))
    );
    let row = store
        .read()
        .imports()
        .map_get(SOURCE, ImportMapKind::Content, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.ferrite_id, row.hash, row.version),
        (id.as_uuid(), "hash-1".into(), 1)
    );
}

/// P7: published, private and draft items keep their dates and author exactly.
pub async fn explicit_dates_author_status(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let author = user(store, Role::Author).await;
    let mut private = item(1, "private-one", author);
    private.status = ContentStatus::Private;
    private.published_at = Some(micros(1_500_000_000_000_001));
    private.created_at = micros(1_500_000_000_000_001);
    private.updated_at = micros(1_500_000_900_000_002);
    let mut published = item(2, "published-one", author);
    published.published_at = Some(micros(1_400_000_000_000_003));
    published.created_at = micros(1_400_000_000_000_003);
    published.updated_at = micros(1_400_000_000_000_004);
    let mut draft = item(3, "draft-one", author);
    draft.status = ContentStatus::Draft;
    draft.published_at = None;
    let mut seo = item(4, "with-seo", author);
    seo.excerpt = Some("Hand-written".into());
    seo.seo = Some(SeoPatch {
        title: Some(Some("SEO title".into())),
        description: None,
        canonical: Some(Some("https://example.org/orig/".into())),
        robots: Some(Some(vec![Robots::NoIndex])),
    });
    seo.meta = vec![("wp.quotes".into(), json!(["a"]))];
    let wanted = [
        private.clone(),
        published.clone(),
        draft.clone(),
        seo.clone(),
    ];
    let out = import(db, &admin, vec![private, published, draft, seo]).await;
    for (c, o) in wanted.iter().zip(&out) {
        let got = store.read().content().get(o.id).await.unwrap().unwrap();
        assert_eq!(got.status, c.status, "{}", c.title);
        assert_eq!(got.published_at, c.published_at, "{}", c.title);
        assert_eq!(got.created_at, c.created_at, "{}", c.title);
        assert_eq!(got.updated_at, c.updated_at, "{}", c.title);
        assert_eq!(got.author_id, author, "{}", c.title);
        assert_eq!(got.scheduled_at, None);
    }
    let seo_id = out[3].id;
    let got = store.read().content().get(seo_id).await.unwrap().unwrap();
    assert_eq!(got.excerpt, "Hand-written");
    let meta = store.read().meta();
    assert_eq!(
        meta.get(seo_id, "seo.title").await.unwrap(),
        Some(json!("SEO title"))
    );
    assert_eq!(
        meta.get(seo_id, "seo.canonical").await.unwrap(),
        Some(json!("https://example.org/orig/"))
    );
    assert_eq!(
        meta.get(seo_id, "seo.robots").await.unwrap(),
        Some(json!(["noindex"]))
    );
    assert_eq!(
        meta.get(seo_id, "wp.quotes").await.unwrap(),
        Some(json!(["a"]))
    );
    assert_eq!(meta.get(seo_id, WP_ID_META).await.unwrap(), Some(json!(4)));

    // A reserved meta key or an unknown author fails the item.
    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let mut bad = item(5, "bad-meta", author);
    bad.meta = vec![("import.other".into(), json!(1))];
    let err = batch.content(bad).await.unwrap_err();
    assert!(matches!(err, CoreError::InvalidKey { .. }), "{err:?}");
    batch.rollback().await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let err = batch
        .content(item(6, "nobody", UserId::new()))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "author",
                ..
            }
        ),
        "{err:?}"
    );
    batch.rollback().await.unwrap();
}

/// P7: `future` with a date ahead is scheduled with its `content.publish_scheduled` job.
pub async fn future_date_schedules_with_job(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let at = from_now(3600);
    let mut c = item(1, "later", admin.user);
    c.status = ContentStatus::Scheduled;
    c.published_at = Some(at);
    let out = import(db, &admin, vec![c]).await;
    assert!(!out[0].schedule_past);
    let got = store
        .read()
        .content()
        .get(out[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status, ContentStatus::Scheduled);
    assert_eq!(got.scheduled_at, Some(at));
    assert_eq!(got.published_at, None);
    let jobs = jobs_of(store, PUBLISH_SCHEDULED_KIND).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let want = publish_scheduled_job(out[0].id, at);
    assert_eq!(jobs[0].unique_key, want.unique_key);
    assert_eq!(jobs[0].run_at, at);
    // Scheduled without a date is refused.
    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let mut c = item(2, "undated", admin.user);
    c.status = ContentStatus::Scheduled;
    c.published_at = None;
    let err = batch.content(c).await.unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
    batch.rollback().await.unwrap();
}

/// P7: `future` whose date has passed (WP missed it) is published at that date and
/// flagged for the report; no job.
pub async fn past_future_is_published_and_flagged(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let at = from_now(-3600);
    let mut c = item(1, "missed", admin.user);
    c.status = ContentStatus::Scheduled;
    c.published_at = Some(at);
    let out = import(db, &admin, vec![c]).await;
    assert!(out[0].schedule_past);
    assert_eq!(out[0].result, Written::Created);
    let got = store
        .read()
        .content()
        .get(out[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status, ContentStatus::Published);
    assert_eq!(got.published_at, Some(at));
    assert_eq!(got.scheduled_at, None);
    assert!(jobs_of(store, PUBLISH_SCHEDULED_KIND).await.is_empty());
}

/// P8: the WP slug is kept when free; a taken or reserved one gets core's `-N`, and the
/// slug used is reported. An empty WP slug derives one from the title, unreported.
pub async fn exact_slug_or_reported_suffix(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let mut untitled = item(4, "", admin.user);
    untitled.slug = None;
    untitled.title = "From The Title".into();
    let out = import(
        db,
        &admin,
        vec![
            item(1, "x", admin.user),
            item(2, "x", admin.user),
            item(3, "admin", admin.user),
            untitled,
        ],
    )
    .await;
    assert_eq!(out[0].slug_changed, None);
    assert_eq!(out[1].slug_changed.as_deref(), Some("x-2"));
    assert_eq!(out[2].slug_changed.as_deref(), Some("admin-2"));
    assert_eq!(out[3].slug_changed, None);
    let slugs: Vec<String> = {
        let mut v = Vec::new();
        for o in &out {
            v.push(
                store
                    .read()
                    .content()
                    .get(o.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .slug,
            );
        }
        v
    };
    assert_eq!(slugs, ["x", "x-2", "admin-2", "from-the-title"]);
}

/// P4: same hash → unchanged; new hash → updated (dates as given); edited in FerriteCMS
/// since → kept, body untouched; mapped item deleted → created anew.
pub async fn rerun_semantics(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = content(db);
    let first = import(db, &admin, vec![item(1, "entry", admin.user)]).await;
    let id = first[0].id;

    // Same hash: nothing written, no event.
    let seq = store.read().events().max_seq().await.unwrap();
    let again = import(db, &admin, vec![item(1, "entry", admin.user)]).await;
    assert_eq!((again[0].id, again[0].result), (id, Written::Unchanged));
    assert!(invalidations_since(store, seq).await.is_empty());
    assert_eq!(revisions(store, id).await, 1);

    // New hash: updated in place, with the new values and dates.
    let mut changed = item(1, "entry", admin.user);
    changed.hash = "hash-1b".into();
    changed.title = "New title".into();
    changed.body = body("updatedzebra text");
    changed.updated_at = micros(1_700_000_000_000_000);
    let seq = store.read().events().max_seq().await.unwrap();
    let out = import(db, &admin, vec![changed]).await;
    assert_eq!((out[0].id, out[0].result), (id, Written::Updated));
    let got = svc.get(id).await.unwrap();
    assert_eq!(got.title, "New title");
    assert_eq!(got.slug, "entry");
    assert_eq!(got.updated_at, micros(1_700_000_000_000_000));
    assert_eq!(got.created_at, micros(1_600_000_000_000_000));
    assert_eq!(got.version, 2);
    assert_eq!(search(store, "updatedzebra").await, [id]);
    assert_eq!(revisions(store, id).await, 2);
    let events = invalidations_since(store, seq).await;
    assert_eq!(events.len(), 1);
    assert!(events[0].contains(&post_key(id)));
    assert!(events[0].contains(&archive_key("post")));
    let row = store
        .read()
        .imports()
        .map_get(SOURCE, ImportMapKind::Content, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.hash.as_str(), row.version), ("hash-1b", 2));

    // Edited in FerriteCMS: a changed WP item is kept as edited.
    let edited = svc
        .update(
            &admin,
            id,
            got.version,
            ContentDraft {
                kind: "post".into(),
                title: "Local title".into(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: body("local edit"),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    let mut later = item(1, "entry", admin.user);
    later.hash = "hash-1c".into();
    later.body = body("from wordpress again");
    let out = import(db, &admin, vec![later]).await;
    assert_eq!((out[0].id, out[0].result), (id, Written::KeptEdited));
    let got = svc.get(id).await.unwrap();
    assert_eq!(got.body, body("local edit"));
    assert_eq!(got.version, edited.version);

    // Deleted: the stale map row is not a match; the item is created anew.
    let trashed = svc.trash(&admin, id, got.version).await.unwrap();
    svc.delete(&admin, id, trashed.version).await.unwrap();
    let out = import(db, &admin, vec![item(1, "entry", admin.user)]).await;
    assert_eq!(out[0].result, Written::Created);
    assert_ne!(out[0].id, id);
    assert_eq!(svc.get(out[0].id).await.unwrap().slug, "entry");
}

/// An update that moves a published item's path records the redirect as a service
/// update does; its keys join the batch's one event.
pub async fn rerun_slug_change_redirects(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let out = import(db, &admin, vec![item(1, "old-slug", admin.user)]).await;
    let mut moved = item(1, "new-slug", admin.user);
    moved.hash = "hash-1b".into();
    let seq = store.read().events().max_seq().await.unwrap();
    let again = import(db, &admin, vec![moved]).await;
    assert_eq!(
        (again[0].id, again[0].result),
        (out[0].id, Written::Updated)
    );
    assert_eq!(again[0].slug_changed, None);
    let r = store
        .read()
        .redirects()
        .get_by_source("/old-slug/")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((r.target.as_str(), r.code), ("/new-slug/", 301));
    let events = invalidations_since(store, seq).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].contains(&"redirect:/old-slug/".to_owned()),
        "{events:?}"
    );
}

/// A re-run that renames an item and also unpublishes it records no redirect: the new
/// path is not live.
pub async fn rerun_rename_and_unpublish_records_no_redirect(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let out = import(db, &admin, vec![item(1, "was-live", admin.user)]).await;
    let mut draft = item(1, "now-draft", admin.user);
    draft.hash = "hash-1b".into();
    draft.status = ContentStatus::Draft;
    let again = import(db, &admin, vec![draft]).await;
    assert_eq!(
        (again[0].id, again[0].result),
        (out[0].id, Written::Updated)
    );
    let got = store
        .read()
        .content()
        .get(out[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (got.status, got.slug.as_str()),
        (ContentStatus::Draft, "now-draft")
    );
    let redirects = store.read().redirects();
    assert_eq!(redirects.get_by_source("/was-live/").await.unwrap(), None);
    assert_eq!(redirects.get_by_source("/was-live").await.unwrap(), None);
}

/// `set_body_and_fields` (the link fix-up) rewrites the body, keeps `updated_at` and moves the
/// map's version, so the item still counts as not edited locally. An item edited in
/// FerriteCMS is `Conflict` and left alone.
pub async fn set_body_keeps_item_unedited(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = content(db);
    let out = import(db, &admin, vec![item(1, "linked", admin.user)]).await;
    let id = out[0].id;

    let imp = importer(db);
    let run = imp.start(&admin, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&imp).await.unwrap();
    batch
        .set_body_and_fields(id, 1, body("fixedzebra link"), json!({}))
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    batch.commit().await.unwrap();
    run.finish(&imp, ferrite_domain::ImportRunState::Done, json!({}))
        .await
        .unwrap();
    let got = svc.get(id).await.unwrap();
    assert_eq!(got.body, body("fixedzebra link"));
    assert_eq!(got.version, 2);
    assert_eq!(got.updated_at, micros(1_600_000_100_000_000));
    assert_eq!(search(store, "fixedzebra").await, [id]);

    // Still "not edited": a changed WP item updates it.
    let mut changed = item(1, "linked", admin.user);
    changed.hash = "hash-1b".into();
    let out = import(db, &admin, vec![changed]).await;
    assert_eq!(out[0].result, Written::Updated);

    // Edited locally: refused, nothing written.
    let got = svc.get(id).await.unwrap();
    let edited = svc
        .update(
            &admin,
            id,
            got.version,
            ContentDraft {
                kind: "post".into(),
                title: got.title.clone(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: body("local"),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    let run = imp.start(&admin, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&imp).await.unwrap();
    let err = batch
        .set_body_and_fields(id, 1, body("fix"), json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "{err:?}");
    // Any error: the caller rolls the batch back and opens a new one.
    batch.rollback().await.unwrap();
    // Unmapped WP id, or a mapped one naming another item: NotFound.
    for (other, wp_id) in [(id, 99), (ContentId::new(), 1)] {
        let mut batch = run.batch(&imp).await.unwrap();
        let err = batch
            .set_body_and_fields(other, wp_id, body("fix"), json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound), "{err:?}");
        batch.rollback().await.unwrap();
    }
    assert_eq!(svc.get(id).await.unwrap().version, edited.version);
}

/// `set_body_and_fields` (the fix-up of links and relations) writes both in one
/// version, checks the fields, rewrites the search entry and refuses an item edited
/// here; `path` gives an imported item's path under the batch's rules.
pub async fn set_body_and_fields_and_path(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = content(db);
    let out = import(db, &admin, vec![item(1, "linked", admin.user)]).await;
    let id = out[0].id;

    let imp = importer(db);
    let run = imp.start(&admin, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&imp).await.unwrap();
    let path = batch.path(id).await.unwrap().unwrap();
    assert!(
        path.starts_with('/') && path.ends_with("/linked/"),
        "{path}"
    );
    assert_eq!(batch.path(ContentId::new()).await.unwrap(), None);
    let wrote = batch
        .set_body_and_fields(id, 1, body("quokkafixed"), json!({"x": 1}))
        .await
        .unwrap();
    assert!(wrote);
    // The same again writes nothing.
    let wrote = batch
        .set_body_and_fields(id, 1, body("quokkafixed"), json!({"x": 1}))
        .await
        .unwrap();
    assert!(!wrote);
    assert_eq!(batch.len(), 1);
    batch.commit().await.unwrap();
    let got = svc.get(id).await.unwrap();
    assert_eq!(got.body, body("quokkafixed"));
    assert_eq!(got.fields, json!({"x": 1}));
    assert_eq!(got.version, 2, "one version for both");
    assert_eq!(got.updated_at, micros(1_600_000_100_000_000));
    assert_eq!(search(store, "quokkafixed").await, [id]);

    // Fields the type refuses: `InvalidField`, nothing written.
    let mut batch = run.batch(&imp).await.unwrap();
    let err = batch
        .set_body_and_fields(id, 1, body("x"), json!(5))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::InvalidField { .. }), "{err:?}");
    batch.rollback().await.unwrap();

    // Edited here since: `Conflict`, nothing written.
    let edited = svc
        .update(
            &admin,
            id,
            got.version,
            ContentDraft {
                kind: "post".into(),
                title: got.title.clone(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: body("local"),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    let mut batch = run.batch(&imp).await.unwrap();
    let err = batch
        .set_body_and_fields(id, 1, body("fix"), json!({"x": 2}))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "{err:?}");
    batch.rollback().await.unwrap();
    run.finish(&imp, ferrite_domain::ImportRunState::Done, json!({}))
        .await
        .unwrap();
    let got = svc.get(id).await.unwrap();
    assert_eq!((got.version, got.body), (edited.version, body("local")));
}

/// Installs an enabled plugin subscribed to the content action hooks.
async fn subscriber(store: Arc<dyn Store>) {
    let artifact = Artifact {
        hash: ArtifactHash::parse(&"01".repeat(32)).unwrap(),
        kind: ArtifactKind::Plugin,
        bytes: 1,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    let now = Timestamp::now();
    let svc = ExtensionService::new(store);
    svc.install_plugin(
        &artifact,
        &InstalledPlugin {
            name: "sub".into(),
            version: "0.1.0".into(),
            artifact_hash: artifact.hash.clone(),
            manifest: json!({"plugin": {"name": "sub"},
                "hooks": {"post.saved": 10, "post.published": 10}}),
            granted: Value::Null,
            enabled: true,
            disabled_reason: None,
            installed_at: now,
            updated_at: now,
        },
    )
    .await
    .unwrap();
    svc.set_plugin_enabled("sub", true).await.unwrap();
}

/// P6: imported items enqueue no `post.saved`/`post.published` actions.
pub async fn no_plugin_actions(db: &TestDb) {
    let store = db.store();
    subscriber(db.shared_store()).await;
    let admin = admin(store).await;
    let mut scheduled = item(3, "three", admin.user);
    scheduled.status = ContentStatus::Scheduled;
    scheduled.published_at = Some(from_now(3600));
    let out = import(
        db,
        &admin,
        vec![
            item(1, "one", admin.user),
            item(2, "two", admin.user),
            scheduled,
        ],
    )
    .await;
    let imp = importer(db);
    let run = imp.start(&admin, SOURCE, false, false, None).await.unwrap();
    let mut batch = run.batch(&imp).await.unwrap();
    batch
        .set_body_and_fields(out[0].id, 1, body("fixed"), json!({}))
        .await
        .unwrap();
    let mut changed = item(2, "two", admin.user);
    changed.hash = "new".into();
    assert_eq!(
        batch.content(changed).await.unwrap().result,
        Written::Updated
    );
    batch.commit().await.unwrap();
    assert!(jobs_of(store, PLUGIN_ACTION_KIND).await.is_empty());

    // The subscriber is live: a service save does enqueue.
    content(db)
        .create(
            &admin,
            ContentDraft {
                kind: "post".into(),
                title: "Service".into(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: body("x"),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    let claimed = store
        .read()
        .jobs()
        .claim(
            NodeId::new(),
            &[PLUGIN_ACTION_KIND.to_owned()],
            100,
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1, "{claimed:?}");
}

backend_tests!(
    imported_item_matches_service_item,
    explicit_dates_author_status,
    future_date_schedules_with_job,
    past_future_is_published_and_flagged,
    exact_slug_or_reported_suffix,
    rerun_semantics,
    rerun_slug_change_redirects,
    rerun_rename_and_unpublish_records_no_redirect,
    set_body_keeps_item_unedited,
    set_body_and_fields_and_path,
    no_plugin_actions,
);
