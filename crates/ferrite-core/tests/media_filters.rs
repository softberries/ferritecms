//! MediaService lists on both backends (M2b-2 Task 2): search, kind filters, the trash
//! view and restore.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentService, CoreError, MediaItem, MediaQuery, MediaService, NewUpload,
    PermalinkRules, Permalinks, SiteReader,
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, MediaKind, NewUser, Role,
    Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};

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

fn services(store: Arc<dyn Store>) -> (MediaService, Arc<ContentService>) {
    let rules: Arc<dyn Permalinks> = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    (MediaService::new(store, content.clone(), rules), content)
}

fn reader(store: Arc<dyn Store>) -> SiteReader {
    SiteReader::new(
        store,
        Arc::new(PermalinkRules::default()),
        Arc::new(|hash: &ArtifactHash, name: &str| format!("/_media/{hash}/{name}")),
        Arc::new(Timestamp::now),
    )
}

fn hash(seed: u8) -> ArtifactHash {
    ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap()
}

fn upload(seed: u8, mime: &str, name: &str, title: Option<&str>) -> NewUpload {
    NewUpload {
        artifact: Artifact {
            hash: hash(seed),
            kind: ArtifactKind::Media,
            bytes: 1234,
            mime: mime.into(),
            created_at: Timestamp::now(),
        },
        mime: mime.into(),
        bytes: 1234,
        original_name: name.into(),
        title: title.map(str::to_owned),
        alt: String::new(),
        sanitized: mime == "image/svg+xml",
        raster: false,
    }
}

fn query() -> MediaQuery {
    MediaQuery {
        limit: 50,
        cursor: None,
        author: None,
        q: None,
        kind: None,
        trash: false,
    }
}

fn search(q: &str) -> MediaQuery {
    MediaQuery {
        q: Some(q.into()),
        ..query()
    }
}

async fn ids(media: &MediaService, who: &Actor, q: MediaQuery) -> Vec<ContentId> {
    media
        .list(who, q)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|m| m.item.id)
        .collect()
}

fn invalid_field<T: std::fmt::Debug>(r: Result<T, CoreError>, want: &str) {
    match r {
        Err(CoreError::InvalidField { field, .. }) => assert_eq!(field, want),
        other => panic!("expected InvalidField({want}), got {other:?}"),
    }
}

/// The index holds a media item's title (and its empty body), not its original file
/// name: search finds the title, never the file name.
async fn media_search_by_title_and_original_name(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let photo = media
        .record_upload(
            &author,
            upload(
                1,
                "image/png",
                "zebra-crossing.png",
                Some("Quokka at sunset"),
            ),
        )
        .await
        .unwrap();
    let clip = media
        .record_upload(&author, upload(2, "video/mp4", "quokka-run.mp4", None))
        .await
        .unwrap();
    assert_eq!(clip.item.title, "quokka-run");

    // By title (the clip's title comes from its file name, so it matches too).
    assert_eq!(
        ids(&media, &author, search("  quokka ")).await.len(),
        2,
        "both titles hold quokka"
    );
    assert_eq!(
        ids(&media, &author, search("sunset")).await,
        vec![photo.item.id]
    );
    // The original name is not indexed.
    assert!(ids(&media, &author, search("zebra")).await.is_empty());
    // Blank text is no search.
    assert_eq!(ids(&media, &author, search("   ")).await.len(), 2);
    // `kind` combines with `q` and with the plain list.
    let kinded = |q: Option<&str>, kind| MediaQuery {
        q: q.map(str::to_owned),
        kind: Some(kind),
        ..query()
    };
    assert_eq!(
        ids(&media, &author, kinded(Some("quokka"), MediaKind::Image)).await,
        vec![photo.item.id]
    );
    assert_eq!(
        ids(&media, &author, kinded(Some("quokka"), MediaKind::Video)).await,
        vec![clip.item.id]
    );
    assert!(
        ids(&media, &author, kinded(Some("quokka"), MediaKind::Audio))
            .await
            .is_empty()
    );
    assert_eq!(
        ids(&media, &author, kinded(None, MediaKind::Video)).await,
        vec![clip.item.id]
    );
    assert!(
        ids(&media, &author, kinded(None, MediaKind::Document))
            .await
            .is_empty()
    );
}

/// `q` with `author` or `trash` is refused (F19; trashed items leave the index).
async fn media_q_with_author_is_invalid(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let r = media
        .list(
            &author,
            MediaQuery {
                author: Some(author.user),
                ..search("quokka")
            },
        )
        .await;
    invalid_field(r, "q");
    // Blank text is no search, so `author` is fine then.
    media
        .list(
            &author,
            MediaQuery {
                author: Some(author.user),
                ..search(" ")
            },
        )
        .await
        .unwrap();
}

async fn media_q_with_trash_is_invalid(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let r = media
        .list(
            &author,
            MediaQuery {
                trash: true,
                ..search("quokka")
            },
        )
        .await;
    invalid_field(r, "q");
}

/// An author does not find another author's unpublished upload, by search, by kind or
/// in the trash view; an editor does.
async fn readable_by_still_applies(db: &TestDb) {
    let (media, content) = services(db.shared_store());
    let a = actor(db.store(), Role::Author).await;
    let b = actor(db.store(), Role::Author).await;
    let editor = actor(db.store(), Role::Editor).await;
    let hidden = media
        .record_upload(
            &a,
            upload(10, "application/pdf", "q.pdf", Some("Quokka plan")),
        )
        .await
        .unwrap();
    content
        .unpublish(&a, hidden.item.id, hidden.item.version)
        .await
        .unwrap();
    let trashed = media
        .record_upload(
            &a,
            upload(11, "application/pdf", "t.pdf", Some("Quokka old")),
        )
        .await
        .unwrap();
    let trashed_item = media
        .trash(&a, trashed.item.id, trashed.item.version)
        .await
        .unwrap();
    assert_eq!(trashed_item.status, ContentStatus::Trash);

    let docs = MediaQuery {
        kind: Some(MediaKind::Document),
        ..query()
    };
    let in_trash = MediaQuery {
        trash: true,
        ..query()
    };
    assert!(ids(&media, &b, search("quokka")).await.is_empty());
    assert!(ids(&media, &b, docs.clone()).await.is_empty());
    assert!(ids(&media, &b, in_trash.clone()).await.is_empty());
    assert_eq!(
        ids(&media, &a, search("quokka")).await,
        vec![hidden.item.id]
    );
    assert_eq!(
        ids(&media, &editor, search("quokka")).await,
        vec![hidden.item.id]
    );
    assert_eq!(ids(&media, &editor, docs).await, vec![hidden.item.id]);
    assert_eq!(
        ids(&media, &a, in_trash.clone()).await,
        vec![trashed.item.id]
    );
    assert_eq!(ids(&media, &editor, in_trash).await, vec![trashed.item.id]);
}

/// Trashed items are listed only with `trash`; restore publishes them again (same
/// `published_at`), lists them and serves their file; contributors cannot restore (R8).
async fn media_trash_list_and_restore(db: &TestDb) {
    let (media, _) = services(db.shared_store());
    let site = reader(db.shared_store());
    let author = actor(db.store(), Role::Author).await;
    let contributor = actor(db.store(), Role::Contributor).await;
    let kept = media
        .record_upload(&author, upload(20, "image/png", "kept.png", None))
        .await
        .unwrap();
    let gone = media
        .record_upload(&author, upload(21, "audio/mpeg", "gone.mp3", None))
        .await
        .unwrap();
    assert!(site.media_file(&hash(21)).await.unwrap().is_some());
    // Restoring an item that is not trashed is Invalid, as for content.
    assert!(matches!(
        media
            .restore(&author, gone.item.id, gone.item.version)
            .await,
        Err(CoreError::Invalid(_))
    ));
    let trashed = media
        .trash(&author, gone.item.id, gone.item.version)
        .await
        .unwrap();
    assert!(site.media_file(&hash(21)).await.unwrap().is_none());

    let in_trash = MediaQuery {
        trash: true,
        ..query()
    };
    assert_eq!(ids(&media, &author, query()).await, vec![kept.item.id]);
    assert_eq!(
        ids(&media, &author, in_trash.clone()).await,
        vec![gone.item.id]
    );
    // `kind` applies in the trash view too.
    assert_eq!(
        ids(
            &media,
            &author,
            MediaQuery {
                kind: Some(MediaKind::Audio),
                ..in_trash.clone()
            }
        )
        .await,
        vec![gone.item.id]
    );
    assert!(
        ids(
            &media,
            &author,
            MediaQuery {
                kind: Some(MediaKind::Image),
                ..in_trash.clone()
            }
        )
        .await
        .is_empty()
    );

    // R8: a contributor cannot restore.
    assert!(matches!(
        media
            .restore(&contributor, gone.item.id, trashed.version)
            .await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    // A stale version is a conflict.
    assert!(matches!(
        media
            .restore(&author, gone.item.id, gone.item.version)
            .await,
        Err(CoreError::Conflict { .. })
    ));
    let restored: MediaItem = media
        .restore(&author, gone.item.id, trashed.version)
        .await
        .unwrap();
    assert_eq!(restored.item.status, ContentStatus::Published);
    assert_eq!(restored.item.published_at, gone.item.published_at);
    assert_eq!(restored.media.artifact_hash, hash(21));
    assert!(ids(&media, &author, in_trash).await.is_empty());
    assert_eq!(
        ids(&media, &author, query()).await,
        vec![gone.item.id, kept.item.id]
    );
    assert!(site.media_file(&hash(21)).await.unwrap().is_some());
    // Searchable again.
    assert_eq!(
        ids(&media, &author, search("gone")).await,
        vec![gone.item.id]
    );
}

/// The meta row a media trash leaves (P53), read straight from the store.
async fn trashed_from(store: &dyn Store, id: ContentId) -> Option<serde_json::Value> {
    store
        .read()
        .meta()
        .get(id, "media.trashed_from")
        .await
        .unwrap()
}

/// P53: a media item unpublished before it was trashed comes back as a draft; trash
/// writes the marker (`"draft"`, or `"published"`) and restore deletes it.
async fn media_restore_keeps_draft_status(db: &TestDb) {
    let (media, content) = services(db.shared_store());
    let editor = actor(db.store(), Role::Editor).await;
    let up = media
        .record_upload(&editor, upload(30, "image/png", "draft.png", None))
        .await
        .unwrap();
    let drafted = content
        .unpublish(&editor, up.item.id, up.item.version)
        .await
        .unwrap();
    assert_eq!(drafted.status, ContentStatus::Draft);
    let trashed = media
        .trash(&editor, up.item.id, drafted.version)
        .await
        .unwrap();
    assert_eq!(
        trashed_from(db.store(), up.item.id).await,
        Some(serde_json::json!("draft"))
    );
    let restored = media
        .restore(&editor, up.item.id, trashed.version)
        .await
        .unwrap();
    assert_eq!(restored.item.status, ContentStatus::Draft);
    assert_eq!(trashed_from(db.store(), up.item.id).await, None);

    // A published one is marked `published` and comes back published.
    let live = media
        .record_upload(&editor, upload(31, "image/png", "live.png", None))
        .await
        .unwrap();
    let trashed = media
        .trash(&editor, live.item.id, live.item.version)
        .await
        .unwrap();
    assert_eq!(
        trashed_from(db.store(), live.item.id).await,
        Some(serde_json::json!("published"))
    );
    let restored = media
        .restore(&editor, live.item.id, trashed.version)
        .await
        .unwrap();
    assert_eq!(restored.item.status, ContentStatus::Published);
    assert_eq!(restored.item.published_at, live.item.published_at);
    assert_eq!(trashed_from(db.store(), live.item.id).await, None);
}

/// P53: an item trashed without the marker (before the change, or through the content
/// lifecycle) restores as published (R23).
async fn media_restore_without_marker_publishes(db: &TestDb) {
    let (media, content) = services(db.shared_store());
    let editor = actor(db.store(), Role::Editor).await;
    let up = media
        .record_upload(&editor, upload(32, "image/png", "old.png", None))
        .await
        .unwrap();
    let drafted = content
        .unpublish(&editor, up.item.id, up.item.version)
        .await
        .unwrap();
    let trashed = content
        .trash(&editor, up.item.id, drafted.version)
        .await
        .unwrap();
    assert_eq!(trashed_from(db.store(), up.item.id).await, None);
    let restored = media
        .restore(&editor, up.item.id, trashed.version)
        .await
        .unwrap();
    assert_eq!(restored.item.status, ContentStatus::Published);
}

backend_tests!(
    media_search_by_title_and_original_name,
    media_q_with_author_is_invalid,
    media_q_with_trash_is_invalid,
    readable_by_still_applies,
    media_trash_list_and_restore,
    media_restore_keeps_draft_status,
    media_restore_without_marker_publishes,
);
