//! Contract tests for the artifact GC queries (`0011_artifact_gc`, M4-1 P10, P11).

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, InstalledTheme, Media, Timestamp,
};
use ferrite_store::{MarkCounts, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
    import::new_run,
};

fn hash(c: char) -> ArtifactHash {
    ArtifactHash::parse(&c.to_string().repeat(64)).unwrap()
}

fn t(micros: i64) -> Timestamp {
    Timestamp::from_micros(1_700_000_000_000_000 + micros).unwrap()
}

fn artifact(c: char, kind: ArtifactKind) -> Artifact {
    Artifact {
        hash: hash(c),
        kind,
        bytes: 10,
        mime: "application/octet-stream".into(),
        created_at: t(0),
        orphaned_at: None,
    }
}

async fn put(db: &TestDb, c: char, kind: ArtifactKind) {
    let inserted = db
        .store()
        .read()
        .artifacts()
        .insert_if_absent(&artifact(c, kind))
        .await
        .unwrap();
    assert!(inserted);
}

async fn orphaned_at(db: &TestDb, c: char) -> Option<Timestamp> {
    db.store()
        .read()
        .artifacts()
        .get(&hash(c))
        .await
        .unwrap()
        .expect("artifact row")
        .orphaned_at
}

async fn reference_from_theme(db: &TestDb, name: &str, c: char) {
    db.store()
        .read()
        .themes()
        .upsert(&InstalledTheme {
            name: name.into(),
            version: "1.0.0".into(),
            artifact_hash: hash(c),
            manifest: json!({"name": name}),
            active: false,
            installed_at: t(0),
        })
        .await
        .unwrap();
}

pub async fn artifact_list_page_is_ordered_and_paged(db: &TestDb) {
    let artifacts = db.store().read().artifacts();
    for c in ['d', 'a', 'c', 'b', 'e'] {
        put(db, c, ArtifactKind::Media).await;
    }
    let all = artifacts.list_page(None, 100).await.unwrap();
    let hashes: Vec<_> = all.iter().map(|a| a.hash.clone()).collect();
    assert_eq!(hashes, ['a', 'b', 'c', 'd', 'e'].map(hash));
    assert_eq!(all[0], artifact('a', ArtifactKind::Media));

    let first = artifacts.list_page(None, 2).await.unwrap();
    assert_eq!(first.len(), 2);
    let second = artifacts.list_page(Some(&first[1].hash), 2).await.unwrap();
    assert_eq!(
        second.iter().map(|a| a.hash.clone()).collect::<Vec<_>>(),
        [hash('c'), hash('d')]
    );
    let last = artifacts.list_page(Some(&second[1].hash), 2).await.unwrap();
    assert_eq!(
        last.iter().map(|a| a.hash.clone()).collect::<Vec<_>>(),
        [hash('e')]
    );
    assert!(
        artifacts
            .list_page(Some(&hash('e')), 2)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        artifacts.list_page(None, 0).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn existing_returns_present_subset(db: &TestDb) {
    let artifacts = db.store().read().artifacts();
    put(db, 'a', ArtifactKind::Media).await;
    put(db, 'c', ArtifactKind::Media).await;
    let got = artifacts
        .existing(&[hash('c'), hash('b'), hash('a')])
        .await
        .unwrap();
    assert_eq!(got, [hash('a'), hash('c')]);
    assert!(artifacts.existing(&[]).await.unwrap().is_empty());
    assert!(artifacts.existing(&[hash('9')]).await.unwrap().is_empty());

    // 1000 hashes are fine, 1001 are refused.
    let many: Vec<ArtifactHash> = (0..1001)
        .map(|i| ArtifactHash::parse(&format!("{i:064x}")).unwrap())
        .collect();
    assert!(artifacts.existing(&many[..1000]).await.unwrap().is_empty());
    assert!(matches!(
        artifacts.existing(&many).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn mark_orphans_marks_unreferenced_and_clears_referenced(db: &TestDb) {
    let store = db.store();
    let artifacts = store.read().artifacts();
    let author = fixture_user(store).await;

    // a: media original, b: its variant, c: plugin, d: theme, e: import report,
    // f: protected, g: unreferenced.
    for (c, kind) in [
        ('a', ArtifactKind::Media),
        ('b', ArtifactKind::Variant),
        ('c', ArtifactKind::Plugin),
        ('d', ArtifactKind::Theme),
        ('e', ArtifactKind::Import),
        ('f', ArtifactKind::Media),
        ('7', ArtifactKind::Media),
    ] {
        put(db, c, kind).await;
    }
    let mut item = new_post(&author, "photo");
    item.kind = "media".into();
    let item_id = item.id;
    let tx = store.begin().await.unwrap();
    tx.content().insert(&item).await.unwrap();
    tx.commit().await.unwrap();
    store
        .read()
        .media()
        .insert(&Media {
            item_id,
            artifact_hash: hash('a'),
            mime: "image/png".into(),
            bytes: 10,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "p.png".into(),
            variants: json!({"thumb": {"hash": hash('b').as_str(), "w": 1, "h": 1}}),
            sanitized: false,
        })
        .await
        .unwrap();
    store
        .read()
        .plugins()
        .upsert(&InstalledPlugin {
            name: "p".into(),
            version: "1.0.0".into(),
            artifact_hash: hash('c'),
            manifest: json!({"name": "p"}),
            granted: json!({}),
            enabled: true,
            disabled_reason: None,
            installed_at: t(0),
            updated_at: t(0),
        })
        .await
        .unwrap();
    reference_from_theme(db, "th", 'd').await;
    let run = new_run(author.id, "https://wp.example");
    let imports = store.read().imports();
    imports.create_run(&run).await.unwrap();
    imports
        .set_report(run.id, hash('e').as_str())
        .await
        .unwrap();

    let counts = artifacts.mark_orphans(t(100), &[hash('f')]).await.unwrap();
    assert_eq!(
        counts,
        MarkCounts {
            marked: 1,
            unmarked: 0
        }
    );
    for c in ['a', 'b', 'c', 'd', 'e', 'f'] {
        assert_eq!(orphaned_at(db, c).await, None, "{c}");
    }
    assert_eq!(orphaned_at(db, '7').await, Some(t(100)));

    // A second pass at a later time changes nothing and keeps the first mark.
    let counts = artifacts.mark_orphans(t(200), &[hash('f')]).await.unwrap();
    assert_eq!(counts, MarkCounts::default());
    assert_eq!(orphaned_at(db, '7').await, Some(t(100)));

    // Without protection f is marked too; protecting it again clears the mark, and
    // g keeps its original mark.
    let counts = artifacts.mark_orphans(t(300), &[]).await.unwrap();
    assert_eq!(
        counts,
        MarkCounts {
            marked: 1,
            unmarked: 0
        }
    );
    assert_eq!(orphaned_at(db, 'f').await, Some(t(300)));
    let counts = artifacts.mark_orphans(t(400), &[hash('f')]).await.unwrap();
    assert_eq!(
        counts,
        MarkCounts {
            marked: 0,
            unmarked: 1
        }
    );
    assert_eq!(orphaned_at(db, 'f').await, None);
    assert_eq!(orphaned_at(db, '7').await, Some(t(100)));

    // A marked row that gets referenced again loses its mark.
    reference_from_theme(db, "th2", '7').await;
    let counts = artifacts.mark_orphans(t(500), &[hash('f')]).await.unwrap();
    assert_eq!(
        counts,
        MarkCounts {
            marked: 0,
            unmarked: 1
        }
    );
    assert_eq!(orphaned_at(db, '7').await, None);
}

pub async fn insert_if_absent_clears_orphan_mark(db: &TestDb) {
    let artifacts = db.store().read().artifacts();
    put(db, 'a', ArtifactKind::Media).await;
    artifacts.mark_orphans(t(10), &[]).await.unwrap();
    assert_eq!(orphaned_at(db, 'a').await, Some(t(10)));

    let mut again = artifact('a', ArtifactKind::Export);
    again.bytes = 99;
    assert!(!artifacts.insert_if_absent(&again).await.unwrap());
    let row = artifacts.get(&hash('a')).await.unwrap().unwrap();
    assert_eq!(row.orphaned_at, None);
    // The existing row is otherwise untouched.
    assert_eq!(row.kind, ArtifactKind::Media);
    assert_eq!(row.bytes, 10);
}

pub async fn delete_if_orphaned_respects_before_refs_and_protected(db: &TestDb) {
    let artifacts = db.store().read().artifacts();
    // a: young, b: referenced after marking, c: protected, d: never marked, e: deletable.
    for c in ['a', 'b', 'c', 'd', 'e'] {
        put(db, c, ArtifactKind::Theme).await;
    }
    artifacts.mark_orphans(t(100), &[hash('d')]).await.unwrap();
    for c in ['a', 'b', 'c', 'e'] {
        assert_eq!(orphaned_at(db, c).await, Some(t(100)));
    }
    assert_eq!(orphaned_at(db, 'd').await, None);
    reference_from_theme(db, "th", 'b').await;

    let before = t(101);
    // Young: marked at 100, `before` is 100 (strictly earlier required).
    assert!(
        !artifacts
            .delete_if_orphaned(&hash('a'), t(100), &[])
            .await
            .unwrap()
    );
    assert!(
        !artifacts
            .delete_if_orphaned(&hash('b'), before, &[])
            .await
            .unwrap()
    );
    assert!(
        !artifacts
            .delete_if_orphaned(&hash('c'), before, &[hash('c')])
            .await
            .unwrap()
    );
    assert!(
        !artifacts
            .delete_if_orphaned(&hash('d'), before, &[])
            .await
            .unwrap()
    );
    assert!(
        !artifacts
            .delete_if_orphaned(&hash('9'), before, &[])
            .await
            .unwrap()
    );
    for c in ['a', 'b', 'c', 'd', 'e'] {
        assert!(artifacts.get(&hash(c)).await.unwrap().is_some(), "{c}");
    }

    let due = artifacts.orphaned_before(before, 10).await.unwrap();
    assert_eq!(
        due.iter().map(|a| a.hash.clone()).collect::<Vec<_>>(),
        [hash('a'), hash('b'), hash('c'), hash('e')]
    );
    assert!(
        artifacts
            .orphaned_before(t(100), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(artifacts.orphaned_before(before, 2).await.unwrap().len(), 2);

    assert!(
        artifacts
            .delete_if_orphaned(&hash('e'), before, &[hash('c')])
            .await
            .unwrap()
    );
    assert!(artifacts.get(&hash('e')).await.unwrap().is_none());
    assert!(
        !artifacts
            .delete_if_orphaned(&hash('e'), before, &[])
            .await
            .unwrap()
    );
}

/// A marked, old artifact that gets referenced afterwards is kept by
/// `delete_if_orphaned`, for each kind of reference besides themes: a media original,
/// a media variant (`media_variants`, no FK), a plugin and an import report (no FK).
pub async fn delete_if_orphaned_keeps_each_reference_kind(db: &TestDb) {
    let store = db.store();
    let artifacts = store.read().artifacts();
    let author = fixture_user(store).await;
    // a: the media item's original (referenced from the start), b: media original
    // referenced later, c: variant, d: plugin, e: import report.
    put(db, 'a', ArtifactKind::Media).await;
    for (c, kind) in [
        ('b', ArtifactKind::Media),
        ('c', ArtifactKind::Variant),
        ('d', ArtifactKind::Plugin),
        ('e', ArtifactKind::Import),
    ] {
        put(db, c, kind).await;
    }
    let media_for = |item_id, original: char, variants: serde_json::Value| Media {
        item_id,
        artifact_hash: hash(original),
        mime: "image/png".into(),
        bytes: 10,
        width: None,
        height: None,
        alt: String::new(),
        original_name: "p.png".into(),
        variants,
        sanitized: false,
    };
    let mut first = new_post(&author, "first");
    first.kind = "media".into();
    let mut second = new_post(&author, "second");
    second.kind = "media".into();
    let tx = store.begin().await.unwrap();
    tx.content().insert(&first).await.unwrap();
    tx.content().insert(&second).await.unwrap();
    tx.commit().await.unwrap();
    let media = store.read().media();
    media
        .insert(&media_for(first.id, 'a', json!({})))
        .await
        .unwrap();

    artifacts.mark_orphans(t(100), &[]).await.unwrap();
    for c in ['b', 'c', 'd', 'e'] {
        assert_eq!(orphaned_at(db, c).await, Some(t(100)), "{c}");
    }
    assert_eq!(orphaned_at(db, 'a').await, None);

    // Reference them after the mark.
    media
        .insert(&media_for(second.id, 'b', json!({})))
        .await
        .unwrap();
    media
        .update(&media_for(
            first.id,
            'a',
            json!({"thumb": {"hash": hash('c').as_str(), "w": 1, "h": 1}}),
        ))
        .await
        .unwrap();
    store
        .read()
        .plugins()
        .upsert(&InstalledPlugin {
            name: "p".into(),
            version: "1.0.0".into(),
            artifact_hash: hash('d'),
            manifest: json!({"name": "p"}),
            granted: json!({}),
            enabled: true,
            disabled_reason: None,
            installed_at: t(0),
            updated_at: t(0),
        })
        .await
        .unwrap();
    let run = new_run(author.id, "https://wp.example");
    let imports = store.read().imports();
    imports.create_run(&run).await.unwrap();
    imports
        .set_report(run.id, hash('e').as_str())
        .await
        .unwrap();

    for c in ['b', 'c', 'd', 'e'] {
        assert!(
            !artifacts
                .delete_if_orphaned(&hash(c), t(101), &[])
                .await
                .unwrap(),
            "{c} is referenced"
        );
        assert!(artifacts.get(&hash(c)).await.unwrap().is_some(), "{c}");
    }
}

/// `0011_artifact_gc`: the down (not lossy) drops the column (marks go, rows stay) and the up
/// adds it back, unmarked.
pub async fn migration_0011_roundtrip(db: &TestDb) {
    let store = db.store();
    put(db, 'a', ArtifactKind::Media).await;
    store
        .read()
        .artifacts()
        .mark_orphans(t(10), &[])
        .await
        .unwrap();
    assert_eq!(orphaned_at(db, 'a').await, Some(t(10)));

    assert_eq!(
        store.migrate_down(10, true).await.unwrap(),
        vec![14, 13, 12, 11]
    );
    let has_index = |objects: &[String]| {
        objects
            .iter()
            .any(|o| o.contains("artifacts_orphaned_at_idx"))
    };
    assert!(!has_index(&db.user_objects().await));
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![11, 12, 13, 14]
    );
    assert_eq!(orphaned_at(db, 'a').await, None);
    assert!(has_index(&db.user_objects().await));
}
