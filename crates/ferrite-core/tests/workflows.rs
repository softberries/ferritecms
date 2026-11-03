//! Approval workflows, the publish gate and reviews (M6-3 tasks 6 and 7, design 15 §6,
//! ruling P7) on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, PermalinkRules,
    content::allowed_actions,
    jobs::EDITORIAL_NOTIFY_KIND,
    release::{EntryInput, EntryOutcome, NewRelease, ReleaseService},
    workflow::{ReviewService, SubmitInput, WorkflowService, one_editor_preset},
};
use ferrite_domain::{
    Approvers, AuditQuery, ContentItem, ContentStatus, Decision, EntryTransition, JobQuery,
    NewUser, ReviewQuery, ReviewStatus, Role, StagedChange, Timestamp, UserId, UserStatus,
    WorkflowStep,
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
    workflows: WorkflowService,
    reviews: ReviewService,
    admin: Actor,
    editor: Actor,
    editor2: Actor,
    author: Actor,
}

async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let rules = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    let s = Site {
        releases: ReleaseService::new(store.clone(), rules.clone(), content.clone()),
        workflows: WorkflowService::new(store.clone()),
        reviews: ReviewService::new(store.clone(), rules),
        admin: actor(store.as_ref(), Role::Admin).await,
        editor: actor(store.as_ref(), Role::Editor).await,
        editor2: actor(store.as_ref(), Role::Editor).await,
        author: actor(store.as_ref(), Role::Author).await,
        store,
        content,
    };
    s.workflows
        .put(&s.admin, "post", one_editor_preset(), None)
        .await
        .unwrap();
    s
}

fn body(text: &str) -> serde_json::Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

fn draft(title: &str) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
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

impl Site {
    async fn get(&self, it: &ContentItem) -> ContentItem {
        self.store
            .read()
            .content()
            .get(it.id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn submitted(&self, who: &Actor, title: &str) -> ContentItem {
        let it = self.content.create(who, draft(title)).await.unwrap();
        self.content
            .submit_with(who, it.id, it.version, SubmitInput::default())
            .await
            .unwrap()
    }

    async fn open_review(&self, it: &ContentItem) -> ferrite_domain::Review {
        self.store
            .read()
            .reviews()
            .open_for(&ferrite_domain::ReviewTarget::Item(it.id))
            .await
            .unwrap()
            .expect("open review")
    }

    async fn approve(&self, who: &Actor, it: &ContentItem) -> ferrite_domain::Review {
        let r = self.open_review(it).await;
        self.reviews
            .decide(who, r.id, r.version, Decision::Approve, None)
            .await
            .unwrap()
    }

    async fn audit_actions(&self) -> Vec<String> {
        self.store
            .read()
            .audit()
            .list(&AuditQuery {
                limit: 50,
                ..Default::default()
            })
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|a| a.action)
            .collect()
    }
}

fn later() -> Timestamp {
    Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap()
}

// ---- Task 6 ----

async fn no_workflow_behaves_as_before(db: &TestDb) {
    let s = site(db).await;
    let page = s
        .content
        .create(
            &s.author,
            ContentDraft {
                kind: "page".into(),
                ..draft("Page")
            },
        )
        .await
        .unwrap();
    let p = s
        .content
        .publish(&s.author, page.id, page.version)
        .await
        .unwrap();
    s.content
        .update(
            &s.author,
            p.id,
            p.version,
            ContentDraft {
                kind: "page".into(),
                ..draft("Edited live")
            },
        )
        .await
        .unwrap();
}

async fn workflow_blocks_publish_and_schedule_without_approval(db: &TestDb) {
    let s = site(db).await;
    let it = s.content.create(&s.editor, draft("Mine")).await.unwrap();
    let err = s
        .content
        .publish(&s.editor, it.id, it.version)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::ApprovalRequired { step: None }),
        "{err:?}"
    );
    let err = s
        .content
        .schedule(&s.editor, it.id, it.version, later())
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::ApprovalRequired { .. }));
    let it = s
        .content
        .submit_with(&s.editor, it.id, it.version, SubmitInput::default())
        .await
        .unwrap();
    let err = s
        .content
        .publish(&s.editor, it.id, it.version)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CoreError::ApprovalRequired { step: Some(l) } if l == "Editor review"),
        "{err:?}"
    );
    let wf = s.content.workflow_state(&s.editor, &it).await.unwrap();
    let allowed = allowed_actions(&s.editor, &it, &wf);
    assert!(!allowed.publish && allowed.needs_approval && !allowed.r#override);
}

async fn approved_current_review_allows_publish(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.editor, "Mine").await;
    let r = s.approve(&s.editor2, &it).await;
    assert_eq!(r.status, ReviewStatus::Approved);
    let wf = s.content.workflow_state(&s.editor, &it).await.unwrap();
    assert!(allowed_actions(&s.editor, &it, &wf).publish);
    let it = s.get(&it).await;
    s.content
        .publish(&s.editor, it.id, it.version)
        .await
        .unwrap();
}

async fn stale_review_blocks_again(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "Mine").await;
    s.approve(&s.editor, &it).await;
    let it = s
        .content
        .update(&s.author, it.id, it.version, draft("Edited after approval"))
        .await
        .unwrap();
    let err = s
        .content
        .publish(&s.author, it.id, it.version)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::ApprovalRequired { .. }));
    // Resubmit from pending: a new review, no status change.
    let again = s
        .content
        .submit_with(&s.author, it.id, it.version, SubmitInput::default())
        .await
        .unwrap();
    assert_eq!(again.status, ContentStatus::Pending);
    assert_eq!(s.open_review(&it).await.status, ReviewStatus::InReview);
}

async fn published(s: &Site) -> ContentItem {
    let it = s.submitted(&s.author, "Live").await;
    s.approve(&s.editor, &it).await;
    let it = s.get(&it).await;
    s.content
        .publish(&s.author, it.id, it.version)
        .await
        .unwrap()
}

async fn published_item_cannot_be_updated_in_place(db: &TestDb) {
    let s = site(db).await;
    let it = published(&s).await;
    let err = s
        .content
        .update(&s.author, it.id, it.version, draft("typo"))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::StagingRequired), "{err:?}");
    let wf = s.content.workflow_state(&s.author, &it).await.unwrap();
    let allowed = allowed_actions(&s.author, &it, &wf);
    assert!(!allowed.edit && allowed.stage);
    // P7: an approver fixes a typo in place, audited.
    let wf = s.content.workflow_state(&s.editor, &it).await.unwrap();
    assert!(allowed_actions(&s.editor, &it, &wf).edit);
    s.content
        .update(&s.editor, it.id, it.version, draft("typo fixed"))
        .await
        .unwrap();
    assert!(
        s.audit_actions()
            .await
            .contains(&"review.bypassed".to_owned())
    );
}

async fn restore_revision_of_public_item_refused(db: &TestDb) {
    let s = site(db).await;
    let it = published(&s).await;
    let rev = s.content.revisions(&s.author, it.id, 1).await.unwrap()[0].id;
    let err = s
        .content
        .restore_revision(&s.author, it.id, rev, it.version)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::StagingRequired));
}

async fn unpublish_and_trash_not_gated(db: &TestDb) {
    let s = site(db).await;
    let it = published(&s).await;
    let it = s
        .content
        .unpublish(&s.author, it.id, it.version)
        .await
        .unwrap();
    s.content.trash(&s.author, it.id, it.version).await.unwrap();
}

async fn admin_override_needs_reason_and_audits(db: &TestDb) {
    let s = site(db).await;
    let it = s.content.create(&s.admin, draft("Urgent")).await.unwrap();
    let wf = s.content.workflow_state(&s.admin, &it).await.unwrap();
    assert!(allowed_actions(&s.admin, &it, &wf).r#override);
    let err = s
        .content
        .publish_with_override(&s.admin, it.id, it.version, Some("  ".into()))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CoreError::InvalidField {
            field: "override_reason",
            ..
        }
    ));
    let p = s
        .content
        .publish_with_override(&s.admin, it.id, it.version, Some("legal deadline".into()))
        .await
        .unwrap();
    assert_eq!(p.status, ContentStatus::Published);
    assert!(
        s.audit_actions()
            .await
            .contains(&"workflow.override".to_owned())
    );
}

async fn editor_cannot_override(db: &TestDb) {
    let s = site(db).await;
    let it = s.content.create(&s.editor, draft("x")).await.unwrap();
    let err = s
        .content
        .publish_with_override(&s.editor, it.id, it.version, Some("because".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Forbidden("admin")), "{err:?}");
}

async fn release_entry_needs_entry_review(db: &TestDb) {
    let s = site(db).await;
    let it = published(&s).await;
    let r = s
        .releases
        .create(
            &s.author,
            NewRelease {
                title: "Fix".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    let change = StagedChange {
        title: "Better".into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: body("Better"),
        fields: json!({}),
        menu_order: 0,
        terms: vec![],
        seo: None,
    };
    let r = s
        .releases
        .stage(
            &s.author,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::None,
                changes: Some(change.clone()),
            },
        )
        .await
        .unwrap();
    let report = s.releases.check(&s.author, r.id).await.unwrap();
    assert!(matches!(
        report.entries[0].outcome,
        EntryOutcome::NeedsApproval { .. }
    ));
    let review = s
        .reviews
        .submit_entry(&s.author, r.id, it.id, SubmitInput::default())
        .await
        .unwrap();
    s.reviews
        .decide(
            &s.editor,
            review.id,
            review.version,
            Decision::Approve,
            None,
        )
        .await
        .unwrap();
    // An edit of the entry makes the approval stale.
    let mut c2 = change.clone();
    c2.title = "Better still".into();
    let r = s
        .releases
        .stage(
            &s.author,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::None,
                changes: Some(c2),
            },
        )
        .await
        .unwrap();
    assert!(
        s.releases
            .apply(&s.author, r.id, r.version, None)
            .await
            .is_err()
    );
    let review = s
        .reviews
        .submit_entry(&s.author, r.id, it.id, SubmitInput::default())
        .await
        .unwrap();
    s.reviews
        .decide(
            &s.editor,
            review.id,
            review.version,
            Decision::Approve,
            None,
        )
        .await
        .unwrap();
    s.releases
        .apply(&s.author, r.id, r.version, None)
        .await
        .unwrap();
    assert_eq!(s.get(&it).await.title, "Better still");
}

async fn workflow_steps_validated(db: &TestDb) {
    let s = site(db).await;
    let step = |key: &str, roles: Vec<Role>, users: Vec<UserId>, required: u32| WorkflowStep {
        key: key.into(),
        label: "Label".into(),
        approvers: Approvers { roles, users },
        required,
    };
    let bad = [
        vec![],
        vec![step("a", vec![Role::Author], vec![], 1)],
        vec![step("a", vec![], vec![], 1)],
        vec![step("a", vec![Role::Editor], vec![], 0)],
        vec![step("a", vec![Role::Editor], vec![], 6)],
        vec![step("A b", vec![Role::Editor], vec![], 1)],
        vec![
            step("a", vec![Role::Editor], vec![], 1),
            step("a", vec![Role::Editor], vec![], 1),
        ],
        vec![step("a", vec![], vec![s.author.user], 1)],
        (0..6)
            .map(|i| step(&format!("s{i}"), vec![Role::Editor], vec![], 1))
            .collect(),
    ];
    for steps in bad {
        let err = s
            .workflows
            .put(&s.admin, "page", steps.clone(), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidField { field: "steps", .. }),
            "{steps:?}: {err:?}"
        );
    }
    let ok = s
        .workflows
        .put(
            &s.admin,
            "page",
            vec![step("legal", vec![], vec![s.editor.user], 1)],
            None,
        )
        .await
        .unwrap();
    assert_eq!(ok.version, 1);
    assert!(matches!(
        s.workflows
            .put(&s.editor, "page", one_editor_preset(), Some(1))
            .await
            .unwrap_err(),
        CoreError::Forbidden("manage_settings")
    ));
}

async fn deleting_workflow_withdraws_open_reviews(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "x").await;
    let r = s.open_review(&it).await;
    let w = s.workflows.get(&s.admin, "post").await.unwrap();
    s.workflows
        .delete(&s.admin, "post", w.version)
        .await
        .unwrap();
    let got = s.store.read().reviews().get(r.id).await.unwrap().unwrap();
    assert_eq!(got.status, ReviewStatus::Withdrawn);
    // Without a workflow publishing is free again.
    let it = s.get(&it).await;
    s.content
        .publish(&s.author, it.id, it.version)
        .await
        .unwrap();
}

// ---- Task 7 ----

async fn submit_creates_review_at_step_zero(db: &TestDb) {
    let s = site(db).await;
    let it = s.content.create(&s.author, draft("x")).await.unwrap();
    let it = s
        .content
        .submit_with(
            &s.author,
            it.id,
            it.version,
            SubmitInput {
                reviewers: vec![s.editor.user],
                note: Some("please".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(it.status, ContentStatus::Pending);
    let r = s.open_review(&it).await;
    assert_eq!((r.step, r.status), (0, ReviewStatus::InReview));
    assert_eq!(r.note.as_deref(), Some("please"));
    assert!(r.revision_id.is_some());
    let detail = s.reviews.get_for(&s.author, r.id).await.unwrap();
    assert_eq!(detail.assignees, vec![(0, s.editor.user)]);
    assert_eq!(s.reviews.assigned_count(&s.editor).await.unwrap(), 1);
    let q = s
        .reviews
        .list(
            &s.editor,
            ReviewQuery {
                status: Some(ReviewStatus::InReview),
                assignee: Some(s.editor.user),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(q.items.len(), 1);
}

async fn resubmit_after_stale(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "x").await;
    let it = s
        .content
        .update(&s.author, it.id, it.version, draft("edited"))
        .await
        .unwrap();
    let old = s
        .store
        .read()
        .reviews()
        .list(&ReviewQuery {
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(old.items[0].status, ReviewStatus::Stale);
    s.content
        .submit_with(&s.author, it.id, it.version, SubmitInput::default())
        .await
        .unwrap();
    assert_eq!(s.open_review(&it).await.status, ReviewStatus::InReview);
}

async fn submitter_and_author_cannot_approve(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.editor, "editor's own").await;
    let r = s.open_review(&it).await;
    let err = s
        .reviews
        .decide(&s.editor, r.id, r.version, Decision::Approve, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Forbidden(_)));
    // Submitted by someone else, still the author's item.
    let it = s.content.create(&s.editor2, draft("e2")).await.unwrap();
    let it = s
        .content
        .submit_with(&s.admin, it.id, it.version, SubmitInput::default())
        .await
        .unwrap();
    let r = s.open_review(&it).await;
    assert!(
        s.reviews
            .decide(&s.editor2, r.id, r.version, Decision::Approve, None)
            .await
            .is_err()
    );
    // An author is no approver at all.
    assert!(matches!(
        s.reviews
            .decide(&s.author, r.id, r.version, Decision::Approve, None)
            .await
            .unwrap_err(),
        CoreError::Forbidden("edit_others")
    ));
}

async fn required_distinct_approvals_advance_step(db: &TestDb) {
    let s = site(db).await;
    let two = |key: &str, required| WorkflowStep {
        key: key.into(),
        label: key.into(),
        approvers: Approvers {
            roles: vec![Role::Editor, Role::Admin],
            users: vec![],
        },
        required,
    };
    let w = s.workflows.get(&s.admin, "post").await.unwrap();
    s.workflows
        .put(
            &s.admin,
            "post",
            vec![two("copy", 2), two("legal", 1)],
            Some(w.version),
        )
        .await
        .unwrap();
    let it = s.submitted(&s.author, "x").await;
    let r = s.approve(&s.editor, &it).await;
    assert_eq!((r.step, r.status), (0, ReviewStatus::InReview));
    assert!(
        s.reviews
            .decide(&s.editor, r.id, r.version, Decision::Approve, None)
            .await
            .is_err(),
        "once per step"
    );
    let r = s.approve(&s.editor2, &it).await;
    assert_eq!((r.step, r.status), (1, ReviewStatus::InReview));
    let r = s.approve(&s.editor, &it).await;
    assert_eq!(r.status, ReviewStatus::Approved);
}

async fn request_changes_sends_item_to_draft(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "x").await;
    let r = s.open_review(&it).await;
    let r = s
        .reviews
        .decide(
            &s.editor,
            r.id,
            r.version,
            Decision::RequestChanges,
            Some("fix the intro".into()),
        )
        .await
        .unwrap();
    assert_eq!(r.status, ReviewStatus::ChangesRequested);
    assert_eq!(s.get(&it).await.status, ContentStatus::Draft);
    let d = s.reviews.get_for(&s.author, r.id).await.unwrap();
    assert_eq!(d.decisions[0].comment.as_deref(), Some("fix the intro"));
}

async fn withdraw(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "x").await;
    let r = s.open_review(&it).await;
    let other = actor(s.store.as_ref(), Role::Author).await;
    assert!(s.reviews.withdraw(&other, r.id, r.version).await.is_err());
    let r = s
        .reviews
        .withdraw(&s.author, r.id, r.version)
        .await
        .unwrap();
    assert_eq!(r.status, ReviewStatus::Withdrawn);
    assert_eq!(s.get(&it).await.status, ContentStatus::Draft);
}

async fn assignees_must_be_eligible(db: &TestDb) {
    let s = site(db).await;
    let it = s.content.create(&s.author, draft("x")).await.unwrap();
    let err = s
        .content
        .submit_with(
            &s.author,
            it.id,
            it.version,
            SubmitInput {
                reviewers: vec![s.author.user],
                note: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CoreError::InvalidField {
            field: "reviewers",
            ..
        }
    ));
    let it = s.submitted(&s.author, "y").await;
    let r = s.open_review(&it).await;
    let got = s
        .reviews
        .assign(&s.author, r.id, 0, vec![s.editor.user, s.editor2.user])
        .await
        .unwrap();
    assert_eq!(got.len(), 2);
}

async fn steps_snapshot_survives_workflow_edit(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "x").await;
    let w = s.workflows.get(&s.admin, "post").await.unwrap();
    let mut steps = one_editor_preset();
    steps[0].required = 3;
    s.workflows
        .put(&s.admin, "post", steps, Some(w.version))
        .await
        .unwrap();
    let r = s.approve(&s.editor, &it).await;
    assert_eq!(
        r.status,
        ReviewStatus::Approved,
        "the review keeps required = 1"
    );
}

async fn save_marks_review_stale(db: &TestDb) {
    let s = site(db).await;
    let it = s.submitted(&s.author, "x").await;
    let r = s.open_review(&it).await;
    s.content
        .update(&s.author, it.id, it.version, draft("y"))
        .await
        .unwrap();
    let got = s.store.read().reviews().get(r.id).await.unwrap().unwrap();
    assert_eq!(got.status, ReviewStatus::Stale);
}

async fn entry_edit_marks_entry_review_stale(db: &TestDb) {
    let s = site(db).await;
    let it = published(&s).await;
    let r = s
        .releases
        .create(
            &s.author,
            NewRelease {
                title: "R".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    let mk = |t: &str| StagedChange {
        title: t.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: body(t),
        fields: json!({}),
        menu_order: 0,
        terms: vec![],
        seo: None,
    };
    let input = |t: &str| EntryInput {
        item: it.id,
        transition: EntryTransition::None,
        changes: Some(mk(t)),
    };
    let r = s
        .releases
        .stage(&s.author, r.id, r.version, input("a"))
        .await
        .unwrap();
    let rv = s
        .reviews
        .submit_entry(&s.author, r.id, it.id, SubmitInput::default())
        .await
        .unwrap();
    s.releases
        .stage(&s.author, r.id, r.version, input("b"))
        .await
        .unwrap();
    let got = s.store.read().reviews().get(rv.id).await.unwrap().unwrap();
    assert_eq!(got.status, ReviewStatus::Stale);
}

async fn notify_job_enqueued_in_same_tx(db: &TestDb) {
    let s = site(db).await;
    s.submitted(&s.author, "x").await;
    let jobs = s
        .store
        .read()
        .jobs()
        .list(&JobQuery {
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        jobs.items
            .iter()
            .any(|j| j.kind == EDITORIAL_NOTIFY_KIND && j.payload["event"] == "review.submitted")
    );
}

/// Review finding 8: whoever staged an entry's change cannot approve it.
async fn stager_cannot_approve_entry(db: &TestDb) {
    let s = site(db).await;
    let it = published(&s).await;
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
    s.releases
        .stage(
            &s.editor,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::None,
                changes: Some(StagedChange {
                    title: "Mine".into(),
                    slug: None,
                    parent_id: None,
                    excerpt: None,
                    body: body("Mine"),
                    fields: json!({}),
                    menu_order: 0,
                    terms: vec![],
                    seo: None,
                }),
            },
        )
        .await
        .unwrap();
    let review = s
        .reviews
        .submit_entry(&s.editor2, r.id, it.id, SubmitInput::default())
        .await
        .unwrap();
    let err = s
        .reviews
        .decide(
            &s.editor,
            review.id,
            review.version,
            Decision::Approve,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
}

backend_tests!(
    stager_cannot_approve_entry,
    no_workflow_behaves_as_before,
    workflow_blocks_publish_and_schedule_without_approval,
    approved_current_review_allows_publish,
    stale_review_blocks_again,
    published_item_cannot_be_updated_in_place,
    restore_revision_of_public_item_refused,
    unpublish_and_trash_not_gated,
    admin_override_needs_reason_and_audits,
    editor_cannot_override,
    release_entry_needs_entry_review,
    workflow_steps_validated,
    deleting_workflow_withdraws_open_reviews,
    submit_creates_review_at_step_zero,
    resubmit_after_stale,
    submitter_and_author_cannot_approve,
    required_distinct_approvals_advance_step,
    request_changes_sends_item_to_draft,
    withdraw,
    assignees_must_be_eligible,
    steps_snapshot_survives_workflow_edit,
    save_marks_review_stale,
    entry_edit_marks_entry_review_stale,
    notify_job_enqueued_in_same_tx,
    withdraw_and_request_changes_emit_content_updated,
);

/// A `content.*` endpoint; the `content.updated` events written so far.
async fn content_hook(s: &Site) -> ferrite_core::webhooks::WebhookService {
    let hooks = ferrite_core::webhooks::WebhookService::new(
        s.store.clone(),
        ferrite_core::auth::SiteKey::from_hex(&"ab".repeat(32)).unwrap(),
        ferrite_core::webhooks::UrlPolicy {
            local_targets: false,
            allow_ip: |_| true,
        },
    );
    hooks
        .create(
            &s.admin,
            ferrite_core::webhooks::WebhookInput {
                name: "content".into(),
                url: "https://hooks.example.com/x".into(),
                events: vec!["content.*".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    hooks
}

async fn updated_events(s: &Site, item: &ContentItem) -> Vec<serde_json::Value> {
    let w = s.store.read().webhooks();
    let endpoint = &w.list_endpoints().await.unwrap()[0];
    let page = w
        .list_deliveries(
            endpoint.id,
            &ferrite_domain::DeliveryQuery {
                limit: 200,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    for l in page.items {
        let ev = w.get_event(l.delivery.event_id).await.unwrap().unwrap();
        if ev.kind == "content.updated" && ev.data["id"] == item.id.to_string() {
            out.push(ev.data);
        }
    }
    out
}

async fn withdraw_and_request_changes_emit_content_updated(db: &TestDb) {
    let s = site(db).await;
    let _hooks = content_hook(&s).await;
    // Withdraw.
    let it = s.submitted(&s.author, "w").await;
    let r = s.open_review(&it).await;
    s.reviews
        .withdraw(&s.author, r.id, r.version)
        .await
        .unwrap();
    let now = s.get(&it).await;
    assert_eq!(now.status, ContentStatus::Draft);
    let got = updated_events(&s, &it).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["status"], "draft");
    assert_eq!(got[0]["version"], now.version);
    assert_eq!(got[0]["locale"], now.locale);
    assert_eq!(
        got[0]["translation_group"],
        now.translation_group.to_string()
    );
    // Request changes.
    let it = s.submitted(&s.author, "rc").await;
    let r = s.open_review(&it).await;
    s.reviews
        .decide(&s.editor, r.id, r.version, Decision::RequestChanges, None)
        .await
        .unwrap();
    let got = updated_events(&s, &it).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["status"], "draft");
    assert_eq!(got[0]["actor"], s.editor.user.to_string());
}
