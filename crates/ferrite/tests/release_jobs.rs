//! The M6-3 job handlers (design 15 §9.3): `release.apply`, `release.apply_due` and
//! `editorial.notify`, on both backends.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite::jobs::{EditorialNotifyHandler, ReleaseApplyDueHandler, ReleaseApplyHandler};
use ferrite_cluster::{JobError, JobHandler};
use ferrite_core::{
    Actor, ContentDraft, ContentService, PermalinkRules,
    jobs::{EditorialNotify, release_apply_job},
    mail::MemoryMailer,
    release::{EntryInput, NewRelease, ReleaseService},
    workflow::{SubmitInput, WorkflowService, one_editor_preset},
};
use ferrite_domain::{
    ContentStatus, EntryTransition, Job, JobId, JobStatus, NewUser, ReleaseStatus, Role, Timestamp,
    UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

fn job(kind: &str, payload: Value) -> Job {
    let now = Timestamp::now();
    Job {
        id: JobId::new(),
        kind: kind.into(),
        payload,
        unique_key: None,
        run_at: now,
        priority: 0,
        status: JobStatus::Running,
        attempts: 1,
        max_attempts: 3,
        locked_by: None,
        locked_until: None,
        last_error: None,
        created_at: now,
        finished_at: None,
    }
}

async fn actor(store: &dyn Store, role: Role, email: &str) -> Actor {
    let id = UserId::new();
    store
        .read()
        .users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: email.into(),
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
    releases: Arc<ReleaseService>,
    editor: Actor,
}

async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let rules = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    Site {
        releases: Arc::new(ReleaseService::new(store.clone(), rules, content.clone())),
        editor: actor(store.as_ref(), Role::Editor, "ed@example.com").await,
        store,
        content,
    }
}

fn draft(title: &str) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

/// A release publishing one new post, scheduled an hour ahead.
async fn scheduled(
    s: &Site,
) -> (
    ferrite_domain::Release,
    ferrite_domain::ContentItem,
    Timestamp,
) {
    let it = s.content.create(&s.editor, draft("A")).await.unwrap();
    let r = s
        .releases
        .create(
            &s.editor,
            NewRelease {
                title: "R".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    let r = s
        .releases
        .stage(
            &s.editor,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::Publish,
                changes: None,
            },
        )
        .await
        .unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap();
    let r = s
        .releases
        .schedule(&s.editor, r.id, r.version, at, None)
        .await
        .unwrap();
    (r, it, at)
}

async fn release_apply_handler_marks_conflicts_failed(db: &TestDb) {
    let s = site(db).await;
    let (r, it, at) = scheduled(&s).await;
    let h = ReleaseApplyHandler {
        releases: s.releases.clone(),
    };
    // A system path moved the item: the release fails, the job completes.
    let mut moved = s.store.read().content().get(it.id).await.unwrap().unwrap();
    moved.title = "imported".into();
    s.store.read().content().update(&moved).await.unwrap();
    let payload = release_apply_job(r.id, at).payload;
    h.run(&job("release.apply", payload)).await.unwrap();
    let got = s.store.read().releases().get(r.id).await.unwrap().unwrap();
    assert_eq!(got.status, ReleaseStatus::Failed);
    // A malformed payload is permanent.
    assert!(matches!(
        h.run(&job("release.apply", json!({"v": 1}))).await,
        Err(JobError::Permanent(_))
    ));
}

async fn release_apply_due_catches_missing_job(db: &TestDb) {
    let s = site(db).await;
    let (r, it, at) = scheduled(&s).await;
    // The scheduled time passes and its job is gone.
    let mut late = s.store.read().releases().get(r.id).await.unwrap().unwrap();
    let past = Timestamp::from_micros(Timestamp::now().as_micros() - 1_000_000).unwrap();
    late.scheduled_at = Some(past);
    s.store.read().releases().update(&late).await.unwrap();
    let _ = at;
    let h = ReleaseApplyDueHandler {
        store: s.store.clone(),
        releases: s.releases.clone(),
    };
    h.run(&job("release.apply_due", json!({"v": 1})))
        .await
        .unwrap();
    let got = s.store.read().content().get(it.id).await.unwrap().unwrap();
    assert_eq!(got.status, ContentStatus::Published);
}

async fn notify_recipients_and_subjects(db: &TestDb) {
    let s = site(db).await;
    let admin = actor(s.store.as_ref(), Role::Admin, "admin@example.com").await;
    let author = actor(s.store.as_ref(), Role::Author, "author@example.com").await;
    WorkflowService::new(s.store.clone())
        .put(&admin, "post", one_editor_preset(), None)
        .await
        .unwrap();
    let it = s
        .content
        .create(&author, draft("Line\u{202e}one"))
        .await
        .unwrap();
    s.content
        .submit_with(&author, it.id, it.version, SubmitInput::default())
        .await
        .unwrap();
    let review = s
        .store
        .read()
        .reviews()
        .open_for(&ferrite_domain::ReviewTarget::Item(it.id))
        .await
        .unwrap()
        .unwrap();
    let mailer = Arc::new(MemoryMailer::new());
    let h = EditorialNotifyHandler {
        store: s.store.clone(),
        mailer: Some(mailer.clone()),
        site_url: url::Url::parse("https://example.com/").unwrap(),
        site_title: "Blog".into(),
    };
    let payload = EditorialNotify {
        v: 1,
        event: "review.submitted".into(),
        review: Some(review.id),
        release: None,
    };
    h.run(&job(
        "editorial.notify",
        serde_json::to_value(&payload).unwrap(),
    ))
    .await
    .unwrap();
    let sent = mailer.sent();
    assert_eq!(sent.len(), 1);
    let mut bcc = sent[0].bcc.clone();
    bcc.sort();
    // Every eligible approver (editors and admins), never the author.
    assert_eq!(bcc, ["admin@example.com", "ed@example.com"]);
    assert_eq!(sent[0].subject, "[Blog] Review requested: Lineone");
    assert!(sent[0].text.contains(&format!(
        "https://example.com/admin/reviews?review={}",
        review.id
    )));
    // No [mail]: completes.
    let quiet = EditorialNotifyHandler {
        store: s.store.clone(),
        mailer: None,
        site_url: url::Url::parse("https://example.com/").unwrap(),
        site_title: "Blog".into(),
    };
    quiet
        .run(&job(
            "editorial.notify",
            serde_json::to_value(&payload).unwrap(),
        ))
        .await
        .unwrap();
}

backend_tests!(
    release_apply_handler_marks_conflicts_failed,
    release_apply_due_catches_missing_job,
    notify_recipients_and_subjects,
);
