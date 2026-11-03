//! Releases (M6-3 tasks 4 and 5, design 15 §4): staging, locks, apply in one
//! transaction, check, schedule, failures — on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, AutosaveDraft, ContentDraft, ContentService, CoreError, PermalinkRules, events,
    jobs::{EDITORIAL_NOTIFY_KIND, RELEASE_APPLY_KIND},
    release::{EntryInput, EntryOutcome, NewRelease, ReleaseService},
};
use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, EntryTransition, JobQuery, NewUser, PreviewLink,
    PreviewLinkId, PreviewTarget, Release, ReleaseStatus, Role, StagedChange, Timestamp, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    store
        .read()
        .users()
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
    Actor::new(id, role)
}

struct Site {
    store: Arc<dyn Store>,
    content: Arc<ContentService>,
    releases: ReleaseService,
}

fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let rules = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    let releases = ReleaseService::new(store.clone(), rules, content.clone());
    Site {
        store,
        content,
        releases,
    }
}

fn body(text: &str) -> serde_json::Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

fn draft(kind: &str, title: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: body(title),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

fn change(title: &str) -> StagedChange {
    StagedChange {
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: body(title),
        fields: json!({}),
        menu_order: 0,
        terms: vec![],
        seo: None,
    }
}

impl Site {
    async fn item(&self, who: &Actor, kind: &str, title: &str) -> ContentItem {
        self.content.create(who, draft(kind, title)).await.unwrap()
    }

    async fn published(&self, who: &Actor, title: &str) -> ContentItem {
        let it = self.item(who, "post", title).await;
        self.content.publish(who, it.id, it.version).await.unwrap()
    }

    async fn release(&self, who: &Actor) -> Release {
        self.releases
            .create(
                who,
                NewRelease {
                    title: "Spring launch".into(),
                    description: String::new(),
                },
            )
            .await
            .unwrap()
    }

    async fn stage(
        &self,
        who: &Actor,
        r: &Release,
        item: ContentId,
        transition: EntryTransition,
        changes: Option<StagedChange>,
    ) -> Result<Release, CoreError> {
        self.releases
            .stage(
                who,
                r.id,
                r.version,
                EntryInput {
                    item,
                    transition,
                    changes,
                },
            )
            .await
    }

    async fn get(&self, id: ContentId) -> ContentItem {
        self.store.read().content().get(id).await.unwrap().unwrap()
    }

    async fn stored(&self, id: ferrite_domain::ReleaseId) -> Release {
        self.store.read().releases().get(id).await.unwrap().unwrap()
    }
}

// ---- Task 4 ----

async fn contributor_creates_and_stages_own_draft(db: &TestDb) {
    let s = site(db);
    let c = actor(s.store.as_ref(), Role::Contributor).await;
    let it = s.item(&c, "post", "Mine").await;
    let r = s.release(&c).await;
    let r = s
        .stage(
            &c,
            &r,
            it.id,
            EntryTransition::None,
            Some(change("Changed")),
        )
        .await
        .unwrap();
    assert_eq!(r.version, 2);
    let view = s.releases.get_for(&c, r.id).await.unwrap();
    assert_eq!(view.entries.len(), 1);
    assert_eq!(view.entries[0].entry.base_version, it.version);
    assert!(!view.entries[0].conflict);
}

async fn cannot_stage_others_item_without_edit_others(db: &TestDb) {
    let s = site(db);
    let a = actor(s.store.as_ref(), Role::Author).await;
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let theirs = s.published(&e, "Editor's").await;
    let r = s.release(&a).await;
    let err = s
        .stage(&a, &r, theirs.id, EntryTransition::None, Some(change("x")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Forbidden("edit_others")),
        "{err:?}"
    );
}

async fn publish_entry_needs_publish_right(db: &TestDb) {
    let s = site(db);
    let c = actor(s.store.as_ref(), Role::Contributor).await;
    let it = s.item(&c, "post", "Mine").await;
    let r = s.release(&c).await;
    let err = s
        .stage(&c, &r, it.id, EntryTransition::Publish, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Forbidden("publish_own")),
        "{err:?}"
    );
}

async fn publish_entry_refused_for_published_or_scheduled_item(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let live = s.published(&e, "Live").await;
    let r = s.release(&e).await;
    let err = s
        .stage(&e, &r, live.id, EntryTransition::Publish, None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "transition",
                ..
            }
        ),
        "{err:?}"
    );
    let it = s.item(&e, "post", "Later").await;
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap();
    let it = s.content.schedule(&e, it.id, it.version, at).await.unwrap();
    let err = s
        .stage(&e, &r, it.id, EntryTransition::None, Some(change("x")))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(m) if m.contains("unschedule")));
}

async fn entry_needs_change_or_transition(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let it = s.item(&e, "post", "x").await;
    let r = s.release(&e).await;
    let err = s
        .stage(&e, &r, it.id, EntryTransition::None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
}

async fn staged_change_is_validated(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let it = s.item(&e, "post", "x").await;
    let r = s.release(&e).await;
    let mut bad = change("x");
    bad.body = json!([]);
    let err = s
        .stage(&e, &r, it.id, EntryTransition::None, Some(bad))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::InvalidField { field: "body", .. }));
    let mut bad = change("x");
    bad.slug = Some("Not A Slug!".into());
    assert!(
        s.stage(&e, &r, it.id, EntryTransition::None, Some(bad))
            .await
            .is_err()
    );
    let mut bad = change("x");
    bad.seo = Some(json!({"title": 3}));
    let err = s
        .stage(&e, &r, it.id, EntryTransition::None, Some(bad))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::InvalidField { field: "seo", .. }));
    // Nothing was staged.
    assert_eq!(s.stored(r.id).await.version, 1);
}

async fn two_hundred_entries_max(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let r = s.release(&e).await;
    for i in 0..200 {
        let it = s
            .store
            .read()
            .content()
            .insert(&ferrite_store_tests::fixtures::new_post(
                &s.store.read().users().get(e.user).await.unwrap().unwrap(),
                &format!("p{i}"),
            ))
            .await
            .unwrap();
        s.store
            .read()
            .releases()
            .put_entry(&ferrite_domain::ReleaseEntry {
                release_id: r.id,
                item_id: it.id,
                position: 0,
                transition: EntryTransition::Publish,
                changes: None,
                base_version: it.version,
                entry_version: 1,
                added_by: None,
                updated_at: Timestamp::now(),
            })
            .await
            .unwrap();
    }
    let extra = s.item(&e, "post", "one too many").await;
    let err = s
        .stage(&e, &r, extra.id, EntryTransition::Publish, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(m) if m.contains("200")));
}

async fn rebase_bumps_entry_version(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let it = s.item(&e, "post", "x").await;
    let r = s.release(&e).await;
    let r = s
        .stage(&e, &r, it.id, EntryTransition::None, Some(change("staged")))
        .await
        .unwrap();
    let moved = s
        .content
        .update(&e, it.id, it.version, draft("post", "edited elsewhere"))
        .await
        .unwrap();
    let view = s.releases.get_for(&e, r.id).await.unwrap();
    assert!(view.entries[0].conflict);
    let r = s.releases.rebase(&e, r.id, r.version, it.id).await.unwrap();
    let view = s.releases.get_for(&e, r.id).await.unwrap();
    let entry = &view.entries[0].entry;
    assert_eq!(
        (entry.base_version, entry.entry_version),
        (moved.version, 2)
    );
    assert!(!view.entries[0].conflict);
    assert_eq!(entry.changes.as_ref().unwrap().title, "staged");
}

async fn author_cannot_see_others_release(db: &TestDb) {
    let s = site(db);
    let a = actor(s.store.as_ref(), Role::Author).await;
    let b = actor(s.store.as_ref(), Role::Author).await;
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let r = s.release(&a).await;
    assert!(matches!(
        s.releases.get_for(&b, r.id).await.unwrap_err(),
        CoreError::NotFound
    ));
    assert!(s.releases.get_for(&e, r.id).await.is_ok());
    let (page, _) = s
        .releases
        .list(&b, Default::default(), false)
        .await
        .unwrap();
    assert!(page.items.is_empty());
    let (page, counts) = s
        .releases
        .list(&e, Default::default(), false)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(counts.is_empty(), "no entries yet");
}

async fn locked_item_refuses_update_publish_trash_restore_revision(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let it = s.item(&e, "post", "x").await;
    let r = s.release(&e).await;
    s.store.read().releases().lock(it.id, r.id).await.unwrap();
    let locked = |e: CoreError| matches!(e, CoreError::Locked { release } if release == r.id);
    assert!(locked(
        s.content
            .update(&e, it.id, it.version, draft("post", "y"))
            .await
            .unwrap_err()
    ));
    assert!(locked(
        s.content.publish(&e, it.id, it.version).await.unwrap_err()
    ));
    assert!(locked(
        s.content.trash(&e, it.id, it.version).await.unwrap_err()
    ));
    let rev = s.content.revisions(&e, it.id, 1).await.unwrap()[0].id;
    assert!(locked(
        s.content
            .restore_revision(&e, it.id, rev, it.version)
            .await
            .unwrap_err()
    ));
    s.store.read().releases().unlock_all(r.id).await.unwrap();
    s.content.publish(&e, it.id, it.version).await.unwrap();
}

async fn autosave_is_not_locked(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let it = s.item(&e, "post", "x").await;
    let r = s.release(&e).await;
    s.store.read().releases().lock(it.id, r.id).await.unwrap();
    s.content
        .put_autosave(
            &e,
            it.id,
            AutosaveDraft {
                base_version: it.version,
                title: "working".into(),
                excerpt: None,
                slug: None,
                parent_id: None,
                menu_order: None,
                terms: None,
                body: body("w"),
                fields: json!({}),
                seo: None,
            },
        )
        .await
        .unwrap();
}

// ---- Task 5 ----

async fn apply_is_all_or_nothing(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let parent = s.item(&e, "page", "Parent").await;
    let mut r = s.release(&e).await;
    let mut items = Vec::new();
    for i in 0..40 {
        let it = s.item(&e, "page", &format!("Page {i}")).await;
        let mut c = change(&format!("New {i}"));
        if i == 36 {
            c.parent_id = Some(parent.id);
        }
        r = s
            .stage(&e, &r, it.id, EntryTransition::Publish, Some(c))
            .await
            .unwrap();
        items.push(it);
    }
    // The 37th entry's parent goes to the trash after staging.
    let mut p = s.get(parent.id).await;
    p.status = ContentStatus::Trash;
    s.store.read().content().update(&p).await.unwrap();
    let before = s.store.read().events().max_seq().await.unwrap();
    let err = s
        .releases
        .apply(&e, r.id, r.version, None)
        .await
        .unwrap_err();
    match err {
        CoreError::ReleaseConflict(entries) => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].item, items[36].id);
            assert!(matches!(entries[0].outcome, EntryOutcome::Invalid { .. }));
        }
        other => panic!("{other:?}"),
    }
    for it in &items {
        let now = s.get(it.id).await;
        assert_eq!(
            (now.version, now.status),
            (it.version, ContentStatus::Draft)
        );
    }
    assert_eq!(s.store.read().events().max_seq().await.unwrap(), before);
    assert_eq!(s.stored(r.id).await.status, ReleaseStatus::Open);
}

async fn apply_writes_one_invalidate_event_with_all_keys(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let b = s.published(&e, "B").await;
    let r = s.release(&e).await;
    let r = s
        .stage(&e, &r, a.id, EntryTransition::Publish, Some(change("A2")))
        .await
        .unwrap();
    let r = s
        .stage(&e, &r, b.id, EntryTransition::Unpublish, None)
        .await
        .unwrap();
    let before = s.store.read().events().max_seq().await.unwrap();
    let applied = s.releases.apply(&e, r.id, r.version, None).await.unwrap();
    assert_eq!(applied.status, ReleaseStatus::Applied);
    let evs = s.store.read().events().since(before, 100).await.unwrap();
    let inval: Vec<_> = evs
        .iter()
        .filter(|ev| ev.kind == events::INVALIDATE)
        .collect();
    assert_eq!(inval.len(), 1, "{evs:?}");
    for k in [events::post_key(a.id), events::post_key(b.id)] {
        assert!(inval[0].keys.contains(&k), "{k}");
    }
    assert_eq!(s.get(a.id).await.status, ContentStatus::Published);
    assert_eq!(s.get(a.id).await.title, "A2");
    assert_eq!(s.get(b.id).await.status, ContentStatus::Draft);
}

async fn apply_writes_revisions_with_release_note(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.published(&e, "A").await;
    let r = s.release(&e).await;
    let r = s
        .stage(&e, &r, a.id, EntryTransition::None, Some(change("A2")))
        .await
        .unwrap();
    s.releases.apply(&e, r.id, r.version, None).await.unwrap();
    let revs = s.content.revisions(&e, a.id, 1).await.unwrap();
    assert_eq!(revs[0].note.as_deref(), Some("Release: Spring launch"));
    assert_eq!(revs[0].title, "A2");
}

async fn apply_runs_publish_side_effects(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let r = s.release(&e).await;
    let r = s
        .stage(&e, &r, a.id, EntryTransition::Publish, None)
        .await
        .unwrap();
    s.releases.apply(&e, r.id, r.version, None).await.unwrap();
    let now = s.get(a.id).await;
    assert_eq!(now.status, ContentStatus::Published);
    assert!(now.published_at.is_some());
    let audit = s
        .store
        .read()
        .audit()
        .list(&ferrite_domain::AuditQuery {
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(audit.items.iter().any(|a| a.action == "release.apply"));
}

async fn conflict_blocks_apply_and_lists_entries(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let b = s.item(&e, "post", "B").await;
    let mut r = s.release(&e).await;
    for it in [&a, &b] {
        r = s
            .stage(&e, &r, it.id, EntryTransition::Publish, None)
            .await
            .unwrap();
    }
    let moved = s
        .content
        .update(&e, b.id, b.version, draft("post", "B moved"))
        .await
        .unwrap();
    let err = s
        .releases
        .apply(&e, r.id, r.version, None)
        .await
        .unwrap_err();
    match err {
        CoreError::ReleaseConflict(entries) => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].item, b.id);
            assert_eq!(
                entries[0].outcome,
                EntryOutcome::Conflict {
                    current_version: moved.version
                }
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.get(a.id).await.status, ContentStatus::Draft);
}

async fn check_reports_every_problem_and_writes_nothing(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let b = s.item(&e, "post", "B").await;
    let c = s.item(&e, "post", "C").await;
    let mut r = s.release(&e).await;
    for it in [&a, &b, &c] {
        r = s
            .stage(&e, &r, it.id, EntryTransition::Publish, None)
            .await
            .unwrap();
    }
    let b2 = s
        .content
        .update(&e, b.id, b.version, draft("post", "B2"))
        .await
        .unwrap();
    // C goes to the trash behind the release's back.
    let mut tc = s.get(c.id).await;
    tc.status = ContentStatus::Trash;
    s.store.read().content().update(&tc).await.unwrap();
    let before = s.store.read().events().max_seq().await.unwrap();
    let report = s.releases.check(&e, r.id).await.unwrap();
    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.entries[0].outcome, EntryOutcome::Ok);
    assert_eq!(
        report.entries[1].outcome,
        EntryOutcome::Conflict {
            current_version: b2.version
        }
    );
    // Applied over the moved version, the publish of a trashed item is refused.
    assert!(matches!(
        report.entries[2].outcome,
        EntryOutcome::Invalid { .. }
    ));
    assert!(!report.is_clean());
    assert_eq!(s.get(a.id).await.status, ContentStatus::Draft);
    assert_eq!(s.store.read().events().max_seq().await.unwrap(), before);
}

fn later(secs: i64) -> Timestamp {
    Timestamp::from_micros(Timestamp::now().as_micros() + secs * 1_000_000).unwrap()
}

async fn schedule_requires_clean_check_and_locks(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let r = s.release(&e).await;
    let r = s
        .stage(&e, &r, a.id, EntryTransition::Publish, None)
        .await
        .unwrap();
    // A moved item: not ready.
    let a2 = s
        .content
        .update(&e, a.id, a.version, draft("post", "A2"))
        .await
        .unwrap();
    let err = s
        .releases
        .schedule(&e, r.id, r.version, later(3600), None)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::ReleaseNotReady(_)), "{err:?}");
    let r = s.releases.rebase(&e, r.id, r.version, a.id).await.unwrap();
    let at = later(3600);
    let r = s
        .releases
        .schedule(&e, r.id, r.version, at, None)
        .await
        .unwrap();
    assert_eq!(
        (r.status, r.scheduled_at, r.scheduled_by),
        (ReleaseStatus::Scheduled, Some(at), Some(e.user))
    );
    assert_eq!(
        s.store.read().releases().lock_of(a.id).await.unwrap(),
        Some(r.id)
    );
    let jobs = s
        .store
        .read()
        .jobs()
        .list(&JobQuery {
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(jobs.items.iter().any(|j| j.kind == RELEASE_APPLY_KIND));
    assert!(matches!(
        s.content
            .update(&e, a.id, a2.version, draft("post", "x"))
            .await,
        Err(CoreError::Locked { .. })
    ));
    // In the past: refused.
    let r2 = s.release(&e).await;
    assert!(
        s.releases
            .schedule(&e, r2.id, r2.version, later(-10), None)
            .await
            .is_err()
    );
}

async fn second_scheduled_release_cannot_lock_same_item(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let r1 = s.release(&e).await;
    let r1 = s
        .stage(&e, &r1, a.id, EntryTransition::Publish, None)
        .await
        .unwrap();
    let r2 = s.release(&e).await;
    let r2 = s
        .stage(&e, &r2, a.id, EntryTransition::Publish, None)
        .await
        .unwrap();
    s.releases
        .schedule(&e, r1.id, r1.version, later(3600), None)
        .await
        .unwrap();
    let err = s
        .releases
        .schedule(&e, r2.id, r2.version, later(3600), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Locked { release } if release == r1.id),
        "{err:?}"
    );
}

async fn scheduled(s: &Site, e: &Actor) -> (Release, ContentItem, Timestamp) {
    let a = s.item(e, "post", "A").await;
    let r = s.release(e).await;
    let r = s
        .stage(e, &r, a.id, EntryTransition::Publish, None)
        .await
        .unwrap();
    let at = later(3600);
    let r = s
        .releases
        .schedule(e, r.id, r.version, at, None)
        .await
        .unwrap();
    (r, a, at)
}

async fn scheduled_apply_runs_as_scheduler_reread(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let (r, a, at) = scheduled(&s, &e).await;
    // The scheduler is demoted: the apply fails as them.
    let mut u = s.store.read().users().get(e.user).await.unwrap().unwrap();
    u.role = Role::Contributor;
    s.store.read().users().update(&u).await.unwrap();
    assert!(!s.releases.apply_scheduled(r.id, at).await.unwrap());
    let failed = s.stored(r.id).await;
    assert_eq!(failed.status, ReleaseStatus::Failed);
    assert_eq!(s.get(a.id).await.status, ContentStatus::Draft);
}

async fn failed_release_is_unlocked_with_report_and_notify_job(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let (r, a, at) = scheduled(&s, &e).await;
    // A system path (an import) moves the item while it is locked.
    let mut moved = s.get(a.id).await;
    moved.title = "imported".into();
    s.store.read().content().update(&moved).await.unwrap();
    assert!(!s.releases.apply_scheduled(r.id, at).await.unwrap());
    let failed = s.stored(r.id).await;
    assert_eq!(failed.status, ReleaseStatus::Failed);
    let failure = failed.failure.unwrap();
    assert_eq!(failure["entries"][0]["outcome"], "conflict");
    assert_eq!(s.store.read().releases().lock_of(a.id).await.unwrap(), None);
    let jobs = s
        .store
        .read()
        .jobs()
        .list(&JobQuery {
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        jobs.items
            .iter()
            .any(|j| j.kind == EDITORIAL_NOTIFY_KIND && j.payload["event"] == "release.failed")
    );
    // A failed release can be fixed and applied by hand.
    let fixed = s
        .releases
        .rebase(&e, r.id, failed.version, a.id)
        .await
        .unwrap();
    s.releases
        .apply(&e, r.id, fixed.version, None)
        .await
        .unwrap();
}

async fn job_and_periodic_race_apply_once(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let (r, a, at) = scheduled(&s, &e).await;
    assert!(s.releases.apply_scheduled(r.id, at).await.unwrap());
    assert!(!s.releases.apply_scheduled(r.id, at).await.unwrap());
    assert_eq!(s.releases.apply_due(later(7200)).await.unwrap(), 0);
    assert_eq!(s.get(a.id).await.status, ContentStatus::Published);
    assert_eq!(s.stored(r.id).await.status, ReleaseStatus::Applied);
}

async fn apply_due_applies_due_releases(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let (r, a, at) = scheduled(&s, &e).await;
    assert_eq!(s.releases.apply_due(Timestamp::now()).await.unwrap(), 0);
    let after = Timestamp::from_micros(at.as_micros() + 1).unwrap();
    assert_eq!(s.releases.apply_due(after).await.unwrap(), 1);
    assert_eq!(s.get(a.id).await.status, ContentStatus::Published);
    assert_eq!(s.stored(r.id).await.status, ReleaseStatus::Applied);
}

async fn unschedule_makes_job_noop(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let (r, a, at) = scheduled(&s, &e).await;
    let r = s.releases.unschedule(&e, r.id, r.version).await.unwrap();
    assert_eq!(r.status, ReleaseStatus::Open);
    assert_eq!(s.store.read().releases().lock_of(a.id).await.unwrap(), None);
    assert!(!s.releases.apply_scheduled(r.id, at).await.unwrap());
    assert_eq!(s.get(a.id).await.status, ContentStatus::Draft);
}

async fn applied_release_deletes_its_preview_links(db: &TestDb) {
    let s = site(db);
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let a = s.item(&e, "post", "A").await;
    let r = s.release(&e).await;
    let r = s
        .stage(&e, &r, a.id, EntryTransition::Publish, None)
        .await
        .unwrap();
    let link = PreviewLink {
        id: PreviewLinkId::new(),
        target: PreviewTarget::Release(r.id),
        created_by: e.user,
        created_at: Timestamp::now(),
        expires_at: later(3600),
        last_used_at: None,
    };
    s.store
        .read()
        .preview_links()
        .insert(&link, "hash")
        .await
        .unwrap();
    s.releases.apply(&e, r.id, r.version, None).await.unwrap();
    assert_eq!(
        s.store.read().preview_links().get(link.id).await.unwrap(),
        None
    );
}

/// P4: a release publishing an item that fails the type's quality gate is refused as a
/// whole; nothing of it is written.
async fn quality_gate_refuses_the_whole_release(db: &TestDb) {
    let s = site(db);
    let admin = actor(s.store.as_ref(), Role::Admin).await;
    ferrite_core::ContentTypeService::new(s.store.clone())
        .update(
            &admin,
            "post",
            ferrite_core::TypePatch {
                quality_gate: Some(ferrite_domain::QualityGate::Errors),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let e = actor(s.store.as_ref(), Role::Editor).await;
    let good = s.item(&e, "post", "Good").await;
    let mut bad = draft("post", "Bad");
    bad.body = json!({"type": "doc", "content": [{"type": "image", "attrs": {"src": "/a.png"}}]});
    let bad = s.content.create(&e, bad).await.unwrap();
    let mut r = s.release(&e).await;
    for it in [&good, &bad] {
        r = s
            .stage(&e, &r, it.id, EntryTransition::Publish, None)
            .await
            .unwrap();
    }
    let report = s.releases.check(&e, r.id).await.unwrap();
    assert!(matches!(
        report.entries[1].outcome,
        EntryOutcome::Invalid { .. }
    ));
    let err = s
        .releases
        .apply(&e, r.id, r.version, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::ReleaseConflict(_)), "{err:?}");
    assert_eq!(s.get(good.id).await.status, ContentStatus::Draft);
    assert_eq!(s.get(bad.id).await.status, ContentStatus::Draft);
}

backend_tests!(
    quality_gate_refuses_the_whole_release,
    contributor_creates_and_stages_own_draft,
    cannot_stage_others_item_without_edit_others,
    publish_entry_needs_publish_right,
    publish_entry_refused_for_published_or_scheduled_item,
    entry_needs_change_or_transition,
    staged_change_is_validated,
    two_hundred_entries_max,
    rebase_bumps_entry_version,
    author_cannot_see_others_release,
    locked_item_refuses_update_publish_trash_restore_revision,
    autosave_is_not_locked,
    apply_is_all_or_nothing,
    apply_writes_one_invalidate_event_with_all_keys,
    apply_writes_revisions_with_release_note,
    apply_runs_publish_side_effects,
    conflict_blocks_apply_and_lists_entries,
    check_reports_every_problem_and_writes_nothing,
    schedule_requires_clean_check_and_locks,
    second_scheduled_release_cannot_lock_same_item,
    scheduled_apply_runs_as_scheduler_reread,
    failed_release_is_unlocked_with_report_and_notify_job,
    job_and_periodic_race_apply_once,
    apply_due_applies_due_releases,
    unschedule_makes_job_noop,
    applied_release_deletes_its_preview_links,
);
