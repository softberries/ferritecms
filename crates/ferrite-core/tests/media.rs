//! MediaService on both backends: upload recording, variants, updates and reads.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, MediaItem, MediaPatch, MediaQuery,
    MediaService, NewUpload, PermalinkRules, Permalinks, Scopes,
    events::post_key,
    jobs::{MEDIA_VARIANTS_KIND, MediaVariants, media_variants_job},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, Job, JobId, JobLease,
    JobStatus, NewJob, NewUser, NodeId, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, Backend, Channel, CommentRepo, ContentRepo,
    ContentTypeRepo, EventRepo, IdempotencyRepo, JobRepo, LockGuard, MediaRepo, MenuRepo, MetaRepo,
    MigrationStatus, NodeRepo, OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo,
    RateLimitRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo, SearchRepo, SessionRepo,
    Store, StoreError, Subscription, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
use ferrite_store_tests::{TestDb, backend_tests};
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
    Actor::new(id, role)
}

fn scoped(actor: Actor, scopes: &[&str]) -> Actor {
    let names: Vec<String> = scopes.iter().map(|s| (*s).to_owned()).collect();
    actor.with_scopes(Scopes::from_strs(&names).unwrap())
}

fn services(store: Arc<dyn Store>) -> (MediaService, Arc<ContentService>) {
    let rules: Arc<dyn Permalinks> = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    (MediaService::new(store, content.clone(), rules), content)
}

fn hash(seed: u8) -> ArtifactHash {
    ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap()
}

fn artifact(seed: u8, kind: ArtifactKind, mime: &str) -> Artifact {
    Artifact {
        hash: hash(seed),
        kind,
        bytes: 1234,
        mime: mime.into(),
        created_at: Timestamp::now(),
    }
}

fn upload(seed: u8, mime: &str, name: &str, raster: bool) -> NewUpload {
    NewUpload {
        artifact: artifact(seed, ArtifactKind::Media, mime),
        mime: mime.into(),
        bytes: 1234,
        original_name: name.into(),
        title: None,
        alt: "An image".into(),
        sanitized: false,
        raster,
    }
}

fn png(seed: u8) -> NewUpload {
    upload(seed, "image/png", "photo.png", true)
}

async fn max_seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

/// Every key of every event after `seq`.
async fn keys_since(store: &dyn Store, seq: i64) -> Vec<String> {
    store
        .read()
        .events()
        .since(seq, 100)
        .await
        .unwrap()
        .into_iter()
        .flat_map(|e| e.keys)
        .collect()
}

async fn variant_jobs(store: &dyn Store) -> Vec<Job> {
    store
        .read()
        .jobs()
        .claim(
            NodeId::new(),
            &[MEDIA_VARIANTS_KIND.to_owned()],
            50,
            Duration::from_secs(60),
        )
        .await
        .unwrap()
}

async fn media_items(store: &dyn Store) -> usize {
    store
        .read()
        .content()
        .list(&ferrite_domain::ContentQuery {
            kind: Some("media".into()),
            limit: 200,
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len()
}

fn forbidden<T: std::fmt::Debug>(r: Result<T, CoreError>, what: &str) {
    match r {
        Err(CoreError::Forbidden(w)) => assert_eq!(w, what),
        other => panic!("expected Forbidden({what}), got {other:?}"),
    }
}

fn invalid_field<T: std::fmt::Debug>(r: Result<T, CoreError>, want: &str) {
    match r {
        Err(CoreError::InvalidField { field, .. }) => assert_eq!(field, want),
        other => panic!("expected InvalidField({want}), got {other:?}"),
    }
}

async fn contributor_cannot_upload(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let contributor = actor(db.store(), Role::Contributor).await;
    forbidden(media.authorize_upload(&contributor), "upload_media");
    forbidden(
        media.record_upload(&contributor, png(1)).await,
        "upload_media",
    );

    // An author's token without `media:write`.
    let author = actor(db.store(), Role::Author).await;
    let token = scoped(author, &["content:write", "media:read"]);
    forbidden(media.authorize_upload(&token), "media:write");
    forbidden(media.record_upload(&token, png(1)).await, "media:write");
    assert_eq!(media_items(db.store()).await, 0);

    // `media:write` alone is enough: the upload does not need `content:write`.
    let token = scoped(author, &["media:write"]);
    media.authorize_upload(&token).unwrap();
    let got = media.record_upload(&token, png(1)).await.unwrap();
    assert_eq!(got.item.status, ContentStatus::Published);
    assert_eq!(got.item.author_id, author.user);
}

async fn upload_records_item_row_event_and_job_in_one_tx(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let before = max_seq(db.store()).await;

    let got = media.record_upload(&author, png(2)).await.unwrap();
    let id = got.item.id;
    assert_eq!(got.item.kind, "media");
    assert_eq!(got.item.status, ContentStatus::Published);
    assert!(got.item.published_at.is_some());
    assert_eq!(got.item.title, "photo", "title from the file name");
    assert_eq!(got.media.item_id, id);
    assert_eq!(got.media.artifact_hash, hash(2));
    assert_eq!(got.media.mime, "image/png");
    assert_eq!(got.media.bytes, 1234);
    assert_eq!(got.media.alt, "An image");
    assert_eq!(got.media.original_name, "photo.png");
    assert_eq!((got.media.width, got.media.height), (None, None));
    assert_eq!(got.media.variants, json!({}));
    assert!(!got.media.sanitized);
    let row = db.store().read().media().get(id).await.unwrap().unwrap();
    assert_eq!(row, got.media);
    let stored = db.store().read().artifacts().get(&hash(2)).await.unwrap();
    assert_eq!(stored.unwrap().kind, ArtifactKind::Media);
    assert!(keys_since(db.store(), before).await.contains(&post_key(id)));

    let jobs = variant_jobs(db.store()).await;
    assert_eq!(jobs.len(), 1, "one media.variants job for a raster upload");
    let want = media_variants_job(&MediaVariants {
        v: 1,
        item: id,
        artifact: hash(2).to_string(),
    });
    assert_eq!(jobs[0].payload, want.payload);
    assert_eq!(
        jobs[0].unique_key.as_deref(),
        Some(format!("media.variants:{id}:{}", hash(2)).as_str())
    );
    assert_eq!(jobs[0].max_attempts, ferrite_domain::DEFAULT_MAX_ATTEMPTS);

    // Documents (PDF, and the R1 classes) get no job.
    let pdf = media
        .record_upload(
            &author,
            upload(3, "application/pdf", "Report 2026.pdf", false),
        )
        .await
        .unwrap();
    assert_eq!(pdf.item.title, "Report 2026");
    let docx = upload(
        4,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "letter.docx",
        false,
    );
    media.record_upload(&author, docx).await.unwrap();
    assert!(variant_jobs(db.store()).await.is_empty());

    // An explicit title wins; a path in the name keeps only its last segment, and the
    // same file uploaded twice shares its artifact row.
    let mut named = png(2);
    named.title = Some("Holiday".into());
    named.original_name = r"C:\Users\me\..\evil/../holiday.png".into();
    let again = media.record_upload(&author, named).await.unwrap();
    assert_eq!(again.item.title, "Holiday");
    assert_eq!(again.media.original_name, "holiday.png");
    assert_ne!(again.item.id, id);
    assert_ne!(again.item.slug, got.item.slug);

    // Validation runs before anything is written.
    let count = media_items(db.store()).await;
    let seq = max_seq(db.store()).await;
    let mut bad = png(5);
    bad.title = Some("t".repeat(201));
    invalid_field(media.record_upload(&author, bad).await, "title");
    let mut bad = png(5);
    bad.title = Some("line\nbreak".into());
    invalid_field(media.record_upload(&author, bad).await, "title");
    let mut bad = png(5);
    bad.alt = "a".repeat(1001);
    invalid_field(media.record_upload(&author, bad).await, "alt");
    let mut bad = png(5);
    bad.alt = "bell\u{7}".into();
    invalid_field(media.record_upload(&author, bad).await, "alt");
    // Control and format characters (bidi overrides disguise the extension:
    // `report\u{202e}fdp.zip` shows as `reportpiz.pdf`; zero-width chars, the BOM).
    for name in [
        "",
        "dir/",
        "x\u{0}.png",
        "tab\t.png",
        "\u{202e}gnp.exe",
        "report\u{202e}fdp.zip",
        "a\u{200b}.png",
        "\u{feff}a.png",
        "a\u{2067}b.png",
    ] {
        let mut bad = png(5);
        bad.original_name = name.into();
        invalid_field(media.record_upload(&author, bad).await, "file");
    }
    let mut bad = png(5);
    bad.original_name = format!("{}.png", "n".repeat(252));
    invalid_field(media.record_upload(&author, bad).await, "file");
    let mut ok = png(6);
    ok.original_name = format!("{}.png", "n".repeat(251));
    media.record_upload(&author, ok).await.unwrap();
    // Backstops: only an SVG carries the sanitised marker and an SVG must carry it;
    // variants only for raster types; type and size must match the artifact.
    let invalid = |r: Result<MediaItem, CoreError>| {
        assert!(matches!(r, Err(CoreError::Invalid(_))), "{r:?}");
    };
    let mut bad = png(5);
    bad.sanitized = true;
    invalid(media.record_upload(&author, bad).await);
    invalid(
        media
            .record_upload(&author, upload(5, "image/svg+xml", "raw.svg", false))
            .await,
    );
    invalid(
        media
            .record_upload(&author, upload(5, "application/pdf", "x.pdf", true))
            .await,
    );
    let mut bad = png(5);
    bad.mime = "image/jpeg".into();
    invalid(media.record_upload(&author, bad).await);
    let mut bad = png(5);
    bad.bytes = 1235;
    invalid(media.record_upload(&author, bad).await);
    let mut bad = png(5);
    bad.artifact.kind = ArtifactKind::Variant;
    invalid(media.record_upload(&author, bad).await);
    let mut svg = upload(7, "image/svg+xml", "logo.svg", false);
    svg.sanitized = true;
    assert!(
        media
            .record_upload(&author, svg)
            .await
            .unwrap()
            .media
            .sanitized
    );
    assert_eq!(media_items(db.store()).await, count + 2);
    assert!(max_seq(db.store()).await > seq);
    // Claimed here, so the check below sees only new jobs.
    assert_eq!(
        variant_jobs(db.store()).await.len(),
        2,
        "the two PNGs since"
    );

    // A failing write inside the Tx (the variants job, the last one) leaves nothing.
    let failing: Arc<dyn Store> = Arc::new(FailingStore {
        inner: db.shared_store(),
    });
    let (broken, _) = services(failing);
    let count = media_items(db.store()).await;
    let seq = max_seq(db.store()).await;
    let r = broken.record_upload(&author, png(8)).await;
    assert!(matches!(r, Err(CoreError::Store(_))), "{r:?}");
    assert_eq!(media_items(db.store()).await, count, "no item");
    assert_eq!(max_seq(db.store()).await, seq, "no event");
    assert!(
        db.store()
            .read()
            .artifacts()
            .get(&hash(8))
            .await
            .unwrap()
            .is_none(),
        "no artifact row"
    );
    assert!(variant_jobs(db.store()).await.is_empty(), "no job");
}

async fn variants_recorded_conditionally(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let got = media.record_upload(&author, png(10)).await.unwrap();
    let id = got.item.id;
    let variants = json!({"thumbnail": {"hash": hash(11).to_string(), "w": 150, "h": 150, "mime": "image/png"}});
    let thumb = artifact(11, ArtifactKind::Variant, "image/png");

    let before = max_seq(db.store()).await;
    assert!(
        media
            .record_variants(id, &hash(10), 2000, 1200, variants.clone(), &[thumb])
            .await
            .unwrap()
    );
    let row = db.store().read().media().get(id).await.unwrap().unwrap();
    assert_eq!((row.width, row.height), (Some(2000), Some(1200)));
    assert_eq!(row.variants, variants);
    assert_eq!(row.alt, "An image", "the rest of the row is kept");
    let stored = db.store().read().artifacts().get(&hash(11)).await.unwrap();
    assert_eq!(stored.unwrap().kind, ArtifactKind::Variant);
    assert_eq!(keys_since(db.store(), before).await, vec![post_key(id)]);

    // The row no longer holds that artifact (the file was replaced): nothing written.
    let before = max_seq(db.store()).await;
    let other = artifact(12, ArtifactKind::Variant, "image/png");
    assert!(
        !media
            .record_variants(id, &hash(13), 10, 10, json!({}), &[other])
            .await
            .unwrap()
    );
    let row = db.store().read().media().get(id).await.unwrap().unwrap();
    assert_eq!((row.width, row.height), (Some(2000), Some(1200)));
    assert_eq!(row.variants, variants);
    assert!(
        db.store()
            .read()
            .artifacts()
            .get(&hash(12))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(max_seq(db.store()).await, before);

    // A missing row: nothing written.
    assert!(
        !media
            .record_variants(ContentId::new(), &hash(10), 10, 10, json!({}), &[])
            .await
            .unwrap()
    );
    assert_eq!(max_seq(db.store()).await, before);

    // Variants must be an object; dimensions must fit the row.
    assert!(matches!(
        media
            .record_variants(id, &hash(10), 10, 10, json!([]), &[])
            .await,
        Err(CoreError::Invalid(_))
    ));
    assert!(matches!(
        media
            .record_variants(id, &hash(10), u32::MAX, 10, json!({}), &[])
            .await,
        Err(CoreError::Invalid(_))
    ));
}

async fn media_update_needs_version(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let got = media.record_upload(&author, png(20)).await.unwrap();
    let id = got.item.id;
    let version = got.item.version;
    let patch = MediaPatch {
        title: Some("Sunset".into()),
        alt: Some("A red sky".into()),
    };

    let r = media.update(&author, id, version - 1, patch.clone()).await;
    assert!(
        matches!(r, Err(CoreError::Conflict { current_version }) if current_version == version),
        "{r:?}"
    );
    assert_eq!(
        db.store()
            .read()
            .media()
            .get(id)
            .await
            .unwrap()
            .unwrap()
            .alt,
        "An image",
        "a stale update writes nothing"
    );

    let before = max_seq(db.store()).await;
    let updated = media.update(&author, id, version, patch).await.unwrap();
    assert_eq!(updated.item.title, "Sunset");
    assert_eq!(updated.item.status, ContentStatus::Published);
    assert!(updated.item.version > version);
    assert_eq!(updated.media.alt, "A red sky");
    let row = db.store().read().media().get(id).await.unwrap().unwrap();
    assert_eq!(row.alt, "A red sky");
    assert!(keys_since(db.store(), before).await.contains(&post_key(id)));

    // Alt only: the title stays, the version still moves.
    let alt_only = MediaPatch {
        title: None,
        alt: Some(String::new()),
    };
    let again = media
        .update(&author, id, updated.item.version, alt_only)
        .await
        .unwrap();
    assert_eq!(again.item.title, "Sunset");
    assert_eq!(again.media.alt, "");
    assert!(again.item.version > updated.item.version);

    // Field limits, then rights: another author may not edit; an editor may; a token
    // needs `media:write`.
    let long = MediaPatch {
        title: None,
        alt: Some("a".repeat(1001)),
    };
    invalid_field(
        media.update(&author, id, again.item.version, long).await,
        "alt",
    );
    let other = actor(db.store(), Role::Author).await;
    let p = MediaPatch {
        title: Some("Mine".into()),
        alt: None,
    };
    forbidden(
        media
            .update(&other, id, again.item.version, p.clone())
            .await,
        "edit_others",
    );
    forbidden(
        media
            .update(
                &scoped(author, &["content:write"]),
                id,
                again.item.version,
                p.clone(),
            )
            .await,
        "media:write",
    );
    let editor = actor(db.store(), Role::Editor).await;
    let edited = media
        .update(&scoped(editor, &["media:write"]), id, again.item.version, p)
        .await
        .unwrap();
    assert_eq!(edited.item.title, "Mine");
}

async fn media_reads_respect_can_read(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let a = actor(db.store(), Role::Author).await;
    let b = actor(db.store(), Role::Author).await;
    let editor = actor(db.store(), Role::Editor).await;
    let kept = media.record_upload(&a, png(30)).await.unwrap();
    let gone = media.record_upload(&a, png(31)).await.unwrap();
    let trashed = media
        .trash(&a, gone.item.id, gone.item.version)
        .await
        .unwrap();
    assert_eq!(trashed.status, ContentStatus::Trash);
    let own_b = media.record_upload(&b, png(32)).await.unwrap();

    // B reads A's published media, not A's trashed media.
    let got: MediaItem = media.get(&b, kept.item.id).await.unwrap();
    assert_eq!(got.media.artifact_hash, hash(30));
    assert!(matches!(
        media.get(&b, gone.item.id).await,
        Err(CoreError::NotFound)
    ));
    // The author and an editor do.
    assert_eq!(
        media.get(&a, gone.item.id).await.unwrap().item.status,
        ContentStatus::Trash
    );
    assert_eq!(
        media.get(&editor, gone.item.id).await.unwrap().item.id,
        gone.item.id
    );
    // A token needs `media:read`.
    forbidden(
        media
            .get(&scoped(editor, &["content:read"]), kept.item.id)
            .await,
        "media:read",
    );
    forbidden(
        media
            .list(&scoped(editor, &["content:read"]), query(50))
            .await,
        "media:read",
    );

    // Lists: every non-trash status, newest update first, joined with the rows.
    let ids = |page: Vec<MediaItem>| page.into_iter().map(|m| m.item.id).collect::<Vec<_>>();
    let listed = media.list(&b, query(50)).await.unwrap();
    assert_eq!(listed.next_cursor, None);
    assert_eq!(ids(listed.items), vec![own_b.item.id, kept.item.id]);
    let by_a = media
        .list(
            &editor,
            MediaQuery {
                author: Some(a.user),
                ..query(50)
            },
        )
        .await
        .unwrap();
    assert_eq!(ids(by_a.items), vec![kept.item.id]);
    let first = media.list(&editor, query(1)).await.unwrap();
    assert_eq!(ids(first.items), vec![own_b.item.id]);
    let next = media
        .list(
            &editor,
            MediaQuery {
                cursor: first.next_cursor,
                ..query(1)
            },
        )
        .await
        .unwrap();
    assert_eq!(ids(next.items), vec![kept.item.id]);

    // An unpublished media item of A's (unpublished through the content service) is
    // listed for A and the editor, not for B.
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    content
        .unpublish(&a, kept.item.id, kept.item.version)
        .await
        .unwrap();
    assert_eq!(
        ids(media.list(&b, query(50)).await.unwrap().items),
        vec![own_b.item.id]
    );
    assert_eq!(media.list(&a, query(50)).await.unwrap().items.len(), 2);
    assert_eq!(media.list(&editor, query(50)).await.unwrap().items.len(), 2);

    // Delete: only from trash; the media row goes with the item.
    let r = media.delete(&a, own_b.item.id, own_b.item.version).await;
    forbidden(r, "edit_others");
    media
        .delete(&a, gone.item.id, trashed.version)
        .await
        .unwrap();
    assert!(
        db.store()
            .read()
            .media()
            .get(gone.item.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        media.get(&a, gone.item.id).await,
        Err(CoreError::NotFound)
    ));
}

fn query(limit: u32) -> MediaQuery {
    MediaQuery {
        limit,
        cursor: None,
        author: None,
    }
}

async fn non_media_item_is_not_found(db: &TestDb) {
    let (media, content) = services(db.shared_store());
    let editor = actor(db.store(), Role::Editor).await;
    let post = content
        .create(
            &editor,
            ContentDraft {
                kind: "post".into(),
                title: "A post".into(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
            },
        )
        .await
        .unwrap();
    let id = post.id;
    let not_found =
        |r: Result<(), CoreError>| assert!(matches!(r, Err(CoreError::NotFound)), "{r:?}");
    not_found(media.get(&editor, id).await.map(drop));
    not_found(
        media
            .update(
                &editor,
                id,
                post.version,
                MediaPatch {
                    title: Some("x".into()),
                    alt: None,
                },
            )
            .await
            .map(drop),
    );
    not_found(media.trash(&editor, id, post.version).await.map(drop));
    not_found(media.delete(&editor, id, post.version).await);
    not_found(media.get(&editor, ContentId::new()).await.map(drop));
    assert!(
        !media
            .record_variants(id, &hash(40), 1, 1, json!({}), &[])
            .await
            .unwrap()
    );
    // The post is untouched.
    assert_eq!(content.get(id).await.unwrap().version, post.version);

    // A media item whose row is missing (written before the service existed) is
    // `NotFound` too.
    let bare = content
        .create(
            &editor,
            ContentDraft {
                kind: "media".into(),
                title: "Bare".into(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
            },
        )
        .await
        .unwrap();
    not_found(media.get(&editor, bare.id).await.map(drop));
    assert!(
        media
            .list(&editor, query(50))
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

backend_tests!(
    contributor_cannot_upload,
    upload_records_item_row_event_and_job_in_one_tx,
    variants_recorded_conditionally,
    media_update_needs_version,
    media_reads_respect_can_read,
    non_media_item_is_not_found,
);

/// A store whose transactions refuse to enqueue `media.variants` jobs (F11): the last
/// write of `record_upload` fails inside its `Tx`.
struct FailingStore {
    inner: Arc<dyn Store>,
}

struct FailingTx {
    inner: Box<dyn Tx>,
}

#[async_trait]
impl Store for FailingStore {
    fn backend(&self) -> Backend {
        self.inner.backend()
    }
    fn read(&self) -> &dyn Repos {
        self.inner.read()
    }
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        Ok(Box::new(FailingTx {
            inner: self.inner.begin().await?,
        }))
    }
    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        self.inner.subscribe(channel).await
    }
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        self.inner.try_lock(name).await
    }
    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.inner.migration_status(binary).await
    }
    async fn migrate_up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError> {
        self.inner.migrate_up(to).await
    }
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.inner.migrate_down(to, allow_lossy).await
    }
    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.inner.verify_migrations().await
    }
    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.inner.snapshot(dest).await
    }
    async fn close(&self) {
        self.inner.close().await;
    }
}

#[async_trait]
impl Tx for FailingTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        self.inner.commit().await
    }
    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.inner.rollback().await
    }
    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError> {
        self.inner.begin_nested().await
    }
}

impl Repos for FailingTx {
    fn content(&self) -> &dyn ContentRepo {
        self.inner.content()
    }
    fn meta(&self) -> &dyn MetaRepo {
        self.inner.meta()
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self.inner.revisions()
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        self.inner.types()
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        self.inner.taxonomies()
    }
    fn terms(&self) -> &dyn TermRepo {
        self.inner.terms()
    }
    fn menus(&self) -> &dyn MenuRepo {
        self.inner.menus()
    }
    fn options(&self) -> &dyn OptionRepo {
        self.inner.options()
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        self.inner.redirects()
    }
    fn users(&self) -> &dyn UserRepo {
        self.inner.users()
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self.inner.artifacts()
    }
    fn media(&self) -> &dyn MediaRepo {
        self.inner.media()
    }
    fn plugins(&self) -> &dyn PluginRepo {
        self.inner.plugins()
    }
    fn plugin_failures(&self) -> &dyn PluginFailureRepo {
        self.inner.plugin_failures()
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        self.inner.plugin_kv()
    }
    fn themes(&self) -> &dyn ThemeRepo {
        self.inner.themes()
    }
    fn events(&self) -> &dyn EventRepo {
        self.inner.events()
    }
    fn jobs(&self) -> &dyn JobRepo {
        self
    }
    fn nodes(&self) -> &dyn NodeRepo {
        self.inner.nodes()
    }
    fn search(&self) -> &dyn SearchRepo {
        self.inner.search()
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self.inner.schema_meta()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.inner.sessions()
    }
    fn api_tokens(&self) -> &dyn ApiTokenRepo {
        self.inner.api_tokens()
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.inner.rate_limits()
    }
    fn audit(&self) -> &dyn AuditRepo {
        self.inner.audit()
    }
    fn comments(&self) -> &dyn CommentRepo {
        self.inner.comments()
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self.inner.idempotency()
    }
}

#[async_trait]
impl JobRepo for FailingTx {
    async fn enqueue(&self, job: &NewJob) -> Result<Option<JobId>, StoreError> {
        if job.kind == MEDIA_VARIANTS_KIND {
            return Err(StoreError::Database("injected enqueue failure".into()));
        }
        self.inner.jobs().enqueue(job).await
    }
    async fn claim(
        &self,
        node: NodeId,
        kinds: &[String],
        limit: u32,
        lease: Duration,
    ) -> Result<Vec<Job>, StoreError> {
        self.inner.jobs().claim(node, kinds, limit, lease).await
    }
    async fn extend(&self, lease: &JobLease, duration: Duration) -> Result<bool, StoreError> {
        self.inner.jobs().extend(lease, duration).await
    }
    async fn complete(&self, lease: &JobLease) -> Result<bool, StoreError> {
        self.inner.jobs().complete(lease).await
    }
    async fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        permanent: bool,
    ) -> Result<Option<JobStatus>, StoreError> {
        self.inner.jobs().fail(lease, error, permanent).await
    }
    async fn release(
        &self,
        lease: &JobLease,
        delay: Duration,
        count_attempt: bool,
    ) -> Result<bool, StoreError> {
        self.inner.jobs().release(lease, delay, count_attempt).await
    }
    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.inner.jobs().get(id).await
    }
    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError> {
        self.inner.jobs().counts().await
    }
    async fn prune(
        &self,
        done_before: Timestamp,
        dead_before: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        self.inner
            .jobs()
            .prune(done_before, dead_before, limit)
            .await
    }
}
