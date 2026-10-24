//! `SystemService` (M2a-2b P15, P18): system reads need `manage_system` and
//! `system:read`, and node error texts are scrubbed again on read, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_core::{Actor, CoreError, Scopes, SystemService};
use ferrite_domain::{JobStatus, NewJob, NewNode, NewUser, NodeId, Role, UserId, UserStatus};
use ferrite_store::{Compat, Store};
use std::{sync::Arc, time::Duration};

use ferrite_store_tests::{TestDb, backend_tests, backend_tests_spawning};
use serde_json::json;

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    store
        .read()
        .users()
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
    Actor::new(id, role)
}

fn binary() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()
}

fn scopes(names: &[&str]) -> Scopes {
    let names: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    Scopes::from_strs(&names).unwrap()
}

/// Every read refuses an editor (`manage_system`) and an admin token without
/// `system:read`; an admin, or an admin token with `system:read`, reads.
async fn system_reads_need_manage_system(db: &TestDb) {
    let store = db.shared_store();
    let system = SystemService::new(store.clone());
    let editor = actor(store.as_ref(), Role::Editor).await;
    let admin = actor(store.as_ref(), Role::Admin).await;
    let content_token = admin.with_scopes(scopes(&["content:read", "extensions:write"]));
    let system_token = admin.with_scopes(scopes(&["system:read"]));
    let bin = binary();

    for (who, want) in [(&editor, "manage_system"), (&content_token, "system:read")] {
        let forbidden = |r: Result<(), CoreError>| match r {
            Err(CoreError::Forbidden(name)) => assert_eq!(name, want),
            other => panic!("{who:?}: {other:?}"),
        };
        forbidden(system.nodes(who).await.map(drop));
        forbidden(system.audit(who, 50, None).await.map(drop));
        forbidden(system.migration_status(who, &bin).await.map(drop));
        forbidden(system.job_counts(who).await.map(drop));
        forbidden(system.active_theme(who).await.map(drop));
    }
    for who in [&admin, &system_token] {
        system.nodes(who).await.unwrap();
        system.audit(who, 50, None).await.unwrap();
        let status = system.migration_status(who, &bin).await.unwrap();
        assert_eq!(status.verdict, Compat::UpToDate);
        assert!(status.pending.is_empty());
        let counts = system.job_counts(who).await.unwrap();
        assert_eq!(counts.len(), 4, "{counts:?}");
        assert_eq!(system.active_theme(who).await.unwrap(), None);
    }

    // Job counts per status.
    store
        .read()
        .jobs()
        .enqueue(&NewJob::new("test.kind", json!({"v": 1})))
        .await
        .unwrap();
    let counts = system.job_counts(&admin).await.unwrap();
    assert_eq!(counts[&JobStatus::Queued], 1, "{counts:?}");
    assert_eq!(counts[&JobStatus::Dead], 0, "{counts:?}");
}

/// A node row written with unscrubbed errors (an older node in a rolling deploy) is
/// scrubbed by `nodes`; other `meta` members are left as they are.
async fn node_meta_errors_are_scrubbed_on_read(db: &TestDb) {
    let store = db.shared_store();
    let system = SystemService::new(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let id = NodeId::new();
    let leak = "storage backend error: Unable to open file /var/lib/ferrite/media/artifacts/ab/x: \
                Permission denied; GET https://AKIAKEY:hunter2@minio.internal:9000/b failed at 10.0.0.5:9000";
    store
        .read()
        .nodes()
        .register(&NewNode {
            id,
            name: "old-node".into(),
            version: "0.0.9".into(),
            schema_version: 5,
            meta: json!({
                "site_url": "https://example.com",
                "theme": {"name": "blog", "error": leak, "fallback": true},
                "plugins": {
                    "seo": {"status": "failed", "error": leak, "version": "1.0.0"},
                    "ok": {"status": "loaded", "error": null},
                },
                "plugins_error": leak,
                "permalinks_error": leak,
            }),
        })
        .await
        .unwrap();
    let nodes = system.nodes(&admin).await.unwrap();
    let node = nodes.iter().find(|n| n.id == id).unwrap();
    let text = node.meta.to_string();
    for secret in [
        "/var/lib",
        "AKIAKEY",
        "hunter2",
        "minio.internal",
        "10.0.0.5",
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let want = "storage backend error: Unable to open file <path>: Permission denied; GET <url> \
                failed at <addr>";
    assert_eq!(node.meta["theme"]["error"], want);
    assert_eq!(node.meta["plugins"]["seo"]["error"], want);
    assert_eq!(node.meta["plugins_error"], want);
    assert_eq!(node.meta["permalinks_error"], want);
    // Everything else is kept.
    assert_eq!(node.meta["site_url"], "https://example.com");
    assert_eq!(node.meta["theme"]["fallback"], true);
    assert!(node.meta["plugins"]["ok"]["error"].is_null());
    assert_eq!(node.meta["plugins"]["seo"]["version"], "1.0.0");
}

backend_tests!(
    system_reads_need_manage_system,
    node_meta_errors_are_scrubbed_on_read,
);

/// Review M1: two nodes (Postgres `peer()`; SQLite one store, whose writes are
/// serialised) retry the same dead job at once. Exactly one wins; the other sees a
/// queued job (`status`); one `job.retry` audit row.
pub async fn concurrent_retry_one_wins(db: Arc<TestDb>) {
    let store = db.shared_store();
    let other = db.peer().await.unwrap_or_else(|| db.shared_store());
    let admin = actor(store.as_ref(), Role::Admin).await;
    for round in 0..4 {
        let kind = format!("race.k{round}");
        let mut new = NewJob::new(kind.clone(), json!({"v": 1}));
        new.max_attempts = 1;
        let jobs = store.read().jobs();
        let id = jobs.enqueue(&new).await.unwrap().unwrap();
        let lease = jobs
            .claim(
                NodeId::new(),
                std::slice::from_ref(&kind),
                1,
                Duration::from_secs(60),
            )
            .await
            .unwrap()[0]
            .lease()
            .unwrap();
        jobs.fail(&lease, "boom", true).await.unwrap();
        let audits_before = store
            .read()
            .audit()
            .list(200, None)
            .await
            .unwrap()
            .items
            .len();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let spawn = |store: Arc<dyn Store>| {
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                SystemService::new(store).retry_job(&admin, id).await
            })
        };
        let (a, b) = (spawn(store.clone()), spawn(other.clone()));
        let results = [a.await.unwrap(), b.await.unwrap()];
        let wins: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        assert_eq!(wins.len(), 1, "round {round}: {results:?}");
        assert_eq!(wins[0].status, JobStatus::Queued);
        let lost = results.iter().find_map(|r| r.as_ref().err()).unwrap();
        assert!(
            matches!(
                lost,
                CoreError::InvalidField {
                    field: "status",
                    ..
                }
            ),
            "round {round}: {lost:?}"
        );
        let audits = store.read().audit().list(200, None).await.unwrap().items;
        assert_eq!(
            audits.len(),
            audits_before + 1,
            "round {round}: one audit row"
        );
        assert_eq!(audits[0].action, "job.retry");
        assert_eq!(
            audits[0].target_id.as_deref(),
            Some(id.to_string().as_str())
        );
    }
}

backend_tests_spawning!(concurrent_retry_one_wins);
