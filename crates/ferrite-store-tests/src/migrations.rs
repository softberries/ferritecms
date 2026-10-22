//! Migration contract tests (02 §7–§8).

use ferrite_store::{Compat, SCHEMA_META_MIN_BINARY, StoreError};
use semver::Version;

use crate::TestDb;

fn binary() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
}

pub async fn fresh_db_is_up_to_date(db: &TestDb) {
    let store = db.store();
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::UpToDate);
    assert_eq!((s.db_version, s.binary_version), (1, 1));
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

    assert_eq!(store.migrate_down(0, false).await.unwrap(), vec![1]);
    assert_eq!(db.user_objects().await, Vec::<String>::new());
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!((s.db_version, s.verdict), (0, Compat::NeedsUp));

    assert_eq!(store.migrate_up(None).await.unwrap(), vec![1]);
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
    // Only 0001 exists until Task 10 adds 0002; then this goes down to 1 instead.
    let store = db.store();
    store.migrate_down(0, false).await.unwrap();
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::NeedsUp);
    assert_eq!(s.pending, vec![1]);
    assert_eq!(s.db_version, 0);
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
