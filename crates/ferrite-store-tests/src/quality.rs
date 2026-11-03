//! Contract tests for `0019_content_health` (M6-4 task 3): the type gate column,
//! suppressions, health runs and findings, and the link cache.

use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, FindingQuery, HealthCounters, HealthRun, HealthRunId,
    HealthRunStatus, HealthTrigger, LinkCheck, LinkStatus, NewFinding, QualityGate, Redirect,
    RedirectId, RedirectOrigin, Revision, RevisionId, Suppression, Timestamp, User,
};
use ferrite_store::{RunEnd, Store, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
    migrations::{down_from, up_from},
};

const SECOND: i64 = 1_000_000;
const DAY: i64 = 24 * 3600 * SECOND;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

async fn post(store: &dyn Store, user: &User, slug: &str, status: ContentStatus) -> ContentItem {
    let mut n = new_post(user, slug);
    n.status = status;
    if status == ContentStatus::Published {
        n.published_at = Some(Timestamp::now());
    }
    if status == ContentStatus::Scheduled {
        n.scheduled_at = Some(at(Timestamp::now().as_micros() + DAY));
    }
    store.read().content().insert(&n).await.unwrap()
}

fn suppression(item: ContentId, rule: &str, key: &str, user: Option<&User>) -> Suppression {
    Suppression {
        item_id: item,
        rule: rule.into(),
        key: key.into(),
        reason: "decorative divider".into(),
        user_id: user.map(|u| u.id),
        created_at: at(1_700_000_000 * SECOND),
    }
}

fn run(status: HealthRunStatus, started: Timestamp) -> HealthRun {
    HealthRun {
        id: HealthRunId::new(),
        status,
        trigger: HealthTrigger::Schedule,
        requested_by: None,
        node: "n1".into(),
        started_at: started,
        finished_at: None,
        counters: HealthCounters::default(),
        totals: json!({}),
        truncated: false,
        error: None,
    }
}

fn finding(rule: &str, severity: &str, category: &str, item: Option<ContentId>) -> NewFinding {
    NewFinding {
        rule: rule.into(),
        severity: severity.into(),
        category: category.into(),
        item_id: item,
        redirect_id: None,
        item_type: item.map(|_| "post".into()),
        locale: item.map(|_| "en".into()),
        key: "/a.png".into(),
        context: None,
        detail: json!({"src": "/a.png"}),
    }
}

fn link(url: &str, seen: Timestamp) -> LinkCheck {
    LinkCheck {
        url: url.into(),
        host: "example.org".into(),
        status: LinkStatus::Ok,
        http_status: Some(200),
        detail: None,
        failures: 0,
        first_failed_at: None,
        checked_at: seen,
        last_seen_at: seen,
    }
}

pub async fn migration_0019_roundtrip(db: &TestDb) {
    const TABLES: [&str; 4] = [
        "quality_suppressions",
        "health_runs",
        "health_findings",
        "link_checks",
    ];
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    let user = fixture_user(store).await;
    let item = post(store, &user, "q", ContentStatus::Draft).await;
    let q = store.read().quality();
    q.suppress(&suppression(item.id, "image-alt-missing", "", Some(&user)))
        .await
        .unwrap();
    let mut ty = store.read().types().get("post").await.unwrap().unwrap();
    ty.quality_gate = QualityGate::Errors;
    store.read().types().upsert(&ty).await.unwrap();

    // Lossy: refused without allow_lossy.
    let err = store.migrate_down(18, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert_eq!(store.migrate_down(18, true).await.unwrap(), down_from(18));
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(18));
    let ty = store.read().types().get("post").await.unwrap().unwrap();
    assert_eq!(ty.quality_gate, QualityGate::Off);
    assert!(
        store
            .read()
            .quality()
            .suppressions_for(&[item.id])
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn quality_gate_defaults_off_and_round_trips(db: &TestDb) {
    let store = db.store();
    let types = store.read().types().list().await.unwrap();
    assert!(!types.is_empty());
    assert!(types.iter().all(|t| t.quality_gate == QualityGate::Off));
    let mut ty = store.read().types().get("page").await.unwrap().unwrap();
    ty.quality_gate = QualityGate::Errors;
    store.read().types().upsert(&ty).await.unwrap();
    let back = store.read().types().get("page").await.unwrap().unwrap();
    assert_eq!(back.quality_gate, QualityGate::Errors);
    let mut new = back.clone();
    new.name = "guide".into();
    new.built_in = false;
    store.read().types().insert(&new).await.unwrap();
    assert_eq!(
        store
            .read()
            .types()
            .get("guide")
            .await
            .unwrap()
            .unwrap()
            .quality_gate,
        QualityGate::Errors
    );
}

pub async fn suppression_upsert_and_delete(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = post(store, &user, "a", ContentStatus::Draft).await;
    let b = post(store, &user, "b", ContentStatus::Draft).await;
    let q = store.read().quality();
    q.suppress(&suppression(
        a.id,
        "image-alt-missing",
        "/x.png",
        Some(&user),
    ))
    .await
    .unwrap();
    let mut again = suppression(a.id, "image-alt-missing", "/x.png", None);
    again.reason = "changed".into();
    q.suppress(&again).await.unwrap();
    q.suppress(&suppression(a.id, "link-text-vague", "", None))
        .await
        .unwrap();
    q.suppress(&suppression(b.id, "heading-empty", "h2:", None))
        .await
        .unwrap();
    let got = q.suppressions_for(&[a.id]).await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].rule, "image-alt-missing");
    assert_eq!(got[0].reason, "changed");
    assert_eq!(got[0].user_id, None);
    assert_eq!(q.suppressions_for(&[a.id, b.id]).await.unwrap().len(), 3);
    assert!(q.suppressions_for(&[]).await.unwrap().is_empty());
    assert!(
        q.unsuppress(a.id, "image-alt-missing", "/x.png")
            .await
            .unwrap()
    );
    assert!(
        !q.unsuppress(a.id, "image-alt-missing", "/x.png")
            .await
            .unwrap()
    );
    assert_eq!(q.suppressions_for(&[a.id]).await.unwrap().len(), 1);
    // NUL is refused.
    assert!(matches!(
        q.suppress(&suppression(a.id, "x\0", "", None)).await,
        Err(StoreError::Invalid(_))
    ));
    let many: Vec<ContentId> = (0..201).map(|_| ContentId::new()).collect();
    assert!(matches!(
        q.suppressions_for(&many).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn suppressions_cascade_with_item(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = post(store, &user, "a", ContentStatus::Draft).await;
    let q = store.read().quality();
    q.suppress(&suppression(a.id, "image-alt-missing", "", Some(&user)))
        .await
        .unwrap();
    store.read().content().delete(a.id).await.unwrap();
    assert!(q.suppressions_for(&[a.id]).await.unwrap().is_empty());
}

pub async fn published_page_skips_drafts_media_and_trash(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let mut want = Vec::new();
    for (slug, status) in [
        ("p1", ContentStatus::Published),
        ("d1", ContentStatus::Draft),
        ("s1", ContentStatus::Scheduled),
        ("t1", ContentStatus::Trash),
        ("p2", ContentStatus::Published),
        ("v1", ContentStatus::Private),
        ("r1", ContentStatus::Pending),
    ] {
        let item = post(store, &user, slug, status).await;
        if matches!(status, ContentStatus::Published | ContentStatus::Scheduled) {
            want.push(item.id);
        }
    }
    let mut media = new_post(&user, "m1");
    media.kind = "media".into();
    media.status = ContentStatus::Published;
    store.read().content().insert(&media).await.unwrap();
    want.sort();
    let q = store.read().quality();
    let first = q.published_page(None, 2).await.unwrap();
    assert_eq!(
        first.iter().map(|i| i.id).collect::<Vec<_>>(),
        want[..2].to_vec()
    );
    let rest = q.published_page(Some(first[1].id), 200).await.unwrap();
    assert_eq!(
        rest.iter().map(|i| i.id).collect::<Vec<_>>(),
        want[2..].to_vec()
    );
    assert!(matches!(
        q.published_page(None, 0).await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn last_revision_at_groups_per_item(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = post(store, &user, "a", ContentStatus::Published).await;
    let b = post(store, &user, "b", ContentStatus::Published).await;
    let c = post(store, &user, "c", ContentStatus::Published).await;
    for (item, micros) in [(a.id, 10), (a.id, 30), (a.id, 20), (b.id, 5)] {
        store
            .read()
            .revisions()
            .insert(&Revision {
                id: RevisionId::new(),
                item_id: item,
                author_id: user.id,
                created_at: at(1_700_000_000 * SECOND + micros * SECOND),
                title: "t".into(),
                excerpt: String::new(),
                body: json!({}),
                fields: json!({}),
                meta: json!({}),
                note: None,
            })
            .await
            .unwrap();
    }
    let mut got = store
        .read()
        .quality()
        .last_revision_at(&[a.id, b.id, c.id])
        .await
        .unwrap();
    got.sort();
    let mut want = vec![
        (a.id, at(1_700_000_000 * SECOND + 30 * SECOND)),
        (b.id, at(1_700_000_000 * SECOND + 5 * SECOND)),
    ];
    want.sort();
    assert_eq!(got, want);
}

pub async fn findings_page_filters_and_cursor(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = post(store, &user, "a", ContentStatus::Published).await;
    let q = store.read().quality();
    let r = run(HealthRunStatus::Running, Timestamp::now());
    q.insert_run(&r).await.unwrap();
    let mut batch = Vec::new();
    for i in 0..5 {
        batch.push(finding(
            "image-alt-missing",
            "error",
            "accessibility",
            Some(a.id),
        ));
        let mut stale = finding("stale", "notice", "freshness", Some(a.id));
        stale.locale = Some(if i % 2 == 0 { "en" } else { "de" }.into());
        batch.push(stale);
    }
    let mut site = finding("redirect-loop", "error", "site", None);
    site.context = Some("/a/ → /b/ → /a/".into());
    batch.push(site);
    q.insert_findings(r.id, 0, &batch).await.unwrap();
    // The page and its cursor.
    let mut query = FindingQuery {
        run: Some(r.id),
        limit: 4,
        ..Default::default()
    };
    let page = q.findings(&query).await.unwrap();
    assert_eq!(
        page.items.iter().map(|f| f.seq).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    query.cursor = page.next_cursor;
    let page = q.findings(&query).await.unwrap();
    assert_eq!(page.items[0].seq, 4);
    query.cursor = page.next_cursor;
    let page = q.findings(&query).await.unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.next_cursor, None);
    assert_eq!(
        page.items[2].finding.context.as_deref(),
        Some("/a/ → /b/ → /a/")
    );
    // Filters.
    let only = |query: FindingQuery| async move { q.findings(&query).await.unwrap().items.len() };
    let base = FindingQuery {
        run: Some(r.id),
        limit: 200,
        ..Default::default()
    };
    assert_eq!(
        only(FindingQuery {
            category: Some("freshness".into()),
            ..base.clone()
        })
        .await,
        5
    );
    assert_eq!(
        only(FindingQuery {
            severity: Some("error".into()),
            ..base.clone()
        })
        .await,
        6
    );
    assert_eq!(
        only(FindingQuery {
            rule: Some("stale".into()),
            locale: Some("de".into()),
            ..base.clone()
        })
        .await,
        2
    );
    assert_eq!(
        only(FindingQuery {
            item_type: Some("post".into()),
            ..base.clone()
        })
        .await,
        10
    );
    // A cursor of another run, or none at all, is refused.
    let other = run(HealthRunStatus::Done, Timestamp::now());
    q.insert_run(&other).await.unwrap();
    let cursor = q
        .findings(&FindingQuery {
            limit: 1,
            ..base.clone()
        })
        .await
        .unwrap()
        .next_cursor;
    assert!(matches!(
        q.findings(&FindingQuery {
            run: Some(other.id),
            cursor,
            ..base.clone()
        })
        .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        q.findings(&FindingQuery {
            limit: 10,
            ..Default::default()
        })
        .await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn findings_cascade_with_run_and_item(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = post(store, &user, "a", ContentStatus::Published).await;
    let b = post(store, &user, "b", ContentStatus::Published).await;
    let redirect = store
        .read()
        .redirects()
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: "/old/".into(),
            target: "/old/".into(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::Manual,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();
    let q = store.read().quality();
    let r = run(HealthRunStatus::Done, Timestamp::now());
    q.insert_run(&r).await.unwrap();
    let mut on_redirect = finding("redirect-loop", "error", "site", None);
    on_redirect.redirect_id = Some(redirect.id);
    q.insert_findings(
        r.id,
        0,
        &[
            finding("stale", "notice", "freshness", Some(a.id)),
            finding("stale", "notice", "freshness", Some(b.id)),
            on_redirect,
        ],
    )
    .await
    .unwrap();
    let all = || FindingQuery {
        run: Some(r.id),
        limit: 200,
        ..Default::default()
    };
    store.read().content().delete(a.id).await.unwrap();
    store.read().redirects().delete(redirect.id).await.unwrap();
    let left = q.findings(&all()).await.unwrap().items;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].finding.item_id, Some(b.id));
    // The run goes with its findings.
    q.prune_runs(at(Timestamp::now().as_micros() + DAY), None)
        .await
        .unwrap();
    assert_eq!(q.get_run(r.id).await.unwrap(), None);
    assert!(q.findings(&all()).await.unwrap().items.is_empty());
}

pub async fn delete_findings_except_keeps_one_run(db: &TestDb) {
    let store = db.store();
    let q = store.read().quality();
    let old = run(HealthRunStatus::Done, at(1_700_000_000 * SECOND));
    let new = run(HealthRunStatus::Done, at(1_700_000_000 * SECOND + DAY));
    for r in [&old, &new] {
        q.insert_run(r).await.unwrap();
        q.insert_findings(r.id, 0, &[finding("redirect-loop", "error", "site", None)])
            .await
            .unwrap();
    }
    assert_eq!(q.delete_findings_except(new.id).await.unwrap(), 1);
    let page = |id| FindingQuery {
        run: Some(id),
        limit: 10,
        ..Default::default()
    };
    assert!(q.findings(&page(old.id)).await.unwrap().items.is_empty());
    assert_eq!(q.findings(&page(new.id)).await.unwrap().items.len(), 1);
    // Prune keeps the run named, older ones go.
    assert_eq!(
        q.prune_runs(at(1_700_000_000 * SECOND + 2 * DAY), Some(new.id))
            .await
            .unwrap(),
        1
    );
    assert!(q.get_run(old.id).await.unwrap().is_none());
    assert!(q.get_run(new.id).await.unwrap().is_some());
}

pub async fn abandon_running_marks_old_runs(db: &TestDb) {
    let store = db.store();
    let q = store.read().quality();
    let a = run(HealthRunStatus::Running, at(1_700_000_000 * SECOND));
    let b = run(HealthRunStatus::Done, at(1_700_000_000 * SECOND + SECOND));
    q.insert_run(&a).await.unwrap();
    q.insert_run(&b).await.unwrap();
    assert_eq!(
        q.latest_run(HealthRunStatus::Running)
            .await
            .unwrap()
            .map(|r| r.id),
        Some(a.id)
    );
    let now = at(1_700_000_000 * SECOND + 2 * SECOND);
    assert_eq!(q.abandon_running(now).await.unwrap(), 1);
    let got = q.get_run(a.id).await.unwrap().unwrap();
    assert_eq!(got.status, HealthRunStatus::Abandoned);
    assert_eq!(got.finished_at, Some(now));
    assert_eq!(q.latest_run(HealthRunStatus::Running).await.unwrap(), None);
    // Finishing writes every counter.
    let c = run(
        HealthRunStatus::Running,
        at(1_700_000_000 * SECOND + 3 * SECOND),
    );
    q.insert_run(&c).await.unwrap();
    let end = RunEnd {
        status: HealthRunStatus::Done,
        finished_at: at(1_700_000_000 * SECOND + 4 * SECOND),
        counters: HealthCounters {
            items: 3,
            paths: 2,
            urls: 1,
        },
        totals: json!({"rules": {"stale": 1}, "severity": {"notice": 1}}),
        truncated: true,
        error: None,
    };
    q.finish_run(c.id, &end).await.unwrap();
    let got = q.latest_run(HealthRunStatus::Done).await.unwrap().unwrap();
    assert_eq!(got.id, c.id);
    assert_eq!(got.counters, end.counters);
    assert_eq!(got.totals, end.totals);
    assert!(got.truncated);
    assert!(matches!(
        q.finish_run(HealthRunId::new(), &end).await,
        Err(StoreError::NotFound)
    ));
}

pub async fn link_cache_put_get_prune(db: &TestDb) {
    let store = db.store();
    let q = store.read().quality();
    let t0 = at(1_700_000_000 * SECOND);
    let t1 = at(1_700_000_000 * SECOND + 40 * DAY);
    q.link_put(&link("https://example.org/a", t0))
        .await
        .unwrap();
    q.link_put(&link("https://example.org/b", t1))
        .await
        .unwrap();
    let mut broken = link("https://example.org/a", t1);
    broken.status = LinkStatus::Failing;
    broken.http_status = None;
    broken.detail = Some("timeout".into());
    broken.failures = 1;
    broken.first_failed_at = Some(t1);
    q.link_put(&broken).await.unwrap();
    let mut got = q
        .link_get(&[
            "https://example.org/a".into(),
            "https://example.org/b".into(),
            "https://example.org/none".into(),
        ])
        .await
        .unwrap();
    got.sort_by(|a, b| a.url.cmp(&b.url));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], broken);
    assert_eq!(q.link_prune(at(t1.as_micros() - DAY)).await.unwrap(), 0);
    assert_eq!(q.link_prune(at(t1.as_micros() + SECOND)).await.unwrap(), 2);
    assert!(
        q.link_get(&["https://example.org/a".into()])
            .await
            .unwrap()
            .is_empty()
    );
}
