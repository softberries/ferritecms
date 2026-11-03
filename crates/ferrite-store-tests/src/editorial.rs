//! Contract tests for `0017_releases_workflows` (M6-3 tasks 1 and 2): releases, entries
//! and locks, workflows, reviews and preview links.

use ferrite_domain::{
    Approvers, ContentId, ContentItem, ContentType, Decision, EntryTransition, PreviewLink,
    PreviewLinkId, PreviewTarget, Release, ReleaseEntry, ReleaseId, ReleaseQuery, ReleaseStatus,
    Review, ReviewDecision, ReviewId, ReviewQuery, ReviewStatus, ReviewTarget, Role, StagedChange,
    Timestamp, User, Workflow, WorkflowStep,
};
use ferrite_store::{RELEASE_CURSOR_TAG, Store, StoreError};
use serde_json::json;
use uuid::Uuid;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

const SECOND: i64 = 1_000_000;

fn plus(t: Timestamp, micros: i64) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + micros).unwrap()
}

pub(crate) fn release(by: Option<&User>, title: &str, at: Timestamp) -> Release {
    Release {
        id: ReleaseId::new(),
        title: title.into(),
        description: String::new(),
        status: ReleaseStatus::Open,
        scheduled_at: None,
        scheduled_by: None,
        override_reason: None,
        failure: None,
        created_by: by.map(|u| u.id),
        created_at: at,
        updated_at: at,
        applied_at: None,
        applied_by: None,
        version: 1,
    }
}

fn entry(r: ReleaseId, item: ContentId, transition: EntryTransition) -> ReleaseEntry {
    ReleaseEntry {
        release_id: r,
        item_id: item,
        position: 0,
        transition,
        changes: None,
        base_version: 1,
        entry_version: 1,
        added_by: None,
        updated_at: Timestamp::now(),
    }
}

fn change(title: &str) -> StagedChange {
    StagedChange {
        title: title.into(),
        slug: Some("staged".into()),
        parent_id: None,
        excerpt: None,
        body: json!([{"type": "paragraph", "text": title}]),
        fields: json!({}),
        menu_order: 0,
        terms: vec![],
        seo: Some(json!({"title": "SEO"})),
    }
}

async fn post(store: &dyn Store, user: &User, slug: &str) -> ContentItem {
    store
        .read()
        .content()
        .insert(&new_post(user, slug))
        .await
        .unwrap()
}

async fn stored_release(store: &dyn Store, user: &User, title: &str) -> Release {
    let r = release(Some(user), title, Timestamp::now());
    store.read().releases().insert(&r).await.unwrap();
    r
}

fn step(key: &str) -> WorkflowStep {
    WorkflowStep {
        key: key.into(),
        label: format!("Step {key}"),
        approvers: Approvers {
            roles: vec![Role::Editor],
            users: vec![],
        },
        required: 1,
    }
}

pub(crate) fn review(target: ReviewTarget, by: Option<&User>, at: Timestamp) -> Review {
    Review {
        id: ReviewId::new(),
        target,
        revision_id: None,
        entry_version: match target {
            ReviewTarget::Item(_) => None,
            ReviewTarget::Entry(..) => Some(1),
        },
        status: ReviewStatus::InReview,
        step: 0,
        steps: vec![step("copy"), step("legal")],
        note: Some("please".into()),
        submitted_by: by.map(|u| u.id),
        submitted_at: at,
        decided_at: None,
        version: 1,
    }
}

fn unique(err: &StoreError) -> &str {
    match err {
        StoreError::UniqueViolation { constraint } => constraint,
        other => panic!("expected a unique violation, got {other:?}"),
    }
}

// ---- Task 1: releases, entries, locks ----

pub async fn migration_0017_roundtrip(db: &TestDb) {
    const TABLES: [&str; 8] = [
        "releases",
        "release_items",
        "release_locks",
        "workflows",
        "reviews",
        "review_decisions",
        "review_assignees",
        "preview_links",
    ];
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    assert!(has(&objects, "reviews_open_target_uq"), "{objects:?}");
    let user = fixture_user(store).await;
    stored_release(store, &user, "Spring").await;

    // Later migrations (M6-4's 0019 is lossy too) go first, with their own consent.
    let later = store.migrate_down(17, true).await.unwrap();
    let err = store.migrate_down(16, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 17 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(store.migrate_down(16, true).await.unwrap(), vec![17]);
    assert_eq!(
        later.iter().chain([17].iter()).copied().collect::<Vec<_>>(),
        crate::migrations::down_from(16)
    );
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        crate::migrations::up_from(16)
    );
    let page = store
        .read()
        .releases()
        .list(&ReleaseQuery {
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(page.items.is_empty());
}

pub async fn release_version_conflict(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let r = stored_release(store, &user, "Spring").await;
    let repo = store.read().releases();
    assert_eq!(repo.get(r.id).await.unwrap().unwrap(), r);
    let mut changed = r.clone();
    changed.title = "Summer".into();
    changed.failure = Some(json!({"reason": "x"}));
    let stored = repo.update(&changed).await.unwrap();
    assert_eq!((stored.version, stored.title.as_str()), (2, "Summer"));
    assert_eq!(stored.failure, Some(json!({"reason": "x"})));
    assert_eq!(stored.created_by, Some(user.id));
    let err = repo.update(&changed).await.unwrap_err();
    assert!(
        matches!(err, StoreError::Conflict { current_version: 2 }),
        "{err:?}"
    );
    let mut missing = changed.clone();
    missing.id = ReleaseId::new();
    assert!(matches!(
        repo.update(&missing).await.unwrap_err(),
        StoreError::NotFound
    ));
    let mut nul = stored.clone();
    nul.title = "a\0b".into();
    assert!(matches!(
        repo.update(&nul).await.unwrap_err(),
        StoreError::Invalid(_)
    ));
    assert!(repo.delete(r.id).await.unwrap());
    assert!(!repo.delete(r.id).await.unwrap());
    assert_eq!(repo.get(r.id).await.unwrap(), None);
}

pub async fn put_entry_replaces_and_bumps_entry_version(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let r = stored_release(store, &user, "R").await;
    let repo = store.read().releases();
    let mut e = entry(r.id, item.id, EntryTransition::Publish);
    e.added_by = Some(user.id);
    let first = repo.put_entry(&e).await.unwrap();
    assert_eq!((first.position, first.entry_version), (1, 1));
    assert_eq!(first.added_by, Some(user.id));
    assert_eq!(first.changes, None);
    e.changes = Some(change("New"));
    e.base_version = 3;
    e.added_by = None;
    let second = repo.put_entry(&e).await.unwrap();
    assert_eq!((second.position, second.entry_version), (1, 2));
    assert_eq!(second.added_by, Some(user.id), "added_by is kept");
    assert_eq!(second.base_version, 3);
    assert_eq!(second.changes, Some(change("New")));
    assert_eq!(repo.entry(r.id, item.id).await.unwrap(), Some(second));
    // Unknown item: foreign key.
    let err = repo
        .put_entry(&entry(r.id, ContentId::new(), EntryTransition::None))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::ForeignKey { .. }), "{err:?}");
    assert!(repo.delete_entry(r.id, item.id).await.unwrap());
    assert!(!repo.delete_entry(r.id, item.id).await.unwrap());
    // NUL inside the staged JSON is refused on both backends.
    let mut bad = entry(r.id, item.id, EntryTransition::None);
    bad.changes = Some(change("a\0b"));
    assert!(matches!(
        repo.put_entry(&bad).await.unwrap_err(),
        StoreError::Invalid(_)
    ));
}

pub async fn entries_in_position_order(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let r = stored_release(store, &user, "R").await;
    let other = stored_release(store, &user, "Other").await;
    let repo = store.read().releases();
    let mut ids = Vec::new();
    for slug in ["c", "a", "b"] {
        let item = post(store, &user, slug).await;
        repo.put_entry(&entry(r.id, item.id, EntryTransition::None))
            .await
            .unwrap();
        ids.push(item.id);
    }
    repo.put_entry(&entry(other.id, ids[0], EntryTransition::None))
        .await
        .unwrap();
    let got: Vec<_> = repo
        .entries(r.id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.item_id, e.position))
        .collect();
    assert_eq!(got, vec![(ids[0], 1), (ids[1], 2), (ids[2], 3)]);
    let counts = repo.entry_counts(&[r.id, other.id]).await.unwrap();
    assert!(counts.contains(&(r.id, 3)) && counts.contains(&(other.id, 1)));
    let of: Vec<_> = repo
        .releases_of(ids[0])
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(of, vec![other.id, r.id]);
    // Applied releases are history, not listed for the item.
    let mut applied = repo.get(other.id).await.unwrap().unwrap();
    applied.status = ReleaseStatus::Applied;
    repo.update(&applied).await.unwrap();
    assert_eq!(repo.releases_of(ids[0]).await.unwrap().len(), 1);
}

pub async fn deleting_item_drops_its_entries_and_lock(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let keep = post(store, &user, "b").await;
    let r = stored_release(store, &user, "R").await;
    let repo = store.read().releases();
    for i in [item.id, keep.id] {
        repo.put_entry(&entry(r.id, i, EntryTransition::None))
            .await
            .unwrap();
        repo.lock(i, r.id).await.unwrap();
    }
    store.read().content().delete(item.id).await.unwrap();
    let left: Vec<_> = repo
        .entries(r.id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.item_id)
        .collect();
    assert_eq!(left, vec![keep.id]);
    assert_eq!(repo.lock_of(item.id).await.unwrap(), None);
    assert_eq!(repo.lock_of(keep.id).await.unwrap(), Some(r.id));
}

pub async fn lock_is_exclusive_per_item(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let a = stored_release(store, &user, "A").await;
    let b = stored_release(store, &user, "B").await;
    let repo = store.read().releases();
    repo.lock(item.id, a.id).await.unwrap();
    let err = repo.lock(item.id, b.id).await.unwrap_err();
    assert_eq!(unique(&err), "release_locks_pkey");
    assert_eq!(repo.lock_of(item.id).await.unwrap(), Some(a.id));
}

pub async fn unlock_all_frees_items(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = stored_release(store, &user, "A").await;
    let b = stored_release(store, &user, "B").await;
    let repo = store.read().releases();
    let i1 = post(store, &user, "a").await;
    let i2 = post(store, &user, "b").await;
    let i3 = post(store, &user, "c").await;
    repo.lock(i1.id, a.id).await.unwrap();
    repo.lock(i2.id, a.id).await.unwrap();
    repo.lock(i3.id, b.id).await.unwrap();
    assert_eq!(repo.unlock_all(a.id).await.unwrap(), 2);
    assert_eq!(repo.lock_of(i1.id).await.unwrap(), None);
    assert_eq!(repo.lock_of(i3.id).await.unwrap(), Some(b.id));
    // Deleting a release frees its locks too.
    repo.delete(b.id).await.unwrap();
    assert_eq!(repo.lock_of(i3.id).await.unwrap(), None);
}

pub async fn due_lists_scheduled_releases_by_time(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let repo = store.read().releases();
    let mut ids = Vec::new();
    for (title, offset) in [("late", 30), ("early", -60), ("mid", -10), ("future", 600)] {
        let mut r = release(Some(&user), title, now);
        r.status = ReleaseStatus::Scheduled;
        r.scheduled_at = Some(plus(now, offset * SECOND));
        repo.insert(&r).await.unwrap();
        ids.push(r.id);
    }
    let mut open = release(Some(&user), "open", now);
    open.scheduled_at = Some(plus(now, -100 * SECOND));
    repo.insert(&open).await.unwrap();
    let due: Vec<_> = repo
        .due(plus(now, 31 * SECOND), 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect();
    assert_eq!(due, ["early", "mid", "late"]);
    assert_eq!(repo.due(now, 1).await.unwrap().len(), 1);
    assert!(repo.due(now, 0).await.is_err());
}

pub async fn list_filters_and_pages(db: &TestDb) {
    let store = db.store();
    let a = fixture_user(store).await;
    let b = fixture_user(store).await;
    let now = Timestamp::now();
    let repo = store.read().releases();
    let mut made = Vec::new();
    for i in 0..5 {
        let mut r = release(
            Some(if i % 2 == 0 { &a } else { &b }),
            &format!("r{i}"),
            plus(now, i),
        );
        if i == 4 {
            r.status = ReleaseStatus::Applied;
        }
        repo.insert(&r).await.unwrap();
        made.push(r.title);
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = repo
            .list(&ReleaseQuery {
                cursor: cursor.clone(),
                limit: 2,
                ..Default::default()
            })
            .await
            .unwrap();
        seen.extend(page.items.into_iter().map(|r| r.title));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, ["r4", "r3", "r2", "r1", "r0"]);
    let mine: Vec<_> = repo
        .list(&ReleaseQuery {
            created_by: Some(a.id),
            status: Some(ReleaseStatus::Open),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|r| r.title)
        .collect();
    assert_eq!(mine, ["r2", "r0"]);
    let foreign = ferrite_domain::Cursor::encode("other", &json!(1), Uuid::now_v7());
    assert!(
        repo.list(&ReleaseQuery {
            cursor: Some(foreign),
            limit: 2,
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert_eq!(RELEASE_CURSOR_TAG, "rl:created");
}

pub async fn deleting_user_keeps_release(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &fixture_user(store).await, "a").await;
    let mut r = stored_release(store, &user, "R").await;
    r.scheduled_by = Some(user.id);
    r.applied_by = Some(user.id);
    let r = store.read().releases().update(&r).await.unwrap();
    let mut e = entry(r.id, item.id, EntryTransition::None);
    e.added_by = Some(user.id);
    store.read().releases().put_entry(&e).await.unwrap();
    store.read().users().delete(user.id).await.unwrap();
    let kept = store.read().releases().get(r.id).await.unwrap().unwrap();
    assert_eq!(
        (kept.created_by, kept.scheduled_by, kept.applied_by),
        (None, None, None)
    );
    let e = store
        .read()
        .releases()
        .entry(r.id, item.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.added_by, None);
}

// ---- Task 2: workflows, reviews, preview links ----

async fn custom_type(store: &dyn Store, name: &str) {
    store
        .read()
        .types()
        .insert(&ContentType {
            name: name.into(),
            label: name.into(),
            hierarchical: false,
            has_archive: true,
            fields: json!({}),
            built_in: false,
            created_at: Timestamp::now(),
            quality_gate: ferrite_domain::QualityGate::Off,
        })
        .await
        .unwrap();
}

pub async fn workflow_version_conflict(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let repo = store.read().workflows();
    let w = Workflow {
        type_name: "post".into(),
        steps: vec![step("copy")],
        updated_by: Some(user.id),
        updated_at: Timestamp::now(),
        version: 0,
    };
    let stored = repo.put(&w, None).await.unwrap();
    assert_eq!(
        (stored.version, stored.steps.clone()),
        (1, vec![step("copy")])
    );
    assert_eq!(
        unique(&repo.put(&w, None).await.unwrap_err()),
        "workflows_pkey"
    );
    let mut w2 = w.clone();
    w2.steps.push(step("legal"));
    let v2 = repo.put(&w2, Some(1)).await.unwrap();
    assert_eq!(v2.version, 2);
    assert!(matches!(
        repo.put(&w2, Some(1)).await.unwrap_err(),
        StoreError::Conflict { current_version: 2 }
    ));
    let mut none = w2.clone();
    none.type_name = "page".into();
    assert!(matches!(
        repo.put(&none, Some(1)).await.unwrap_err(),
        StoreError::NotFound
    ));
    let mut unknown = w.clone();
    unknown.type_name = "nope".into();
    assert!(matches!(
        repo.put(&unknown, None).await.unwrap_err(),
        StoreError::ForeignKey { .. }
    ));
    repo.put(&none, None).await.unwrap();
    let names: Vec<_> = repo
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|w| w.type_name)
        .collect();
    assert_eq!(names, ["page", "post"]);
    assert!(repo.delete("page").await.unwrap());
    assert!(!repo.delete("page").await.unwrap());
    assert_eq!(repo.get("page").await.unwrap(), None);
}

pub async fn deleting_type_deletes_workflow(db: &TestDb) {
    let store = db.store();
    custom_type(store, "event").await;
    let repo = store.read().workflows();
    repo.put(
        &Workflow {
            type_name: "event".into(),
            steps: vec![step("copy")],
            updated_by: None,
            updated_at: Timestamp::now(),
            version: 0,
        },
        None,
    )
    .await
    .unwrap();
    store.read().types().delete("event").await.unwrap();
    assert_eq!(repo.get("event").await.unwrap(), None);
}

pub async fn one_open_review_per_target(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let repo = store.read().reviews();
    let t = ReviewTarget::Item(item.id);
    let r = review(t, Some(&user), Timestamp::now());
    repo.insert(&r).await.unwrap();
    assert_eq!(repo.get(r.id).await.unwrap(), Some(r.clone()));
    assert_eq!(repo.open_for(&t).await.unwrap(), Some(r.clone()));
    let err = repo
        .insert(&review(t, Some(&user), Timestamp::now()))
        .await
        .unwrap_err();
    assert_eq!(unique(&err), "reviews_open_target_uq");
    // Approved is still open.
    let mut a = r.clone();
    a.status = ReviewStatus::Approved;
    let a = repo.update(&a).await.unwrap();
    assert_eq!(a.version, 2);
    assert!(
        repo.insert(&review(t, None, Timestamp::now()))
            .await
            .is_err()
    );
    assert!(matches!(
        repo.update(&r).await.unwrap_err(),
        StoreError::Conflict { current_version: 2 }
    ));
}

pub async fn closed_reviews_do_not_block_a_new_one(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let repo = store.read().reviews();
    let t = ReviewTarget::Item(item.id);
    for status in [
        ReviewStatus::Stale,
        ReviewStatus::Withdrawn,
        ReviewStatus::ChangesRequested,
    ] {
        let mut r = review(t, Some(&user), Timestamp::now());
        repo.insert(&r).await.unwrap();
        r.status = status;
        repo.update(&r).await.unwrap();
    }
    repo.insert(&review(t, Some(&user), Timestamp::now()))
        .await
        .unwrap();
    assert!(repo.open_for(&t).await.unwrap().is_some());
}

pub async fn mark_stale_item_closes_only_open_item_reviews(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let other = post(store, &user, "b").await;
    let rel = stored_release(store, &user, "R").await;
    store
        .read()
        .releases()
        .put_entry(&entry(rel.id, item.id, EntryTransition::None))
        .await
        .unwrap();
    let repo = store.read().reviews();
    let now = Timestamp::now();
    let item_review = review(ReviewTarget::Item(item.id), None, now);
    let entry_review = review(ReviewTarget::Entry(rel.id, item.id), None, now);
    let other_review = review(ReviewTarget::Item(other.id), None, now);
    for r in [&item_review, &entry_review, &other_review] {
        repo.insert(r).await.unwrap();
    }
    assert_eq!(repo.mark_stale_item(item.id, now).await.unwrap(), 1);
    assert_eq!(repo.mark_stale_item(item.id, now).await.unwrap(), 0);
    let got = repo.get(item_review.id).await.unwrap().unwrap();
    assert_eq!(
        (got.status, got.decided_at),
        (ReviewStatus::Stale, Some(now))
    );
    assert_eq!(
        repo.get(entry_review.id).await.unwrap().unwrap().status,
        ReviewStatus::InReview
    );
    assert_eq!(
        repo.get(other_review.id).await.unwrap().unwrap().status,
        ReviewStatus::InReview
    );
}

pub async fn mark_stale_entry(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let rel = stored_release(store, &user, "R").await;
    store
        .read()
        .releases()
        .put_entry(&entry(rel.id, item.id, EntryTransition::None))
        .await
        .unwrap();
    let repo = store.read().reviews();
    let now = Timestamp::now();
    let e = review(ReviewTarget::Entry(rel.id, item.id), None, now);
    let i = review(ReviewTarget::Item(item.id), None, now);
    repo.insert(&e).await.unwrap();
    repo.insert(&i).await.unwrap();
    assert_eq!(
        repo.mark_stale_entry(rel.id, item.id, now).await.unwrap(),
        1
    );
    assert_eq!(
        repo.get(e.id).await.unwrap().unwrap().status,
        ReviewStatus::Stale
    );
    assert_eq!(
        repo.get(i.id).await.unwrap().unwrap().status,
        ReviewStatus::InReview
    );
}

pub async fn decision_once_per_step_and_user(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let repo = store.read().reviews();
    let r = review(ReviewTarget::Item(item.id), None, Timestamp::now());
    repo.insert(&r).await.unwrap();
    let d = |step, decision| ReviewDecision {
        id: Uuid::now_v7(),
        review_id: r.id,
        step,
        user_id: Some(user.id),
        decision,
        comment: Some("ok".into()),
        at: Timestamp::now(),
    };
    let first = d(0, Decision::Approve);
    repo.add_decision(&first).await.unwrap();
    let err = repo
        .add_decision(&d(0, Decision::RequestChanges))
        .await
        .unwrap_err();
    assert_eq!(unique(&err), "review_decisions_step_user_uq");
    let second = d(1, Decision::RequestChanges);
    repo.add_decision(&second).await.unwrap();
    assert_eq!(repo.decisions(r.id).await.unwrap(), vec![first, second]);
}

pub async fn assignees_replace_per_step(db: &TestDb) {
    let store = db.store();
    let a = fixture_user(store).await;
    let b = fixture_user(store).await;
    let c = fixture_user(store).await;
    let item = post(store, &a, "a").await;
    let repo = store.read().reviews();
    let r = review(ReviewTarget::Item(item.id), None, Timestamp::now());
    repo.insert(&r).await.unwrap();
    repo.set_assignees(r.id, 0, &[a.id, b.id]).await.unwrap();
    repo.set_assignees(r.id, 1, &[c.id]).await.unwrap();
    repo.set_assignees(r.id, 0, &[b.id, b.id]).await.unwrap();
    let mut got = repo.assignees(r.id).await.unwrap();
    got.sort();
    let mut want = vec![(0, b.id), (1, c.id)];
    want.sort();
    assert_eq!(got, want);
    assert_eq!(repo.counts_for(b.id).await.unwrap(), 1);
    assert_eq!(
        repo.counts_for(c.id).await.unwrap(),
        0,
        "not the current step"
    );
    assert_eq!(repo.counts_for(a.id).await.unwrap(), 0);
    // A deleted user leaves the assignees.
    store.read().users().delete(c.id).await.unwrap();
    assert_eq!(repo.assignees(r.id).await.unwrap(), vec![(0, b.id)]);
}

pub async fn deleting_release_entry_cascades_its_reviews(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let rel = stored_release(store, &user, "R").await;
    store
        .read()
        .releases()
        .put_entry(&entry(rel.id, item.id, EntryTransition::None))
        .await
        .unwrap();
    let repo = store.read().reviews();
    let e = review(ReviewTarget::Entry(rel.id, item.id), None, Timestamp::now());
    repo.insert(&e).await.unwrap();
    store
        .read()
        .releases()
        .delete_entry(rel.id, item.id)
        .await
        .unwrap();
    assert_eq!(repo.get(e.id).await.unwrap(), None);
    // An entry review of a missing entry is a foreign key error.
    let err = repo
        .insert(&review(
            ReviewTarget::Entry(rel.id, item.id),
            None,
            Timestamp::now(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::ForeignKey { .. }), "{err:?}");
}

pub async fn item_review_ignores_release_fk(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    // No release_items row at all: MATCH SIMPLE skips the check for a NULL release.
    let r = review(ReviewTarget::Item(item.id), None, Timestamp::now());
    store.read().reviews().insert(&r).await.unwrap();
    store.read().content().delete(item.id).await.unwrap();
    assert_eq!(store.read().reviews().get(r.id).await.unwrap(), None);
}

pub async fn withdraw_for_type(db: &TestDb) {
    let store = db.store();
    custom_type(store, "event").await;
    let user = fixture_user(store).await;
    let p = post(store, &user, "a").await;
    let mut ev = new_post(&user, "e");
    ev.kind = "event".into();
    let ev = store.read().content().insert(&ev).await.unwrap();
    let repo = store.read().reviews();
    let now = Timestamp::now();
    let rp = review(ReviewTarget::Item(p.id), None, now);
    let re = review(ReviewTarget::Item(ev.id), None, now);
    repo.insert(&rp).await.unwrap();
    repo.insert(&re).await.unwrap();
    assert_eq!(repo.withdraw_for_type("event", now).await.unwrap(), 1);
    assert_eq!(
        repo.get(re.id).await.unwrap().unwrap().status,
        ReviewStatus::Withdrawn
    );
    assert_eq!(
        repo.get(rp.id).await.unwrap().unwrap().status,
        ReviewStatus::InReview
    );
}

pub async fn counts_for_user(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let me = fixture_user(store).await;
    let repo = store.read().reviews();
    let now = Timestamp::now();
    let mut ids = Vec::new();
    for (i, slug) in ["a", "b", "c"].into_iter().enumerate() {
        let item = post(store, &user, slug).await;
        let r = review(
            ReviewTarget::Item(item.id),
            Some(&user),
            plus(now, i as i64),
        );
        repo.insert(&r).await.unwrap();
        repo.set_assignees(r.id, 0, &[me.id]).await.unwrap();
        ids.push(r);
    }
    let mut done = ids[1].clone();
    done.status = ReviewStatus::Approved;
    repo.update(&done).await.unwrap();
    assert_eq!(repo.counts_for(me.id).await.unwrap(), 2);
    // The queue: assignee and status filters, oldest first, paged.
    let page = repo
        .list(&ReviewQuery {
            status: Some(ReviewStatus::InReview),
            assignee: Some(me.id),
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items[0].id, ids[0].id);
    let next = repo
        .list(&ReviewQuery {
            status: Some(ReviewStatus::InReview),
            assignee: Some(me.id),
            cursor: page.next_cursor,
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(next.items[0].id, ids[2].id);
    assert!(next.next_cursor.is_none());
    let by_type = repo
        .list(&ReviewQuery {
            kind: Some("page".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(by_type.items.is_empty());
    let mine = repo
        .list(&ReviewQuery {
            submitted_by: Some(user.id),
            kind: Some("post".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(mine.items.len(), 3);
}

fn link(target: PreviewTarget, by: &User, expires: Timestamp) -> PreviewLink {
    PreviewLink {
        id: PreviewLinkId::new(),
        target,
        created_by: by.id,
        created_at: Timestamp::now(),
        expires_at: expires,
        last_used_at: None,
    }
}

pub async fn preview_link_by_hash(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = post(store, &user, "a").await;
    let repo = store.read().preview_links();
    let l = link(
        PreviewTarget::Item(item.id),
        &user,
        plus(Timestamp::now(), 3600 * SECOND),
    );
    repo.insert(&l, "hash-a").await.unwrap();
    assert_eq!(repo.get_by_hash("hash-a").await.unwrap(), Some(l.clone()));
    assert_eq!(repo.get(l.id).await.unwrap(), Some(l.clone()));
    assert_eq!(repo.get_by_hash("hash-b").await.unwrap(), None);
    let err = repo
        .insert(
            &link(PreviewTarget::Item(item.id), &user, l.expires_at),
            "hash-a",
        )
        .await
        .unwrap_err();
    assert_eq!(unique(&err), "preview_links_token_hash_uq");
    let at = Timestamp::now();
    repo.touch(l.id, at).await.unwrap();
    assert_eq!(
        repo.get(l.id).await.unwrap().unwrap().last_used_at,
        Some(at)
    );
    assert_eq!(
        repo.list_for(&PreviewTarget::Item(item.id))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(repo.delete(l.id).await.unwrap());
    assert!(!repo.delete(l.id).await.unwrap());
}

pub async fn prune_removes_expired_links(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let repo = store.read().preview_links();
    let r = ReleaseId::new();
    let old = link(PreviewTarget::Release(r), &user, plus(now, -SECOND));
    let live = link(PreviewTarget::Release(r), &user, plus(now, SECOND));
    repo.insert(&old, "old").await.unwrap();
    repo.insert(&live, "live").await.unwrap();
    assert_eq!(repo.prune(now, 100).await.unwrap(), 1);
    assert_eq!(repo.get(old.id).await.unwrap(), None);
    assert!(repo.get(live.id).await.unwrap().is_some());
    assert!(repo.prune(now, 0).await.is_err());
}

pub async fn delete_for_target(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let repo = store.read().preview_links();
    let exp = plus(Timestamp::now(), 3600 * SECOND);
    let (r1, r2) = (ReleaseId::new(), ReleaseId::new());
    repo.insert(&link(PreviewTarget::Release(r1), &user, exp), "1")
        .await
        .unwrap();
    repo.insert(&link(PreviewTarget::Release(r1), &user, exp), "2")
        .await
        .unwrap();
    let keep = link(PreviewTarget::Release(r2), &user, exp);
    repo.insert(&keep, "3").await.unwrap();
    // Same uuid, other kind: not a match.
    let item = link(
        PreviewTarget::Item(ContentId::from_uuid(r1.as_uuid())),
        &user,
        exp,
    );
    repo.insert(&item, "4").await.unwrap();
    assert_eq!(
        repo.delete_for_target(&PreviewTarget::Release(r1))
            .await
            .unwrap(),
        2
    );
    assert!(repo.get(keep.id).await.unwrap().is_some());
    assert!(repo.get(item.id).await.unwrap().is_some());
}

pub async fn deleting_user_deletes_links(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let repo = store.read().preview_links();
    let l = link(
        PreviewTarget::Release(ReleaseId::new()),
        &user,
        plus(Timestamp::now(), SECOND * 60),
    );
    repo.insert(&l, "h").await.unwrap();
    store.read().users().delete(user.id).await.unwrap();
    assert_eq!(repo.get(l.id).await.unwrap(), None);
}
