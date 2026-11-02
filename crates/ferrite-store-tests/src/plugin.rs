//! Contract tests for artifacts, media, plugins, plugin key-value storage and themes.

use std::sync::Arc;

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, InstalledPlugin, InstalledTheme, Media,
    Timestamp,
};
use ferrite_store::{Store, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

fn hash(c: char) -> ArtifactHash {
    ArtifactHash::parse(&c.to_string().repeat(64)).unwrap()
}

fn artifact(c: char, kind: ArtifactKind) -> Artifact {
    Artifact {
        hash: hash(c),
        kind,
        bytes: 1234,
        mime: "image/png".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    }
}

async fn put_artifact(store: &dyn Store, a: &Artifact) {
    assert!(store.read().artifacts().insert_if_absent(a).await.unwrap());
}

pub async fn artifact_insert_if_absent(db: &TestDb) {
    let artifacts = db.store().read().artifacts();
    let first = artifact('a', ArtifactKind::Media);
    assert!(artifacts.insert_if_absent(&first).await.unwrap());
    assert_eq!(artifacts.get(&hash('a')).await.unwrap().unwrap(), first);

    let mut other = first.clone();
    other.kind = ArtifactKind::Export;
    other.bytes = 1;
    other.mime = "text/plain".into();
    assert!(!artifacts.insert_if_absent(&other).await.unwrap());
    assert_eq!(artifacts.get(&hash('a')).await.unwrap().unwrap(), first);

    assert!(artifacts.get(&hash('b')).await.unwrap().is_none());
    artifacts.delete(&hash('a')).await.unwrap();
    artifacts.delete(&hash('a')).await.unwrap();
    assert!(artifacts.get(&hash('a')).await.unwrap().is_none());
    assert!(matches!(
        artifacts
            .insert_if_absent(&Artifact {
                mime: "a\0b".into(),
                ..first
            })
            .await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn media_insert_get_update_variants(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let mut item = new_post(&author, "photo");
    item.kind = "media".into();
    let item_id = item.id;
    let tx = store.begin().await.unwrap();
    tx.content().insert(&item).await.unwrap();
    tx.commit().await.unwrap();
    put_artifact(store, &artifact('a', ArtifactKind::Media)).await;
    put_artifact(store, &artifact('b', ArtifactKind::Variant)).await;

    let mut media = Media {
        item_id,
        artifact_hash: hash('a'),
        mime: "image/png".into(),
        bytes: 1234,
        width: Some(800),
        height: None,
        alt: "A ☃ photo".into(),
        original_name: "Ünï.png".into(),
        variants: json!({}),
        sanitized: false,
    };
    let repo = store.read().media();
    repo.insert(&media).await.unwrap();
    assert_eq!(repo.get(item_id).await.unwrap().unwrap(), media);
    assert!(repo.get(ContentId::new()).await.unwrap().is_none());
    // One media row per item.
    assert!(matches!(
        repo.insert(&media).await,
        Err(StoreError::UniqueViolation { .. })
    ));

    media.alt = "new alt".into();
    media.mime = "image/webp".into();
    media.bytes = 99;
    media.width = None;
    media.height = Some(600);
    media.variants = json!({"thumb": {"hash": hash('b').as_str(), "w": 150, "h": 100}});
    repo.update(&media).await.unwrap();
    assert_eq!(repo.get(item_id).await.unwrap().unwrap(), media);

    let missing = Media {
        item_id: ContentId::new(),
        ..media.clone()
    };
    assert!(matches!(
        repo.update(&missing).await,
        Err(StoreError::NotFound)
    ));
    let mut bad = media.clone();
    bad.variants = json!({"k": "a\0b"});
    assert!(matches!(
        repo.update(&bad).await,
        Err(StoreError::Invalid(_))
    ));

    // Foreign keys: missing artifact and missing content item.
    let mut item2 = new_post(&author, "photo2");
    item2.kind = "media".into();
    let tx = store.begin().await.unwrap();
    tx.content().insert(&item2).await.unwrap();
    tx.commit().await.unwrap();
    let no_artifact = Media {
        item_id: item2.id,
        artifact_hash: hash('f'),
        ..media.clone()
    };
    assert!(matches!(
        repo.insert(&no_artifact).await,
        Err(StoreError::ForeignKey { .. })
    ));
    let no_item = Media {
        item_id: ContentId::new(),
        ..media
    };
    assert!(matches!(
        repo.insert(&no_item).await,
        Err(StoreError::ForeignKey { .. })
    ));
}

fn plugin(name: &str, c: char) -> InstalledPlugin {
    InstalledPlugin {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: hash(c),
        manifest: json!({"name": name, "caps": ["kv", "http"], "n": 1.5}),
        granted: json!({"kv": true}),
        enabled: true,
        disabled_reason: None,
        installed_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    }
}

/// `find_by_artifact`: media rows whose original or any variant (`variants.<name>.hash`)
/// is the hash; malformed variant values are ignored; `limit` caps the result.
pub async fn media_find_by_artifact(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    for (c, kind) in [
        ('a', ArtifactKind::Media),
        ('b', ArtifactKind::Variant),
        ('c', ArtifactKind::Media),
        ('d', ArtifactKind::Media),
    ] {
        put_artifact(store, &artifact(c, kind)).await;
    }
    let mut ids = Vec::new();
    let rows = [
        (
            'a',
            json!({"thumb": {"hash": hash('b').as_str(), "w": 150}}),
        ),
        ('a', json!({})),
        (
            'c',
            json!({"x": {"hash": hash('a').as_str()}, "bad": 5, "nohash": {"w": 1}}),
        ),
        ('d', json!({"n": {"hash": 7}})),
    ];
    for (i, (orig, variants)) in rows.into_iter().enumerate() {
        let mut item = new_post(&author, &format!("m{i}"));
        item.kind = "media".into();
        let tx = store.begin().await.unwrap();
        tx.content().insert(&item).await.unwrap();
        tx.commit().await.unwrap();
        store
            .read()
            .media()
            .insert(&Media {
                item_id: item.id,
                artifact_hash: hash(orig),
                mime: "image/png".into(),
                bytes: 1,
                width: None,
                height: None,
                alt: String::new(),
                original_name: format!("m{i}.png"),
                variants,
                sanitized: false,
            })
            .await
            .unwrap();
        ids.push(item.id);
    }
    let repo = store.read().media();
    let found = |v: Vec<Media>| {
        let mut got: Vec<ContentId> = v.into_iter().map(|m| m.item_id).collect();
        got.sort();
        got
    };
    let mut want = vec![ids[0], ids[1], ids[2]];
    want.sort();
    assert_eq!(
        found(repo.find_by_artifact(&hash('a'), 10).await.unwrap()),
        want
    );
    assert_eq!(
        found(repo.find_by_artifact(&hash('b'), 10).await.unwrap()),
        [ids[0]]
    );
    assert_eq!(
        found(repo.find_by_artifact(&hash('c'), 10).await.unwrap()),
        [ids[2]]
    );
    assert_eq!(
        found(repo.find_by_artifact(&hash('d'), 10).await.unwrap()),
        [ids[3]]
    );
    assert!(
        repo.find_by_artifact(&hash('e'), 10)
            .await
            .unwrap()
            .is_empty()
    );
    // The rows come back whole.
    let one = repo.find_by_artifact(&hash('b'), 10).await.unwrap();
    assert_eq!(Some(&one[0]), repo.get(ids[0]).await.unwrap().as_ref());
    assert_eq!(repo.find_by_artifact(&hash('a'), 2).await.unwrap().len(), 2);
    assert!(matches!(
        repo.find_by_artifact(&hash('a'), 0).await,
        Err(StoreError::Invalid(_))
    ));

    // Updates rebuild the variant rows: the old variant hash no longer matches.
    let mut m0 = repo.get(ids[0]).await.unwrap().unwrap();
    m0.variants = json!({"small": {"hash": hash('d').as_str()}});
    repo.update(&m0).await.unwrap();
    assert!(
        repo.find_by_artifact(&hash('b'), 10)
            .await
            .unwrap()
            .is_empty()
    );
    let mut want = vec![ids[0], ids[3]];
    want.sort();
    assert_eq!(
        found(repo.find_by_artifact(&hash('d'), 10).await.unwrap()),
        want
    );
    // Inside a transaction the rows move with it: a rollback restores them.
    let tx = store.begin().await.unwrap();
    let mut m0b = m0.clone();
    m0b.variants = json!({});
    tx.media().update(&m0b).await.unwrap();
    assert!(!found(tx.media().find_by_artifact(&hash('d'), 10).await.unwrap()).contains(&ids[0]));
    tx.rollback().await.unwrap();
    assert!(found(repo.find_by_artifact(&hash('d'), 10).await.unwrap()).contains(&ids[0]));

    // Only media and variant files count: a media row naming a plugin artifact is
    // never found.
    put_artifact(store, &artifact('f', ArtifactKind::Plugin)).await;
    let mut item = new_post(&author, "plugin-file");
    item.kind = "media".into();
    let tx = store.begin().await.unwrap();
    tx.content().insert(&item).await.unwrap();
    tx.media()
        .insert(&Media {
            item_id: item.id,
            artifact_hash: hash('f'),
            variants: json!({}),
            ..m0.clone()
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        repo.find_by_artifact(&hash('f'), 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Deleting the item cascades to its media and variant rows.
    store.read().content().delete(ids[0]).await.unwrap();
    assert_eq!(
        found(repo.find_by_artifact(&hash('d'), 10).await.unwrap()),
        [ids[3]]
    );
}

/// `media().get_for_update` reads like `get`; inside a `Tx` the row stays locked until
/// the `Tx` ends. Postgres: a peer's locking read waits for the commit, so two
/// read-modify-writes of the row (alt text, variants) never lose each other's change.
/// SQLite: the `Tx` holds the single writer, so the equivalent is the plain read.
pub async fn media_get_for_update_locks_the_row(db: Arc<TestDb>) {
    let store = db.shared_store();
    let author = fixture_user(store.as_ref()).await;
    let mut item = new_post(&author, "locked-media");
    item.kind = "media".into();
    store.read().content().insert(&item).await.unwrap();
    put_artifact(store.as_ref(), &artifact('a', ArtifactKind::Media)).await;
    let media = Media {
        item_id: item.id,
        artifact_hash: hash('a'),
        mime: "image/png".into(),
        bytes: 1234,
        width: None,
        height: None,
        alt: "before".into(),
        original_name: "a.png".into(),
        variants: json!({}),
        sanitized: false,
    };
    store.read().media().insert(&media).await.unwrap();
    let repo = store.read().media();
    assert_eq!(
        repo.get_for_update(item.id).await.unwrap().as_ref(),
        Some(&media),
        "outside a Tx it is `get`"
    );
    assert!(
        repo.get_for_update(ContentId::new())
            .await
            .unwrap()
            .is_none()
    );

    let tx = store.begin().await.unwrap();
    let mut row = tx.media().get_for_update(item.id).await.unwrap().unwrap();
    assert_eq!(row, media);
    let Some(peer) = db.peer().await else {
        // SQLite: one writer; the Tx's own writes still work.
        row.alt = "in tx".into();
        tx.media().update(&row).await.unwrap();
        tx.rollback().await.unwrap();
        return;
    };
    let id = item.id;
    let write = tokio::spawn(async move {
        let ptx = peer.begin().await.unwrap();
        let mut theirs = ptx.media().get_for_update(id).await.unwrap().unwrap();
        theirs.width = Some(10);
        theirs.height = Some(20);
        ptx.media().update(&theirs).await.unwrap();
        ptx.commit().await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !write.is_finished(),
        "the peer's locking read waits for the row lock"
    );
    row.alt = "in tx".into();
    tx.media().update(&row).await.unwrap();
    tx.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), write)
        .await
        .unwrap()
        .unwrap();
    // The peer read the row after the commit, so neither change is lost.
    let got = store.read().media().get(item.id).await.unwrap().unwrap();
    assert_eq!(got.alt, "in tx");
    assert_eq!((got.width, got.height), (Some(10), Some(20)));
}

/// `media.sanitized` (0006): a row written without it reads `false`; `true` is stored
/// by `insert` and `update` and comes back from every read path.
pub async fn media_sanitized_roundtrip(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    put_artifact(store, &artifact('a', ArtifactKind::Media)).await;
    let mut items = Vec::new();
    for slug in ["svg-raw", "svg-marked"] {
        let mut item = new_post(&author, slug);
        item.kind = "media".into();
        store.read().content().insert(&item).await.unwrap();
        items.push(item.id);
    }
    // A row inserted without the column (as before 0006) gets the default.
    let id = items[0].as_uuid();
    if db.is_sqlite() {
        db.exec_raw(&format!(
            "INSERT INTO media (item_id, artifact_hash, mime, bytes, alt, original_name, \
             variants) VALUES (x'{}', '{}', 'image/svg+xml', 5, '', 'a.svg', '{{}}')",
            id.simple(),
            hash('a').as_str()
        ))
        .await;
    } else {
        db.exec_raw(&format!(
            "INSERT INTO media (item_id, artifact_hash, mime, bytes, alt, original_name, \
             variants) VALUES ('{id}', '{}', 'image/svg+xml', 5, '', 'a.svg', '{{}}')",
            hash('a').as_str()
        ))
        .await;
    }
    let repo = store.read().media();
    let mut raw = repo.get(items[0]).await.unwrap().unwrap();
    assert!(!raw.sanitized);

    // `update` writes the marker; `get`, `get_many` and `find_by_artifact` read it.
    raw.sanitized = true;
    repo.update(&raw).await.unwrap();
    assert!(repo.get(items[0]).await.unwrap().unwrap().sanitized);
    let marked = Media {
        item_id: items[1],
        sanitized: true,
        original_name: "b.svg".into(),
        ..raw.clone()
    };
    repo.insert(&marked).await.unwrap();
    assert_eq!(repo.get(items[1]).await.unwrap().unwrap(), marked);
    // A later update that keeps the marker keeps it.
    let mut alt = marked.clone();
    alt.alt = "described".into();
    repo.update(&alt).await.unwrap();
    assert_eq!(repo.get(items[1]).await.unwrap().unwrap(), alt);
    assert!(
        repo.get_many(&items)
            .await
            .unwrap()
            .iter()
            .all(|m| m.sanitized)
    );
    let found = repo.find_by_artifact(&hash('a'), 10).await.unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|m| m.sanitized), "{found:?}");
    // And it can be cleared again.
    raw.sanitized = false;
    repo.update(&raw).await.unwrap();
    assert!(!repo.get(items[0]).await.unwrap().unwrap().sanitized);
}

/// Migration 0003 backfills `media_variants` from the existing `variants` JSON: after
/// down to 2 (table dropped) and up again, variant lookups work for rows written
/// before.
pub async fn media_variants_backfill(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    put_artifact(store, &artifact('a', ArtifactKind::Media)).await;
    put_artifact(store, &artifact('b', ArtifactKind::Variant)).await;
    let mut item = new_post(&author, "backfilled");
    item.kind = "media".into();
    let tx = store.begin().await.unwrap();
    tx.content().insert(&item).await.unwrap();
    tx.commit().await.unwrap();
    store
        .read()
        .media()
        .insert(&Media {
            item_id: item.id,
            artifact_hash: hash('a'),
            mime: "image/png".into(),
            bytes: 1,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "x.png".into(),
            variants: json!({"thumb": {"hash": hash('b').as_str()}, "junk": [1], "n": {"hash": 1}}),
            sanitized: false,
        })
        .await
        .unwrap();
    assert_eq!(
        store.migrate_down(2, true).await.unwrap(),
        vec![14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3]
    );
    assert!(
        !db.user_objects()
            .await
            .iter()
            .any(|o| o.contains("media_variants"))
    );
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        vec![3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
    );
    let repo = store.read().media();
    let got = repo.find_by_artifact(&hash('b'), 10).await.unwrap();
    assert_eq!(got.iter().map(|m| m.item_id).collect::<Vec<_>>(), [item.id]);
    assert_eq!(
        repo.find_by_artifact(&hash('a'), 10).await.unwrap().len(),
        1
    );
}

/// The lookup's plan uses the artifacts key and both hash indexes, and never scans
/// `media` (ruling R29).
pub async fn media_lookup_uses_indexes(db: &TestDb) {
    let plan = db.explain_media_lookup(hash('a').as_str()).await;
    println!("{plan}");
    for index in ["media_artifact_idx", "media_variants_hash_idx"] {
        assert!(plan.contains(index), "no {index} in plan:\n{plan}");
    }
    if db.is_sqlite() {
        assert!(plan.contains("SEARCH artifacts"), "{plan}");
        assert!(!plan.contains("SCAN media"), "{plan}");
    } else {
        assert!(plan.contains("artifacts_pkey"), "{plan}");
        assert!(!plan.contains("Seq Scan"), "{plan}");
    }
}

pub async fn plugins_upsert_list_enable(db: &TestDb) {
    let store = db.store();
    put_artifact(store, &artifact('a', ArtifactKind::Plugin)).await;
    put_artifact(store, &artifact('b', ArtifactKind::Plugin)).await;
    let plugins = store.read().plugins();

    let zed = plugin("Zed", 'a');
    let alpha = plugin("alpha", 'a');
    let beta = plugin("beta", 'a');
    for p in [&zed, &alpha, &beta] {
        plugins.upsert(p).await.unwrap();
    }
    assert_eq!(plugins.get("Zed").await.unwrap().unwrap(), zed);
    assert!(plugins.get("nope").await.unwrap().is_none());
    assert_eq!(plugins.is_enabled("Zed").await.unwrap(), Some(true));
    assert_eq!(plugins.is_enabled("nope").await.unwrap(), None);
    assert!(matches!(
        plugins.is_enabled("a\0b").await,
        Err(StoreError::Invalid(_))
    ));
    // Binary order: uppercase sorts before lowercase.
    let names: Vec<_> = plugins
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Zed", "alpha", "beta"]);

    let mut newer = zed.clone();
    newer.version = "2.0.0".into();
    newer.artifact_hash = hash('b');
    newer.granted = json!({"kv": false, "x": [1, 2]});
    newer.enabled = false;
    newer.installed_at = Timestamp::now();
    plugins.upsert(&newer).await.unwrap();
    let got = plugins.get("Zed").await.unwrap().unwrap();
    assert_eq!(got.version, "2.0.0");
    assert_eq!(got.artifact_hash, hash('b'));
    assert_eq!(got.granted, newer.granted);
    assert!(!got.enabled);
    // The original install time survives a replace.
    assert_eq!(got.installed_at, zed.installed_at);
    assert_eq!(plugins.list().await.unwrap().len(), 3);

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    plugins
        .set_enabled("beta", false, Some("auto: 20 failures"))
        .await
        .unwrap();
    let got = plugins.get("beta").await.unwrap().unwrap();
    assert!(!got.enabled);
    assert_eq!(got.disabled_reason.as_deref(), Some("auto: 20 failures"));
    assert_eq!(plugins.is_enabled("beta").await.unwrap(), Some(false));
    assert!(got.updated_at > beta.updated_at);
    plugins.set_enabled("beta", true, None).await.unwrap();
    let got = plugins.get("beta").await.unwrap().unwrap();
    assert!(got.enabled);
    assert_eq!(got.disabled_reason, None);
    assert!(matches!(
        plugins.set_enabled("nope", true, None).await,
        Err(StoreError::NotFound)
    ));

    let mut nul = plugin("nul", 'a');
    nul.manifest = json!({"a": "x\0y"});
    assert!(matches!(
        plugins.upsert(&nul).await,
        Err(StoreError::Invalid(_))
    ));
    let mut missing_artifact = plugin("ghost", 'a');
    missing_artifact.artifact_hash = hash('f');
    assert!(matches!(
        plugins.upsert(&missing_artifact).await,
        Err(StoreError::ForeignKey { .. })
    ));

    for r in [
        plugins.set_enabled("a\0b", true, None).await,
        plugins.set_enabled("beta", true, Some("a\0b")).await,
        plugins.delete("a\0b").await,
    ] {
        assert!(matches!(r, Err(StoreError::Invalid(_))));
    }

    plugins.delete("beta").await.unwrap();
    plugins.delete("beta").await.unwrap();
    assert!(plugins.get("beta").await.unwrap().is_none());
    assert_eq!(plugins.list().await.unwrap().len(), 2);
    assert_eq!(plugins.is_enabled("beta").await.unwrap(), None);
}

/// `retained` (M2b-3 P47): the deletion markers `plugin.<name>.deleted_at` whose
/// value is unix micros and whose plugin is not installed, by name in binary order.
/// Settings options, other keys, names with a `.` and non-integer values are not
/// markers.
pub async fn retained_plugin_markers(db: &TestDb) {
    let store = db.store();
    put_artifact(store, &artifact('a', ArtifactKind::Plugin)).await;
    let plugins = store.read().plugins();
    let options = store.read().options();
    assert!(plugins.retained().await.unwrap().is_empty());

    for (key, value) in [
        ("plugin.gone.deleted_at", json!(1_000_000)),
        ("plugin.a-b.deleted_at", json!(3_000_000)),
        ("plugin.a.deleted_at", json!(4_000_000)),
        ("plugin.back.deleted_at", json!(2_000_000)),
        ("plugin.gone.settings.k", json!("v")),
        ("plugin.x.settings.deleted_at", json!(5_000_000)),
        ("plugin.bad.deleted_at", json!("nope")),
        ("plugin..deleted_at", json!(6_000_000)),
        ("plugin.z.deleted_at.more", json!(7_000_000)),
        ("plugins.q.deleted_at", json!(8_000_000)),
    ] {
        options.set(key, &value, false).await.unwrap();
    }
    // Installed again: its marker is not retained data.
    plugins.upsert(&plugin("back", 'a')).await.unwrap();

    let got: Vec<(String, i64)> = plugins
        .retained()
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.name, r.deleted_at.as_micros()))
        .collect();
    assert_eq!(
        got,
        [
            ("a".to_owned(), 4_000_000),
            ("a-b".to_owned(), 3_000_000),
            ("gone".to_owned(), 1_000_000),
        ]
    );

    plugins.delete("back").await.unwrap();
    options.delete("plugin.a.deleted_at").await.unwrap();
    let names: Vec<String> = plugins
        .retained()
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["a-b", "back", "gone"]);
}

/// `action_subscribers`: enabled rows whose manifest `hooks` object has the hook as a
/// key (dotted names matched whole, never as a JSON path), binary name order; read
/// through a `Tx` it sees the `Tx`'s own writes.
pub async fn plugins_action_subscribers(db: &TestDb) {
    let store = db.store();
    put_artifact(store, &artifact('a', ArtifactKind::Plugin)).await;
    let with_hooks = |name: &str, hooks: serde_json::Value, enabled: bool| InstalledPlugin {
        manifest: json!({"plugin": {"name": name}, "hooks": hooks}),
        enabled,
        ..plugin(name, 'a')
    };
    let plugins = store.read().plugins();
    for p in [
        with_hooks("b-sub", json!({"post.saved": 10, "render.head": 5}), true),
        with_hooks("A-sub", json!({"post.saved": 1}), true),
        with_hooks("off", json!({"post.saved": 10}), false),
        // A nested `post` object must not match `post.saved` as a path.
        with_hooks("nested", json!({"post": {"saved": 1}}), true),
        // Not an object: subscribes to nothing.
        with_hooks("array", json!(["post.saved"]), true),
        with_hooks("quoted", json!({"we\"ird.hook": 1}), true),
        plugin("plain", 'a'),
    ] {
        plugins.upsert(&p).await.unwrap();
    }
    assert_eq!(
        plugins.action_subscribers("post.saved").await.unwrap(),
        ["A-sub", "b-sub"]
    );
    assert_eq!(
        plugins.action_subscribers("render.head").await.unwrap(),
        ["b-sub"]
    );
    assert_eq!(
        plugins.action_subscribers("we\"ird.hook").await.unwrap(),
        ["quoted"]
    );
    // `nested` has the key `post` itself (not `post.saved`, checked above).
    assert_eq!(
        plugins.action_subscribers("post").await.unwrap(),
        ["nested"]
    );
    assert!(
        plugins
            .action_subscribers("user.created")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        plugins.action_subscribers("a\0b").await,
        Err(StoreError::Invalid(_))
    ));

    let tx = store.begin().await.unwrap();
    tx.plugins().set_enabled("off", true, None).await.unwrap();
    assert_eq!(
        tx.plugins().action_subscribers("post.saved").await.unwrap(),
        ["A-sub", "b-sub", "off"]
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        plugins.action_subscribers("post.saved").await.unwrap(),
        ["A-sub", "b-sub"]
    );
}

pub async fn plugin_kv_crud_and_usage(db: &TestDb) {
    let kv = db.store().read().plugin_kv();
    assert!(kv.get("p", "k").await.unwrap().is_none());

    let binary = vec![0x00, 0xFF, 0x00, 0x7F, 0xFF];
    kv.set("p", "bin", &binary).await.unwrap();
    kv.set("p", "empty", &[]).await.unwrap();
    assert_eq!(kv.get("p", "bin").await.unwrap().unwrap(), binary);
    assert_eq!(
        kv.get("p", "empty").await.unwrap().unwrap(),
        Vec::<u8>::new()
    );

    kv.set("p", "a/2", b"22").await.unwrap();
    kv.set("p", "a/1", b"1").await.unwrap();
    kv.set("p", "a/3", b"333").await.unwrap();
    kv.set("p", "B", b"x").await.unwrap();
    kv.set("p", "ünï", b"u").await.unwrap();
    kv.set("p", "100%_", b"x").await.unwrap();
    kv.set("p", "100xx", b"x").await.unwrap();
    kv.set("other", "a/9", b"other-plugin").await.unwrap();

    assert_eq!(
        kv.list_keys("p", "a/", 200).await.unwrap(),
        ["a/1", "a/2", "a/3"]
    );
    assert_eq!(kv.list_keys("p", "a/", 2).await.unwrap(), ["a/1", "a/2"]);
    // LIKE wildcards in the prefix are literal.
    assert_eq!(kv.list_keys("p", "100%", 10).await.unwrap(), ["100%_"]);
    // Binary key order, uppercase first, non-ASCII last.
    assert_eq!(
        kv.list_keys("p", "", 200).await.unwrap(),
        [
            "100%_", "100xx", "B", "a/1", "a/2", "a/3", "bin", "empty", "ünï"
        ]
    );
    assert_eq!(kv.list_keys("other", "", 10).await.unwrap(), ["a/9"]);
    assert!(kv.list_keys("none", "", 10).await.unwrap().is_empty());
    for bad in [0, 201] {
        assert!(matches!(
            kv.list_keys("p", "", bad).await,
            Err(StoreError::Invalid(_))
        ));
    }

    // Value lengths: bin 5, empty 0, a/2 2, a/1 1, a/3 3, B 1, ünï 1, 100%_ 1, 100xx 1.
    let expected: u64 = [5, 0, 2, 1, 3, 1, 1, 1, 1].iter().sum();
    assert_eq!(kv.usage("p").await.unwrap(), (expected, 9));
    assert_eq!(kv.usage("other").await.unwrap(), (12, 1));
    assert_eq!(kv.usage("none").await.unwrap(), (0, 0));

    kv.set("p", "bin", b"z").await.unwrap();
    assert_eq!(kv.get("p", "bin").await.unwrap().unwrap(), b"z");
    assert_eq!(kv.usage("p").await.unwrap(), (expected - 4, 9));
    kv.delete("p", "bin").await.unwrap();
    kv.delete("p", "bin").await.unwrap();
    assert!(kv.get("p", "bin").await.unwrap().is_none());
    assert_eq!(kv.usage("p").await.unwrap(), (expected - 5, 8));

    assert!(matches!(
        kv.set("p", "a\0b", b"x").await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        kv.delete("p", "a\0b").await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        kv.delete("p\0", "a").await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn plugin_kv_delete_all_in_batches(db: &TestDb) {
    let kv = db.store().read().plugin_kv();
    for i in 0..25 {
        kv.set("a", &format!("k{i:02}"), b"v").await.unwrap();
    }
    for i in 0..3 {
        kv.set("b", &format!("k{i}"), b"v").await.unwrap();
    }
    for expected in [10, 10, 5, 0] {
        assert_eq!(kv.delete_all("a", 10).await.unwrap(), expected);
    }
    assert_eq!(kv.usage("a").await.unwrap(), (0, 0));
    assert_eq!(kv.usage("b").await.unwrap(), (3, 3));
    for bad in [0, ferrite_store::MAX_PRUNE_BATCH + 1] {
        assert!(matches!(
            kv.delete_all("b", bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert!(matches!(
        kv.delete_all("b\0", 10).await,
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(kv.usage("b").await.unwrap(), (3, 3));
}

fn theme(name: &str) -> InstalledTheme {
    InstalledTheme {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: hash('c'),
        manifest: json!({"name": name}),
        active: false,
        installed_at: Timestamp::now(),
    }
}

pub async fn themes_single_active(db: &TestDb) {
    let store = db.store();
    put_artifact(store, &artifact('c', ArtifactKind::Theme)).await;
    let themes = store.read().themes();
    assert!(themes.active().await.unwrap().is_none());
    let (a, b) = (theme("alpha"), theme("Beta"));
    themes.upsert(&a).await.unwrap();
    themes.upsert(&b).await.unwrap();
    assert_eq!(themes.get("alpha").await.unwrap().unwrap(), a);
    assert!(themes.get("zz").await.unwrap().is_none());
    // Binary order: uppercase sorts before lowercase.
    let names: Vec<_> = themes
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["Beta", "alpha"]);

    themes.activate("alpha").await.unwrap();
    assert_eq!(themes.active().await.unwrap().unwrap().name, "alpha");
    themes.activate("Beta").await.unwrap();
    themes.activate("Beta").await.unwrap();
    assert_eq!(themes.active().await.unwrap().unwrap().name, "Beta");
    let active: Vec<_> = themes
        .list()
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.active)
        .map(|t| t.name)
        .collect();
    assert_eq!(active, ["Beta"]);

    // Unknown theme: NotFound and the previous activation survives.
    assert!(matches!(
        themes.activate("nope").await,
        Err(StoreError::NotFound)
    ));
    assert_eq!(themes.active().await.unwrap().unwrap().name, "Beta");

    // Same inside a caller's transaction: the failure leaves the transaction usable.
    let tx = store.begin().await.unwrap();
    assert!(matches!(
        tx.themes().activate("nope").await,
        Err(StoreError::NotFound)
    ));
    assert_eq!(tx.themes().active().await.unwrap().unwrap().name, "Beta");
    tx.themes().activate("alpha").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(themes.active().await.unwrap().unwrap().name, "alpha");

    // Inserting an active theme while another is active violates the single-active index.
    let mut rival = theme("rival");
    rival.active = true;
    assert!(matches!(
        themes.upsert(&rival).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert!(themes.get("rival").await.unwrap().is_none());
    assert!(matches!(
        themes.activate("a\0b").await,
        Err(StoreError::Invalid(_))
    ));

    // Replacing a theme keeps its active state, whatever `active` says.
    let mut flipped = theme("Beta");
    flipped.active = true;
    themes.upsert(&flipped).await.unwrap();
    assert!(!themes.get("Beta").await.unwrap().unwrap().active);
    assert_eq!(themes.active().await.unwrap().unwrap().name, "alpha");
    let mut a3 = theme("alpha");
    a3.active = false;
    themes.upsert(&a3).await.unwrap();
    assert!(themes.get("alpha").await.unwrap().unwrap().active);

    let mut a2 = theme("alpha");
    a2.active = false;
    a2.version = "2.0.0".into();
    themes.upsert(&a2).await.unwrap();
    let got = themes.get("alpha").await.unwrap().unwrap();
    assert_eq!(got.version, "2.0.0");
    assert!(got.active);
}

pub async fn kv_quota_semantics(db: &TestDb) {
    let kv = db.store().read().plugin_kv();
    // 10 bytes, 3 keys.
    assert!(kv.set_with_quota("p", "a", &[1; 4], 10, 3).await.unwrap());
    assert!(kv.set_with_quota("p", "b", &[2; 4], 10, 3).await.unwrap());
    // 4 + 4 + 4 = 12 > 10: refused, nothing written.
    assert!(!kv.set_with_quota("p", "c", &[3; 4], 10, 3).await.unwrap());
    assert!(kv.get("p", "c").await.unwrap().is_none());
    assert_eq!(kv.usage("p").await.unwrap(), (8, 2));
    // Exactly at the limit is fine.
    assert!(kv.set_with_quota("p", "c", &[3; 2], 10, 3).await.unwrap());
    assert_eq!(kv.usage("p").await.unwrap(), (10, 3));
    // Overwriting counts the new size, not old + new: 4 -> 4 and 4 -> 2 stay in quota.
    assert!(kv.set_with_quota("p", "a", &[9; 4], 10, 3).await.unwrap());
    assert_eq!(kv.get("p", "a").await.unwrap().unwrap(), [9; 4]);
    assert!(kv.set_with_quota("p", "a", &[8; 2], 10, 3).await.unwrap());
    assert_eq!(kv.usage("p").await.unwrap(), (8, 3));
    // Growing an overwrite past the quota is refused and keeps the old value.
    assert!(!kv.set_with_quota("p", "a", &[7; 5], 10, 3).await.unwrap());
    assert_eq!(kv.get("p", "a").await.unwrap().unwrap(), [8; 2]);
    // A new key beyond max_keys is refused, an overwrite of an existing one is not.
    assert!(!kv.set_with_quota("p", "d", &[], 100, 3).await.unwrap());
    assert!(kv.set_with_quota("p", "a", &[], 100, 3).await.unwrap());
    assert_eq!(kv.usage("p").await.unwrap(), (6, 3));
    // Other plugins have their own totals; a zero quota admits nothing.
    assert!(kv.set_with_quota("q", "a", &[0; 10], 10, 1).await.unwrap());
    assert!(!kv.set_with_quota("q", "z", &[], 10, 0).await.unwrap());
    assert!(!kv.set_with_quota("q", "z", &[0], 0, 5).await.unwrap());
    // u64 limits beyond i64 behave as unlimited.
    assert!(
        kv.set_with_quota("q", "big", &[0; 5], u64::MAX, u64::MAX)
            .await
            .unwrap()
    );
    assert!(matches!(
        kv.set_with_quota("a\0b", "k", &[], 10, 10).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        kv.set_with_quota("p", "a\0b", &[], 10, 10).await,
        Err(StoreError::Invalid(_))
    ));

    // Inside a transaction: visible to it, undone by rollback (also the quota use).
    let store = db.store();
    let tx = store.begin().await.unwrap();
    assert!(
        tx.plugin_kv()
            .set_with_quota("t", "k", &[0; 10], 10, 1)
            .await
            .unwrap()
    );
    assert!(
        !tx.plugin_kv()
            .set_with_quota("t", "k2", &[0], 10, 5)
            .await
            .unwrap()
    );
    tx.rollback().await.unwrap();
    assert_eq!(kv.usage("t").await.unwrap(), (0, 0));
    // A refusal does not poison the transaction.
    let tx = store.begin().await.unwrap();
    assert!(
        !tx.plugin_kv()
            .set_with_quota("t", "k", &[0; 11], 10, 1)
            .await
            .unwrap()
    );
    assert!(
        tx.plugin_kv()
            .set_with_quota("t", "k", &[0; 10], 10, 1)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(kv.usage("t").await.unwrap(), (10, 1));
}

/// P2: 8 tasks race to write distinct 100-byte keys into a 1000-byte quota. Exactly ten
/// writes succeed, on every backend, and the stored total never exceeds the quota.
pub async fn kv_quota_never_exceeded(db: Arc<TestDb>) {
    const TASKS: usize = 8;
    const KEYS_PER_TASK: usize = 5;
    let handles: Vec<_> = (0..TASKS)
        .map(|t| {
            let db = db.clone();
            tokio::spawn(async move {
                let mut accepted = Vec::new();
                for i in 0..KEYS_PER_TASK {
                    let key = format!("t{t}-k{i}");
                    let ok = db
                        .store()
                        .read()
                        .plugin_kv()
                        .set_with_quota("race", &key, &[t as u8; 100], 1000, 1000)
                        .await
                        .unwrap();
                    if ok {
                        accepted.push(key);
                    }
                }
                accepted
            })
        })
        .collect();
    let mut accepted = Vec::new();
    for h in handles {
        accepted.extend(h.await.unwrap());
    }
    let kv = db.store().read().plugin_kv();
    assert_eq!(accepted.len(), 10, "{accepted:?}");
    assert_eq!(kv.usage("race").await.unwrap(), (1000, 10));
    let mut stored = kv.list_keys("race", "", 200).await.unwrap();
    stored.sort();
    accepted.sort();
    assert_eq!(stored, accepted);

    // The same through transactions (the lock is held until commit).
    let handles: Vec<_> = (0..TASKS)
        .map(|t| {
            let db = db.clone();
            tokio::spawn(async move {
                let mut n = 0;
                for i in 0..KEYS_PER_TASK {
                    let tx = db.store().begin().await.unwrap();
                    let key = format!("t{t}-k{i}");
                    let ok = tx
                        .plugin_kv()
                        .set_with_quota("race-tx", &key, &[0; 100], 1000, 1000)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                    n += usize::from(ok);
                }
                n
            })
        })
        .collect();
    let mut total = 0;
    for h in handles {
        total += h.await.unwrap();
    }
    assert_eq!(total, 10);
    assert_eq!(kv.usage("race-tx").await.unwrap(), (1000, 10));

    // Overwrites of one key race too: the total is always one 100-byte value.
    let handles: Vec<_> = (0..TASKS)
        .map(|_| {
            let db = db.clone();
            tokio::spawn(async move {
                for _ in 0..10 {
                    let ok = db
                        .store()
                        .read()
                        .plugin_kv()
                        .set_with_quota("race-one", "k", &[1; 100], 150, 1)
                        .await
                        .unwrap();
                    assert!(ok);
                }
            })
        })
        .collect();
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(kv.usage("race-one").await.unwrap(), (100, 1));
}
