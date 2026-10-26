//! Migration contract tests (02 §7–§8).

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, IDEMPOTENCY_STALE, IdemBegin, IdempotencyKey, Media,
    Timestamp,
};
use ferrite_store::{Compat, Phase, SCHEMA_META_MIN_BINARY, StoreError};
use semver::Version;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn binary() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
}

pub async fn fresh_db_is_up_to_date(db: &TestDb) {
    let store = db.store();
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::UpToDate);
    assert_eq!((s.db_version, s.binary_version), (8, 8));
    assert!(s.pending.is_empty() && s.unknown_applied.is_empty());
    assert_eq!(s.min_binary, None);

    let meta = store.read().schema_meta();
    let site_id = meta.get("site_id").await.unwrap().expect("site_id written");
    uuid::Uuid::parse_str(&site_id).expect("site_id is a uuid");
    assert!(meta.get("created_by_version").await.unwrap().is_some());
    store.verify_migrations().await.unwrap();
}

pub async fn up_down_up_roundtrip(db: &TestDb) {
    let store = db.store();
    let site_id = store.read().schema_meta().get("site_id").await.unwrap();
    assert!(
        db.user_objects()
            .await
            .iter()
            .any(|o| o.ends_with(":content_items"))
    );

    assert!(
        db.user_objects()
            .await
            .iter()
            .any(|o| o.ends_with(":search_fts") || o.ends_with(":search_docs"))
    );

    let objects = db.user_objects().await;
    assert!(objects.iter().any(|o| o.ends_with(":jobs")), "{objects:?}");
    assert!(objects.iter().any(|o| o.ends_with(":nodes")), "{objects:?}");
    for table in [
        "sessions",
        "api_tokens",
        "rate_limits",
        "audit_log",
        "audit_log_actor_idx",
    ] {
        assert!(
            objects.iter().any(|o| o.ends_with(&format!(":{table}"))),
            "{table}: {objects:?}"
        );
    }

    // 0008's down loses autosaved SEO, 0007's autosaves (0006's idempotency keys,
    // 0005's sessions and the audit log): refused without allow_lossy, nothing reverted.
    let err = store.migrate_down(0, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 8 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        8
    );
    // 0005 goes down and up again (the SQLite `DROP COLUMN` included).
    assert_eq!(store.migrate_down(4, true).await.unwrap(), vec![8, 7, 6, 5]);
    assert!(
        !db.user_objects()
            .await
            .iter()
            .any(|o| o.contains("sessions") || o.contains("audit_log")),
    );
    assert_eq!(store.migrate_up(None).await.unwrap(), vec![5, 6, 7, 8]);
    assert_eq!(
        store.migrate_down(0, true).await.unwrap(),
        vec![8, 7, 6, 5, 4, 3, 2, 1]
    );
    assert_eq!(db.user_objects().await, Vec::<String>::new());
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!((s.db_version, s.verdict), (0, Compat::NeedsUp));

    assert_eq!(
        store.migrate_up(None).await.unwrap(),
        vec![1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(store.migrate_up(None).await.unwrap(), Vec::<i64>::new());
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::UpToDate);
    assert!(
        db.user_objects()
            .await
            .iter()
            .any(|o| o.ends_with(":content_items"))
    );
    // Down to 0 dropped schema_meta, so the re-created schema is a new site.
    let new_id = store.read().schema_meta().get("site_id").await.unwrap();
    assert!(new_id.is_some() && new_id != site_id);
}

pub async fn verdict_after_partial_down(db: &TestDb) {
    let store = db.store();
    assert_eq!(
        store.migrate_down(1, true).await.unwrap(),
        vec![8, 7, 6, 5, 4, 3, 2]
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::NeedsUp);
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        vec![2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!((s.db_version, s.binary_version), (1, 8));
    let objects = db.user_objects().await;
    assert!(objects.iter().any(|o| o.ends_with(":content_items")));
    assert!(!objects.iter().any(|o| o.contains("search")), "{objects:?}");
    assert!(
        !objects.iter().any(|o| o.contains("media_variants")),
        "{objects:?}"
    );
    assert!(
        !objects
            .iter()
            .any(|o| o.ends_with(":jobs") || o.ends_with(":nodes")),
        "{objects:?}"
    );
    assert!(
        !objects.iter().any(|o| o.ends_with(":sessions")),
        "{objects:?}"
    );

    // Up to a target version stops there.
    assert_eq!(store.migrate_up(Some(2)).await.unwrap(), vec![2]);
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::NeedsUp
    );
    assert_eq!(
        store.migrate_up(None).await.unwrap(),
        vec![3, 4, 5, 6, 7, 8]
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

pub async fn verify_detects_tampered_checksum(db: &TestDb) {
    let store = db.store();
    store.verify_migrations().await.unwrap();
    db.tamper_checksum(1).await;
    let err = store.verify_migrations().await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m)
            if m.contains("checksum mismatch for applied migration 1")),
        "unexpected error: {err:?}"
    );
}

/// An older binary must not erase or lower the min-binary a newer one recorded.
pub async fn min_binary_survives_older_binary(db: &TestDb) {
    let store = db.store();
    let meta = store.read().schema_meta();
    meta.set(SCHEMA_META_MIN_BINARY, "9.9.9").await.unwrap();
    db.insert_unknown_applied(9999).await;

    assert_eq!(store.migrate_up(None).await.unwrap(), Vec::<i64>::new());
    assert_eq!(
        meta.get(SCHEMA_META_MIN_BINARY).await.unwrap().as_deref(),
        Some("9.9.9")
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.unknown_applied, vec![9999]);
    assert_eq!(
        s.verdict,
        Compat::Incompatible {
            requires: Version::new(9, 9, 9)
        }
    );
}

/// Pending migrations carry their description and parsed directives.
pub async fn status_reports_pending_directives(db: &TestDb) {
    let store = db.store();
    assert_eq!(
        store.migrate_down(2, true).await.unwrap(),
        vec![8, 7, 6, 5, 4, 3]
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.pending.len(), 6);
    let pending = &s.pending[0];
    assert_eq!(pending.version, 3);
    assert!(!pending.description.is_empty());
    assert_eq!(pending.directives.phase, Phase::Expand);
    assert_eq!(pending.directives.min_binary, None);
    assert!(!pending.directives.lossy_down);
    let cluster = &s.pending[1];
    assert_eq!(
        (cluster.version, cluster.description.as_str()),
        (4, "cluster")
    );
    assert_eq!(cluster.directives.phase, Phase::Expand);
    assert!(cluster.directives.lossy_down);
    let auth = &s.pending[2];
    assert_eq!((auth.version, auth.description.as_str()), (5, "auth"));
    assert_eq!(auth.directives.phase, Phase::Expand);
    assert!(auth.directives.lossy_down);
    let media = &s.pending[3];
    assert_eq!(
        (media.version, media.description.as_str()),
        (6, "media comments")
    );
    assert_eq!(media.directives.phase, Phase::Expand);
    assert!(media.directives.lossy_down);
    let autosaves = &s.pending[4];
    assert_eq!(
        (autosaves.version, autosaves.description.as_str()),
        (7, "autosaves")
    );
    assert_eq!(autosaves.directives.phase, Phase::Expand);
    assert_eq!(autosaves.directives.min_binary, None);
    assert!(autosaves.directives.lossy_down);
    let seo = &s.pending[5];
    assert_eq!((seo.version, seo.description.as_str()), (8, "autosave seo"));
    assert_eq!(seo.directives.phase, Phase::Expand);
    assert_eq!(seo.directives.min_binary, None);
    assert!(seo.directives.lossy_down);
}

/// `0006_media_comments` (M2a-2b P2): its objects exist; the down is lossy (refused
/// without `allow_lossy`) and drops idempotency keys and the sanitised marker; up again
/// restores the schema with the marker back at its default.
pub async fn migration_0006_roundtrip(db: &TestDb) {
    let store = db.store();
    const OBJECTS: [&str; 9] = [
        "idempotency_keys",
        "idempotency_keys_created_idx",
        "comments_status_created_idx",
        "comments_item_status_idx",
        "comments_created_idx",
        "comments_author_email_idx",
        "comments_user_idx",
        "jobs_status_created_idx",
        "jobs_created_idx",
    ];
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }

    // A marked media row and a pending key, to see what the down loses.
    let author = fixture_user(store).await;
    let mut item = new_post(&author, "marked");
    item.kind = "media".into();
    store.read().content().insert(&item).await.unwrap();
    let artifact = Artifact {
        hash: ArtifactHash::parse(&"a".repeat(64)).unwrap(),
        kind: ArtifactKind::Media,
        bytes: 5,
        mime: "image/svg+xml".into(),
        created_at: Timestamp::now(),
    };
    store
        .read()
        .artifacts()
        .insert_if_absent(&artifact)
        .await
        .unwrap();
    let media = Media {
        item_id: item.id,
        artifact_hash: artifact.hash.clone(),
        mime: "image/svg+xml".into(),
        bytes: 5,
        width: None,
        height: None,
        alt: String::new(),
        original_name: "a.svg".into(),
        variants: serde_json::json!({}),
        sanitized: true,
    };
    store.read().media().insert(&media).await.unwrap();
    let key = IdempotencyKey {
        user: author.id,
        key_hash: "b".repeat(64),
        method: "POST".into(),
        path: "/api/v1/media".into(),
    };
    store
        .read()
        .idempotency()
        .begin(&key, IDEMPOTENCY_STALE)
        .await
        .unwrap();

    // 0007 and 0008 sit above it and are lossy too; refused first, nothing reverted.
    let err = store.migrate_down(5, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 8 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        8
    );
    assert_eq!(store.migrate_down(5, true).await.unwrap(), vec![8, 7, 6]);
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [6, 7, 8]
    );
    assert!(s.pending[0].directives.lossy_down);

    // Up again re-adds the column (so the down really dropped it) at its default.
    assert_eq!(store.migrate_up(None).await.unwrap(), vec![6, 7, 8]);
    let got = store.read().media().get(item.id).await.unwrap().unwrap();
    assert_eq!(
        got,
        Media {
            sanitized: false,
            ..media
        }
    );
    // The key is gone: a new use starts.
    assert!(matches!(
        store
            .read()
            .idempotency()
            .begin(&key, IDEMPOTENCY_STALE)
            .await
            .unwrap(),
        IdemBegin::Started { .. }
    ));
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0007_autosaves` (M2b P15): its objects exist; the down is lossy (refused without
/// `allow_lossy`, nothing reverted) and drops every autosave; up again restores an
/// empty slot table that takes new rows.
pub async fn migration_0007_roundtrip(db: &TestDb) {
    let store = db.store();
    const OBJECTS: [&str; 2] = ["autosaves", "autosaves_user_idx"];
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }

    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "autosaved"))
        .await
        .unwrap();
    let slot = crate::autosave::full_autosave(item.id, author.id);
    store.read().autosaves().put(&slot).await.unwrap();

    let err = store.migrate_down(6, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 8 has a lossy down")),
        "{err:?}"
    );
    assert!(has(&db.user_objects().await, "autosaves"));
    assert_eq!(store.migrate_down(6, true).await.unwrap(), vec![8, 7]);
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [7, 8]
    );
    assert!(s.pending[0].directives.lossy_down);
    assert_eq!(s.pending[0].directives.phase, Phase::Expand);

    assert_eq!(store.migrate_up(None).await.unwrap(), vec![7, 8]);
    // The row did not survive; the table takes it again.
    let repos = store.read();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        None
    );
    repos.autosaves().put(&slot).await.unwrap();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        Some(slot)
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0008_autosave_seo` (M2b-2 Task 1): the down is lossy (refused without
/// `allow_lossy`, nothing reverted) and drops `autosaves.seo`, keeping the slot rows;
/// up again re-adds the column empty (so the down really dropped it).
pub async fn migration_0008_roundtrip(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "seo-autosaved"))
        .await
        .unwrap();
    let slot = crate::autosave::full_autosave(item.id, author.id);
    assert!(slot.seo.is_some());
    store.read().autosaves().put(&slot).await.unwrap();

    let err = store.migrate_down(7, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 8 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        8
    );
    assert_eq!(store.migrate_down(7, true).await.unwrap(), vec![8]);
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.pending.iter().map(|p| p.version).collect::<Vec<_>>(), [8]);
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("autosave seo", Phase::Expand)
    );
    assert!(s.pending[0].directives.lossy_down);

    assert_eq!(store.migrate_up(None).await.unwrap(), vec![8]);
    // The slot survived without its SEO member.
    let repos = store.read();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        Some(ferrite_domain::Autosave {
            seo: None,
            ..slot.clone()
        })
    );
    repos.autosaves().put(&slot).await.unwrap();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        Some(slot)
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}
