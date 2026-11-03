//! M6-4: `QualityService` checks and suppressions, and the per-type publish gate
//! (design 16 §5, §7.2) on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use ferrite_core::{
    Actor, ContentDraft, ContentService, ContentTypeService, CoreError, PermalinkRules, TypePatch,
    quality::{CheckDraft, PathProbe, PathStatus, QualityService},
    seed::DemoBatch,
};
use ferrite_domain::{
    ContentId, ContentStatus, MEDIA_KIND, NewContent, NewUser, QualityGate, Role, Timestamp,
    UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

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

fn content(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

fn quality(db: &TestDb) -> QualityService {
    QualityService::new(db.shared_store(), "https://example.com".into())
}

fn doc(content: Value) -> Value {
    json!({"type": "doc", "content": content})
}

/// An image without alt: one accessibility error.
fn bad_body() -> Value {
    doc(json!([{"type": "image", "attrs": {"src": "/a.png"}}]))
}

fn good_body() -> Value {
    doc(json!([{"type": "paragraph", "content": [{"type": "text", "text": "fine"}]}]))
}

fn draft(body: Value) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
        title: "Hello".into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body,
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

async fn gate(db: &TestDb, kind: &str, gate: QualityGate) -> Result<(), CoreError> {
    let admin = actor(db.store(), Role::Admin).await;
    ContentTypeService::new(db.shared_store())
        .update(
            &admin,
            kind,
            TypePatch {
                quality_gate: Some(gate),
                ..TypePatch::default()
            },
        )
        .await
        .map(|_| ())
}

fn blocked(r: Result<impl std::fmt::Debug, CoreError>) -> Vec<&'static str> {
    match r {
        Err(CoreError::QualityBlocked(f)) => f.iter().map(|f| f.rule.id).collect(),
        other => panic!("expected QualityBlocked, got {other:?}"),
    }
}

/// A probe that answers from a fixed list and counts its calls.
struct FakeProbe {
    calls: AtomicUsize,
}

#[async_trait]
impl PathProbe for FakeProbe {
    async fn probe(&self, path: &str) -> PathStatus {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match path {
            "/gone/" => PathStatus::Gone,
            "/moved/" => PathStatus::Redirected {
                to: "/new/".into(),
                item: None,
            },
            p if p.starts_with("/ok") => PathStatus::Ok { item: None },
            _ => PathStatus::NotFound,
        }
    }
}

async fn media(store: &dyn Store, author: &Actor, status: ContentStatus) -> ContentId {
    let id = ContentId::new();
    store
        .read()
        .content()
        .insert(&NewContent {
            id,
            kind: MEDIA_KIND.into(),
            status,
            slug: format!("m-{id}"),
            parent_id: None,
            author_id: author.user,
            title: "pic".into(),
            excerpt: String::new(),
            body: json!({}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: Some(Timestamp::now()),
            scheduled_at: None,
            created_at: None,
            updated_at: None,
            locale: "en".into(),
            translation_group: None,
        })
        .await
        .unwrap();
    id
}

fn check_draft(body: Value) -> CheckDraft {
    CheckDraft {
        id: None,
        title: "T".into(),
        body,
        fields: json!({}),
        locale: None,
    }
}

async fn check_reports_media_missing_and_trashed(db: &TestDb) {
    let author = actor(db.store(), Role::Author).await;
    let trashed = media(db.store(), &author, ContentStatus::Trash).await;
    let missing = ContentId::new();
    let body = doc(json!([
        {"type": "image", "attrs": {"src": format!("ferrite:media/{trashed}"), "alt": "a"}},
        {"type": "image", "attrs": {"src": format!("ferrite:media/{missing}"), "alt": "b"}},
    ]));
    let r = quality(db)
        .check(&author, "post", check_draft(body), None)
        .await
        .unwrap();
    let rules: Vec<_> = r.findings.iter().map(|f| f.finding.rule.id).collect();
    assert_eq!(rules, ["media-missing", "media-missing"]);
    assert_eq!(r.findings[0].finding.detail["reason"], "not_public");
    assert_eq!(r.findings[1].finding.detail["reason"], "missing");
    assert_eq!(r.counts.error, 2);
}

async fn check_hides_media_details_from_unreadable(db: &TestDb) {
    let author = actor(db.store(), Role::Author).await;
    let other = actor(db.store(), Role::Author).await;
    let trashed = media(db.store(), &other, ContentStatus::Trash).await;
    let body = doc(
        json!([{"type": "image", "attrs": {"src": format!("ferrite:media/{trashed}"), "alt": "a"}}]),
    );
    let r = quality(db)
        .check(&author, "post", check_draft(body), None)
        .await
        .unwrap();
    let detail = &r.findings[0].finding.detail;
    // Only the id the caller sent and the fact: no title, file or URL.
    let keys: Vec<_> = detail.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["id", "reason"]);
}

async fn check_marks_suppressed_and_excludes_from_counts(db: &TestDb) {
    let author = actor(db.store(), Role::Author).await;
    let item = content(db)
        .create(&author, draft(bad_body()))
        .await
        .unwrap();
    let q = quality(db);
    q.suppress(&author, item.id, "image-alt-missing", "/a.png", "a divider")
        .await
        .unwrap();
    let mut d = check_draft(bad_body());
    d.id = Some(item.id);
    let r = q.check(&author, "post", d, None).await.unwrap();
    assert_eq!(r.findings.len(), 1);
    assert!(r.findings[0].suppressed.is_some());
    assert_eq!((r.counts.error, r.counts.suppressed), (0, 1));
    assert_eq!(q.suppressions(&author, item.id).await.unwrap().len(), 1);
    // An item of another type is not found under this one.
    let mut d = check_draft(bad_body());
    d.id = Some(item.id);
    assert!(matches!(
        q.check(&author, "page", d, None).await,
        Err(CoreError::NotFound)
    ));
}

async fn check_probe_capped_at_200_paths(db: &TestDb) {
    let author = actor(db.store(), Role::Author).await;
    let mut inline = vec![
        json!({"type": "text", "text": "Gone page", "marks": [{"type": "link", "attrs": {"href": "/gone/"}}]}),
        json!({"type": "text", "text": " "}),
        json!({"type": "text", "text": "Moved page", "marks": [{"type": "link", "attrs": {"href": "https://example.com/moved/"}}]}),
        json!({"type": "text", "text": " "}),
    ];
    for i in 0..300 {
        inline.push(json!({"type": "text", "text": format!("Page {i}"), "marks": [{"type": "link", "attrs": {"href": format!("/ok/{i}")}}]}));
        inline.push(json!({"type": "text", "text": " "}));
    }
    let probe = FakeProbe {
        calls: AtomicUsize::new(0),
    };
    let r = quality(db)
        .check(
            &author,
            "post",
            check_draft(doc(json!([{"type": "paragraph", "content": inline}]))),
            Some(&probe),
        )
        .await
        .unwrap();
    assert_eq!(probe.calls.load(Ordering::SeqCst), 200);
    let rules: Vec<_> = r.findings.iter().map(|f| f.finding.rule.id).collect();
    assert_eq!(rules, ["link-internal-broken", "link-internal-redirected"]);
    assert_eq!(r.findings[0].finding.detail["status"], 410);
    assert_eq!(r.findings[1].finding.detail["to"], "/new/");
}

async fn publish_refused_on_gated_type_with_errors(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let c = content(db);
    let item = c.create(&editor, draft(bad_body())).await.unwrap();
    assert_eq!(
        blocked(c.publish(&editor, item.id, item.version).await),
        ["image-alt-missing"]
    );
    assert_eq!(c.get(item.id).await.unwrap().status, ContentStatus::Draft);
    // Pages are not gated.
    let mut p = draft(bad_body());
    p.kind = "page".into();
    let page = c.create(&editor, p).await.unwrap();
    c.publish(&editor, page.id, page.version).await.unwrap();
    // The gate's report mirrors it.
    let mut d = check_draft(bad_body());
    d.id = Some(item.id);
    let r = quality(db).check(&editor, "post", d, None).await.unwrap();
    assert!(r.gate.blocked);
    assert_eq!(r.gate.mode, QualityGate::Errors);
}

async fn publish_allowed_with_warnings_only(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let c = content(db);
    let body = doc(json!([
        {"type": "image", "attrs": {"src": "/a.png", "alt": ""}},
        {"type": "heading", "attrs": {"level": 4}, "content": [{"type": "text", "text": "Deep"}]},
    ]));
    let item = c.create(&editor, draft(body)).await.unwrap();
    c.publish(&editor, item.id, item.version).await.unwrap();
}

async fn publish_allowed_when_errors_suppressed(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let c = content(db);
    let item = c.create(&editor, draft(bad_body())).await.unwrap();
    quality(db)
        .suppress(&editor, item.id, "image-alt-missing", "", "decorative")
        .await
        .unwrap();
    c.publish(&editor, item.id, item.version).await.unwrap();
}

async fn schedule_refused_on_gated_type(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let c = content(db);
    let item = c.create(&editor, draft(bad_body())).await.unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap();
    blocked(c.schedule(&editor, item.id, item.version, at).await);
    // Submitting for review is not gated.
    let author = actor(db.store(), Role::Contributor).await;
    let mine = c.create(&author, draft(bad_body())).await.unwrap();
    c.submit(&author, mine.id, mine.version).await.unwrap();
}

async fn publish_due_and_publish_scheduled_never_gated(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    let c = content(db);
    let a = c.create(&editor, draft(bad_body())).await.unwrap();
    let b = c.create(&editor, draft(bad_body())).await.unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 60_000_000).unwrap();
    let a = c.schedule(&editor, a.id, a.version, at).await.unwrap();
    c.schedule(&editor, b.id, b.version, at).await.unwrap();
    gate(db, "post", QualityGate::Errors).await.unwrap();
    assert!(c.publish_scheduled(a.id, at).await.unwrap());
    assert_eq!(c.publish_due(at).await.unwrap(), 1);
    assert_eq!(c.get(b.id).await.unwrap().status, ContentStatus::Published);
}

async fn publish_in_not_gated(db: &TestDb) {
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let editor = actor(db.store(), Role::Editor).await;
    let batch = DemoBatch::begin(db.shared_store(), Arc::new(PermalinkRules::default()))
        .await
        .unwrap();
    let item = batch
        .publish(&editor, draft(bad_body()), None)
        .await
        .unwrap();
    batch.commit().await.unwrap();
    assert_eq!(
        content(db).get(item.id).await.unwrap().status,
        ContentStatus::Published
    );
}

async fn update_of_live_item_refused_only_when_errors_increase(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    let c = content(db);
    let item = c.create(&editor, draft(bad_body())).await.unwrap();
    let item = c.publish(&editor, item.id, item.version).await.unwrap();
    gate(db, "post", QualityGate::Errors).await.unwrap();
    // The old error stays: a typo fix passes.
    let mut d = draft(bad_body());
    d.title = "Hello, fixed".into();
    let item = c.update(&editor, item.id, item.version, d).await.unwrap();
    // A second error is refused.
    let two = doc(json!([
        {"type": "image", "attrs": {"src": "/a.png"}},
        {"type": "image", "attrs": {"src": "/b.png"}},
    ]));
    assert_eq!(
        blocked(c.update(&editor, item.id, item.version, draft(two)).await),
        ["image-alt-missing", "image-alt-missing"]
    );
    // Fixing it passes; drafts are never gated.
    let item = c
        .update(&editor, item.id, item.version, draft(good_body()))
        .await
        .unwrap();
    let d = c.create(&editor, draft(good_body())).await.unwrap();
    c.update(&editor, d.id, d.version, draft(bad_body()))
        .await
        .unwrap();
    assert_eq!(item.status, ContentStatus::Published);
}

async fn restore_revision_of_live_item_gated(db: &TestDb) {
    let editor = actor(db.store(), Role::Editor).await;
    let c = content(db);
    let item = c.create(&editor, draft(bad_body())).await.unwrap();
    let item = c
        .update(&editor, item.id, item.version, draft(good_body()))
        .await
        .unwrap();
    let item = c.publish(&editor, item.id, item.version).await.unwrap();
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let revisions = c.revisions(&editor, item.id, 50).await.unwrap();
    let bad = revisions
        .iter()
        .find(|r| r.body == bad_body())
        .expect("the first revision");
    blocked(
        c.restore_revision(&editor, item.id, bad.id, item.version)
            .await,
    );
}

async fn author_cannot_suppress_error_on_gated_type(db: &TestDb) {
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let author = actor(db.store(), Role::Author).await;
    let item = content(db)
        .create(&author, draft(bad_body()))
        .await
        .unwrap();
    let q = quality(db);
    assert!(matches!(
        q.suppress(&author, item.id, "image-alt-missing", "", "why")
            .await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    // A warning is the author's call.
    q.suppress(&author, item.id, "heading-level-skipped", "", "design")
        .await
        .unwrap();
}

async fn editor_can_suppress_error(db: &TestDb) {
    gate(db, "post", QualityGate::Errors).await.unwrap();
    let author = actor(db.store(), Role::Author).await;
    let editor = actor(db.store(), Role::Editor).await;
    let item = content(db)
        .create(&author, draft(bad_body()))
        .await
        .unwrap();
    let q = quality(db);
    q.suppress(
        &editor,
        item.id,
        "image-alt-missing",
        "/a.png",
        "  decorative ",
    )
    .await
    .unwrap();
    let s = q.suppressions(&editor, item.id).await.unwrap();
    assert_eq!(s[0].reason, "decorative");
    assert_eq!(s[0].user_id, Some(editor.user));
    q.unsuppress(&editor, item.id, "image-alt-missing", "/a.png")
        .await
        .unwrap();
    assert!(matches!(
        q.unsuppress(&editor, item.id, "image-alt-missing", "/a.png")
            .await,
        Err(CoreError::NotFound)
    ));
}

async fn author_can_suppress_warning(db: &TestDb) {
    let author = actor(db.store(), Role::Author).await;
    let item = content(db)
        .create(&author, draft(bad_body()))
        .await
        .unwrap();
    let q = quality(db);
    q.suppress(&author, item.id, "image-alt-empty", "", "ok")
        .await
        .unwrap();
    // Off gate: errors are the author's too.
    q.suppress(&author, item.id, "image-alt-missing", "", "ok")
        .await
        .unwrap();
    // Bad input.
    for (rule, key, reason, field) in [
        ("nope", "", "r", "rule"),
        ("redirect-loop", "", "r", "rule"),
        ("findings-truncated", "", "r", "rule"),
        ("image-alt-missing", "", "", "reason"),
        ("image-alt-missing", "", "a\u{0007}", "reason"),
        ("image-alt-missing", "a\nb", "r", "key"),
    ] {
        let long = "x".repeat(501);
        let reason = if reason == "long" { &long } else { reason };
        match q.suppress(&author, item.id, rule, key, reason).await {
            Err(CoreError::InvalidField { field: f, .. }) => assert_eq!(f, field, "{rule}"),
            other => panic!("{rule}/{key}/{reason}: {other:?}"),
        }
    }
    assert!(matches!(
        q.suppress(&author, item.id, "orphan", "", &"x".repeat(501))
            .await,
        Err(CoreError::InvalidField {
            field: "reason",
            ..
        })
    ));
    // Someone else's item needs edit_others.
    let other = actor(db.store(), Role::Author).await;
    assert!(q.suppress(&other, item.id, "stale", "", "r").await.is_err());
}

async fn suppress_audited(db: &TestDb) {
    let author = actor(db.store(), Role::Author).await;
    let item = content(db)
        .create(&author, draft(bad_body()))
        .await
        .unwrap();
    let q = quality(db);
    q.suppress(&author, item.id, "image-alt-missing", "", "r")
        .await
        .unwrap();
    q.unsuppress(&author, item.id, "image-alt-missing", "")
        .await
        .unwrap();
    let rows = db
        .store()
        .read()
        .audit()
        .list(&ferrite_domain::AuditQuery {
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap()
        .items;
    let actions: Vec<_> = rows.iter().map(|r| r.action.as_str()).collect();
    assert!(actions.contains(&"quality.suppress"), "{actions:?}");
    assert!(actions.contains(&"quality.unsuppress"), "{actions:?}");
    let row = rows
        .iter()
        .find(|r| r.action == "quality.suppress")
        .unwrap();
    assert_eq!(row.target_id.as_deref(), Some(item.id.to_string().as_str()));
    assert_eq!(row.details["rule"], "image-alt-missing");
}

async fn media_type_refuses_errors_gate(db: &TestDb) {
    assert!(matches!(
        gate(db, "media", QualityGate::Errors).await,
        Err(CoreError::InvalidField {
            field: "quality_gate",
            ..
        })
    ));
    gate(db, "media", QualityGate::Off).await.unwrap();
    gate(db, "page", QualityGate::Errors).await.unwrap();
    let ty = db
        .store()
        .read()
        .types()
        .get("page")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ty.quality_gate, QualityGate::Errors);
}

backend_tests!(
    check_reports_media_missing_and_trashed,
    check_hides_media_details_from_unreadable,
    check_marks_suppressed_and_excludes_from_counts,
    check_probe_capped_at_200_paths,
    publish_refused_on_gated_type_with_errors,
    publish_allowed_with_warnings_only,
    publish_allowed_when_errors_suppressed,
    schedule_refused_on_gated_type,
    publish_due_and_publish_scheduled_never_gated,
    publish_in_not_gated,
    update_of_live_item_refused_only_when_errors_increase,
    restore_revision_of_live_item_gated,
    author_cannot_suppress_error_on_gated_type,
    editor_can_suppress_error,
    author_can_suppress_warning,
    suppress_audited,
    media_type_refuses_errors_gate,
);
