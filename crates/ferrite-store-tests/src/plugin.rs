//! Contract tests for artifacts, media, plugins, plugin key-value storage and themes.

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
