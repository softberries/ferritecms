//! M6-4 task 8: `HealthRunner` (design 16 §6) on both backends, with fake probes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use ferrite_core::{
    Actor, ContentDraft, ContentService, PermalinkRules,
    quality::{
        HealthRunner, LinkOutcome, LinkProbe, PathProbe, PathStatus, RobotsFetch, external::Pacing,
    },
};
use ferrite_domain::{
    ContentId, ContentStatus, FindingQuery, HealthRunStatus, HealthTrigger, LinkCheck, LinkStatus,
    Menu, MenuItem, MenuItemId, MenuItemKind, NewContent, NewUser, Redirect, RedirectId,
    RedirectOrigin, Revision, RevisionId, Role, StoredFinding, Suppression, Timestamp, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{FailingOptionsStore, TestDb, backend_tests};
use serde_json::{Value, json};

const DAY: i64 = 24 * 3600 * 1_000_000;

async fn actor(store: &dyn Store) -> Actor {
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
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    Actor::new(id, Role::Admin)
}

/// Answers from a map (default `NotFound`) and counts the calls per path.
#[derive(Default)]
struct FakePaths {
    answers: Mutex<HashMap<String, PathStatus>>,
    calls: Mutex<HashMap<String, usize>>,
}

impl FakePaths {
    fn set(&self, path: &str, s: PathStatus) {
        self.answers.lock().unwrap().insert(path.into(), s);
    }
    fn calls(&self, path: &str) -> usize {
        self.calls.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

#[async_trait]
impl PathProbe for FakePaths {
    async fn probe(&self, path: &str) -> PathStatus {
        *self.calls.lock().unwrap().entry(path.into()).or_default() += 1;
        self.answers
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .unwrap_or(PathStatus::NotFound)
    }
}

#[derive(Default)]
struct FakeLinks {
    calls: AtomicUsize,
    robots: AtomicUsize,
    times: Mutex<Vec<Instant>>,
}

#[async_trait]
impl LinkProbe for FakeLinks {
    async fn check(&self, url: &url::Url) -> LinkOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.times.lock().unwrap().push(Instant::now());
        match url.path() {
            "/missing" => LinkOutcome::NotFound(404),
            "/flaky" => LinkOutcome::Failed("timeout".into()),
            _ => LinkOutcome::Ok(200),
        }
    }
    async fn robots(&self, _: &url::Url) -> RobotsFetch {
        self.robots.fetch_add(1, Ordering::SeqCst);
        RobotsFetch::Body(b"User-agent: *\nDisallow: /secret\n".to_vec())
    }
}

fn runner(db: &TestDb, paths: Arc<FakePaths>, links: Option<Arc<FakeLinks>>) -> HealthRunner {
    HealthRunner::new(
        db.shared_store(),
        "https://example.com".into(),
        paths,
        links.map(|l| l as Arc<dyn LinkProbe>),
        "node-1".into(),
    )
    .with_pacing(Pacing {
        concurrency: 4,
        host_gap: Duration::from_millis(20),
        budget: 1000,
        phase: Duration::from_secs(60),
    })
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

fn doc(content: Value) -> Value {
    json!({"type": "doc", "content": content})
}

fn link(text: &str, href: &str) -> Value {
    json!({"type": "paragraph", "content": [
        {"type": "text", "text": text, "marks": [{"type": "link", "attrs": {"href": href}}]}]})
}

fn no_alt() -> Value {
    doc(json!([{"type": "image", "attrs": {"src": "/a.png"}}]))
}

/// A published item of `kind` with `body` (through the service: a revision is written).
async fn published(
    db: &TestDb,
    who: &Actor,
    kind: &str,
    title: &str,
    body: Value,
    parent: Option<ContentId>,
) -> ContentId {
    let svc = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let item = svc
        .create(
            who,
            ContentDraft {
                kind: kind.into(),
                title: title.into(),
                slug: None,
                parent_id: parent,
                excerpt: None,
                body,
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    svc.publish(who, item.id, item.version).await.unwrap().id
}

async fn findings(db: &TestDb) -> Vec<StoredFinding> {
    let q = db.store().read().quality();
    let run = q
        .latest_run(HealthRunStatus::Done)
        .await
        .unwrap()
        .expect("a done run");
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let page = q
            .findings(&FindingQuery {
                run: Some(run.id),
                limit: 200,
                cursor,
                ..Default::default()
            })
            .await
            .unwrap();
        out.extend(page.items);
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => return out,
        }
    }
}

fn rules_of(f: &[StoredFinding]) -> Vec<&str> {
    let mut v: Vec<&str> = f.iter().map(|f| f.finding.rule.as_str()).collect();
    v.sort();
    v
}

async fn run_writes_item_findings_per_page(db: &TestDb) {
    let who = actor(db.store()).await;
    for i in 0..3 {
        published(db, &who, "post", &format!("P{i}"), no_alt(), None).await;
    }
    let svc = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let mut d = ContentDraft {
        kind: "post".into(),
        title: "Draft".into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: no_alt(),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    };
    d.title = "Draft".into();
    svc.create(&who, d).await.unwrap();
    let run = runner(db, Arc::default(), None)
        .run(HealthTrigger::Manual, Some(who.user), far())
        .await
        .unwrap();
    assert_eq!(run.status, HealthRunStatus::Done);
    assert_eq!(run.counters.items, 3);
    assert_eq!(run.totals["rules"]["image-alt-missing"], 3);
    assert_eq!(run.totals["severity"]["error"], 3);
    let f = findings(db).await;
    assert_eq!(rules_of(&f), ["image-alt-missing"; 3]);
    assert_eq!(f[0].finding.item_type.as_deref(), Some("post"));
    assert_eq!(f[0].finding.locale.as_deref(), Some("en"));
    assert_eq!(
        f[0].finding.detail["location"],
        json!({"kind": "body", "path": [0]})
    );
}

async fn internal_broken_link_reported_per_referrer(db: &TestDb) {
    let who = actor(db.store()).await;
    published(
        db,
        &who,
        "post",
        "A",
        doc(json!([link("The guide", "/missing/")])),
        None,
    )
    .await;
    published(
        db,
        &who,
        "post",
        "B",
        doc(json!([link("Our guide", "https://example.com/missing/")])),
        None,
    )
    .await;
    let paths = Arc::new(FakePaths::default());
    runner(db, paths.clone(), None)
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    assert_eq!(paths.calls("/missing/"), 1, "each path probed once");
    assert_eq!(rules_of(&findings(db).await), ["link-internal-broken"; 2]);
}

async fn orphan_rules(db: &TestDb) {
    let who = actor(db.store()).await;
    let front = published(db, &who, "page", "Front", doc(json!([])), None).await;
    let in_menu = published(db, &who, "page", "Menu", doc(json!([])), None).await;
    let child = published(db, &who, "page", "Child", doc(json!([])), Some(in_menu)).await;
    let linked = published(db, &who, "page", "Linked", doc(json!([])), None).await;
    let orphan = published(db, &who, "page", "Orphan", doc(json!([])), None).await;
    let other_locale_menu = published(db, &who, "page", "Elsewhere", doc(json!([])), None).await;
    published(
        db,
        &who,
        "post",
        "Post",
        doc(json!([link("The linked page", "/linked/")])),
        None,
    )
    .await;
    let store = db.store();
    store
        .read()
        .options()
        .set("reading.front_page", &json!(front.to_string()), true)
        .await
        .unwrap();
    let item = |id: ContentId, kind| MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position: 0,
        kind,
        target_id: Some(id.as_uuid()),
        url: None,
        label: "x".into(),
    };
    store
        .read()
        .menus()
        .put(&Menu {
            name: "main".into(),
            label: "Main".into(),
            items: vec![item(in_menu, MenuItemKind::Item)],
            locale: "en".into(),
            translation_of: None,
        })
        .await
        .unwrap();
    store
        .read()
        .menus()
        .put(&Menu {
            name: "main-de".into(),
            label: "Haupt".into(),
            items: vec![item(other_locale_menu, MenuItemKind::Item)],
            locale: "de".into(),
            translation_of: None,
        })
        .await
        .unwrap();
    let paths = Arc::new(FakePaths::default());
    paths.set("/linked/", PathStatus::Ok { item: Some(linked) });
    runner(db, paths, None)
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    let mut orphans: Vec<ContentId> = findings(db)
        .await
        .into_iter()
        .filter(|f| f.finding.rule == "orphan")
        .filter_map(|f| f.finding.item_id)
        .collect();
    orphans.sort();
    let mut want = vec![orphan, other_locale_menu];
    want.sort();
    assert_eq!(orphans, want);
    let _ = child;
}

async fn redirect_chain_loop_and_broken_target(db: &TestDb) {
    let r = |src: &str, to: &str| Redirect {
        id: RedirectId::new(),
        source_path: src.into(),
        target: to.into(),
        code: 301,
        hits: 0,
        origin: RedirectOrigin::Manual,
        created_at: Timestamp::now(),
        last_hit_at: None,
    };
    let repo = db.store().read().redirects();
    for (s, t) in [
        ("/a/", "/b/"),
        ("/b/", "/c/"),
        ("/loop1/", "/loop2/"),
        ("/loop2/", "/loop1"),
        ("/dead/", "/nothing-here/"),
        ("/fine/", "/ok/"),
    ] {
        repo.upsert(&r(s, t)).await.unwrap();
    }
    let paths = Arc::new(FakePaths::default());
    paths.set("/c/", PathStatus::Ok { item: None });
    paths.set("/ok/", PathStatus::Ok { item: None });
    runner(db, paths, None)
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    let f = findings(db).await;
    let of = |rule: &str| -> Vec<String> {
        let mut v: Vec<String> = f
            .iter()
            .filter(|x| x.finding.rule == rule)
            .map(|x| x.finding.key.clone())
            .collect();
        v.sort();
        v
    };
    assert_eq!(of("redirect-chain"), ["/a/"]);
    assert_eq!(of("redirect-loop"), ["/loop1/", "/loop2/"]);
    assert_eq!(of("redirect-target-broken"), ["/dead/"]);
    assert!(f.iter().all(|x| x.finding.redirect_id.is_some()));
}

async fn stale_uses_newest_revision(db: &TestDb) {
    let who = actor(db.store()).await;
    let old = Timestamp::from_micros(Timestamp::now().as_micros() - 400 * DAY).unwrap();
    let mut ids = Vec::new();
    for slug in ["old-a", "old-b"] {
        let id = ContentId::new();
        db.store()
            .read()
            .content()
            .insert(&NewContent {
                id,
                kind: "page".into(),
                status: ContentStatus::Published,
                slug: slug.into(),
                parent_id: None,
                author_id: who.user,
                title: slug.into(),
                excerpt: String::new(),
                body: doc(json!([])),
                body_html: None,
                fields: json!({}),
                menu_order: 0,
                published_at: Some(old),
                scheduled_at: None,
                created_at: Some(old),
                updated_at: Some(Timestamp::now()),
                locale: "en".into(),
                translation_group: None,
            })
            .await
            .unwrap();
        ids.push(id);
    }
    // A fresh revision of the second.
    db.store()
        .read()
        .revisions()
        .insert(&Revision {
            id: RevisionId::new(),
            item_id: ids[1],
            author_id: who.user,
            created_at: Timestamp::now(),
            title: "t".into(),
            excerpt: String::new(),
            body: json!({}),
            fields: json!({}),
            meta: json!({}),
            note: None,
        })
        .await
        .unwrap();
    runner(db, Arc::default(), None)
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    let stale: Vec<ContentId> = findings(db)
        .await
        .into_iter()
        .filter(|f| f.finding.rule == "stale")
        .filter_map(|f| f.finding.item_id)
        .collect();
    assert_eq!(stale, [ids[0]]);
}

async fn external_off_by_default_contacts_nothing(db: &TestDb) {
    let who = actor(db.store()).await;
    published(
        db,
        &who,
        "post",
        "A",
        doc(json!([link("W3C", "https://w3.org/x")])),
        None,
    )
    .await;
    let links = Arc::new(FakeLinks::default());
    let run = runner(db, Arc::default(), Some(links.clone()))
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    assert_eq!(links.calls.load(Ordering::SeqCst), 0);
    assert_eq!(links.robots.load(Ordering::SeqCst), 0);
    assert_eq!(run.counters.urls, 0);
}

async fn external_on(db: &TestDb) {
    db.store()
        .read()
        .options()
        .set("health.external_links", &json!(true), true)
        .await
        .unwrap();
}

async fn external_cache_reuses_ok_for_7_days(db: &TestDb) {
    external_on(db).await;
    let who = actor(db.store()).await;
    published(
        db,
        &who,
        "post",
        "A",
        doc(json!([
            link("W3C", "https://w3.org/ok"),
            link("Secret", "https://w3.org/secret")
        ])),
        None,
    )
    .await;
    let links = Arc::new(FakeLinks::default());
    let r = runner(db, Arc::default(), Some(links.clone()));
    r.run(HealthTrigger::Schedule, None, far()).await.unwrap();
    assert_eq!(
        links.calls.load(Ordering::SeqCst),
        1,
        "robots disallows /secret"
    );
    r.run(HealthTrigger::Schedule, None, far()).await.unwrap();
    // `/ok` is reused; the skipped one is due again but robots still says no.
    assert_eq!(links.calls.load(Ordering::SeqCst), 1);
    let rows = db
        .store()
        .read()
        .quality()
        .link_get(&["https://w3.org/ok".into(), "https://w3.org/secret".into()])
        .await
        .unwrap();
    let by: HashMap<String, LinkCheck> = rows.into_iter().map(|r| (r.url.clone(), r)).collect();
    assert_eq!(by["https://w3.org/ok"].status, LinkStatus::Ok);
    assert_eq!(by["https://w3.org/secret"].status, LinkStatus::Skipped);
    assert_eq!(
        by["https://w3.org/secret"].detail.as_deref(),
        Some("robots")
    );
}

async fn external_broken_after_two_failures_20h_apart(db: &TestDb) {
    external_on(db).await;
    let who = actor(db.store()).await;
    published(
        db,
        &who,
        "post",
        "A",
        doc(json!([link("Flaky", "https://flaky.example/flaky")])),
        None,
    )
    .await;
    let long_ago = Timestamp::from_micros(Timestamp::now().as_micros() - DAY).unwrap();
    db.store()
        .read()
        .quality()
        .link_put(&LinkCheck {
            url: "https://flaky.example/flaky".into(),
            host: "flaky.example".into(),
            status: LinkStatus::Failing,
            http_status: None,
            detail: Some("timeout".into()),
            failures: 1,
            first_failed_at: Some(long_ago),
            checked_at: long_ago,
            last_seen_at: long_ago,
        })
        .await
        .unwrap();
    runner(db, Arc::default(), Some(Arc::new(FakeLinks::default())))
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    assert_eq!(rules_of(&findings(db).await), ["link-external-broken"]);
}

async fn external_404_broken_at_once(db: &TestDb) {
    external_on(db).await;
    let who = actor(db.store()).await;
    published(
        db,
        &who,
        "post",
        "A",
        doc(json!([link("Gone page", "https://w3.org/missing")])),
        None,
    )
    .await;
    runner(db, Arc::default(), Some(Arc::new(FakeLinks::default())))
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    let f = findings(db).await;
    assert_eq!(rules_of(&f), ["link-external-broken"]);
    assert_eq!(f[0].finding.detail["status"], 404);
}

async fn external_budget_and_per_host_spacing(db: &TestDb) {
    external_on(db).await;
    let who = actor(db.store()).await;
    published(
        db,
        &who,
        "post",
        "A",
        doc(json!([
            link("One", "https://w3.org/1"),
            link("Two", "https://w3.org/2"),
            link("Three", "https://w3.org/3")
        ])),
        None,
    )
    .await;
    let links = Arc::new(FakeLinks::default());
    let r = HealthRunner::new(
        db.shared_store(),
        "https://example.com".into(),
        Arc::new(FakePaths::default()),
        Some(links.clone()),
        "n".into(),
    )
    .with_pacing(Pacing {
        concurrency: 4,
        host_gap: Duration::from_millis(100),
        budget: 2,
        phase: Duration::from_secs(60),
    });
    let run = r.run(HealthTrigger::Schedule, None, far()).await.unwrap();
    assert_eq!(links.calls.load(Ordering::SeqCst), 2, "the budget");
    assert_eq!(run.counters.urls, 2);
    let times = links.times.lock().unwrap().clone();
    assert!(times[1].duration_since(times[0]) >= Duration::from_millis(90));
}

async fn findings_cap_sets_truncated(db: &TestDb) {
    let who = actor(db.store()).await;
    for i in 0..5 {
        published(db, &who, "post", &format!("P{i}"), no_alt(), None).await;
    }
    let run = runner(db, Arc::default(), None)
        .with_finding_cap(3)
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    assert!(run.truncated);
    let f = findings(db).await;
    assert_eq!(f.len(), 4);
    assert_eq!(f.last().unwrap().finding.rule, "findings-truncated");
}

async fn old_findings_deleted_on_done_and_event_appended(db: &TestDb) {
    let who = actor(db.store()).await;
    published(db, &who, "post", "A", no_alt(), None).await;
    let r = runner(db, Arc::default(), None);
    let first = r.run(HealthTrigger::Schedule, None, far()).await.unwrap();
    let second = r
        .run(HealthTrigger::Manual, Some(who.user), far())
        .await
        .unwrap();
    let q = db.store().read().quality();
    let old = q
        .findings(&FindingQuery {
            run: Some(first.id),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(old.items.is_empty());
    assert_eq!(findings(db).await.len(), 1);
    let events = db.store().read().events().since(0, 1000).await.unwrap();
    let health: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "health.completed")
        .collect();
    assert_eq!(health.len(), 2);
    assert_eq!(health[1].keys, [format!("run:{}", second.id)]);
}

async fn failed_run_marked_failed(db: &TestDb) {
    let failing: Arc<dyn Store> = Arc::new(FailingOptionsStore::new(
        db.shared_store(),
        "health.external_links",
    ));
    let r = HealthRunner::new(
        failing,
        "https://example.com".into(),
        Arc::new(FakePaths::default()),
        None,
        "n".into(),
    );
    assert!(r.run(HealthTrigger::Schedule, None, far()).await.is_err());
    let run = db
        .store()
        .read()
        .quality()
        .latest_run(HealthRunStatus::Failed)
        .await
        .unwrap()
        .expect("a failed run");
    assert!(run.error.is_some());
    assert!(run.finished_at.is_some());
}

async fn suppressed_findings_excluded_from_totals(db: &TestDb) {
    let who = actor(db.store()).await;
    let a = published(db, &who, "post", "A", no_alt(), None).await;
    published(db, &who, "post", "B", no_alt(), None).await;
    db.store()
        .read()
        .quality()
        .suppress(&Suppression {
            item_id: a,
            rule: "image-alt-missing".into(),
            key: String::new(),
            reason: "decorative".into(),
            user_id: None,
            created_at: Timestamp::now(),
        })
        .await
        .unwrap();
    let run = runner(db, Arc::default(), None)
        .run(HealthTrigger::Schedule, None, far())
        .await
        .unwrap();
    assert_eq!(run.totals["rules"]["image-alt-missing"], 1);
    assert_eq!(
        findings(db).await.len(),
        2,
        "suppressed ones are still stored"
    );
}

backend_tests!(
    run_writes_item_findings_per_page,
    internal_broken_link_reported_per_referrer,
    orphan_rules,
    redirect_chain_loop_and_broken_target,
    stale_uses_newest_revision,
    external_off_by_default_contacts_nothing,
    external_cache_reuses_ok_for_7_days,
    external_broken_after_two_failures_20h_apart,
    external_404_broken_at_once,
    external_budget_and_per_host_spacing,
    findings_cap_sets_truncated,
    old_findings_deleted_on_done_and_event_appended,
    failed_run_marked_failed,
    suppressed_findings_excluded_from_totals,
);
