//! Action hooks enqueued by core in the write `Tx` (M2a-2 P20): `post.saved`,
//! `post.published`, `user.created`, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, ExtensionService, NewAccount, PermalinkRules,
    UserService, actions,
    jobs::{
        PLUGIN_ACTION_KIND, PLUGIN_ACTION_PRIORITY, PLUGIN_ACTION_VERSION, PUBLISH_DUE_KIND,
        PluginAction,
    },
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, DEFAULT_MAX_ATTEMPTS, InstalledPlugin, Job, NewJob,
    NodeId, Role, Timestamp,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests, fixtures::fixture_user};
use serde_json::{Value, json};

/// Installs plugin `name` subscribed to `hooks` (enabled or not) through core.
async fn install(store: Arc<dyn Store>, name: &str, seed: u8, hooks: &[&str], enabled: bool) {
    let artifact = Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ArtifactKind::Plugin,
        bytes: 1,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
    };
    let hooks: serde_json::Map<String, Value> =
        hooks.iter().map(|h| ((*h).to_owned(), json!(10))).collect();
    let now = Timestamp::now();
    let svc = ExtensionService::new(store);
    svc.install_plugin(
        &artifact,
        &InstalledPlugin {
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
    // A new plugin is installed disabled.
    svc.set_plugin_enabled(name, enabled).await.unwrap();
}

/// The `plugin.action` jobs that are due and not yet claimed, in claim order (the
/// claim takes them, so the next call sees only later ones).
async fn take_actions(store: &dyn Store) -> Vec<Job> {
    store
        .read()
        .jobs()
        .claim(
            NodeId::new(),
            &[PLUGIN_ACTION_KIND.to_owned()],
            100,
            Duration::from_secs(60),
        )
        .await
        .unwrap()
}

fn payload(job: &Job) -> PluginAction {
    serde_json::from_value(job.payload.clone()).unwrap()
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
    }
}

pub async fn actions_enqueued_in_the_write_tx(db: &TestDb) {
    let store = db.shared_store();
    install(
        store.clone(),
        "sub",
        1,
        &["post.saved", "post.published"],
        true,
    )
    .await;
    install(
        store.clone(),
        "off",
        2,
        &["post.saved", "post.published"],
        false,
    )
    .await;
    install(store.clone(), "head-only", 3, &["render.head"], true).await;
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));

    // Create: one `post.saved` for the enabled subscriber only, ids only.
    let item = svc.create(&actor, draft("hello")).await.unwrap();
    let jobs = take_actions(store.as_ref()).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let job = &jobs[0];
    assert_eq!(job.max_attempts, DEFAULT_MAX_ATTEMPTS);
    assert_eq!(job.unique_key, None);
    assert_eq!(job.priority, PLUGIN_ACTION_PRIORITY);
    let id = item.id.to_string();
    assert_eq!(
        job.payload,
        json!({
            "v": 1,
            "plugin": "sub",
            "hook": "post.saved",
            "subject": id,
            "payload": {"post": id},
        })
    );
    assert_eq!(payload(job).v, PLUGIN_ACTION_VERSION);

    // A rolled-back write (version conflict) enqueues nothing.
    let err = svc
        .update(&actor, item.id, item.version + 7, draft("hello"))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "{err:?}");
    let err = svc
        .publish(&actor, item.id, item.version + 7)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "{err:?}");
    assert!(take_actions(store.as_ref()).await.is_empty());

    // An enqueue whose `Tx` rolls back leaves no job, on both backends.
    let tx = store.begin().await.unwrap();
    let n = actions::enqueue(
        tx.as_ref(),
        actions::POST_SAVED,
        Some(id.clone()),
        json!({"post": id}),
    )
    .await
    .unwrap();
    assert_eq!(n, 1);
    tx.rollback().await.unwrap();
    assert!(take_actions(store.as_ref()).await.is_empty());

    // An update: `post.saved` again.
    let updated = svc
        .update(&actor, item.id, item.version, draft("hello"))
        .await
        .unwrap();
    let jobs = take_actions(store.as_ref()).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(payload(&jobs[0]).hook, "post.saved");

    // Publish: one `post.published` (a transition writes no revision, so no `post.saved`).
    svc.publish(&actor, item.id, updated.version).await.unwrap();
    let jobs = take_actions(store.as_ref()).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let p = payload(&jobs[0]);
    assert_eq!(
        (p.plugin.as_str(), p.hook.as_str(), p.subject.as_deref()),
        ("sub", "post.published", Some(id.as_str()))
    );
    assert_eq!(p.payload, json!({"post": id}));

    // The scheduler's publish (`publish_one`) enqueues `post.published` too.
    let later = svc.create(&actor, draft("later")).await.unwrap();
    assert_eq!(take_actions(store.as_ref()).await.len(), 1);
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap();
    svc.schedule(&actor, later.id, later.version, at)
        .await
        .unwrap();
    assert!(take_actions(store.as_ref()).await.is_empty(), "schedule");
    let due = Timestamp::from_micros(at.as_micros() + 1).unwrap();
    assert_eq!(svc.publish_due(due).await.unwrap(), 1);
    let jobs = take_actions(store.as_ref()).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let p = payload(&jobs[0]);
    assert_eq!(p.hook, "post.published");
    assert_eq!(p.subject, Some(later.id.to_string()));
}

pub async fn user_created_enqueued(db: &TestDb) {
    let store = db.shared_store();
    install(store.clone(), "welcome", 4, &["user.created"], true).await;
    install(store.clone(), "muted", 5, &["user.created"], false).await;
    let admin = fixture_user(store.as_ref()).await;
    let users = UserService::new(store.clone());
    let account = |login: &str| NewAccount {
        login: login.into(),
        email: format!("{login}@example.com"),
        display_name: login.into(),
        role: Role::Author,
        password: None,
        must_reset: false,
    };

    let api = users
        .create(&Actor::new(admin.id, Role::Admin), account("api-user"))
        .await
        .unwrap();
    let cli = users.operator().create(account("cli-user")).await.unwrap();
    let mut got: Vec<PluginAction> = take_actions(store.as_ref())
        .await
        .iter()
        .map(payload)
        .collect();
    got.sort_by(|a, b| a.subject.cmp(&b.subject));
    let mut want: Vec<PluginAction> = [api.id.to_string(), cli.id.to_string()]
        .into_iter()
        .map(|id| PluginAction {
            v: PLUGIN_ACTION_VERSION,
            plugin: "welcome".into(),
            hook: "user.created".into(),
            subject: Some(id.clone()),
            payload: json!({"user": id}),
        })
        .collect();
    want.sort_by(|a, b| a.subject.cmp(&b.subject));
    assert_eq!(got, want);

    // A refused create (login taken) enqueues nothing.
    let err = users
        .operator()
        .create(account("cli-user"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::InvalidField { field: "login", .. }),
        "{err:?}"
    );
    assert!(take_actions(store.as_ref()).await.is_empty());
}

/// Review round 1 (m2): action jobs rank below core jobs, so a claim takes a core job
/// first even when it was enqueued later (claims order by priority, then `run_at`).
pub async fn action_jobs_claim_after_core_jobs(db: &TestDb) {
    let store = db.shared_store();
    install(store.clone(), "sub", 6, &["post.saved"], true).await;
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor::new(user.id, Role::Admin);
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    svc.create(&actor, draft("first")).await.unwrap();
    let core = store
        .read()
        .jobs()
        .enqueue(&NewJob::new(PUBLISH_DUE_KIND, json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    let kinds = [PLUGIN_ACTION_KIND.to_owned(), PUBLISH_DUE_KIND.to_owned()];
    let claim = || async {
        store
            .read()
            .jobs()
            .claim(NodeId::new(), &kinds, 1, Duration::from_secs(60))
            .await
            .unwrap()
    };
    let first = claim().await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, core, "{first:?}");
    let second = claim().await;
    assert_eq!(second[0].kind, PLUGIN_ACTION_KIND, "{second:?}");
}

backend_tests!(
    actions_enqueued_in_the_write_tx,
    user_created_enqueued,
    action_jobs_claim_after_core_jobs
);
