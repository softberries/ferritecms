//! Migration contract tests (02 §7–§8).

use ferrite_store::{Compat, Phase, SCHEMA_META_MIN_BINARY, StoreError};
use semver::Version;

use crate::TestDb;

fn binary() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
}

pub async fn fresh_db_is_up_to_date(db: &TestDb) {
    let store = db.store();
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::UpToDate);
    assert_eq!((s.db_version, s.binary_version), (4, 4));
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

    // 0004's down loses queued jobs: refused without allow_lossy, nothing reverted.
    let err = store.migrate_down(0, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 4 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        4
    );
    assert_eq!(store.migrate_down(0, true).await.unwrap(), vec![4, 3, 2, 1]);
    assert_eq!(db.user_objects().await, Vec::<String>::new());
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!((s.db_version, s.verdict), (0, Compat::NeedsUp));

    assert_eq!(store.migrate_up(None).await.unwrap(), vec![1, 2, 3, 4]);
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
    assert_eq!(store.migrate_down(1, true).await.unwrap(), vec![4, 3, 2]);
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::NeedsUp);
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!((s.db_version, s.binary_version), (1, 4));
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

    // Up to a target version stops there.
    assert_eq!(store.migrate_up(Some(2)).await.unwrap(), vec![2]);
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::NeedsUp
    );
    assert_eq!(store.migrate_up(None).await.unwrap(), vec![3, 4]);
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
    assert_eq!(store.migrate_down(2, true).await.unwrap(), vec![4, 3]);
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.pending.len(), 2);
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
}
