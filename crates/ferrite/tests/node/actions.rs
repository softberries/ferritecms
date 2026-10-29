//! `plugin.action` jobs (M2a-2 P20, P29, F34, F35): on a real node a save runs
//! meta-writer's `post.saved` through the runner; actions for removed or disabled
//! plugins are dropped; at host level (a gated `HostServices`, F34) an action in flight
//! survives a hot swap on the instance it started on (R13), and the handler's outcomes.

use ferrite::jobs::PluginActionHandler;
use ferrite_core::jobs::{PLUGIN_ACTION_VERSION, PluginAction, plugin_action_job};
use ferrite_plugin_host::{
    FilterValue, Head, HostConfig, HostServices, InMemoryServices, KindOk, Manifest, PluginHost,
    Post, RequestInfo, SettingSpec,
};
use tokio::sync::Notify;

use super::*;

/// A plugin row named `name` with `hooks`, enabled or not, through core (no artifact
/// bytes: the handler reads only the row).
async fn plugin_row(store: &Arc<dyn Store>, name: &str, seed: u8, hooks: &[&str], enabled: bool) {
    let artifact = Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ArtifactKind::Plugin,
        bytes: 1,
        mime: PLUGIN_MIME.into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    let hooks: serde_json::Map<String, Value> =
        hooks.iter().map(|h| ((*h).to_owned(), json!(10))).collect();
    let now = Timestamp::now();
    let svc = ExtensionService::new(store.clone());
    svc.install_plugin(
        &artifact,
        &ferrite_domain::InstalledPlugin {
            name: name.into(),
            version: "0.1.0".into(),
            artifact_hash: artifact.hash.clone(),
            manifest: json!({"plugin": {"name": name}, "hooks": hooks}),
            granted: Value::Null,
            enabled,
            disabled_reason: None,
            installed_at: now,
            updated_at: now,
        },
    )
    .await
    .unwrap();
    svc.set_plugin_enabled(name, enabled).await.unwrap();
}

/// Enqueues a `plugin.action` job (due now) and returns it as stored.
async fn action_job(store: &Arc<dyn Store>, plugin: &str, hook: &str, subject: &str) -> Job {
    let job = plugin_action_job(&PluginAction {
        v: PLUGIN_ACTION_VERSION,
        plugin: plugin.into(),
        hook: hook.into(),
        subject: Some(subject.into()),
        payload: json!({"post": subject}),
    });
    let id = store.read().jobs().enqueue(&job).await.unwrap().unwrap();
    store.read().jobs().get(id).await.unwrap().unwrap()
}

fn draft(slug: &str) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
        title: format!("Title {slug}"),
        slug: Some(slug.into()),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

async fn seen(store: &dyn Store, id: ferrite_domain::ContentId) -> Option<Value> {
    store.read().meta().get(id, "seo.seen").await.unwrap()
}

/// A save enqueues `post.saved` in its `Tx`; the node's runner claims it and
/// meta-writer writes `seo.seen = "action"` on the item. The item is published before
/// the plugin is installed: `meta.set` reaches only items the plugin can see (Task 13,
/// so an action cannot annotate a draft).
async fn post_saved_runs_meta_writer(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    };
    let node = site.start(&db, opts).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    let item = svc.create(&actor, draft("acted")).await.unwrap();
    let item = svc.publish(&actor, item.id, item.version).await.unwrap();
    install_plugin_file(
        &store,
        &node.state.media,
        &site.config.plugins,
        &manifest("meta-writer"),
        &test_support::plugin_wasm("meta-writer"),
        true,
    )
    .await
    .unwrap();
    eventually(PLUGIN_LOAD, "meta-writer loaded", || async {
        node.state.plugins.is_enabled("meta-writer")
    })
    .await;
    let item = svc
        .update(&actor, item.id, item.version, draft("acted"))
        .await
        .unwrap();
    eventually(Duration::from_secs(5), "seo.seen written", || {
        let store = store.clone();
        async move { seen(store.as_ref(), item.id).await == Some(json!("action")) }
    })
    .await;
    eventually(Duration::from_secs(2), "action job done", || {
        let store = store.clone();
        async move {
            let c = counts(store.as_ref()).await;
            c[&JobStatus::Done] == 1 && c[&JobStatus::Queued] == 0 && c[&JobStatus::Running] == 0
        }
    })
    .await;
    assert_eq!(counts(store.as_ref()).await[&JobStatus::Dead], 0);
    node.shutdown().await.unwrap();
}

/// Payloads carry ids only and the plugin reads the subject through its own imports,
/// which serve what the public site serves: slow-import's `post.saved` finds a
/// published item but not a draft (its call fails, so the job is retried).
async fn action_sees_only_public_items(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let store = db.shared_store();
    install_plugin_file(
        &store,
        &site.media(),
        &site.config.plugins,
        &manifest("hostile/slow-import"),
        &test_support::plugin_wasm("slow-import"),
        true,
    )
    .await
    .unwrap();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    let public = svc.create(&actor, draft("public")).await.unwrap();
    svc.publish(&actor, public.id, public.version)
        .await
        .unwrap();
    let hidden = svc.create(&actor, draft("hidden")).await.unwrap();
    let jobs = store
        .read()
        .jobs()
        .claim(
            ferrite_domain::NodeId::new(),
            &[ferrite_core::jobs::PLUGIN_ACTION_KIND.to_owned()],
            10,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    // Claimed only to learn the ids: released at once with the attempt refunded.
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    for job in &jobs {
        let lease = job.lease().unwrap();
        let released = store
            .read()
            .jobs()
            .release(&lease, Duration::ZERO, false)
            .await
            .unwrap();
        assert!(released);
    }
    let job_of = |id: ferrite_domain::ContentId| {
        jobs.iter()
            .find(|j| j.payload["subject"] == id.to_string())
            .unwrap()
            .id
    };
    let (public_job, hidden_job) = (job_of(public.id), job_of(hidden.id));

    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    };
    let node = site.start(&db, opts).await;
    eventually(Duration::from_secs(10), "both actions attempted", || {
        let store = store.clone();
        async move {
            let jobs = store.read().jobs();
            let public = jobs.get(public_job).await.unwrap().unwrap();
            let hidden = jobs.get(hidden_job).await.unwrap().unwrap();
            public.status == JobStatus::Done && hidden.last_error.is_some()
        }
    })
    .await;
    let hidden = store.read().jobs().get(hidden_job).await.unwrap().unwrap();
    assert_eq!(hidden.status, JobStatus::Queued, "retried: {hidden:?}");
    assert_eq!(hidden.attempts, 1, "one real attempt: {hidden:?}");
    let error = hidden.last_error.unwrap();
    assert!(error.contains("no post"), "{error}");
    node.shutdown().await.unwrap();
}

/// P29: an action enqueued for a plugin that is disabled afterwards, and one for a
/// plugin that is not installed (deleted), complete on the first attempt without
/// running and without a retry.
async fn action_for_removed_plugin_is_dropped(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let store = db.shared_store();
    install_plugin_file(
        &store,
        &site.media(),
        &site.config.plugins,
        &manifest("meta-writer"),
        &test_support::plugin_wasm("meta-writer"),
        true,
    )
    .await
    .unwrap();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    let item = svc.create(&actor, draft("dropped")).await.unwrap();
    assert_eq!(counts(store.as_ref()).await[&JobStatus::Queued], 1);
    ExtensionService::new(store.clone())
        .set_plugin_enabled("meta-writer", false)
        .await
        .unwrap();
    let gone = action_job(&store, "gone", "post.saved", &item.id.to_string()).await;

    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    };
    let node = site.start(&db, opts).await;
    eventually(Duration::from_secs(5), "both jobs done", || {
        let store = store.clone();
        async move {
            let c = counts(store.as_ref()).await;
            c[&JobStatus::Done] == 2 && c[&JobStatus::Queued] == 0 && c[&JobStatus::Running] == 0
        }
    })
    .await;
    let done = store.read().jobs().get(gone.id).await.unwrap().unwrap();
    assert_eq!(done.attempts, 1, "{done:?}");
    assert_eq!(done.last_error, None, "{done:?}");
    assert_eq!(counts(store.as_ref()).await[&JobStatus::Dead], 0);
    assert_eq!(
        seen(store.as_ref(), item.id).await,
        None,
        "the action did not run"
    );
    node.shutdown().await.unwrap();
}

/// Services whose `get_post` signals `entered`, then waits for `release`; everything
/// else is the empty in-memory services.
#[derive(Default)]
struct GatedServices {
    entered: Notify,
    release: Notify,
    rest: InMemoryServices,
}

#[async_trait]
impl HostServices for GatedServices {
    async fn get_post(&self, id: &str, _kind_ok: &KindOk) -> Option<Post> {
        self.entered.notify_one();
        self.release.notified().await;
        Some(Post {
            id: id.into(),
            kind: "post".into(),
            slug: id.into(),
            title: String::new(),
            excerpt: String::new(),
            status: "publish".into(),
            published: None,
            url: String::new(),
            author: String::new(),
            terms: Vec::new(),
        })
    }
    async fn post_kind(&self, id: &str) -> Option<String> {
        self.rest.post_kind(id).await
    }
    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post> {
        self.rest.get_post_by_slug(kind, slug).await
    }
    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post> {
        self.rest.list_posts(kind, limit, offset).await
    }
    async fn meta_get(&self, post: &str, key: &str) -> Option<String> {
        self.rest.meta_get(post, key).await
    }
    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String> {
        self.rest.meta_set(post, key, value).await
    }
    async fn option_get(&self, key: &str) -> Option<String> {
        self.rest.option_get(key).await
    }
    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>> {
        self.rest.kv_get(plugin, key).await
    }
    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.rest.kv_set(plugin, key, value).await
    }
    async fn kv_delete(&self, plugin: &str, key: &str) {
        self.rest.kv_delete(plugin, key).await;
    }
    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String> {
        self.rest.kv_list(plugin, prefix, limit).await
    }
    async fn setting_get(&self, plugin: &str, key: &str, spec: &SettingSpec) -> Option<String> {
        self.rest.setting_get(plugin, key, spec).await
    }
}

fn slow_manifest() -> Manifest {
    Manifest::parse(&manifest("hostile/slow-import")).unwrap()
}

/// R13 (F34: host level): a reload of `slow-import` while its `post.saved` action is
/// parked in a host import does not affect that call: it completes on the old instance
/// and the job is done. The next call runs the new instance.
async fn action_survives_hot_swap(db: Arc<TestDb>) {
    let store = db.shared_store();
    plugin_row(
        &store,
        "slow-import",
        0x51,
        &["render.head", "post.saved"],
        true,
    )
    .await;
    let services = Arc::new(GatedServices::default());
    let cfg = HostConfig {
        action_deadline: Duration::from_secs(20),
        ..HostConfig::default()
    };
    let host = Arc::new(PluginHost::new(cfg, services.clone()).unwrap());
    host.install(slow_manifest(), &test_support::plugin_wasm("slow-import"))
        .unwrap();
    let handler = Arc::new(PluginActionHandler::new(store.clone(), host.clone(), 2));
    let job = action_job(&store, "slow-import", "post.saved", "p1").await;

    let in_flight = tokio::spawn({
        let handler = handler.clone();
        let job = job.clone();
        async move { handler.run(&job).await }
    });
    tokio::time::timeout(Duration::from_secs(10), services.entered.notified())
        .await
        .unwrap();
    // New version of `slow-import`: hello's component (it handles no action).
    let next = host
        .prepare(slow_manifest(), &test_support::plugin_wasm("hello"))
        .unwrap();
    host.commit(vec![next], &[]);
    services.release.notify_one();
    let old = tokio::time::timeout(Duration::from_secs(10), in_flight)
        .await
        .unwrap()
        .unwrap();
    assert!(old.is_ok(), "finished on the old instance: {old:?}");
    assert!(host.failures().is_empty(), "{:?}", host.failures());

    // The new instance has no `post.saved` action: a failed call, retried and counted.
    let new = handler.run(&job).await;
    assert!(matches!(new, Err(JobError::Retry(_))), "{new:?}");
    assert_eq!(host.failures().len(), 1, "{:?}", host.failures());
}

/// Task 11 carry: at most `slots` (`cluster.workers - 1`) actions run at once on a
/// node, so a slow plugin cannot hold every job worker; the next one is released at
/// once (`Busy`: attempt refunded, R20(3)) instead of waiting, and nothing counts
/// against the plugin.
async fn action_slots_leave_a_worker_free(db: Arc<TestDb>) {
    let store = db.shared_store();
    plugin_row(&store, "slow-import", 0x71, &["post.saved"], true).await;
    let services = Arc::new(GatedServices::default());
    let host = Arc::new(PluginHost::new(HostConfig::default(), services.clone()).unwrap());
    host.install(slow_manifest(), &test_support::plugin_wasm("slow-import"))
        .unwrap();
    let handler = Arc::new(PluginActionHandler::new(store.clone(), host.clone(), 1));
    let job = action_job(&store, "slow-import", "post.saved", "p1").await;
    let in_flight = tokio::spawn({
        let handler = handler.clone();
        let job = job.clone();
        async move { handler.run(&job).await }
    });
    tokio::time::timeout(Duration::from_secs(10), services.entered.notified())
        .await
        .unwrap();
    let start = std::time::Instant::now();
    match handler.run(&job).await {
        Err(JobError::Busy(e)) => assert!(e.contains("busy"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert!(start.elapsed() < Duration::from_millis(500), "did not wait");
    services.release.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(10), in_flight)
        .await
        .unwrap()
        .unwrap();
    assert!(first.is_ok(), "{first:?}");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
}

/// F35 / R20(3): a host with no call slot free within the action deadline answers
/// `Busy`, which the handler passes on as `JobError::Busy` (released with the attempt
/// refunded); it is not a plugin failure.
async fn busy_host_is_retried_without_failure(db: Arc<TestDb>) {
    let store = db.shared_store();
    plugin_row(&store, "slow-import", 0x72, &["post.saved"], true).await;
    let services = Arc::new(GatedServices::default());
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        render_deadline: Duration::from_secs(10),
        action_deadline: Duration::from_millis(200),
        ..HostConfig::default()
    };
    let host = Arc::new(PluginHost::new(cfg, services.clone()).unwrap());
    host.install(slow_manifest(), &test_support::plugin_wasm("slow-import"))
        .unwrap();
    let handler = PluginActionHandler::new(store.clone(), host.clone(), 4);
    let job = action_job(&store, "slow-import", "post.saved", "p1").await;
    // A page's `render.head` call holds the host's only call slot.
    let in_flight = tokio::spawn({
        let host = host.clone();
        async move {
            let req = RequestInfo {
                path: "/".into(),
                locale: "en".into(),
                is_logged_in: false,
                post: Some("p1".into()),
            };
            host.apply_filter_traced("render.head", FilterValue::Head(Head::default()), &req)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), services.entered.notified())
        .await
        .unwrap();
    match handler.run(&job).await {
        Err(JobError::Busy(e)) => assert!(e.contains("busy"), "{e}"),
        other => panic!("{other:?}"),
    }
    services.release.notify_one();
    let page = tokio::time::timeout(Duration::from_secs(10), in_flight)
        .await
        .unwrap()
        .unwrap();
    assert!(!page.volatile, "the filter call completed: {page:?}");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
}

/// The handler's outcomes (P29, F35, review m4): done for success, a hook neither the
/// row nor the loaded plugin handles, and a missing or disabled row; retried for a
/// plugin not loaded on this node yet, a loaded version without a hook the row lists,
/// one disabled on this host only, and a failed call; an unreadable payload is
/// permanent.
async fn action_outcomes(db: Arc<TestDb>) {
    let store = db.shared_store();
    // The row also lists `user.created`, which the loaded version does not handle.
    plugin_row(
        &store,
        "meta-writer",
        0x61,
        &["post.saved", "user.created"],
        true,
    )
    .await;
    plugin_row(&store, "elsewhere", 0x62, &["post.saved"], true).await;
    let services = Arc::new(InMemoryServices::new());
    // A `post`: meta-writer's `content.read` type (P23).
    services.insert_post(Post {
        id: "p1".into(),
        kind: "post".into(),
        slug: "p1".into(),
        title: String::new(),
        excerpt: String::new(),
        status: "publish".into(),
        published: None,
        url: String::new(),
        author: String::new(),
        terms: Vec::new(),
    });
    let host = Arc::new(PluginHost::new(HostConfig::default(), services.clone()).unwrap());
    host.install(
        Manifest::parse(&manifest("meta-writer")).unwrap(),
        &test_support::plugin_wasm("meta-writer"),
    )
    .unwrap();
    let handler = PluginActionHandler::new(store.clone(), host.clone(), 2);
    let run = |plugin: &'static str, hook: &'static str| {
        let store = store.clone();
        let handler = &handler;
        async move {
            let job = action_job(&store, plugin, hook, "p1").await;
            handler.run(&job).await
        }
    };

    assert!(run("meta-writer", "post.saved").await.is_ok());
    assert_eq!(
        services.meta_get("p1", "seo.seen").await.as_deref(),
        Some("\"action\"")
    );
    // Neither the row nor the loaded version handles the hook (an update dropped it):
    // done (F35), nothing called.
    assert!(run("meta-writer", "post.published").await.is_ok());
    // The stored row lists the hook but this node still runs a version without it (the
    // reload has not caught up): retried, not lost (review round 1, m4).
    match run("meta-writer", "user.created").await {
        Err(JobError::Retry(e)) => assert!(e.contains("not reloaded on this node yet"), "{e}"),
        other => panic!("{other:?}"),
    }
    // Not installed: dropped.
    assert!(run("gone", "post.saved").await.is_ok());
    // Enabled, but not (yet) loaded on this node: retried.
    match run("elsewhere", "post.saved").await {
        Err(JobError::Retry(e)) => assert!(e.contains("not loaded on this node yet"), "{e}"),
        other => panic!("{other:?}"),
    }
    // Disabled on this host while the row is still enabled (the shared disable is on
    // its way): retried.
    assert!(host.disable("meta-writer", "auto: test"));
    assert!(matches!(
        run("meta-writer", "post.saved").await,
        Err(JobError::Retry(_))
    ));
    // The row disabled: dropped, whatever the host holds.
    ExtensionService::new(store.clone())
        .set_plugin_enabled("meta-writer", false)
        .await
        .unwrap();
    assert!(run("meta-writer", "post.saved").await.is_ok());
    assert!(host.failures().is_empty(), "{:?}", host.failures());

    // A payload this version cannot read.
    let id = store
        .read()
        .jobs()
        .enqueue(&NewJob::new(
            ferrite_core::jobs::PLUGIN_ACTION_KIND,
            json!({"v": 1, "plugin": "meta-writer"}),
        ))
        .await
        .unwrap()
        .unwrap();
    let job = store.read().jobs().get(id).await.unwrap().unwrap();
    assert!(matches!(
        handler.run(&job).await,
        Err(JobError::Permanent(_))
    ));
}

backend_tests_spawning!(
    post_saved_runs_meta_writer,
    action_slots_leave_a_worker_free,
    busy_host_is_retried_without_failure,
    action_for_removed_plugin_is_dropped,
    action_sees_only_public_items,
    action_survives_hot_swap,
    action_outcomes,
);
