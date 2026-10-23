//! Hot reload of the plugin registry from the rows (P18, P19) and the shared failure
//! counters with cluster-wide auto-disable (P20).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ferrite_core::ExtensionService;
use ferrite_core::events::PLUGIN_DISABLED;
use ferrite_domain::{ArtifactHash, Timestamp};
use ferrite_http::{
    PluginReloader, PluginState, PluginStatus, ReloadReport, SharedFailures, StoreHostServices,
};
use ferrite_plugin_host::{FilterValue, Head, HostConfig, PluginHost, RequestInfo};
use ferrite_store::Store;
use tokio_util::sync::CancellationToken;

use crate::common::{self, SITE_URL, Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;

/// A reloader for `site`'s store, media and plugin host.
fn reloader(site: &Site) -> PluginReloader {
    PluginReloader::new(site.store.clone(), site.media.clone(), site.host.clone())
}

fn req() -> RequestInfo {
    RequestInfo {
        path: "/".into(),
        locale: "en".into(),
        is_logged_in: false,
        post: None,
    }
}

/// The `name` metas a `render.head` call on `host` produces.
async fn metas(host: &PluginHost) -> Vec<(String, String)> {
    let FilterValue::Head(h) = host
        .apply_filter("render.head", FilterValue::Head(Head::default()), &req())
        .await
    else {
        panic!("expected head");
    };
    h.metas
        .into_iter()
        .map(|m| (m.name.unwrap_or_default(), m.content))
        .collect()
}

async fn has_generator(host: &PluginHost) -> bool {
    metas(host)
        .await
        .iter()
        .any(|(n, c)| n == "generator" && c == "ferrite-hello")
}

fn status<'a>(report: &'a ReloadReport, name: &str) -> &'a PluginStatus {
    &report
        .states
        .get(name)
        .unwrap_or_else(|| panic!("{name} not in {report:?}"))
        .status
}

async fn row_hash(site: &Site, name: &str) -> ArtifactHash {
    let row = site
        .store
        .read()
        .plugins()
        .get(name)
        .await
        .unwrap()
        .unwrap();
    row.artifact_hash
}

async fn reload_keeps_previous_version_on_bad_update(site: Site) -> Site {
    site.install("hello").await;
    let reloader = reloader(&site);
    let report = reloader.reload().await.unwrap();
    let good = row_hash(&site, "hello").await;
    assert_eq!(
        report.states.get("hello"),
        Some(&PluginState {
            version: "0.1.0".into(),
            hash: good.clone(),
            status: PluginStatus::Loaded,
        })
    );
    assert!(has_generator(&site.host).await);

    // A row update to bytes that are not a component: the old version keeps filtering.
    let corrupt = site.put_artifact(b"not a component".to_vec()).await;
    site.row("hello", &plugin_manifest("hello"), corrupt.clone(), true)
        .await;
    let report = reloader.reload().await.unwrap();
    let PluginStatus::Failed(reason) = status(&report, "hello") else {
        panic!("{report:?}");
    };
    assert!(
        reason.contains(good.as_str()),
        "names the kept version: {reason}"
    );
    assert_eq!(report.states["hello"].hash, corrupt);
    assert_eq!(site.host.loaded(), ["hello"]);
    assert!(
        has_generator(&site.host).await,
        "previous version still filters"
    );

    // A blob that does not match its hash: same.
    let other = site.put_artifact(plugin_wasm("clock")).await;
    site.tamper(&other, b"tampered");
    site.row("hello", &plugin_manifest("hello"), other, true)
        .await;
    let report = reloader.reload().await.unwrap();
    let PluginStatus::Failed(reason) = status(&report, "hello") else {
        panic!("{report:?}");
    };
    assert!(reason.contains("does not match its hash"), "{reason}");
    assert!(has_generator(&site.host).await);

    // Back to the good artifact: loaded again (it never left).
    site.row("hello", &plugin_manifest("hello"), good, true)
        .await;
    let report = reloader.reload().await.unwrap();
    assert_eq!(status(&report, "hello"), &PluginStatus::Loaded);

    // A bad first version of a new plugin: reported, not loaded.
    site.row("clock", &plugin_manifest("clock"), corrupt, true)
        .await;
    let report = reloader.reload().await.unwrap();
    assert!(matches!(status(&report, "clock"), PluginStatus::Failed(_)));
    assert_eq!(site.host.loaded(), ["hello"]);
    site
}

async fn reload_drops_disabled_rows(site: Site) -> Site {
    site.install("hello").await;
    site.install("clock").await;
    let reloader = reloader(&site);
    reloader.reload().await.unwrap();
    assert_eq!(site.host.loaded(), ["clock", "hello"]);

    site.store
        .read()
        .plugins()
        .set_enabled("hello", false, Some("admin said so"))
        .await
        .unwrap();
    let report = reloader.reload().await.unwrap();
    assert_eq!(
        status(&report, "hello"),
        &PluginStatus::Disabled("admin said so".into())
    );
    assert_eq!(site.host.loaded(), ["clock"]);
    assert!(!has_generator(&site.host).await);

    site.store.read().plugins().delete("clock").await.unwrap();
    let report = reloader.reload().await.unwrap();
    assert!(!report.states.contains_key("clock"), "{report:?}");
    assert!(site.host.loaded().is_empty());

    // Enabled again: back.
    site.store
        .read()
        .plugins()
        .set_enabled("hello", true, None)
        .await
        .unwrap();
    let report = reloader.reload().await.unwrap();
    assert_eq!(status(&report, "hello"), &PluginStatus::Loaded);
    assert!(has_generator(&site.host).await);
    site
}

async fn reload_reuses_unchanged_plugins(site: Site) -> Site {
    site.install("hello").await;
    site.install("clock").await;
    let prepared = Arc::new(AtomicUsize::new(0));
    let reloader = reloader(&site).with_prepare({
        let prepared = prepared.clone();
        move |host, manifest, wasm| {
            prepared.fetch_add(1, Ordering::SeqCst);
            host.prepare(manifest, wasm)
        }
    });
    let count = || prepared.load(Ordering::SeqCst);
    reloader.reload().await.unwrap();
    assert_eq!(count(), 2);
    let report = reloader.reload().await.unwrap();
    assert_eq!(count(), 2, "unchanged rows are not recompiled");
    assert_eq!(status(&report, "hello"), &PluginStatus::Loaded);
    assert_eq!(status(&report, "clock"), &PluginStatus::Loaded);

    // A new artifact for `clock` (hello's component under clock's manifest).
    let next = site.put_artifact(plugin_wasm("hello")).await;
    site.row("clock", &plugin_manifest("clock"), next, true)
        .await;
    reloader.reload().await.unwrap();
    assert_eq!(count(), 3, "only the changed row is prepared");

    // A host-side disable sticks while the row is unchanged...
    assert!(site.host.disable("hello", "auto: local"));
    let report = reloader.reload().await.unwrap();
    assert_eq!(count(), 3);
    assert_eq!(
        status(&report, "hello"),
        &PluginStatus::Disabled("auto: local".into())
    );
    assert!(!site.host.is_enabled("hello"));
    // ...until the row is enabled again.
    tokio::time::sleep(Duration::from_millis(2)).await;
    ExtensionService::new(site.store.clone())
        .set_plugin_enabled("hello", true)
        .await
        .unwrap();
    let report = reloader.reload().await.unwrap();
    assert_eq!(count(), 4);
    assert_eq!(status(&report, "hello"), &PluginStatus::Loaded);
    assert!(site.host.is_enabled("hello"));
    site
}

/// `plugin.disabled` events, as `(name, reason)`.
async fn disabled_events(store: &dyn Store) -> Vec<(String, String)> {
    store
        .read()
        .events()
        .since(0, 1_000)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == PLUGIN_DISABLED)
        .map(|e| (e.keys[0].clone(), e.keys[1].clone()))
        .collect()
}

async fn wait_disabled(store: &dyn Store, name: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let row = store.read().plugins().get(name).await.unwrap().unwrap();
        if !row.enabled {
            return row.disabled_reason.unwrap_or_default();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name} not disabled in time"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn enable_clears_failure_counters(site: Site) -> Site {
    site.install("flapper").await;
    let reloader = reloader(&site);
    reloader.reload().await.unwrap();
    let shared = Arc::new(SharedFailures::new(3, Duration::from_secs(600)));
    site.host.set_failure_sink(shared.clone());
    let stop = CancellationToken::new();
    let flusher = shared.clone().spawn_flusher(
        site.store.clone(),
        site.host.clone(),
        Duration::from_millis(50),
        stop.clone(),
    );
    for _ in 0..3 {
        metas(&site.host).await;
    }
    let reason = wait_disabled(site.store.as_ref(), "flapper").await;
    assert_eq!(reason, "auto: 3 failures in 10m");
    assert_eq!(
        disabled_events(site.store.as_ref()).await,
        [("flapper".to_owned(), reason.clone())]
    );
    assert_eq!(
        site.host.disabled_reason("flapper").as_deref(),
        Some(reason.as_str()),
        "the transitioning node disables locally at once"
    );
    let failures = site.store.read().plugin_failures();
    let epoch = Timestamp::from_micros(0).unwrap();
    assert_eq!(failures.sum_since("flapper", epoch).await.unwrap(), 3);

    ExtensionService::new(site.store.clone())
        .set_plugin_enabled("flapper", true)
        .await
        .unwrap();
    assert_eq!(failures.sum_since("flapper", epoch).await.unwrap(), 0);
    let row = site
        .store
        .read()
        .plugins()
        .get("flapper")
        .await
        .unwrap()
        .unwrap();
    assert!(row.enabled);
    assert_eq!(row.disabled_reason, None);
    let report = reloader.reload().await.unwrap();
    assert_eq!(status(&report, "flapper"), &PluginStatus::Loaded);
    assert!(site.host.is_enabled("flapper"));

    // One failure after the clear stays under the threshold.
    metas(&site.host).await;
    stop.cancel();
    flusher.await.unwrap();
    assert_eq!(
        failures.sum_since("flapper", epoch).await.unwrap(),
        1,
        "final flush"
    );
    assert!(
        site.store
            .read()
            .plugins()
            .get("flapper")
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    site
}

/// Counts buffered before the row was enabled again (it changed since the reloader
/// loaded it) are dropped at flush: the re-enabled plugin starts with a clean slate.
async fn stale_counts_dropped_after_reenable(site: Site) -> Site {
    site.install("flapper").await;
    let shared = Arc::new(SharedFailures::new(3, Duration::from_secs(600)));
    let reloader = reloader(&site).with_failures(shared.clone());
    reloader.reload().await.unwrap();
    site.host.set_failure_sink(shared.clone());
    let stop = CancellationToken::new();
    let flusher = shared.clone().spawn_flusher(
        site.store.clone(),
        site.host.clone(),
        Duration::from_secs(3600),
        stop.clone(),
    );
    for _ in 0..3 {
        metas(&site.host).await;
    }
    tokio::time::sleep(Duration::from_millis(2)).await;
    ExtensionService::new(site.store.clone())
        .set_plugin_enabled("flapper", true)
        .await
        .unwrap();
    stop.cancel();
    flusher.await.unwrap();
    let epoch = Timestamp::from_micros(0).unwrap();
    assert_eq!(
        site.store
            .read()
            .plugin_failures()
            .sum_since("flapper", epoch)
            .await
            .unwrap(),
        0,
        "stale counts dropped"
    );
    assert!(disabled_events(site.store.as_ref()).await.is_empty());
    let row = site
        .store
        .read()
        .plugins()
        .get("flapper")
        .await
        .unwrap()
        .unwrap();
    assert!(row.enabled);

    // After the reload picks up the new row, failures count again.
    reloader.reload().await.unwrap();
    let stop = CancellationToken::new();
    let flusher = shared.clone().spawn_flusher(
        site.store.clone(),
        site.host.clone(),
        Duration::from_secs(3600),
        stop.clone(),
    );
    metas(&site.host).await;
    stop.cancel();
    flusher.await.unwrap();
    assert_eq!(
        site.store
            .read()
            .plugin_failures()
            .sum_since("flapper", epoch)
            .await
            .unwrap(),
        1
    );
    site
}

/// R12: the plugin is enabled again between the flusher's threshold check and its
/// conditional disable; the stale observation makes `auto_disable` a no-op.
async fn reenable_during_flush_is_not_disabled_again(site: Site) -> Site {
    site.install("flapper").await;
    reloader(&site).reload().await.unwrap();
    let store = site.store.clone();
    let shared = Arc::new(
        SharedFailures::new(3, Duration::from_secs(600)).with_before_disable(move |name| {
            let store = store.clone();
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(2)).await;
                ExtensionService::new(store)
                    .set_plugin_enabled(&name, true)
                    .await
                    .map_err(|e| ferrite_store::StoreError::Database(e.to_string()))
            })
        }),
    );
    site.host.set_failure_sink(shared.clone());
    let stop = CancellationToken::new();
    let flusher = shared.clone().spawn_flusher(
        site.store.clone(),
        site.host.clone(),
        Duration::from_secs(3600),
        stop.clone(),
    );
    for _ in 0..3 {
        metas(&site.host).await;
    }
    stop.cancel();
    flusher.await.unwrap();
    let row = site
        .store
        .read()
        .plugins()
        .get("flapper")
        .await
        .unwrap()
        .unwrap();
    assert!(row.enabled, "not disabled again");
    assert!(disabled_events(site.store.as_ref()).await.is_empty());
    assert!(site.host.is_enabled("flapper"));
    site
}

/// A threshold check that errors is retried on the next flush even without new failures.
async fn failed_check_is_retried_without_new_failures(site: Site) -> Site {
    site.install("flapper").await;
    reloader(&site).reload().await.unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(
        SharedFailures::new(3, Duration::from_secs(600)).with_before_disable({
            let attempts = attempts.clone();
            move |_name| {
                let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
                Box::pin(async move {
                    if first {
                        Err(ferrite_store::StoreError::Database("injected".into()))
                    } else {
                        Ok(())
                    }
                })
            }
        }),
    );
    site.host.set_failure_sink(shared.clone());
    let stop = CancellationToken::new();
    let flusher = shared.clone().spawn_flusher(
        site.store.clone(),
        site.host.clone(),
        Duration::from_millis(50),
        stop.clone(),
    );
    for _ in 0..3 {
        metas(&site.host).await;
    }
    let reason = wait_disabled(site.store.as_ref(), "flapper").await;
    assert_eq!(reason, "auto: 3 failures in 10m");
    stop.cancel();
    flusher.await.unwrap();
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "one failed check, one retry"
    );
    assert_eq!(disabled_events(site.store.as_ref()).await.len(), 1);
    site
}

profile_tests!(
    stale_counts_dropped_after_reenable,
    reenable_during_flush_is_not_disabled_again,
    failed_check_is_retried_without_new_failures,
    reload_keeps_previous_version_on_bad_update,
    reload_drops_disabled_rows,
    reload_reuses_unchanged_plugins,
    enable_clears_failure_counters,
);

/// Postgres: two nodes (hosts on the store and a peer store), `flapper` failing 10 times
/// on each with a shared threshold of 20: the row is disabled once, one event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_failures_disable_cluster_wide() {
    let Some(db) = ferrite_store_tests::TestDb::postgres().await else {
        return;
    };
    let site = Site::new(db, common::s3_media().await).await;
    let peer = site.db.peer().await.expect("peer store");
    let peer_services = Arc::new(StoreHostServices::new(
        peer.clone(),
        site.reader.clone(),
        SITE_URL,
    ));
    let peer_host = Arc::new(PluginHost::new(HostConfig::default(), peer_services).unwrap());
    site.install("flapper").await;
    reloader(&site).reload().await.unwrap();
    PluginReloader::new(peer.clone(), site.media.clone(), peer_host.clone())
        .reload()
        .await
        .unwrap();

    let stop = CancellationToken::new();
    let window = Duration::from_secs(600);
    let mut flushers = Vec::new();
    for (store, host) in [
        (site.store.clone(), site.host.clone()),
        (peer.clone(), peer_host.clone()),
    ] {
        let shared = Arc::new(SharedFailures::new(20, window));
        host.set_failure_sink(shared.clone());
        flushers.push(shared.spawn_flusher(store, host, Duration::from_millis(50), stop.clone()));
    }
    let calls = |host: Arc<PluginHost>| async move {
        for _ in 0..10 {
            metas(&host).await;
        }
    };
    tokio::join!(calls(site.host.clone()), calls(peer_host.clone()));
    // Locally each host saw 10 < 20 failures.
    assert!(site.host.is_enabled("flapper") || peer_host.is_enabled("flapper"));

    let reason = wait_disabled(site.store.as_ref(), "flapper").await;
    assert_eq!(reason, "auto: 20 failures in 10m");
    // Let both flushers run a few more rounds, then stop (final flush).
    tokio::time::sleep(Duration::from_millis(300)).await;
    stop.cancel();
    for f in flushers {
        f.await.unwrap();
    }
    assert_eq!(
        disabled_events(site.store.as_ref()).await,
        [("flapper".to_owned(), reason.clone())],
        "exactly one plugin.disabled"
    );
    let epoch = Timestamp::from_micros(0).unwrap();
    assert_eq!(
        site.store
            .read()
            .plugin_failures()
            .sum_since("flapper", epoch)
            .await
            .unwrap(),
        20
    );
    // Only the node whose conditional disable won disables locally at once; the other
    // one waits for the `plugin.disabled` event (Task 10's reloader).
    assert!(
        site.host.is_enabled("flapper") != peer_host.is_enabled("flapper"),
        "exactly one host disabled flapper locally"
    );
    let local = if site.host.is_enabled("flapper") {
        &peer_host
    } else {
        &site.host
    };
    assert_eq!(
        local.disabled_reason("flapper").as_deref(),
        Some(reason.as_str())
    );
    site.db.teardown().await;
}
