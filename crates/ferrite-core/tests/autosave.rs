//! The per-user autosave slot in `ContentService` on both backends (M2b P15, F11, F12;
//! Review Focus 3).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, AutosaveDraft, ContentDraft, ContentService, CoreError, PermalinkRules, Scopes,
};
use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, NewUser, Role, SearchQuery, TermId, Timestamp, UserId,
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

fn slot(base_version: i64, title: &str) -> AutosaveDraft {
    AutosaveDraft {
        base_version,
        title: title.into(),
        excerpt: None,
        slug: None,
        parent_id: None,
        menu_order: None,
        terms: None,
        body: json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": title}
        ]}]}),
        fields: json!({}),
        seo: None,
    }
}

async fn create(svc: &ContentService, owner: &Actor, title: &str) -> ContentItem {
    svc.create(owner, draft(title)).await.unwrap()
}

/// Sets `status` through the store (published one hour ago).
async fn force(store: &dyn Store, item: ContentItem, status: ContentStatus) -> ContentItem {
    let mut next = item;
    next.status = status;
    if status == ContentStatus::Published {
        next.published_at =
            Some(Timestamp::from_micros(Timestamp::now().as_micros() - 3_600_000_000).unwrap());
    }
    store.read().content().update(&next).await.unwrap()
}

fn forbidden<T: std::fmt::Debug>(r: Result<T, CoreError>) -> bool {
    matches!(r, Err(CoreError::Forbidden(_)))
}

/// Every member round-trips; `put` replaces; `get` is `None` when there is none.
pub async fn autosave_roundtrip(db: &TestDb) {
    let svc = service(db);
    let author = actor(db.store(), Role::Author).await;
    let item = create(&svc, &author, "Roundtrip").await;
    assert_eq!(svc.get_autosave(&author, item.id).await.unwrap(), None);
    let full = AutosaveDraft {
        excerpt: Some("line one\nline two\t!".into()),
        slug: Some("new-slug".into()),
        parent_id: Some(ContentId::new()),
        menu_order: Some(7),
        terms: Some(vec![TermId::new()]),
        // Not checked against any schema: a draft may be incomplete.
        fields: json!({"anything": [1, 2]}),
        ..slot(item.version, "Working title")
    };
    let saved = svc
        .put_autosave(&author, item.id, full.clone())
        .await
        .unwrap();
    assert_eq!(
        (saved.item_id, saved.user_id, saved.base_version),
        (item.id, author.user, item.version)
    );
    let got = svc.get_autosave(&author, item.id).await.unwrap().unwrap();
    assert_eq!(got, saved);
    assert_eq!(
        (
            got.title.as_str(),
            got.excerpt.as_deref(),
            got.slug.as_deref()
        ),
        ("Working title", full.excerpt.as_deref(), Some("new-slug"))
    );
    assert_eq!(
        (got.parent_id, got.menu_order, got.terms, got.fields),
        (full.parent_id, Some(7), full.terms, full.fields)
    );
    // Last write wins.
    svc.put_autosave(&author, item.id, slot(item.version, "Second"))
        .await
        .unwrap();
    let got = svc.get_autosave(&author, item.id).await.unwrap().unwrap();
    assert_eq!((got.title.as_str(), got.slug), ("Second", None));
    assert!(svc.delete_autosave(&author, item.id).await.unwrap());
    assert!(!svc.delete_autosave(&author, item.id).await.unwrap());
    assert_eq!(svc.get_autosave(&author, item.id).await.unwrap(), None);

    // Text as a content write takes it: control characters other than NUL are kept
    // (text pasted from a word processor carries a vertical tab or a form feed).
    let pasted = AutosaveDraft {
        excerpt: Some("page\u{c}break".into()),
        ..slot(item.version, "Vertical\u{b}tab")
    };
    svc.put_autosave(&author, item.id, pasted).await.unwrap();
    let got = svc.get_autosave(&author, item.id).await.unwrap().unwrap();
    assert_eq!(
        (got.title.as_str(), got.excerpt.as_deref()),
        ("Vertical\u{b}tab", Some("page\u{c}break"))
    );
    // The same title is accepted by the real save.
    svc.update(&author, item.id, item.version, draft("Vertical\u{b}tab"))
        .await
        .unwrap();
    // At most 200 terms.
    let terms = |n: usize| AutosaveDraft {
        terms: Some((0..n).map(|_| TermId::new()).collect()),
        ..slot(1, "t")
    };
    svc.put_autosave(&author, item.id, terms(200))
        .await
        .unwrap();
    svc.delete_autosave(&author, item.id).await.unwrap();
    // Refused: NUL anywhere, control characters in the slug, too many terms.
    for (field, bad) in [
        ("title", slot(1, "a\0b")),
        ("terms", terms(201)),
        (
            "excerpt",
            AutosaveDraft {
                excerpt: Some("a\0b".into()),
                ..slot(1, "t")
            },
        ),
        (
            "slug",
            AutosaveDraft {
                slug: Some("a\u{1b}b".into()),
                ..slot(1, "t")
            },
        ),
        (
            "body",
            AutosaveDraft {
                body: json!({"t": "a\0"}),
                ..slot(1, "t")
            },
        ),
        (
            "fields",
            AutosaveDraft {
                fields: json!({"k\0": 1}),
                ..slot(1, "t")
            },
        ),
    ] {
        let err = svc.put_autosave(&author, item.id, bad).await.unwrap_err();
        assert!(
            matches!(&err, CoreError::InvalidField { field: f, .. } if *f == field),
            "{field}: {err:?}"
        );
    }
    assert_eq!(svc.get_autosave(&author, item.id).await.unwrap(), None);
    // A missing item.
    let missing = ContentId::new();
    for r in [
        svc.put_autosave(&author, missing, slot(1, "x")).await.err(),
        svc.delete_autosave(&author, missing).await.err(),
        svc.get_autosave(&author, missing).await.err(),
    ] {
        assert!(matches!(r, Some(CoreError::NotFound)), "{r:?}");
    }
}

/// Edit rights on the item as stored now: an author on another author's draft and a
/// contributor on its own published post are refused (R8); a read-only token of a role
/// that may edit reads its slot but cannot write it (F12).
pub async fn autosave_needs_edit_rights(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let author = actor(store, Role::Author).await;
    let other = actor(store, Role::Author).await;
    let editor = actor(store, Role::Editor).await;
    let contributor = actor(store, Role::Contributor).await;

    let theirs = create(&svc, &other, "Theirs").await;
    assert!(forbidden(
        svc.put_autosave(&author, theirs.id, slot(1, "x")).await
    ));
    assert!(forbidden(svc.delete_autosave(&author, theirs.id).await));
    // Not readable at all: as for the item itself.
    assert!(matches!(
        svc.get_autosave(&author, theirs.id).await,
        Err(CoreError::NotFound)
    ));
    // An editor may.
    svc.put_autosave(&editor, theirs.id, slot(1, "e"))
        .await
        .unwrap();
    assert!(
        svc.get_autosave(&editor, theirs.id)
            .await
            .unwrap()
            .is_some()
    );

    let own = create(&svc, &contributor, "Contributed").await;
    svc.put_autosave(&contributor, own.id, slot(1, "c"))
        .await
        .unwrap();
    let own = force(store, own, ContentStatus::Published).await;
    assert!(forbidden(
        svc.put_autosave(&contributor, own.id, slot(2, "c")).await
    ));
    assert!(forbidden(svc.delete_autosave(&contributor, own.id).await));
    assert!(forbidden(svc.get_autosave(&contributor, own.id).await));

    // An author on another author's published post: readable, not editable.
    let public = force(
        store,
        create(&svc, &other, "Public").await,
        ContentStatus::Published,
    )
    .await;
    assert!(forbidden(
        svc.put_autosave(&author, public.id, slot(2, "x")).await
    ));
    assert!(forbidden(svc.get_autosave(&author, public.id).await));

    // Scopes: `content:read` reads, only `content:write` writes.
    let mine = create(&svc, &author, "Mine").await;
    svc.put_autosave(&author, mine.id, slot(1, "m"))
        .await
        .unwrap();
    let read_only = author.with_scopes(Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    assert!(
        svc.get_autosave(&read_only, mine.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(forbidden(
        svc.put_autosave(&read_only, mine.id, slot(1, "m")).await
    ));
    assert!(forbidden(svc.delete_autosave(&read_only, mine.id).await));
    let no_content = author.with_scopes(Scopes::from_strs(&["users:read".to_owned()]).unwrap());
    assert!(forbidden(svc.get_autosave(&no_content, mine.id).await));
}

/// An autosave writes no revision, event, action job, search entry or invalidation,
/// and leaves the item as it was.
pub async fn autosave_writes_nothing_else(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let author = actor(store, Role::Author).await;
    let item = create(&svc, &author, "Quiet").await;
    let repos = store.read();
    let revisions = repos.revisions().list(item.id, 50).await.unwrap().len();
    let seq = repos.events().max_seq().await.unwrap();
    let jobs = repos.jobs().counts().await.unwrap();
    let search = |q: &str| SearchQuery {
        q: q.into(),
        kinds: Vec::new(),
        statuses: Vec::new(),
        readable_by: None,
        limit: 10,
        cursor: None,
        locale: None,
    };

    svc.put_autosave(&author, item.id, slot(item.version, "Zanzibarquux"))
        .await
        .unwrap();
    svc.put_autosave(&author, item.id, slot(item.version, "Zanzibarquux again"))
        .await
        .unwrap();
    svc.delete_autosave(&author, item.id).await.unwrap();
    svc.put_autosave(&author, item.id, slot(item.version, "Zanzibarquux"))
        .await
        .unwrap();

    assert_eq!(
        repos.revisions().list(item.id, 50).await.unwrap().len(),
        revisions
    );
    assert_eq!(repos.events().max_seq().await.unwrap(), seq);
    assert_eq!(repos.jobs().counts().await.unwrap(), jobs);
    assert!(
        repos
            .search()
            .search(&search("Zanzibarquux"))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(repos.content().get(item.id).await.unwrap().unwrap(), item);
}

/// Review Focus 3: two editors autosave the same item; one saves for real, and only
/// that editor's slot is gone (in the save's transaction). A failed save keeps it.
pub async fn patch_clears_own_autosave_only(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let author = actor(store, Role::Author).await;
    let one = actor(store, Role::Editor).await;
    let two = actor(store, Role::Editor).await;
    let item = create(&svc, &author, "Shared").await;
    for (who, title) in [(&one, "one"), (&two, "two"), (&author, "author")] {
        svc.put_autosave(who, item.id, slot(item.version, title))
            .await
            .unwrap();
    }

    // A stale save fails and clears nothing.
    let stale = svc
        .update(&one, item.id, item.version + 5, draft("x"))
        .await;
    assert!(
        matches!(stale, Err(CoreError::Conflict { .. })),
        "{stale:?}"
    );
    assert!(svc.get_autosave(&one, item.id).await.unwrap().is_some());

    let saved = svc
        .update(&one, item.id, item.version, draft("Saved by one"))
        .await
        .unwrap();
    assert_eq!(svc.get_autosave(&one, item.id).await.unwrap(), None);
    assert_eq!(
        svc.get_autosave(&two, item.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "two"
    );
    assert_eq!(
        svc.get_autosave(&author, item.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "author"
    );
    // A revision restore is a real save too.
    let rev = store.read().revisions().list(item.id, 1).await.unwrap()[0].id;
    svc.restore_revision(&two, item.id, rev, saved.version)
        .await
        .unwrap();
    assert_eq!(svc.get_autosave(&two, item.id).await.unwrap(), None);
    assert!(svc.get_autosave(&author, item.id).await.unwrap().is_some());
}

/// A trashed item takes no autosave ("restore first"); an existing slot can still be
/// read and deleted, and a permanent delete removes every slot (cascade).
pub async fn trashed_item_refuses_autosave(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let author = actor(store, Role::Author).await;
    let editor = actor(store, Role::Editor).await;
    let item = create(&svc, &author, "Doomed").await;
    svc.put_autosave(&author, item.id, slot(1, "a"))
        .await
        .unwrap();
    svc.put_autosave(&editor, item.id, slot(1, "e"))
        .await
        .unwrap();
    let trashed = svc.trash(&author, item.id, item.version).await.unwrap();
    let err = svc
        .put_autosave(&author, item.id, slot(trashed.version, "b"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CoreError::Invalid(m) if m == "restore first"),
        "{err:?}"
    );
    assert_eq!(
        svc.get_autosave(&author, item.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "a"
    );
    assert!(svc.delete_autosave(&author, item.id).await.unwrap());
    svc.delete(&author, item.id, trashed.version).await.unwrap();
    assert_eq!(
        store
            .read()
            .autosaves()
            .get(item.id, editor.user)
            .await
            .unwrap(),
        None
    );
}

backend_tests!(
    autosave_roundtrip,
    autosave_needs_edit_rights,
    autosave_writes_nothing_else,
    patch_clears_own_autosave_only,
    trashed_item_refuses_autosave,
);
