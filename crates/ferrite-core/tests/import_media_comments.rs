//! Imported media, comments, menus and redirects (M3-1 Task 4, P13, P17, P18, P19), on
//! both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentService, CoreError, MediaService, NewUpload, PermalinkRules, Permalinks,
    events::{INVALIDATE, menu_key, post_key, redirect_key},
    import::{ContentOutcome, ImportComment, ImportContent, ImportMedia, ImportService, Written},
    jobs::{MEDIA_VARIANTS_KIND, PLUGIN_ACTION_KIND},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, CommentId, CommentStatus, ContentId, ContentStatus,
    ImportMapKind, ImportRunState, JobQuery, MEDIA_KIND, Menu, MenuItem, MenuItemId, MenuItemKind,
    NewUser, RedirectOrigin, Role, Timestamp, UserId, UserStatus,
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

fn micros(m: i64) -> Timestamp {
    Timestamp::from_micros(m).unwrap()
}

fn body(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

fn post(wp_id: i64, slug: &str, status: ContentStatus, author: UserId) -> ImportContent {
    ImportContent {
        wp_id,
        hash: format!("hash-{wp_id}"),
        kind: "post".into(),
        title: format!("Title {slug}"),
        slug: Some(slug.into()),
        parent: None,
        author,
        status,
        published_at: Some(micros(1_600_000_000_000_000)),
        created_at: micros(1_600_000_000_000_000),
        updated_at: micros(1_600_000_100_000_000),
        excerpt: None,
        body: body(slug),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
        meta: Vec::new(),
    }
}

fn artifact(seed: u8, mime: &str) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ArtifactKind::Media,
        bytes: 1234,
        mime: mime.into(),
        created_at: Timestamp::now(),
    }
}

fn upload(seed: u8, mime: &str, name: &str) -> NewUpload {
    NewUpload {
        artifact: artifact(seed, mime),
        mime: mime.into(),
        bytes: 1234,
        original_name: name.into(),
        title: Some("A photo".into()),
        alt: "alt text".into(),
        sanitized: mime == "image/svg+xml",
        raster: mime != "image/svg+xml",
    }
}

fn media(wp_id: i64, seed: u8, author: UserId) -> ImportMedia {
    ImportMedia {
        wp_id,
        hash: format!("mhash-{wp_id}"),
        upload: upload(seed, "image/png", "photo.png"),
        author,
        created_at: micros(1_500_000_000_000_000),
        parent: None,
    }
}

fn comment(wp_id: i64, item: ContentId, status: CommentStatus) -> ImportComment {
    ImportComment {
        wp_id,
        item,
        parent: None,
        user: None,
        author_name: format!("Name {wp_id}"),
        author_email: format!("Person{wp_id}@Example.test"),
        author_url: String::new(),
        body: format!("Body {wp_id}"),
        status,
        created_at: micros(1_550_000_000_000_000 + wp_id * 1_000_000),
    }
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

/// Writes `items` (posts) in one committed batch of a run that is then finished.
async fn import_posts(
    db: &TestDb,
    actor: &Actor,
    items: Vec<ImportContent>,
) -> Vec<ContentOutcome> {
    let svc = importer(db);
    let run = svc.start(actor, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let mut out = Vec::new();
    for c in items {
        out.push(batch.content(c).await.unwrap());
    }
    batch.commit().await.unwrap();
    run.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();
    out
}

/// Runs `f` in one committed batch of its own run.
async fn in_batch<T>(
    db: &TestDb,
    actor: &Actor,
    f: impl AsyncFnOnce(&mut ferrite_core::import::ImportBatch<'_>) -> T,
) -> T {
    let svc = importer(db);
    let run = svc.start(actor, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let out = f(&mut batch).await;
    batch.commit().await.unwrap();
    run.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();
    out
}

pub async fn media_keeps_dates_author_and_queues_variants(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let author = user(store, Role::Author).await;
    let seq = store.read().events().max_seq().await.unwrap();

    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let mut raster = media(1, 0x11, author);
    raster.hash = "h1".into();
    let png = batch.media(raster.clone()).await.unwrap();
    let mut svg = media(2, 0x22, author);
    svg.upload = upload(0x22, "image/svg+xml", "logo.svg");
    let svg = batch.media(svg).await.unwrap();
    assert_eq!(batch.len(), 2);
    batch.commit().await.unwrap();
    run.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();

    assert_eq!(
        (png.result, svg.result),
        (Written::Created, Written::Created)
    );
    let item = store.read().content().get(png.id).await.unwrap().unwrap();
    assert_eq!(item.kind, MEDIA_KIND);
    assert_eq!(item.status, ContentStatus::Published);
    assert_eq!(item.author_id, author);
    assert_eq!(item.created_at, micros(1_500_000_000_000_000));
    assert_eq!(item.published_at, Some(micros(1_500_000_000_000_000)));
    assert_eq!(item.updated_at, micros(1_500_000_000_000_000));
    assert_eq!(item.title, "A photo");
    let row = store.read().media().get(png.id).await.unwrap().unwrap();
    assert_eq!(row.mime, "image/png");
    assert_eq!(row.alt, "alt text");
    assert_eq!(row.original_name, "photo.png");
    assert!(
        store
            .read()
            .artifacts()
            .get(&raster.upload.artifact.hash)
            .await
            .unwrap()
            .is_some()
    );
    // The same revision and search entry a service upload has.
    assert_eq!(
        store
            .read()
            .revisions()
            .list(png.id, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    // One variants job, for the raster image only; no plugin actions.
    let variants = jobs_of(store, MEDIA_VARIANTS_KIND).await;
    assert_eq!(variants.len(), 1);
    assert!(
        variants[0]
            .payload
            .to_string()
            .contains(&png.id.to_string())
    );
    assert!(jobs_of(store, PLUGIN_ACTION_KIND).await.is_empty());
    // One invalidate event for the batch, with both items.
    let events = invalidations_since(store, seq).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].contains(&post_key(png.id)) && events[0].contains(&post_key(svg.id)));
    // Mapped as media, the item is a media item.
    let mapped = store
        .read()
        .imports()
        .map_get(SOURCE, ImportMapKind::Media, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapped.ferrite_id, png.id.as_uuid());
    assert_eq!(mapped.hash, "h1");
    // The service reads it as a media item.
    let service = MediaService::new(
        db.shared_store(),
        Arc::new(ContentService::new(db.shared_store(), permalinks())),
        permalinks(),
    );
    assert_eq!(
        service.get(&admin, png.id).await.unwrap().media.alt,
        "alt text"
    );

    // Re-run: same hash is unchanged (no second job); a new hash updates title, dates
    // and alt; a local edit keeps the item.
    let again = in_batch(db, &admin, async |b| b.media(raster.clone()).await.unwrap()).await;
    assert_eq!((again.id, again.result), (png.id, Written::Unchanged));
    let mut changed = raster.clone();
    changed.hash = "h2".into();
    changed.upload.title = Some("New title".into());
    changed.upload.alt = "new alt".into();
    changed.created_at = micros(1_400_000_000_000_000);
    let updated = in_batch(db, &admin, async |b| {
        b.media(changed.clone()).await.unwrap()
    })
    .await;
    assert_eq!((updated.id, updated.result), (png.id, Written::Updated));
    let item = store.read().content().get(png.id).await.unwrap().unwrap();
    assert_eq!(item.title, "New title");
    assert_eq!(item.published_at, Some(micros(1_400_000_000_000_000)));
    assert_eq!(
        store.read().media().get(png.id).await.unwrap().unwrap().alt,
        "new alt"
    );
    assert_eq!(jobs_of(store, MEDIA_VARIANTS_KIND).await.len(), 1);
    service
        .update(
            &admin,
            png.id,
            item.version,
            ferrite_core::MediaPatch {
                title: Some("Edited here".into()),
                alt: None,
            },
        )
        .await
        .unwrap();
    changed.hash = "h3".into();
    let kept = in_batch(db, &admin, async |b| {
        b.media(changed.clone()).await.unwrap()
    })
    .await;
    assert_eq!(kept.result, Written::KeptEdited);
    assert_eq!(
        store
            .read()
            .content()
            .get(png.id)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Edited here"
    );
}

pub async fn media_rejects_what_an_upload_rejects(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut bad = media(1, 0x33, admin.user);
    bad.upload.sanitized = true; // a PNG cannot be marked sanitised
    let mut batch = run.batch(&svc).await.unwrap();
    let err = batch.media(bad).await.unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
    assert!(batch.is_empty());
    batch.rollback().await.unwrap();
    let mut long = media(2, 0x34, admin.user);
    long.upload.alt = "x".repeat(1001);
    let mut batch = run.batch(&svc).await.unwrap();
    let err = batch.media(long).await.unwrap_err();
    assert!(
        matches!(err, CoreError::InvalidField { field: "alt", .. }),
        "{err:?}"
    );
    batch.rollback().await.unwrap();
}

pub async fn comment_statuses_dates_threading_and_by_account(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let commenter = user(store, Role::Contributor).await;
    let out = import_posts(
        db,
        &admin,
        vec![post(1, "hello", ContentStatus::Published, admin.user)],
    )
    .await;
    let item = out[0].id;
    let seq = store.read().events().max_seq().await.unwrap();

    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let mut batch = run.batch(&svc).await.unwrap();
    let root = batch
        .comment(comment(1, item, CommentStatus::Approved))
        .await
        .unwrap()
        .id;
    let mut reply = comment(2, item, CommentStatus::Approved);
    reply.parent = Some(root);
    reply.user = Some(commenter);
    reply.author_name = "Account".into();
    reply.author_email = "Account@Example.test".into();
    let reply = batch.comment(reply).await.unwrap().id;
    let pending = batch
        .comment(comment(3, item, CommentStatus::Pending))
        .await
        .unwrap()
        .id;
    let spam = batch
        .comment(comment(4, item, CommentStatus::Spam))
        .await
        .unwrap()
        .id;
    let trash = batch
        .comment(comment(5, item, CommentStatus::Trash))
        .await
        .unwrap()
        .id;
    let anon = batch
        .comment(comment(6, item, CommentStatus::Approved))
        .await
        .unwrap()
        .id;
    assert_eq!(batch.len(), 6);
    batch.commit().await.unwrap();
    run.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();

    let get = async |id: CommentId| store.read().comments().get(id).await.unwrap().unwrap();
    let r = get(root).await;
    assert_eq!(r.status, CommentStatus::Approved);
    assert_eq!(r.created_at, micros(1_550_000_001_000_000));
    assert_eq!(r.author_email, "person1@example.test");
    assert_eq!(r.ip_hash, None);
    assert_eq!(r.user_id, None);
    let c = get(reply).await;
    assert_eq!((c.parent_id, c.user_id), (Some(root), Some(commenter)));
    assert_eq!(get(pending).await.status, CommentStatus::Pending);
    assert_eq!(get(spam).await.status, CommentStatus::Spam);
    assert_eq!(get(trash).await.status, CommentStatus::Trash);
    let counts = store.read().comments().counts(Some(item)).await.unwrap();
    assert_eq!(counts.get(&CommentStatus::Approved), Some(&3));
    // Only approved comments touch the page; one event for the batch.
    let events = invalidations_since(store, seq).await;
    assert_eq!(events, vec![vec![post_key(item)]]);

    // by_account (02 section on comments): after the account is deleted the comment is
    // not an anonymous commenter's, while an imported anonymous one is.
    store.read().comments().get(anon).await.unwrap();
    let tx = store.begin().await.unwrap();
    tx.users().delete(commenter).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(get(reply).await.user_id, None);
    let comments = store.read().comments();
    assert!(
        !comments
            .has_approved_from("account@example.test", "Account")
            .await
            .unwrap()
    );
    assert!(
        comments
            .has_approved_from("person6@example.test", "Name 6")
            .await
            .unwrap()
    );

    // Re-run: mapped comments are returned, nothing is duplicated.
    let again = in_batch(db, &admin, async |b| {
        b.comment(comment(1, item, CommentStatus::Spam))
            .await
            .unwrap()
            .id
    })
    .await;
    assert_eq!(again, root);
    assert_eq!(get(root).await.status, CommentStatus::Approved);
    let counts = store.read().comments().counts(Some(item)).await.unwrap();
    assert_eq!(counts.values().sum::<u64>(), 6);
}

pub async fn comment_body_is_cleaned_not_refused(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let item = import_posts(
        db,
        &admin,
        vec![post(1, "p", ContentStatus::Published, admin.user)],
    )
    .await[0]
        .id;
    let max = ferrite_core::comments::MAX_COMMENT_BODY;
    let long = "é".repeat(max + 50);
    let (plain, dirty, cut) = in_batch(db, &admin, async |b| {
        let mut c1 = comment(1, item, CommentStatus::Approved);
        c1.body = "fine\nbody\twith tab".into();
        let mut c2 = comment(2, item, CommentStatus::Approved);
        c2.body = "  a\r\nb\u{0}\u{7}c\u{202e}d\r".into();
        let mut c3 = comment(3, item, CommentStatus::Approved);
        c3.body = long.clone();
        (
            b.comment(c1).await.unwrap(),
            b.comment(c2).await.unwrap(),
            b.comment(c3).await.unwrap(),
        )
    })
    .await;
    assert!(!plain.altered && dirty.altered && cut.altered);
    let body = async |id: CommentId| store.read().comments().get(id).await.unwrap().unwrap().body;
    assert_eq!(body(plain.id).await, "fine\nbody\twith tab");
    // \r\n -> \n, trimmed, controls (NUL, BEL) removed; the Cf mark stays.
    assert_eq!(body(dirty.id).await, "a\nbc\u{202e}d");
    assert_eq!(body(cut.id).await.chars().count(), max);
    assert!(body(cut.id).await.chars().all(|c| c == 'é'));
}

pub async fn comment_on_draft_item_allowed(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let out = import_posts(
        db,
        &admin,
        vec![post(1, "wip", ContentStatus::Draft, admin.user)],
    )
    .await;
    let item = out[0].id;
    let id = in_batch(db, &admin, async |b| {
        b.comment(comment(1, item, CommentStatus::Approved))
            .await
            .unwrap()
            .id
    })
    .await;
    assert_eq!(
        store
            .read()
            .comments()
            .get(id)
            .await
            .unwrap()
            .unwrap()
            .item_id,
        item
    );

    // Checks: unknown item, a parent of another item, an unknown user.
    let other = import_posts(
        db,
        &admin,
        vec![post(2, "other", ContentStatus::Draft, admin.user)],
    )
    .await[0]
        .id;
    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let cases: Vec<(&str, ImportComment)> = vec![
        (
            "item",
            comment(10, ContentId::new(), CommentStatus::Approved),
        ),
        (
            "parent",
            ImportComment {
                parent: Some(id),
                ..comment(11, other, CommentStatus::Approved)
            },
        ),
        (
            "user",
            ImportComment {
                user: Some(UserId::new()),
                ..comment(12, item, CommentStatus::Approved)
            },
        ),
    ];
    for (field, c) in cases {
        let mut batch = run.batch(&svc).await.unwrap();
        let err = batch.comment(c).await.unwrap_err();
        assert!(
            matches!(&err, CoreError::InvalidField { field: f, .. } if *f == field),
            "{field}: {err:?}"
        );
        batch.rollback().await.unwrap();
    }
}

fn menu_item(
    label: &str,
    kind: MenuItemKind,
    target: Option<uuid::Uuid>,
    url: Option<&str>,
) -> MenuItem {
    MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position: 0,
        kind,
        target_id: target,
        url: url.map(str::to_owned),
        label: label.into(),
    }
}

pub async fn menu_put_in_batch(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let out = import_posts(
        db,
        &admin,
        vec![post(1, "about", ContentStatus::Published, admin.user)],
    )
    .await;
    let target = out[0].id;
    let mut child = menu_item("Child", MenuItemKind::Url, None, Some("/x"));
    let parent = menu_item("About", MenuItemKind::Item, Some(target.as_uuid()), None);
    child.parent_id = Some(parent.id);
    let menu = Menu {
        name: "main".into(),
        label: "Main".into(),
        items: vec![parent, child],
    };
    let seq = store.read().events().max_seq().await.unwrap();
    in_batch(db, &admin, async |b| {
        b.menu(&menu).await.unwrap();
        assert_eq!(b.len(), 1);
    })
    .await;
    assert_eq!(
        store.read().menus().get("main").await.unwrap().unwrap(),
        menu
    );
    assert_eq!(
        invalidations_since(store, seq).await,
        vec![vec![menu_key("main")]]
    );

    // Replaces the tree.
    let replaced = Menu {
        items: vec![menu_item(
            "Home",
            MenuItemKind::Url,
            None,
            Some("https://example.test/"),
        )],
        ..menu.clone()
    };
    in_batch(db, &admin, async |b| b.menu(&replaced).await.unwrap()).await;
    assert_eq!(
        store
            .read()
            .menus()
            .get("main")
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1
    );

    // The rules of MenuService::put: a missing target, a long label, a bad name.
    let svc = importer(db);
    let run = svc.start(&admin, SOURCE, false, false).await.unwrap();
    let bad = [
        Menu {
            items: vec![menu_item(
                "Gone",
                MenuItemKind::Item,
                Some(ContentId::new().as_uuid()),
                None,
            )],
            ..menu.clone()
        },
        Menu {
            items: vec![menu_item(
                &"x".repeat(201),
                MenuItemKind::Url,
                None,
                Some("/x"),
            )],
            ..menu.clone()
        },
        Menu {
            name: "Not Valid".into(),
            ..menu.clone()
        },
    ];
    for m in &bad {
        let mut batch = run.batch(&svc).await.unwrap();
        assert!(batch.menu(m).await.is_err());
        batch.rollback().await.unwrap();
    }
    assert_eq!(
        store
            .read()
            .menus()
            .get("main")
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1
    );
}

fn row(source: &str, target: &str) -> (String, String, u16) {
    (source.into(), target.into(), 301)
}

pub async fn redirects_partial_refusal_reports_and_writes_rest(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let seq = store.read().events().max_seq().await.unwrap();
    let rows = vec![
        row("/old-a/", "/new-a/"),
        row("/api/x", "/y/"),
        ("/old-b/".into(), "/new-b/".into(), 999),
        row("/old-a/", "/other/"), // the source again
        row("/loop/", "/loop/"),
        row("/old-c", "/new-c"),
    ];
    let out = in_batch(db, &admin, async |b| {
        let out = b.redirects(&rows).await.unwrap();
        assert_eq!(b.len(), 2);
        out
    })
    .await;
    assert_eq!((out.created, out.updated), (2, 0));
    let refused: Vec<&str> = out.refused.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(refused, ["/api/x", "/old-b/", "/old-a/", "/loop/"]);
    assert!(
        out.refused[0].1.contains("path the site serves itself"),
        "{:?}",
        out.refused
    );
    assert!(out.refused[1].1.contains("301"), "{:?}", out.refused);
    assert!(
        out.refused[2].1.contains("duplicate source"),
        "{:?}",
        out.refused
    );
    for (source, target) in [("/old-a/", "/new-a/"), ("/old-c", "/new-c")] {
        let r = store
            .read()
            .redirects()
            .get_by_source(source)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (r.target.as_str(), r.code, r.origin),
            (target, 301, RedirectOrigin::Import)
        );
    }
    assert!(
        store
            .read()
            .redirects()
            .get_by_source("/api/x")
            .await
            .unwrap()
            .is_none()
    );
    let events = invalidations_since(store, seq).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].contains(&redirect_key("/old-a/")) && events[0].contains(&redirect_key("/old-c"))
    );
    assert!(!events[0].contains(&redirect_key("/api/x")));

    // A second run: the existing source is updated; a row closing a loop with a stored
    // redirect is refused (the stored /old-a/ -> /newer-a/ now).
    let out = in_batch(db, &admin, async |b| {
        b.redirects(&[row("/old-a/", "/newer-a/"), row("/old-d/", "/z/")])
            .await
            .unwrap()
    })
    .await;
    assert_eq!((out.created, out.updated, out.refused.len()), (1, 1, 0));
    // The same rows again: nothing written.
    let out = in_batch(db, &admin, async |b| {
        b.redirects(&[row("/old-a/", "/newer-a/"), row("/old-d/", "/z/")])
            .await
            .unwrap()
    })
    .await;
    assert_eq!((out.created, out.updated, out.unchanged), (0, 0, 2));
    let out = in_batch(db, &admin, async |b| {
        b.redirects(&[row("/newer-a/", "/old-a/")]).await.unwrap()
    })
    .await;
    assert_eq!((out.created, out.updated), (0, 0));
    assert_eq!(out.refused.len(), 1);
    assert_eq!(out.refused[0].0, "/newer-a/");
}

pub async fn redirects_refused_come_in_input_order(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    import_posts(
        db,
        &admin,
        vec![post(1, "live-post", ContentStatus::Published, admin.user)],
    )
    .await;
    let live = {
        let id = store
            .read()
            .content()
            .get_by_slug("post", None, "live-post")
            .await
            .unwrap()
            .unwrap();
        permalinks().path(&ferrite_core::PermalinkInput {
            item: id,
            ancestors: Vec::new(),
            primary_term: None,
        })
    };
    let res = in_batch(db, &admin, async |b| {
        b.redirects(&[
            row("/api/first", "/a/"),
            row(&live, "/b/"),
            row("/ok/", "/c/"),
            ("/bad-code/".into(), "/d/".into(), 999),
            row(&live, "/e/"),
            row("/ok/", "/f/"),
        ])
        .await
        .unwrap()
    })
    .await;
    let order: Vec<&str> = res.refused.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        order,
        [
            "/api/first",
            live.as_str(),
            "/bad-code/",
            live.as_str(),
            "/ok/"
        ]
    );
    assert_eq!(res.refused[1].1, "path is a published item");
    assert_eq!(res.created, 1);
}

pub async fn redirect_from_published_item_path_refused_or_reclaimed(db: &TestDb) {
    let store = db.store();
    let admin = admin(store).await;
    let out = import_posts(
        db,
        &admin,
        vec![
            post(1, "live-post", ContentStatus::Published, admin.user),
            post(2, "a-draft", ContentStatus::Draft, admin.user),
        ],
    )
    .await;
    // The path of the published item, however the rules spell it.
    let item = store
        .read()
        .content()
        .get(out[0].id)
        .await
        .unwrap()
        .unwrap();
    let path = permalinks().path(&ferrite_core::PermalinkInput {
        item,
        ancestors: Vec::new(),
        primary_term: None,
    });
    let toggled = match path.strip_suffix('/') {
        Some(p) => p.to_owned(),
        None => format!("{path}/"),
    };
    let res = in_batch(db, &admin, async |b| {
        b.redirects(&[
            row(&path, "/elsewhere/"),
            row(&toggled, "/elsewhere-2/"),
            row("/a-draft/", "/draft-target/"),
            row("/live-post/child/", "/deep/"),
        ])
        .await
        .unwrap()
    })
    .await;
    assert_eq!(
        res.refused,
        vec![
            (path.clone(), "path is a published item".to_owned()),
            (toggled, "path is a published item".to_owned()),
        ]
    );
    // A draft's path and a path under a published item are not live pages.
    assert_eq!(res.created, 2);
    assert!(
        store
            .read()
            .redirects()
            .get_by_source(&path)
            .await
            .unwrap()
            .is_none()
    );
}

backend_tests!(
    media_keeps_dates_author_and_queues_variants,
    media_rejects_what_an_upload_rejects,
    comment_statuses_dates_threading_and_by_account,
    comment_body_is_cleaned_not_refused,
    comment_on_draft_item_allowed,
    menu_put_in_batch,
    redirects_partial_refusal_reports_and_writes_rest,
    redirects_refused_come_in_input_order,
    redirect_from_published_item_path_refused_or_reclaimed,
);
