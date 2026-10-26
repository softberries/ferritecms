//! SEO title and description on content writes (M2b P28) and the body root check, on
//! both backends (M2b-2 Task 1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, AutosaveDraft, ContentDraft, ContentService, CoreError, PermalinkRules,
    content::seo::{SEO_DESCRIPTION_META, SEO_TITLE_META, Seo, SeoPatch},
};
use ferrite_domain::{NewUser, Revision, RevisionId, Role, Timestamp, UserId, UserStatus};
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
            display_name: format!("Name {id}"),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    Actor::new(id, role)
}

fn service(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
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

fn set(title: Option<&str>, description: Option<&str>) -> SeoPatch {
    SeoPatch {
        title: title.map(|t| Some(t.to_owned())),
        description: description.map(|d| Some(d.to_owned())),
    }
}

fn field<T: std::fmt::Debug>(r: Result<T, CoreError>) -> &'static str {
    match r {
        Err(CoreError::InvalidField { field, .. }) => field,
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

/// `draft` as an update keeping what `create` stored (slug `None` keeps it), with `seo`.
fn same(title: &str, seo: Option<SeoPatch>) -> ContentDraft {
    ContentDraft {
        seo,
        ..draft(title)
    }
}

/// Create sets both rows (trimmed), `seo_of` reads them back; `null` and a blank string
/// delete the rows; an absent member keeps its row.
pub async fn seo_roundtrip_and_delete(db: &TestDb) {
    let svc = service(db);
    let author = actor(db.store(), Role::Author).await;
    let item = svc
        .create(
            &author,
            ContentDraft {
                seo: Some(set(Some("  A title for search "), Some("What it is about"))),
                ..draft("Seo roundtrip")
            },
        )
        .await
        .unwrap();
    assert_eq!(
        svc.seo_of(&author, item.id).await.unwrap(),
        Seo {
            title: Some("A title for search".into()),
            description: Some("What it is about".into()),
        }
    );
    let meta = db.store().read().meta();
    assert_eq!(
        meta.get(item.id, SEO_TITLE_META).await.unwrap(),
        Some(json!("A title for search"))
    );

    // Only the description changes: the title stays.
    let item = svc
        .update(
            &author,
            item.id,
            item.version,
            same(
                "Seo roundtrip",
                Some(SeoPatch {
                    description: Some(Some("Second".into())),
                    ..SeoPatch::default()
                }),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        svc.seo_of(&author, item.id).await.unwrap(),
        Seo {
            title: Some("A title for search".into()),
            description: Some("Second".into()),
        }
    );
    // `None` keeps both.
    let item = svc
        .update(&author, item.id, item.version, same("Seo roundtrip", None))
        .await
        .unwrap();
    assert_eq!(
        svc.seo_of(&author, item.id).await.unwrap().title.as_deref(),
        Some("A title for search")
    );

    // `null` and "  " delete the rows.
    svc.update(
        &author,
        item.id,
        item.version,
        same(
            "Seo roundtrip",
            Some(SeoPatch {
                title: Some(None),
                description: Some(Some("  ".into())),
            }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(svc.seo_of(&author, item.id).await.unwrap(), Seo::default());
    assert_eq!(meta.get(item.id, SEO_TITLE_META).await.unwrap(), None);
    assert_eq!(meta.get(item.id, SEO_DESCRIPTION_META).await.unwrap(), None);

    // A value a plugin stored that is not a string reads as none.
    meta.set(item.id, SEO_TITLE_META, &json!({"not": "text"}))
        .await
        .unwrap();
    assert_eq!(svc.seo_of(&author, item.id).await.unwrap().title, None);
}

/// 201 characters is `seo.title`, NUL or another control character refused; nothing is
/// written. The autosave slot checks the same and stores the patch as given.
pub async fn seo_limits_and_controls(db: &TestDb) {
    let svc = service(db);
    let author = actor(db.store(), Role::Author).await;
    let long = "t".repeat(201);
    let r = svc
        .create(
            &author,
            ContentDraft {
                seo: Some(set(Some(&long), None)),
                ..draft("Too long")
            },
        )
        .await;
    assert_eq!(field(r), "seo.title");
    let r = svc
        .create(
            &author,
            ContentDraft {
                seo: Some(set(None, Some(&"d".repeat(501)))),
                ..draft("Too long")
            },
        )
        .await;
    assert_eq!(field(r), "seo.description");
    // 200 characters (after trimming) is fine.
    let item = svc
        .create(
            &author,
            ContentDraft {
                seo: Some(set(Some(&format!(" {} ", "é".repeat(200))), None)),
                ..draft("Just right")
            },
        )
        .await
        .unwrap();
    for bad in ["a\0b", "line\nbreak"] {
        let r = svc
            .update(
                &author,
                item.id,
                item.version,
                same("Just right", Some(set(None, Some(bad)))),
            )
            .await;
        assert_eq!(field(r), "seo.description", "{bad:?}");
        let r = svc
            .update(
                &author,
                item.id,
                item.version,
                same("Just right", Some(set(Some(bad), None))),
            )
            .await;
        assert_eq!(field(r), "seo.title", "{bad:?}");
    }
    assert_eq!(svc.get(item.id).await.unwrap().version, item.version);

    // The autosave slot: the same checks, stored as given (absent and null apart).
    let slot = |seo| AutosaveDraft {
        base_version: item.version,
        title: "Working".into(),
        excerpt: None,
        slug: None,
        parent_id: None,
        menu_order: None,
        terms: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        seo,
    };
    let r = svc
        .put_autosave(&author, item.id, slot(Some(set(Some(&long), None))))
        .await;
    assert_eq!(field(r), "seo.title");
    let r = svc
        .put_autosave(&author, item.id, slot(Some(set(None, Some("a\0b")))))
        .await;
    assert_eq!(field(r), "seo.description");
    let patch = SeoPatch {
        title: Some(None),
        description: Some(Some(" kept as typed ".into())),
    };
    let saved = svc
        .put_autosave(&author, item.id, slot(Some(patch)))
        .await
        .unwrap();
    assert_eq!(
        saved.seo,
        Some(json!({"title": null, "description": " kept as typed "}))
    );
    let got = svc.get_autosave(&author, item.id).await.unwrap().unwrap();
    assert_eq!(got.seo, saved.seo);
    // None: unchanged (nothing stored).
    let saved = svc
        .put_autosave(&author, item.id, slot(None))
        .await
        .unwrap();
    assert_eq!(saved.seo, None);
}

/// A write that changes only the SEO meta is a full update: version + 1, a revision.
pub async fn seo_only_patch_bumps_version(db: &TestDb) {
    let svc = service(db);
    let author = actor(db.store(), Role::Author).await;
    let item = svc.create(&author, draft("Seo only")).await.unwrap();
    let revisions = |id| async move {
        db.store()
            .read()
            .revisions()
            .list(id, 50)
            .await
            .unwrap()
            .len()
    };
    assert_eq!(revisions(item.id).await, 1);
    let saved = svc
        .update(
            &author,
            item.id,
            item.version,
            same("Seo only", Some(set(Some("Search title"), None))),
        )
        .await
        .unwrap();
    assert_eq!(saved.version, item.version + 1);
    assert_eq!(
        (saved.title.as_str(), &saved.body),
        (item.title.as_str(), &item.body)
    );
    assert_eq!(revisions(item.id).await, 2);
    // The stale version is refused before anything is written.
    let r = svc
        .update(
            &author,
            item.id,
            item.version,
            same("Seo only", Some(set(Some("Other"), None))),
        )
        .await;
    assert!(matches!(r, Err(CoreError::Conflict { .. })), "{r:?}");
    assert_eq!(
        svc.seo_of(&author, item.id).await.unwrap().title.as_deref(),
        Some("Search title")
    );
}

fn bad_bodies() -> [Value; 4] {
    [
        json!({"type": "paragraph"}),
        json!([]),
        json!("x"),
        json!({"content": []}),
    ]
}

/// Create, update, autosave and revision restore refuse a body that is not an object
/// with `"type": "doc"` (`body`); a stored body is never rewritten.
pub async fn body_root_must_be_doc(db: &TestDb) {
    let svc = service(db);
    let author = actor(db.store(), Role::Author).await;
    for body in bad_bodies() {
        let r = svc
            .create(
                &author,
                ContentDraft {
                    body: body.clone(),
                    ..draft("Bad body")
                },
            )
            .await;
        assert_eq!(field(r), "body", "{body}");
    }
    let item = svc.create(&author, draft("Good body")).await.unwrap();
    for body in bad_bodies() {
        let r = svc
            .update(
                &author,
                item.id,
                item.version,
                ContentDraft {
                    body: body.clone(),
                    ..same("Good body", None)
                },
            )
            .await;
        assert_eq!(field(r), "body", "{body}");
        let r = svc
            .put_autosave(
                &author,
                item.id,
                AutosaveDraft {
                    base_version: item.version,
                    title: "Working".into(),
                    excerpt: None,
                    slug: None,
                    parent_id: None,
                    menu_order: None,
                    terms: None,
                    body: body.clone(),
                    fields: json!({}),
                    seo: None,
                },
            )
            .await;
        assert_eq!(field(r), "body", "{body}");
    }
    assert_eq!(svc.get_autosave(&author, item.id).await.unwrap(), None);

    // A revision whose body is not a document (written before the check) cannot be
    // restored; the item keeps its body.
    let rid = RevisionId::new();
    db.store()
        .read()
        .revisions()
        .insert(&Revision {
            id: rid,
            item_id: item.id,
            author_id: author.user,
            created_at: Timestamp::now(),
            title: "Old".into(),
            excerpt: String::new(),
            body: json!({"type": "paragraph"}),
            fields: json!({}),
            meta: json!({}),
            note: None,
        })
        .await
        .unwrap();
    let r = svc
        .restore_revision(&author, item.id, rid, item.version)
        .await;
    assert_eq!(field(r), "body");
    let now = svc.get(item.id).await.unwrap();
    assert_eq!((now.version, &now.body), (item.version, &item.body));
}

/// A PATCH that sends no body (here title and SEO only) of an item whose stored body is
/// not a document succeeds and leaves that body as stored; only a sent body is checked.
pub async fn patch_without_body_keeps_legacy_body(db: &TestDb) {
    let svc = service(db);
    let author = actor(db.store(), Role::Author).await;
    let item = svc.create(&author, draft("Legacy")).await.unwrap();
    // Written before the check existed (straight through the store).
    let legacy = json!({"type": "paragraph", "content": [{"type": "text", "text": "old"}]});
    let mut row = item.clone();
    row.body = legacy.clone();
    let item = db.store().read().content().update(&row).await.unwrap();
    assert_eq!(item.body, legacy);

    // What the API's PATCH builds without `body`: the stored body carried over.
    let saved = svc
        .update(
            &author,
            item.id,
            item.version,
            ContentDraft {
                body: item.body.clone(),
                ..same("Legacy renamed", Some(set(Some("Search title"), None)))
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.version, item.version + 1);
    assert_eq!(saved.title, "Legacy renamed");
    assert_eq!(svc.get(item.id).await.unwrap().body, legacy);
    assert_eq!(
        svc.seo_of(&author, item.id).await.unwrap().title.as_deref(),
        Some("Search title")
    );
    // A body that is sent is still checked.
    let r = svc
        .update(
            &author,
            item.id,
            saved.version,
            ContentDraft {
                body: json!({"type": "heading"}),
                ..same("Legacy renamed", None)
            },
        )
        .await;
    assert_eq!(field(r), "body");
}

backend_tests!(
    patch_without_body_keeps_legacy_body,
    seo_roundtrip_and_delete,
    seo_limits_and_controls,
    seo_only_patch_bumps_version,
    body_root_must_be_doc,
);
