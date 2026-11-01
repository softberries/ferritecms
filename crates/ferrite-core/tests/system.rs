//! `SystemService` (M2a-2b P15, P18): system reads need `manage_system` and
//! `system:read`, and node error texts are scrubbed again on read, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_core::{
    Actor, CoreError, Scopes, SystemService,
    mail::{MailQueue, MemoryMailer},
};
use ferrite_domain::{JobStatus, NewJob, NewNode, NewUser, NodeId, Role, UserId, UserStatus};
use ferrite_store::{Compat, Store};
use std::{sync::Arc, time::Duration};

use ferrite_store_tests::{FailingOptionsStore, TestDb, backend_tests, backend_tests_spawning};
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
        forbidden(
            system
                .audit(who, &ferrite_domain::AuditQuery::first(50))
                .await
                .map(drop),
        );
        forbidden(system.migration_status(who, &bin).await.map(drop));
        forbidden(system.job_counts(who).await.map(drop));
        forbidden(system.active_theme(who).await.map(drop));
    }
    for who in [&admin, &system_token] {
        system.nodes(who).await.unwrap();
        system
            .audit(who, &ferrite_domain::AuditQuery::first(50))
            .await
            .unwrap();
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

/// P77: a failed `site.title` read costs the stored title, not the test mail: it is
/// queued with the configured title in its subject.
async fn test_mail_falls_back_when_the_title_read_fails(db: &TestDb) {
    db.store()
        .read()
        .options()
        .set("site.title", &json!("Stored Title"), true)
        .await
        .unwrap();
    let failing: Arc<dyn Store> =
        Arc::new(FailingOptionsStore::new(db.shared_store(), "site.title"));
    let system = SystemService::new(failing);
    let admin = actor(db.store(), Role::Admin).await;
    let memory = Arc::new(MemoryMailer::new());
    let queue = MailQueue::new(memory.clone());
    system
        .send_test_mail(&admin, Some(&queue), "https://example.test", "Test Site")
        .await
        .unwrap();
    let sent = memory.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].subject, "Test e-mail from Test Site");
}

/// Design 12 §5.1: any actor records its own MCP write as `mcp.<tool>` with the
/// client's name; the action cannot leave `mcp.`.
async fn mcp_audit_rows_name_the_client(db: &TestDb) {
    let store = db.shared_store();
    let system = SystemService::new(store.clone());
    let contributor = actor(store.as_ref(), Role::Contributor).await;
    let narrow = contributor.with_scopes(scopes(&["content:read"]));
    system
        .audit_mcp(
            &narrow,
            "content_create",
            ("content", Some("abc".into())),
            "Claude",
            json!({"type": "post"}),
        )
        .await
        .unwrap();
    let rows = store
        .read()
        .audit()
        .list(&ferrite_domain::AuditQuery::first(10))
        .await
        .unwrap()
        .items;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, "mcp.content_create");
    assert_eq!(rows[0].actor_id, Some(contributor.user));
    assert_eq!(rows[0].target_kind, "content");
    assert_eq!(rows[0].target_id.as_deref(), Some("abc"));
    assert_eq!(rows[0].details, json!({"type": "post", "client": "Claude"}));
    for bad in [
        "",
        "user.delete",
        "a.b",
        "Content",
        "x".repeat(65).as_str(),
        "_x",
    ] {
        let r = system
            .audit_mcp(&narrow, bad, ("content", None), "Claude", json!({}))
            .await;
        assert!(matches!(r, Err(CoreError::Invalid(_))), "{bad}: {r:?}");
    }
}

backend_tests!(
    system_reads_need_manage_system,
    node_meta_errors_are_scrubbed_on_read,
    test_mail_falls_back_when_the_title_read_fails,
    mcp_audit_rows_name_the_client,
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
            .list(&ferrite_domain::AuditQuery::first(200))
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
        let audits = store
            .read()
            .audit()
            .list(&ferrite_domain::AuditQuery::first(200))
            .await
            .unwrap()
            .items;
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
