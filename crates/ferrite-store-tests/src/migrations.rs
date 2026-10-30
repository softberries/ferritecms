//! Migration contract tests (02 §7–§8).

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, IDEMPOTENCY_STALE, IdemBegin, IdempotencyKey,
    ImportMapKind, Media, Timestamp,
};
use ferrite_store::{
    Backend, Compat, MigrateReport, Phase, SCHEMA_META_INSTALL_ID, SCHEMA_META_LAST_BINARY,
    SCHEMA_META_MIN_BINARY, StoreError,
};
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
    assert_eq!((s.db_version, s.binary_version), (12, 12));
    assert!(s.pending.is_empty() && s.unknown_applied.is_empty());
    assert_eq!(s.min_binary, None);

    let meta = store.read().schema_meta();
    let site_id = meta.get("site_id").await.unwrap().expect("site_id written");
    uuid::Uuid::parse_str(&site_id).expect("site_id is a uuid");
    assert!(meta.get("created_by_version").await.unwrap().is_some());
    // R16: this database's own install id, never the site id (copies share that).
    let install = meta
        .get(SCHEMA_META_INSTALL_ID)
        .await
        .unwrap()
        .expect("install_id written");
    uuid::Uuid::parse_str(&install).expect("install_id is a uuid");
    assert_ne!(install, site_id);
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
        matches!(&err, StoreError::Migration(m) if m.contains("migration 10 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        12
    );
    // 0005 goes down and up again (the SQLite `DROP COLUMN` included).
    assert_eq!(
        store.migrate_down(4, true).await.unwrap(),
        vec![12, 11, 10, 9, 8, 7, 6, 5]
    );
    assert!(
        !db.user_objects()
            .await
            .iter()
            .any(|o| o.contains("sessions") || o.contains("audit_log")),
    );
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_eq!(
        store.migrate_down(0, true).await.unwrap(),
        vec![12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
    );
    assert_eq!(db.user_objects().await, Vec::<String>::new());
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!((s.db_version, s.verdict), (0, Compat::NeedsUp));

    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        Vec::<i64>::new()
    );
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
        vec![12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2]
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::NeedsUp);
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_eq!((s.db_version, s.binary_version), (1, 12));
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
    assert_eq!(store.migrate_up(Some(2)).await.unwrap().applied, vec![2]);
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::NeedsUp
    );
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
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

    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        Vec::<i64>::new()
    );
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
        vec![12, 11, 10, 9, 8, 7, 6, 5, 4, 3]
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.pending.len(), 10);
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
    let import = &s.pending[6];
    assert_eq!((import.version, import.description.as_str()), (9, "import"));
    assert_eq!(import.directives.phase, Phase::Expand);
    assert_eq!(import.directives.min_binary, None);
    assert!(import.directives.lossy_down);
    let jobs = &s.pending[7];
    assert_eq!(
        (jobs.version, jobs.description.as_str()),
        (10, "import jobs")
    );
    assert_eq!(jobs.directives.phase, phase_0010(db));
    assert_eq!(jobs.directives.min_binary, None);
    assert!(jobs.directives.lossy_down);
    let gc = &s.pending[8];
    assert_eq!((gc.version, gc.description.as_str()), (11, "artifact gc"));
    assert_eq!(gc.directives.phase, Phase::Expand);
    assert_eq!(gc.directives.min_binary, None);
    assert!(!gc.directives.lossy_down);
    let metrics = &s.pending[9];
    assert_eq!(
        (metrics.version, metrics.description.as_str()),
        (12, "node metrics")
    );
    assert_eq!(metrics.directives.phase, Phase::Expand);
    assert_eq!(metrics.directives.min_binary, None);
    assert!(!metrics.directives.lossy_down);
}

/// `0012_node_metrics` (M4-2 P35): the table exists; the down drops it with its rows
/// (transient data: not declared lossy, like the export skipping it); up again takes rows.
pub async fn migration_0012_roundtrip(db: &TestDb) {
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    assert!(has(&db.user_objects().await, "node_metrics"));
    let minute = Timestamp::from_micros(ferrite_domain::MINUTE_MICROS * 1000).unwrap();
    let node = ferrite_domain::NodeId::new();
    let counts = ferrite_domain::MetricCounts::default();
    store
        .read()
        .node_metrics()
        .insert(node, minute, &counts)
        .await
        .unwrap();
    // Not lossy: no allow_lossy needed.
    assert_eq!(store.migrate_down(11, false).await.unwrap(), vec![12]);
    assert!(!has(&db.user_objects().await, "node_metrics"));
    assert_eq!(store.migrate_up(None).await.unwrap().applied, vec![12]);
    store
        .read()
        .node_metrics()
        .insert(node, minute, &counts)
        .await
        .unwrap();
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0010_import_jobs` is a contract migration on SQLite only: widening the
/// `artifacts.kind` CHECK there is a table rebuild (`DROP TABLE`), while Postgres
/// replaces the constraint in place.
fn phase_0010(db: &TestDb) -> Phase {
    if db.is_sqlite() {
        Phase::Contract
    } else {
        Phase::Expand
    }
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
        orphaned_at: None,
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
        matches!(&err, StoreError::Migration(m) if m.contains("migration 10 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        12
    );
    assert_eq!(
        store.migrate_down(5, true).await.unwrap(),
        vec![12, 11, 10, 9, 8, 7, 6]
    );
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [6, 7, 8, 9, 10, 11, 12]
    );
    assert!(s.pending[0].directives.lossy_down);

    // Up again re-adds the column (so the down really dropped it) at its default.
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![6, 7, 8, 9, 10, 11, 12]
    );
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
        matches!(&err, StoreError::Migration(m) if m.contains("migration 10 has a lossy down")),
        "{err:?}"
    );
    assert!(has(&db.user_objects().await, "autosaves"));
    assert_eq!(
        store.migrate_down(6, true).await.unwrap(),
        vec![12, 11, 10, 9, 8, 7]
    );
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [7, 8, 9, 10, 11, 12]
    );
    assert!(s.pending[0].directives.lossy_down);
    assert_eq!(s.pending[0].directives.phase, Phase::Expand);

    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![7, 8, 9, 10, 11, 12]
    );
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
        matches!(&err, StoreError::Migration(m) if m.contains("migration 10 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        12
    );
    assert_eq!(
        store.migrate_down(7, true).await.unwrap(),
        vec![12, 11, 10, 9, 8]
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [8, 9, 10, 11, 12]
    );
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("autosave seo", Phase::Expand)
    );
    assert!(s.pending[0].directives.lossy_down);

    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![8, 9, 10, 11, 12]
    );
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

/// `0009_import` (M3-1 Task 1): its tables exist; the down is lossy (refused without
/// `allow_lossy`, nothing reverted) and drops every run and map row; up again restores
/// empty tables that take new rows.
pub async fn migration_0009_roundtrip(db: &TestDb) {
    let store = db.store();
    const OBJECTS: [&str; 5] = [
        "import_runs",
        "import_map",
        "import_runs_running_uq",
        "import_runs_started_idx",
        "import_runs_started_by_idx",
    ];
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }

    let admin = fixture_user(store).await;
    let run = crate::import::new_run(admin.id, "https://wp.example");
    store.read().imports().create_run(&run).await.unwrap();
    let map = crate::import::map_row("https://wp.example", ImportMapKind::Content, 7);
    store.read().imports().map_put(&map).await.unwrap();

    let err = store.migrate_down(8, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 10 has a lossy down")),
        "{err:?}"
    );
    assert!(has(&db.user_objects().await, "import_runs"));
    assert_eq!(
        store.migrate_down(8, true).await.unwrap(),
        vec![12, 11, 10, 9]
    );
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [9, 10, 11, 12]
    );
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("import", Phase::Expand)
    );
    assert!(s.pending[0].directives.lossy_down);

    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![9, 10, 11, 12]
    );
    // The rows did not survive; the tables take them again.
    let repos = store.read();
    assert_eq!(repos.imports().get_run(run.id).await.unwrap(), None);
    assert_eq!(
        repos
            .imports()
            .map_get(&map.source, map.kind, map.wp_id)
            .await
            .unwrap(),
        None
    );
    repos.imports().create_run(&run).await.unwrap();
    repos.imports().map_put(&map).await.unwrap();
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0010_import_jobs` (M3-2 Task 1, P23, P28): `artifacts.kind` takes `import` (on
/// SQLite through a table rebuild that keeps the rows and the foreign keys pointing at
/// them), `import_runs` gains `job_id`, `step` and `report_hash` with
/// `import_runs_job_idx`. The down is lossy (refused without `allow_lossy`): it drops the
/// three columns and every `import` artifact row; up again restores the schema.
pub async fn migration_0010_roundtrip(db: &TestDb) {
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    assert!(has(&db.user_objects().await, "import_runs_job_idx"));

    // A media artifact referenced by a media row survives the rebuild.
    let admin = fixture_user(store).await;
    let post = new_post(&admin, "pic");
    store.read().content().insert(&post).await.unwrap();
    let kept = artifact_0010('c', ArtifactKind::Media);
    let import = artifact_0010('d', ArtifactKind::Import);
    let artifacts = store.read().artifacts();
    assert!(artifacts.insert_if_absent(&kept).await.unwrap());
    assert!(artifacts.insert_if_absent(&import).await.unwrap());
    store
        .read()
        .media()
        .insert(&Media {
            item_id: post.id,
            artifact_hash: kept.hash.clone(),
            mime: "image/png".into(),
            bytes: 1,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "pic.png".into(),
            variants: serde_json::json!({}),
            sanitized: false,
        })
        .await
        .unwrap();
    let mut run = crate::import::new_run(admin.id, "https://wp.example");
    run.job_id = Some(ferrite_domain::JobId::new());
    run.step = Some("pass2".into());
    store.read().imports().create_run(&run).await.unwrap();

    let err = store.migrate_down(9, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 10 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(store.migrate_down(9, true).await.unwrap(), vec![12, 11, 10]);
    assert!(!has(&db.user_objects().await, "import_runs_job_idx"));
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        [10, 11, 12]
    );
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("import jobs", phase_0010(db))
    );
    assert!(s.pending[0].directives.lossy_down);
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![10, 11, 12]
    );
    // The import artifact went with the down; the media artifact and its row stayed.
    // (Read after the up: the artifact queries need 0011's column.)
    let artifacts = store.read().artifacts();
    assert_eq!(artifacts.get(&import.hash).await.unwrap(), None);
    assert_eq!(artifacts.get(&kept.hash).await.unwrap(), Some(kept.clone()));
    assert!(store.read().media().get(post.id).await.unwrap().is_some());
    // The run survived without its new columns; they take values again.
    let repos = store.read();
    let back = repos.imports().get_run(run.id).await.unwrap().unwrap();
    assert_eq!(
        (back.job_id, back.step, back.report_hash),
        (None, None, None)
    );
    assert!(repos.artifacts().insert_if_absent(&import).await.unwrap());
    assert!(
        repos
            .imports()
            .set_step(run.id, &run.token, "pass3")
            .await
            .unwrap()
    );
    // The rebuilt table still guards the media foreign key.
    assert!(repos.artifacts().delete(&kept.hash).await.is_err());
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

fn artifact_0010(c: char, kind: ArtifactKind) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&c.to_string().repeat(64)).unwrap(),
        kind,
        bytes: 10,
        mime: "application/octet-stream".into(),
        created_at: Timestamp::from_micros(1_700_000_000_000_000).unwrap(),
        orphaned_at: None,
    }
}

/// M4-2 P30: `migrate_up` reports what it applied and, on SQLite, the pre-migration
/// snapshot it wrote (Postgres takes none). `last_binary_version` is an ordinary
/// `schema_meta` key the migrations' bookkeeping leaves alone.
pub async fn migrate_up_reports_snapshot(db: &TestDb) {
    let store = db.store();
    let meta = store.read().schema_meta();
    meta.set(SCHEMA_META_LAST_BINARY, "0.0.1").await.unwrap();

    let none = store.migrate_up(None).await.unwrap();
    assert_eq!(none, MigrateReport::default(), "nothing pending");

    // Derived, so a new migration needs no edit here.
    let l = store
        .migration_status(&binary())
        .await
        .unwrap()
        .binary_version;
    assert_eq!(
        store.migrate_down(l - 2, true).await.unwrap(),
        vec![l, l - 1]
    );
    let report = store.migrate_up(None).await.unwrap();
    assert_eq!(report.applied, vec![l - 1, l]);
    match store.backend() {
        Backend::Sqlite => {
            let snap = report
                .snapshot
                .expect("SQLite writes a pre-migration snapshot");
            assert!(snap.is_file(), "{}", snap.display());
            let name = snap.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.starts_with(&format!("pre-{}-to-{l}-", l - 2)) && name.ends_with(".db"),
                "{name}"
            );
        }
        Backend::Postgres => assert_eq!(report.snapshot, None),
    }
    assert_eq!(
        meta.get(SCHEMA_META_LAST_BINARY).await.unwrap().as_deref(),
        Some("0.0.1")
    );
}
