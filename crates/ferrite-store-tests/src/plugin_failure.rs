//! Contract tests for the shared plugin failure counters and the one-transition
//! automatic disable (P20).

use std::sync::Arc;
use std::time::Duration;

use ferrite_domain::{Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, Timestamp};
use ferrite_store::{MAX_PRUNE_BATCH, StoreError};
use serde_json::json;

use crate::TestDb;

const MINUTE: i64 = 60_000_000;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

/// Add, sum by plugin and age, clear one plugin, prune in batches; validation.
pub async fn plugin_failures_add_sum_clear_prune(db: &TestDb) {
    let f = db.store().read().plugin_failures();
    // A whole minute, so every bucket below is a minute start.
    let t0 = Timestamp::now().as_micros() / MINUTE * MINUTE;

    assert_eq!(f.sum_since("a", at(t0)).await.unwrap(), 0, "no rows");
    f.add("a", "render.head", at(t0), 3).await.unwrap();
    f.add("a", "render.head", at(t0), 2).await.unwrap();
    assert_eq!(f.sum_since("a", at(t0)).await.unwrap(), 5, "upsert adds");
    f.add("a", "route GET /x", at(t0 + MINUTE), 4)
        .await
        .unwrap();
    f.add("b", "render.head", at(t0), 7).await.unwrap();
    assert_eq!(f.sum_since("a", at(t0)).await.unwrap(), 9);
    assert_eq!(f.sum_since("a", at(t0 - MINUTE)).await.unwrap(), 9);
    assert_eq!(
        f.sum_since("a", at(t0 + 1)).await.unwrap(),
        4,
        "buckets starting before `since` are not counted"
    );
    assert_eq!(f.sum_since("a", at(t0 + 2 * MINUTE)).await.unwrap(), 0);
    assert_eq!(f.sum_since("b", at(t0)).await.unwrap(), 7);
    // Names compare exactly (binary).
    assert_eq!(f.sum_since("A", at(t0)).await.unwrap(), 0);

    // Sums go past u32.
    f.add("big", "render.head", at(t0), u32::MAX).await.unwrap();
    f.add("big", "render.head", at(t0), u32::MAX).await.unwrap();
    assert_eq!(
        f.sum_since("big", at(t0)).await.unwrap(),
        2 * u64::from(u32::MAX)
    );

    f.clear("a").await.unwrap();
    assert_eq!(f.sum_since("a", at(t0 - MINUTE)).await.unwrap(), 0);
    assert_eq!(
        f.sum_since("b", at(t0)).await.unwrap(),
        7,
        "other plugins kept"
    );
    f.clear("a").await.unwrap();

    // Prune: three old buckets, one current.
    for i in 1..=3 {
        f.add("old", "render.head", at(t0 - i * 60 * MINUTE), 1)
            .await
            .unwrap();
    }
    let cut = at(t0 - 30 * MINUTE);
    for bad in [0, MAX_PRUNE_BATCH + 1] {
        assert!(matches!(
            f.prune(cut, bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(f.prune(cut, 2).await.unwrap(), 2);
    assert_eq!(f.prune(cut, 2).await.unwrap(), 1);
    assert_eq!(f.prune(cut, 2).await.unwrap(), 0);
    assert_eq!(f.sum_since("old", at(0)).await.unwrap(), 0);
    assert_eq!(
        f.sum_since("b", at(t0)).await.unwrap(),
        7,
        "newer buckets kept"
    );

    for r in [
        f.add("a\0b", "h", at(t0), 1).await,
        f.add("a", "h\0", at(t0), 1).await,
        f.add("a", "h", at(t0), 0).await,
        f.sum_since("a\0", at(t0)).await.map(|_| ()),
        f.clear("a\0").await,
    ] {
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{r:?}");
    }

    // Inside a Tx: writes are visible to the Tx, and a validation error does not poison it.
    let tx = db.store().begin().await.unwrap();
    tx.plugin_failures()
        .add("t", "render.head", at(t0), 2)
        .await
        .unwrap();
    assert!(
        tx.plugin_failures()
            .add("t", "render.head", at(t0), 0)
            .await
            .is_err()
    );
    assert_eq!(
        tx.plugin_failures().sum_since("t", at(t0)).await.unwrap(),
        2
    );
    tx.commit().await.unwrap();
    assert_eq!(f.sum_since("t", at(t0)).await.unwrap(), 2);
}

fn row(name: &str, hash: &ArtifactHash) -> InstalledPlugin {
    InstalledPlugin {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: hash.clone(),
        manifest: json!({"plugin": {"name": name}}),
        granted: json!(null),
        enabled: true,
        disabled_reason: None,
        installed_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    }
}

async fn plugin_artifact(store: &dyn ferrite_store::Store) -> ArtifactHash {
    let hash = ArtifactHash::parse(&"c".repeat(64)).unwrap();
    store
        .read()
        .artifacts()
        .insert_if_absent(&Artifact {
            hash: hash.clone(),
            kind: ArtifactKind::Plugin,
            bytes: 1,
            mime: "application/wasm".into(),
            created_at: Timestamp::now(),
        })
        .await
        .unwrap();
    hash
}

/// `auto_disable` changes an enabled row once, and only while `updated_at` is still the
/// observed value (R12): the second call is `false` and the first reason stays; a stale
/// observation, disabled and missing rows are `false`.
pub async fn plugin_auto_disable_is_one_transition(db: &TestDb) {
    let store = db.store();
    let hash = plugin_artifact(store).await;
    let plugins = store.read().plugins();
    let flapper = row("flapper", &hash);
    plugins.upsert(&flapper).await.unwrap();
    let mut off = row("off", &hash);
    off.enabled = false;
    plugins.upsert(&off).await.unwrap();
    let observed = plugins.get("flapper").await.unwrap().unwrap().updated_at;
    tokio::time::sleep(Duration::from_millis(5)).await;

    // A stale observation (the row changed since) is no transition.
    let stale = Timestamp::from_micros(observed.as_micros() - 1).unwrap();
    assert!(
        !plugins
            .auto_disable("flapper", "stale", stale)
            .await
            .unwrap()
    );
    assert!(plugins.get("flapper").await.unwrap().unwrap().enabled);

    assert!(
        plugins
            .auto_disable("flapper", "auto: 20 failures in 10m", observed)
            .await
            .unwrap()
    );
    let got = plugins.get("flapper").await.unwrap().unwrap();
    assert!(!got.enabled);
    assert_eq!(
        got.disabled_reason.as_deref(),
        Some("auto: 20 failures in 10m")
    );
    assert!(got.updated_at > flapper.updated_at);

    for token in [observed, got.updated_at] {
        assert!(
            !plugins
                .auto_disable("flapper", "second", token)
                .await
                .unwrap()
        );
    }
    let again = plugins.get("flapper").await.unwrap().unwrap();
    assert_eq!(
        again.disabled_reason.as_deref(),
        Some("auto: 20 failures in 10m")
    );
    assert_eq!(again.updated_at, got.updated_at, "no write");

    let off_at = plugins.get("off").await.unwrap().unwrap().updated_at;
    assert!(!plugins.auto_disable("off", "x", off_at).await.unwrap());
    assert_eq!(
        plugins.get("off").await.unwrap().unwrap().disabled_reason,
        None
    );
    assert!(
        !plugins
            .auto_disable("missing", "x", observed)
            .await
            .unwrap()
    );
    for r in [
        plugins.auto_disable("a\0", "x", observed).await,
        plugins.auto_disable("flapper", "x\0", observed).await,
    ] {
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{r:?}");
    }

    // Re-enabled since the observation: no transition. Inside a Tx, the transition
    // commits with the Tx.
    plugins.set_enabled("flapper", true, None).await.unwrap();
    assert!(
        !plugins
            .auto_disable("flapper", "old", got.updated_at)
            .await
            .unwrap()
    );
    let fresh = plugins.get("flapper").await.unwrap().unwrap().updated_at;
    let tx = store.begin().await.unwrap();
    assert!(
        tx.plugins()
            .auto_disable("flapper", "in tx", fresh)
            .await
            .unwrap()
    );
    assert!(
        !tx.plugins()
            .auto_disable("flapper", "in tx", fresh)
            .await
            .unwrap()
    );
    tx.rollback().await.unwrap();
    assert!(plugins.get("flapper").await.unwrap().unwrap().enabled);
}

/// Concurrent `auto_disable` calls with the same observation, from two stores (Postgres:
/// two pools, as two nodes; SQLite: the shared store): exactly one transition.
pub async fn plugin_auto_disable_concurrent_single_transition(db: Arc<TestDb>) {
    let store = db.shared_store();
    let peer = db.peer().await.unwrap_or_else(|| store.clone());
    let hash = plugin_artifact(store.as_ref()).await;
    for round in 0..5 {
        let name = format!("race{round}");
        store
            .read()
            .plugins()
            .upsert(&row(&name, &hash))
            .await
            .unwrap();
        let observed = store
            .read()
            .plugins()
            .get(&name)
            .await
            .unwrap()
            .unwrap()
            .updated_at;
        let calls: Vec<_> = (0..8)
            .map(|i| {
                let s = if i % 2 == 0 {
                    store.clone()
                } else {
                    peer.clone()
                };
                let name = name.clone();
                tokio::spawn(async move {
                    s.read()
                        .plugins()
                        .auto_disable(&name, &format!("node {i}"), observed)
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut won = Vec::new();
        for (i, c) in calls.into_iter().enumerate() {
            if c.await.unwrap() {
                won.push(i);
            }
        }
        assert_eq!(won.len(), 1, "round {round}: {won:?}");
        let got = store.read().plugins().get(&name).await.unwrap().unwrap();
        assert!(!got.enabled);
        assert_eq!(
            got.disabled_reason,
            Some(format!("node {}", won[0])),
            "the winner's reason"
        );
    }
}
