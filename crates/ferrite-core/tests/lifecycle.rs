//! ContentService lifecycle transitions on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, PermalinkRules,
    events::{archive_key, post_key, term_key},
};
use ferrite_domain::{
    ContentId, ContentStatus, NewUser, Role, SearchQuery, Term, TermId, Timestamp, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests, backend_tests_spawning};
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

fn service(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
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
            limit: 50,
            cursor: None,
        })
        .await
        .unwrap()
        .items
}

async fn max_seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

async fn revision_count(store: &dyn Store, id: ContentId) -> usize {
    store.read().revisions().list(id, 100).await.unwrap().len()
}

async fn last_event_keys(store: &dyn Store) -> Vec<String> {
    let seq = store.read().events().max_seq().await.unwrap();
    let events = store.read().events().since(seq - 1, 10).await.unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, "invalidate");
    let mut keys = last.keys.clone();
    keys.sort();
    keys
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn in_micros(offset: i64) -> Timestamp {
    Timestamp::from_micros(Timestamp::now().as_micros() + offset).unwrap()
}

const HOUR: i64 = 3_600_000_000;

fn invalid(r: Result<impl std::fmt::Debug, CoreError>) -> String {
    match r {
        Err(CoreError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Makes `id` a scheduled item due at `at`, bypassing `schedule` (which needs a future time).
async fn force_scheduled(store: &dyn Store, id: ContentId, at: Timestamp) {
    let mut item = store.read().content().get(id).await.unwrap().unwrap();
    item.status = ContentStatus::Scheduled;
    item.scheduled_at = Some(at);
    store.read().content().update(&item).await.unwrap();
}

pub async fn transitions_and_events(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let t1 = term(store, "one").await;
    let mut d = draft("post", "Life");
    d.terms = vec![t1];
    let item = svc.create(&admin, d).await.unwrap();
    let revs = revision_count(store, item.id).await;
    let want_keys = sorted(vec![post_key(item.id), archive_key("post"), term_key(t1)]);

    // publish: sets published_at once; event covers post, archive and terms.
    let before = max_seq(store).await;
    let published = svc.publish(&admin, item.id, item.version).await.unwrap();
    assert_eq!(published.status, ContentStatus::Published);
    assert_eq!(published.version, item.version + 1);
    let first_published = published.published_at.unwrap();
    assert_eq!(published.scheduled_at, None);
    assert_eq!(max_seq(store).await, before + 1);
    assert_eq!(last_event_keys(store).await, want_keys);
    assert_eq!(svc.get(item.id).await.unwrap(), published);
    assert_eq!(search(store, "zebra").await, [item.id]);

    // unpublish: back to draft, published_at preserved, no redirect.
    let draft_again = svc
        .unpublish(&admin, item.id, published.version)
        .await
        .unwrap();
    assert_eq!(draft_again.status, ContentStatus::Draft);
    assert_eq!(draft_again.published_at, Some(first_published));
    assert!(
        store
            .read()
            .redirects()
            .get_by_source("/life/")
            .await
            .unwrap()
            .is_none()
    );
    // republish keeps the first published_at.
    let again = svc
        .publish(&admin, item.id, draft_again.version)
        .await
        .unwrap();
    assert_eq!(again.published_at, Some(first_published));

    // unpublish, then schedule: scheduled_at set, published_at untouched.
    let d2 = svc.unpublish(&admin, item.id, again.version).await.unwrap();
    let at = in_micros(HOUR);
    let sched = svc.schedule(&admin, item.id, d2.version, at).await.unwrap();
    assert_eq!(sched.status, ContentStatus::Scheduled);
    assert_eq!(sched.scheduled_at, Some(at));
    assert_eq!(sched.published_at, Some(first_published));
    // unpublish a scheduled item: draft, scheduled_at cleared.
    let d3 = svc.unpublish(&admin, item.id, sched.version).await.unwrap();
    assert_eq!(d3.status, ContentStatus::Draft);
    assert_eq!(d3.scheduled_at, None);

    // submit, then publish from pending.
    let pending = svc.submit(&admin, item.id, d3.version).await.unwrap();
    assert_eq!(pending.status, ContentStatus::Pending);
    let pub2 = svc.publish(&admin, item.id, pending.version).await.unwrap();
    assert_eq!(pub2.status, ContentStatus::Published);

    // trash: out of the search index; restore: draft and indexed again.
    let before = max_seq(store).await;
    let trashed = svc.trash(&admin, item.id, pub2.version).await.unwrap();
    assert_eq!(trashed.status, ContentStatus::Trash);
    assert!(search(store, "zebra").await.is_empty());
    assert_eq!(max_seq(store).await, before + 1);
    assert_eq!(last_event_keys(store).await, want_keys);
    let restored = svc.restore(&admin, item.id, trashed.version).await.unwrap();
    assert_eq!(restored.status, ContentStatus::Draft);
    assert_eq!(search(store, "zebra").await, [item.id]);

    // Status-only transitions wrote no revision.
    assert_eq!(revision_count(store, item.id).await, revs);

    // delete: trash first, then the row and its dependents are gone.
    let trashed = svc.trash(&admin, item.id, restored.version).await.unwrap();
    store
        .read()
        .meta()
        .set(item.id, "color", &json!("red"))
        .await
        .unwrap();
    store
        .read()
        .search()
        .index(item.id, "Life", "zebra")
        .await
        .unwrap();
    assert_eq!(search(store, "zebra").await, [item.id]);
    let before = max_seq(store).await;
    svc.delete(&admin, item.id, trashed.version).await.unwrap();
    assert!(store.read().meta().list(item.id).await.unwrap().is_empty());
    assert!(search(store, "zebra").await.is_empty());
    assert!(matches!(svc.get(item.id).await, Err(CoreError::NotFound)));
    assert_eq!(revision_count(store, item.id).await, 0);
    assert!(
        store
            .read()
            .terms()
            .for_item(item.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(max_seq(store).await, before + 1);
    assert_eq!(last_event_keys(store).await, want_keys);

    assert!(matches!(
        svc.publish(&admin, ContentId::new(), 1).await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        svc.delete(&admin, ContentId::new(), 1).await,
        Err(CoreError::NotFound)
    ));
    // End on a committed operation.
    svc.create(&admin, draft("post", "Done")).await.unwrap();
}

pub async fn illegal_transitions_and_conflicts(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let item = svc.create(&admin, draft("post", "Rules")).await.unwrap();
    let seq = max_seq(store).await;

    // Draft: cannot unpublish, restore or delete.
    for r in [
        svc.unpublish(&admin, item.id, 1).await,
        svc.restore(&admin, item.id, 1).await,
    ] {
        assert!(invalid(r).starts_with("illegal transition draft -> "));
    }
    assert!(invalid(svc.delete(&admin, item.id, 1).await).starts_with("illegal transition"));
    // Past or present schedule times.
    assert!(invalid(svc.schedule(&admin, item.id, 1, in_micros(-HOUR)).await).contains("future"));

    // Stale version.
    assert!(matches!(
        svc.publish(&admin, item.id, 99).await,
        Err(CoreError::Conflict { current_version: 1 })
    ));
    assert_eq!(max_seq(store).await, seq);
    assert_eq!(svc.get(item.id).await.unwrap(), item);

    // Trashed: publish, schedule, submit, trash all illegal.
    let trashed = svc.trash(&admin, item.id, 1).await.unwrap();
    let v = trashed.version;
    for r in [
        svc.publish(&admin, item.id, v).await,
        svc.schedule(&admin, item.id, v, in_micros(HOUR)).await,
        svc.submit(&admin, item.id, v).await,
        svc.trash(&admin, item.id, v).await,
        svc.unpublish(&admin, item.id, v).await,
    ] {
        assert!(invalid(r).starts_with("illegal transition trash -> "));
    }
    assert_eq!(svc.get(item.id).await.unwrap(), trashed);

    // A published item cannot be submitted or scheduled; a pending one cannot be submitted.
    let live = svc.restore(&admin, item.id, v).await.unwrap();
    let live = svc.publish(&admin, item.id, live.version).await.unwrap();
    assert!(invalid(svc.submit(&admin, item.id, live.version).await).contains("published"));
    assert!(
        invalid(
            svc.schedule(&admin, item.id, live.version, in_micros(HOUR))
                .await
        )
        .contains("published")
    );
    let d = svc.unpublish(&admin, item.id, live.version).await.unwrap();
    let p = svc.submit(&admin, item.id, d.version).await.unwrap();
    assert!(invalid(svc.submit(&admin, item.id, p.version).await).contains("pending"));
    // End on a committed operation.
    svc.create(&admin, draft("post", "Done")).await.unwrap();
}

pub async fn permissions(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let contributor = actor(store, Role::Contributor).await;
    let author = actor(store, Role::Author).await;
    let other_author = actor(store, Role::Author).await;
    let editor = actor(store, Role::Editor).await;

    // Contributor: submit own draft allowed, publish/schedule forbidden.
    let mine = svc
        .create(&contributor, draft("post", "Mine"))
        .await
        .unwrap();
    assert!(matches!(
        svc.publish(&contributor, mine.id, mine.version).await,
        Err(CoreError::Forbidden("publish_own"))
    ));
    assert!(matches!(
        svc.schedule(&contributor, mine.id, mine.version, in_micros(HOUR))
            .await,
        Err(CoreError::Forbidden("publish_own"))
    ));
    let seq = max_seq(store).await;
    let pending = svc
        .submit(&contributor, mine.id, mine.version)
        .await
        .unwrap();
    assert_eq!(pending.status, ContentStatus::Pending);
    assert_eq!(max_seq(store).await, seq + 1);
    // Someone else's draft cannot be submitted by a contributor.
    let theirs = svc.create(&author, draft("post", "Theirs")).await.unwrap();
    assert!(matches!(
        svc.submit(&contributor, theirs.id, theirs.version).await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    // A contributor may trash own pending, and restore it.
    let trashed = svc
        .trash(&contributor, mine.id, pending.version)
        .await
        .unwrap();
    // ... but neither restore nor delete it.
    for who in [&contributor, &actor(store, Role::Contributor).await] {
        assert!(matches!(
            svc.restore(who, mine.id, trashed.version).await,
            Err(CoreError::Forbidden(_))
        ));
        assert!(matches!(
            svc.delete(who, mine.id, trashed.version).await,
            Err(CoreError::Forbidden(_))
        ));
    }
    assert_eq!(svc.get(mine.id).await.unwrap(), trashed);
    let restored = svc.restore(&author, mine.id, trashed.version).await;
    assert!(matches!(restored, Err(CoreError::Forbidden("edit_others"))));
    let restored = svc
        .restore(&editor, mine.id, trashed.version)
        .await
        .unwrap();
    assert_eq!(restored.status, ContentStatus::Draft);

    // Author publishes own, not others'; editor publishes anyone's.
    assert!(matches!(
        svc.publish(&other_author, theirs.id, theirs.version).await,
        Err(CoreError::Forbidden("publish_others"))
    ));
    let live = svc
        .publish(&author, theirs.id, theirs.version)
        .await
        .unwrap();
    assert_eq!(live.status, ContentStatus::Published);
    // A contributor cannot unpublish/trash a published item, even one they authored.
    let escalate = svc
        .create(&contributor, draft("post", "Esc"))
        .await
        .unwrap();
    let live_esc = svc
        .publish(&editor, escalate.id, escalate.version)
        .await
        .unwrap();
    assert!(matches!(
        svc.trash(&contributor, escalate.id, live_esc.version).await,
        Err(CoreError::Forbidden("edit_own"))
    ));
    assert!(matches!(
        svc.unpublish(&contributor, escalate.id, live_esc.version)
            .await,
        Err(CoreError::Forbidden("publish_own"))
    ));
    // Delete of someone else's trashed item needs edit_others.
    let trashed = svc.trash(&editor, theirs.id, live.version).await.unwrap();
    assert!(matches!(
        svc.delete(&other_author, theirs.id, trashed.version).await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    assert!(matches!(
        svc.restore(&other_author, theirs.id, trashed.version).await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    assert_eq!(svc.get(theirs.id).await.unwrap(), trashed);
    // The author may restore and delete their own trashed item.
    let back = svc
        .restore(&author, theirs.id, trashed.version)
        .await
        .unwrap();
    let again = svc.trash(&author, theirs.id, back.version).await.unwrap();
    svc.delete(&author, theirs.id, again.version).await.unwrap();
}

pub async fn delete_with_children_is_invalid(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let parent = svc.create(&admin, draft("page", "Parent")).await.unwrap();
    let mut d = draft("page", "Child");
    d.parent_id = Some(parent.id);
    let child = svc.create(&admin, d).await.unwrap();
    let trashed = svc.trash(&admin, parent.id, parent.version).await.unwrap();
    let seq = max_seq(store).await;
    assert_eq!(
        invalid(svc.delete(&admin, parent.id, trashed.version).await),
        "has children"
    );
    assert_eq!(svc.get(parent.id).await.unwrap(), trashed);
    assert_eq!(max_seq(store).await, seq);
    // Once the child is gone, the parent can go.
    let c = svc.trash(&admin, child.id, child.version).await.unwrap();
    assert_eq!(c.status, ContentStatus::Trash);
    svc.delete(&admin, child.id, c.version).await.unwrap();
    svc.delete(&admin, parent.id, trashed.version)
        .await
        .unwrap();
    assert!(matches!(svc.get(parent.id).await, Err(CoreError::NotFound)));
}

pub async fn publish_due_publishes_due_items_once(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let t1 = term(store, "news").await;

    let mut d = draft("post", "Due");
    d.terms = vec![t1];
    let due = svc.create(&admin, d).await.unwrap();
    let later = svc.create(&admin, draft("post", "Later")).await.unwrap();
    let plain = svc.create(&admin, draft("post", "Plain")).await.unwrap();
    let past = in_micros(-HOUR);
    force_scheduled(store, due.id, past).await;
    let later = svc
        .schedule(&admin, later.id, later.version, in_micros(2 * HOUR))
        .await
        .unwrap();

    let before = max_seq(store).await;
    let now = in_micros(HOUR);
    assert_eq!(svc.publish_due(now).await.unwrap(), 1);
    let published = svc.get(due.id).await.unwrap();
    assert_eq!(published.status, ContentStatus::Published);
    assert_eq!(published.scheduled_at, None);
    assert_eq!(published.published_at, Some(now));
    assert_eq!(max_seq(store).await, before + 1);
    assert_eq!(
        last_event_keys(store).await,
        sorted(vec![post_key(due.id), archive_key("post"), term_key(t1)])
    );
    assert_eq!(revision_count(store, due.id).await, 1);
    // Not-yet-due and unscheduled items are untouched; a second call finds nothing.
    assert_eq!(svc.get(later.id).await.unwrap(), later);
    assert_eq!(svc.get(plain.id).await.unwrap(), plain);
    assert_eq!(svc.publish_due(now).await.unwrap(), 0);
    // Later it becomes due.
    assert_eq!(svc.publish_due(in_micros(3 * HOUR)).await.unwrap(), 1);
    assert_eq!(
        svc.get(later.id).await.unwrap().status,
        ContentStatus::Published
    );

    // An existing published_at is kept.
    let kept = svc.create(&admin, draft("post", "Kept")).await.unwrap();
    let kept = svc.publish(&admin, kept.id, kept.version).await.unwrap();
    let first = kept.published_at;
    svc.unpublish(&admin, kept.id, kept.version).await.unwrap();
    force_scheduled(store, kept.id, past).await;
    assert_eq!(svc.publish_due(in_micros(0)).await.unwrap(), 1);
    assert_eq!(svc.get(kept.id).await.unwrap().published_at, first);
}

pub async fn publish_due_handles_more_than_one_batch(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let past = in_micros(-HOUR);
    let mut ids = Vec::new();
    for i in 0..105 {
        let it = svc
            .create(&admin, draft("post", &format!("Batch {i}")))
            .await
            .unwrap();
        force_scheduled(store, it.id, past).await;
        ids.push(it.id);
    }
    assert_eq!(svc.publish_due(in_micros(0)).await.unwrap(), 105);
    for id in ids {
        assert_eq!(svc.get(id).await.unwrap().status, ContentStatus::Published);
    }
}

pub async fn publish_due_skips_items_changed_since_listing(db: &TestDb) {
    // An item that is no longer scheduled is never published by publish_due.
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let it = svc.create(&admin, draft("post", "Changed")).await.unwrap();
    force_scheduled(store, it.id, in_micros(-HOUR)).await;
    let scheduled = svc.get(it.id).await.unwrap();
    let back = svc
        .unpublish(&admin, it.id, scheduled.version)
        .await
        .unwrap();
    assert_eq!(svc.publish_due(in_micros(0)).await.unwrap(), 0);
    assert_eq!(svc.get(it.id).await.unwrap(), back);
}

pub async fn concurrent_publish_due_publishes_each_once(db: Arc<TestDb>) {
    let store = db.shared_store();
    let svc = Arc::new(service(&db));
    let admin = actor(store.as_ref(), Role::Admin).await;
    let past = in_micros(-HOUR);
    let mut ids = Vec::new();
    for i in 0..20 {
        let it = svc
            .create(&admin, draft("post", &format!("Race {i}")))
            .await
            .unwrap();
        force_scheduled(store.as_ref(), it.id, past).await;
        ids.push(it.id);
    }
    let versions: Vec<i64> = {
        let mut v = Vec::new();
        for id in &ids {
            v.push(svc.get(*id).await.unwrap().version);
        }
        v
    };
    let seq = max_seq(store.as_ref()).await;
    let now = in_micros(0);
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            let svc = svc.clone();
            tokio::spawn(async move { svc.publish_due(now).await })
        })
        .collect();
    let mut total = 0;
    for t in tasks {
        total += t.await.unwrap().unwrap();
    }
    assert_eq!(total, 20);
    let events = store.read().events().since(seq, 1000).await.unwrap();
    for (id, v) in ids.iter().zip(versions) {
        let item = svc.get(*id).await.unwrap();
        assert_eq!(item.status, ContentStatus::Published);
        // Written exactly once, with exactly one event.
        assert_eq!(item.version, v + 1);
        let key = post_key(*id);
        assert_eq!(events.iter().filter(|e| e.keys.contains(&key)).count(), 1);
    }
    // End on a committed operation.
    svc.create(&admin, draft("post", "Done")).await.unwrap();
}

pub async fn delete_is_guarded_by_version(db: &TestDb) {
    // A restore between the caller's read and its delete must save the item.
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let it = svc.create(&admin, draft("post", "Guard")).await.unwrap();
    let trashed = svc.trash(&admin, it.id, it.version).await.unwrap();
    let restored = svc.restore(&admin, it.id, trashed.version).await.unwrap();
    assert!(matches!(
        svc.delete(&admin, it.id, trashed.version).await,
        Err(CoreError::Conflict { current_version }) if current_version == restored.version
    ));
    assert_eq!(svc.get(it.id).await.unwrap(), restored);
    svc.create(&admin, draft("post", "Done")).await.unwrap();
}

pub async fn stale_version_conflicts(db: &TestDb) {
    // An edit between the caller's read and its transition makes the version stale.
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let it = svc.create(&admin, draft("post", "Stale")).await.unwrap();
    let edited = svc
        .update(&admin, it.id, it.version, draft("post", "Stale 2"))
        .await
        .unwrap();
    assert!(matches!(
        svc.trash(&admin, it.id, it.version).await,
        Err(CoreError::Conflict { current_version }) if current_version == edited.version
    ));
    assert_eq!(svc.get(it.id).await.unwrap(), edited);
    svc.create(&admin, draft("post", "Done")).await.unwrap();
}

backend_tests!(
    transitions_and_events,
    illegal_transitions_and_conflicts,
    permissions,
    delete_with_children_is_invalid,
    publish_due_publishes_due_items_once,
    publish_due_handles_more_than_one_batch,
    publish_due_skips_items_changed_since_listing,
    stale_version_conflicts,
    delete_is_guarded_by_version,
);

backend_tests_spawning!(concurrent_publish_due_publishes_each_once);
