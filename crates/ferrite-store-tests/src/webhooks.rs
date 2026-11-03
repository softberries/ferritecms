//! Contract tests for the webhook tables of `0016_webhooks` (M6-6 task 1): endpoints,
//! events, deliveries and attempts.

use ferrite_domain::{
    Attempt, Delivery, DeliveryId, DeliveryQuery, DeliveryStatus, DeliveryUpdate, DisabledReason,
    Timestamp, WebhookEndpoint, WebhookEventId, WebhookEventRow, WebhookId,
};
use ferrite_store::{Compat, SCHEMA_META_INSTALL_ID, Store, StoreError};
use serde_json::json;

use crate::{TestDb, fixtures::fixture_user};

const SECOND: i64 = 1_000_000;
const DAY: i64 = 24 * 3600 * SECOND;

fn plus(t: Timestamp, micros: i64) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + micros).unwrap()
}

async fn install_id(store: &dyn Store) -> String {
    store
        .read()
        .schema_meta()
        .get(SCHEMA_META_INSTALL_ID)
        .await
        .unwrap()
        .expect("install id")
}

/// An enabled endpoint under `install`.
pub(crate) fn endpoint(name: &str, install: &str, now: Timestamp) -> WebhookEndpoint {
    WebhookEndpoint {
        id: WebhookId::new(),
        name: name.into(),
        url: format!("https://hooks.example.com/{name}?token=t"),
        events: vec!["content.*".into(), "media.uploaded".into()],
        content_types: Some(vec!["post".into()]),
        enabled: true,
        disabled_reason: None,
        disabled_at: None,
        install_id: install.into(),
        secret_sealed: "sealed-current".into(),
        previous_secret_sealed: None,
        previous_secret_expires_at: None,
        payload_version: 1,
        failing_since: None,
        consecutive_failures: 0,
        last_success_at: None,
        last_failure_at: None,
        created_by: None,
        created_at: now,
        updated_at: now,
    }
}

pub(crate) fn event(kind: &str, at: Timestamp) -> WebhookEventRow {
    WebhookEventRow {
        id: WebhookEventId::new(),
        kind: kind.into(),
        data: json!({"id": "x", "title": "Hello"}),
        created_at: at,
    }
}

pub(crate) fn delivery(e: &WebhookEndpoint, ev: &WebhookEventRow, at: Timestamp) -> Delivery {
    Delivery {
        id: DeliveryId::new(),
        endpoint_id: e.id,
        event_id: ev.id,
        status: DeliveryStatus::Pending,
        attempts: 0,
        next_attempt_at: Some(at),
        last_status_code: None,
        last_error: None,
        redelivery_of: None,
        created_at: at,
        finished_at: None,
    }
}

fn attempt(d: &Delivery, n: i32, at: Timestamp) -> Attempt {
    Attempt {
        delivery_id: d.id,
        attempt: n,
        started_at: at,
        duration_ms: 12,
        status_code: Some(500),
        error: Some("status 500".into()),
        response_excerpt: Some("oops".into()),
    }
}

fn failed_update(n: i32, next: Option<Timestamp>) -> DeliveryUpdate {
    DeliveryUpdate {
        status: DeliveryStatus::Pending,
        attempts: n,
        next_attempt_at: next,
        last_status_code: Some(500),
        last_error: Some("status 500".into()),
        finished_at: None,
    }
}

/// `0016_webhooks`: the tables exist; the down drops them (not lossy); up again takes rows.
pub async fn migration_0016_roundtrip(db: &TestDb) {
    const TABLES: [&str; 4] = [
        "webhook_endpoints",
        "webhook_events",
        "webhook_deliveries",
        "webhook_attempts",
    ];
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    assert!(
        has(&objects, "webhook_deliveries_endpoint_idx"),
        "{objects:?}"
    );
    let install = install_id(store).await;
    let now = Timestamp::now();
    let e = endpoint("roundtrip", &install, now);
    store.read().webhooks().insert_endpoint(&e).await.unwrap();

    // Later migrations (0017 is lossy) go first; then down to 0015 (lossy, M6-2), which
    // stays applied.
    store.migrate_down(16, true).await.unwrap();
    assert_eq!(store.migrate_down(15, false).await.unwrap(), vec![16]);
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        crate::migrations::up_from(15)
    );
    let w = store.read().webhooks();
    assert_eq!(
        w.count_endpoints().await.unwrap(),
        0,
        "the down drops the rows"
    );
    w.insert_endpoint(&e).await.unwrap();
    assert_eq!(w.get_endpoint(e.id).await.unwrap(), Some(e));
    assert_eq!(
        store
            .migration_status(&crate::migrations::binary())
            .await
            .unwrap()
            .verdict,
        Compat::UpToDate
    );
}

pub async fn endpoint_crud_and_cascade(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let install = install_id(store).await;
    let now = Timestamp::now();
    let user = fixture_user(store).await;
    let mut b = endpoint("b-hook", &install, now);
    b.created_by = Some(user.id);
    let mut a = endpoint("a-hook", &install, now);
    a.content_types = None;
    a.previous_secret_sealed = Some("sealed-old".into());
    a.previous_secret_expires_at = Some(plus(now, DAY));
    w.insert_endpoint(&b).await.unwrap();
    w.insert_endpoint(&a).await.unwrap();
    assert!(matches!(
        w.insert_endpoint(&a).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert_eq!(w.count_endpoints().await.unwrap(), 2);
    assert_eq!(
        w.list_endpoints().await.unwrap(),
        vec![a.clone(), b.clone()]
    );
    assert_eq!(w.get_endpoint(a.id).await.unwrap(), Some(a.clone()));
    assert_eq!(w.get_endpoint(WebhookId::new()).await.unwrap(), None);
    let tx = store.begin().await.unwrap();
    assert_eq!(
        tx.webhooks().get_endpoint_for_update(a.id).await.unwrap(),
        Some(a.clone())
    );
    tx.rollback().await.unwrap();

    // Whole-row update.
    let mut changed = a.clone();
    changed.name = "a-renamed".into();
    changed.enabled = false;
    changed.disabled_reason = Some(DisabledReason::Gone);
    changed.disabled_at = Some(now);
    changed.failing_since = Some(now);
    changed.consecutive_failures = 3;
    changed.last_failure_at = Some(now);
    changed.updated_at = plus(now, SECOND);
    w.update_endpoint(&changed).await.unwrap();
    assert_eq!(w.get_endpoint(a.id).await.unwrap(), Some(changed.clone()));
    let missing = endpoint("missing", &install, now);
    assert!(matches!(
        w.update_endpoint(&missing).await,
        Err(StoreError::NotFound)
    ));

    // Deliveries and attempts go with their endpoint.
    let ev = event("content.published", now);
    w.insert_event(&ev).await.unwrap();
    assert_eq!(w.get_event(ev.id).await.unwrap(), Some(ev.clone()));
    let d = delivery(&changed, &ev, now);
    w.insert_delivery(&d).await.unwrap();
    assert!(
        w.record_attempt(&attempt(&d, 1, now), &failed_update(1, Some(now)))
            .await
            .unwrap()
    );
    let unknown = delivery(&missing, &ev, now);
    assert!(matches!(
        w.insert_delivery(&unknown).await,
        Err(StoreError::ForeignKey { .. })
    ));
    assert!(w.delete_endpoint(a.id).await.unwrap());
    assert!(!w.delete_endpoint(a.id).await.unwrap());
    assert_eq!(w.get_delivery(d.id).await.unwrap(), None);
    assert_eq!(w.attempts(d.id).await.unwrap(), vec![]);
    // The event stays (other endpoints may have deliveries of it).
    assert!(w.get_event(ev.id).await.unwrap().is_some());

    // Deleting the creator keeps the endpoint.
    store.read().users().delete(user.id).await.unwrap();
    assert_eq!(
        w.get_endpoint(b.id).await.unwrap().unwrap().created_by,
        None
    );
}

pub async fn active_for_emit_needs_enabled_and_install_id(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let install = install_id(store).await;
    let now = Timestamp::now();
    let on = endpoint("on", &install, now);
    let mut off = endpoint("off", &install, now);
    off.enabled = false;
    off.disabled_reason = Some(DisabledReason::Manual);
    let copied = endpoint("copied", "another-install", now);
    for e in [&on, &off, &copied] {
        w.insert_endpoint(e).await.unwrap();
    }
    assert_eq!(w.active_for_emit().await.unwrap(), vec![on.clone()]);
    // Inside a Tx too.
    let tx = store.begin().await.unwrap();
    assert_eq!(tx.webhooks().active_for_emit().await.unwrap(), vec![on]);
    tx.rollback().await.unwrap();
    // A copy (a new install id) pauses every endpoint.
    store
        .read()
        .schema_meta()
        .set(SCHEMA_META_INSTALL_ID, "fresh-copy")
        .await
        .unwrap();
    assert_eq!(w.active_for_emit().await.unwrap(), vec![]);
    store
        .read()
        .schema_meta()
        .set(SCHEMA_META_INSTALL_ID, &install)
        .await
        .unwrap();
}

pub async fn record_attempt_is_single_use(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let e = endpoint("single", &install_id(store).await, now);
    w.insert_endpoint(&e).await.unwrap();
    let ev = event("content.created", now);
    w.insert_event(&ev).await.unwrap();
    let d = delivery(&e, &ev, now);
    w.insert_delivery(&d).await.unwrap();

    let next = plus(now, 5 * SECOND);
    assert!(
        w.record_attempt(&attempt(&d, 1, now), &failed_update(1, Some(next)))
            .await
            .unwrap()
    );
    // The same attempt again, and an attempt out of order, write nothing.
    assert!(
        !w.record_attempt(&attempt(&d, 1, now), &failed_update(1, Some(next)))
            .await
            .unwrap()
    );
    assert!(
        !w.record_attempt(&attempt(&d, 3, now), &failed_update(3, None))
            .await
            .unwrap()
    );
    let got = w.get_delivery(d.id).await.unwrap().unwrap();
    assert_eq!(
        (got.attempts, got.next_attempt_at, got.last_status_code),
        (1, Some(next), Some(500))
    );
    // Attempt 2 delivers; then nothing more is recorded.
    let ok = DeliveryUpdate {
        status: DeliveryStatus::Delivered,
        attempts: 2,
        next_attempt_at: None,
        last_status_code: Some(204),
        last_error: None,
        finished_at: Some(next),
    };
    let mut a2 = attempt(&d, 2, next);
    a2.status_code = Some(204);
    a2.error = None;
    a2.response_excerpt = None;
    assert!(w.record_attempt(&a2, &ok).await.unwrap());
    assert!(
        !w.record_attempt(&attempt(&d, 3, next), &failed_update(3, None))
            .await
            .unwrap()
    );
    let got = w.get_delivery(d.id).await.unwrap().unwrap();
    assert_eq!(got.status, DeliveryStatus::Delivered);
    assert_eq!(got.finished_at, Some(next));
    let attempts = w.attempts(d.id).await.unwrap();
    assert_eq!(attempts, vec![attempt(&d, 1, now), a2]);
    // Unknown delivery.
    let ghost = delivery(&e, &ev, now);
    assert!(
        !w.record_attempt(&attempt(&ghost, 1, now), &failed_update(1, None))
            .await
            .unwrap()
    );
}

pub async fn set_status_only_from_pending(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let e = endpoint("status", &install_id(store).await, now);
    w.insert_endpoint(&e).await.unwrap();
    let ev = event("content.created", now);
    w.insert_event(&ev).await.unwrap();
    let d = delivery(&e, &ev, now);
    w.insert_delivery(&d).await.unwrap();
    assert!(
        w.set_delivery_status(
            d.id,
            DeliveryStatus::Skipped,
            Some("endpoint disabled"),
            now
        )
        .await
        .unwrap()
    );
    assert!(
        !w.set_delivery_status(d.id, DeliveryStatus::Failed, None, now)
            .await
            .unwrap()
    );
    let got = w.get_delivery(d.id).await.unwrap().unwrap();
    assert_eq!(got.status, DeliveryStatus::Skipped);
    assert_eq!(got.last_error.as_deref(), Some("endpoint disabled"));
    assert_eq!((got.finished_at, got.next_attempt_at), (Some(now), None));
    assert!(
        !w.set_delivery_status(DeliveryId::new(), DeliveryStatus::Failed, None, now)
            .await
            .unwrap()
    );
}

pub async fn deliveries_page_newest_first(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let install = install_id(store).await;
    let e = endpoint("paging", &install, now);
    let other = endpoint("other", &install, now);
    w.insert_endpoint(&e).await.unwrap();
    w.insert_endpoint(&other).await.unwrap();
    let mut made = Vec::new();
    for i in 0..5 {
        let at = plus(now, i * SECOND);
        let ev = event(
            if i % 2 == 0 {
                "content.created"
            } else {
                "media.uploaded"
            },
            at,
        );
        w.insert_event(&ev).await.unwrap();
        let d = delivery(&e, &ev, at);
        w.insert_delivery(&d).await.unwrap();
        w.insert_delivery(&delivery(&other, &ev, at)).await.unwrap();
        made.push((d, ev.kind));
    }
    w.set_delivery_status(made[1].0.id, DeliveryStatus::Failed, None, now)
        .await
        .unwrap();
    let page1 = w
        .list_deliveries(
            e.id,
            &DeliveryQuery {
                status: None,
                limit: 2,
                cursor: None,
            },
        )
        .await
        .unwrap();
    let ids: Vec<_> = page1.items.iter().map(|l| l.delivery.id).collect();
    assert_eq!(ids, vec![made[4].0.id, made[3].0.id]);
    assert_eq!(page1.items[0].event_type, "content.created");
    assert_eq!(page1.items[1].event_type, "media.uploaded");
    let page2 = w
        .list_deliveries(
            e.id,
            &DeliveryQuery {
                status: None,
                limit: 10,
                cursor: page1.next_cursor.clone(),
            },
        )
        .await
        .unwrap();
    let ids: Vec<_> = page2.items.iter().map(|l| l.delivery.id).collect();
    assert_eq!(ids, vec![made[2].0.id, made[1].0.id, made[0].0.id]);
    assert_eq!(page2.next_cursor, None);
    let failed = w
        .list_deliveries(
            e.id,
            &DeliveryQuery {
                status: Some(DeliveryStatus::Failed),
                limit: 10,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(failed.items.len(), 1);
    assert_eq!(failed.items[0].delivery.id, made[1].0.id);
    assert!(matches!(
        w.list_deliveries(
            e.id,
            &DeliveryQuery {
                status: None,
                limit: 0,
                cursor: None
            }
        )
        .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(
        w.list_deliveries(
            e.id,
            &DeliveryQuery {
                status: None,
                limit: 5,
                cursor: Some("garbage".into())
            }
        )
        .await
        .is_err()
    );

    // Since a time, by status, oldest first, bounded.
    let since = w
        .deliveries_since(
            e.id,
            plus(now, SECOND),
            &[DeliveryStatus::Pending, DeliveryStatus::Failed],
            2,
        )
        .await
        .unwrap();
    let ids: Vec<_> = since.iter().map(|d| d.id).collect();
    assert_eq!(ids, vec![made[1].0.id, made[2].0.id]);
    let only_failed = w
        .deliveries_since(e.id, now, &[DeliveryStatus::Failed], 100)
        .await
        .unwrap();
    assert_eq!(only_failed.len(), 1);
}

pub async fn delivery_counts_by_status_since(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let e = endpoint("counts", &install_id(store).await, now);
    w.insert_endpoint(&e).await.unwrap();
    let old = plus(now, -2 * DAY);
    for (at, status) in [
        (old, DeliveryStatus::Failed),
        (now, DeliveryStatus::Failed),
        (now, DeliveryStatus::Skipped),
        (now, DeliveryStatus::Pending),
        (now, DeliveryStatus::Pending),
    ] {
        let ev = event("content.created", at);
        w.insert_event(&ev).await.unwrap();
        let d = delivery(&e, &ev, at);
        w.insert_delivery(&d).await.unwrap();
        if status != DeliveryStatus::Pending {
            w.set_delivery_status(d.id, status, None, at).await.unwrap();
        }
    }
    let counts = w.delivery_counts(e.id, plus(now, -DAY)).await.unwrap();
    assert_eq!(
        (
            counts.delivered,
            counts.failed,
            counts.pending,
            counts.skipped
        ),
        (0, 1, 2, 1)
    );
    let all = w.delivery_counts(e.id, plus(now, -3 * DAY)).await.unwrap();
    assert_eq!(all.failed, 2);
}

pub async fn prune_keeps_events_with_pending_deliveries(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let e = endpoint("prune", &install_id(store).await, now);
    w.insert_endpoint(&e).await.unwrap();
    let old = plus(now, -31 * DAY);
    let done = event("content.created", old);
    let pending = event("content.updated", old);
    let lonely = event("content.updated", old);
    let fresh = event("content.updated", now);
    for ev in [&done, &pending, &lonely, &fresh] {
        w.insert_event(ev).await.unwrap();
    }
    let d_done = delivery(&e, &done, old);
    w.insert_delivery(&d_done).await.unwrap();
    w.record_attempt(
        &attempt(&d_done, 1, old),
        &DeliveryUpdate {
            status: DeliveryStatus::Failed,
            attempts: 1,
            next_attempt_at: None,
            last_status_code: Some(410),
            last_error: Some("status 410".into()),
            finished_at: Some(old),
        },
    )
    .await
    .unwrap();
    let d_pending = delivery(&e, &pending, old);
    w.insert_delivery(&d_pending).await.unwrap();
    w.insert_delivery(&delivery(&e, &fresh, now)).await.unwrap();

    let cutoff = plus(now, -30 * DAY);
    assert!(w.prune_events(cutoff, 0).await.is_err());
    assert_eq!(w.prune_events(cutoff, 1).await.unwrap(), 1);
    assert_eq!(w.prune_events(cutoff, 100).await.unwrap(), 1);
    assert_eq!(w.prune_events(cutoff, 100).await.unwrap(), 0);
    assert_eq!(w.get_event(done.id).await.unwrap(), None);
    assert_eq!(w.get_event(lonely.id).await.unwrap(), None);
    assert_eq!(w.get_delivery(d_done.id).await.unwrap(), None);
    assert_eq!(w.attempts(d_done.id).await.unwrap(), vec![]);
    assert!(w.get_event(pending.id).await.unwrap().is_some());
    assert!(w.get_event(fresh.id).await.unwrap().is_some());
}

pub async fn nul_in_text_is_invalid(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let install = install_id(store).await;
    let invalid = |r: Result<(), StoreError>| matches!(r, Err(StoreError::Invalid(_)));
    let mut e = endpoint("nul", &install, now);
    e.name = "a\0b".into();
    assert!(invalid(w.insert_endpoint(&e).await));
    let mut e = endpoint("nul", &install, now);
    e.events = vec!["content.\0".into()];
    assert!(invalid(w.insert_endpoint(&e).await));
    let mut e = endpoint("nul", &install, now);
    e.url = "https://x/\0".into();
    assert!(invalid(w.insert_endpoint(&e).await));
    let e = endpoint("nul", &install, now);
    w.insert_endpoint(&e).await.unwrap();
    let mut bad = e.clone();
    bad.content_types = Some(vec!["po\0st".into()]);
    assert!(invalid(w.update_endpoint(&bad).await));
    let mut ev = event("content.created", now);
    ev.kind = "x\0".into();
    assert!(invalid(w.insert_event(&ev).await));
    let ev = event("content.created", now);
    w.insert_event(&ev).await.unwrap();
    let d = delivery(&e, &ev, now);
    w.insert_delivery(&d).await.unwrap();
    let mut a = attempt(&d, 1, now);
    a.response_excerpt = Some("\0".into());
    assert!(matches!(
        w.record_attempt(&a, &failed_update(1, None)).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        w.set_delivery_status(d.id, DeliveryStatus::Failed, Some("\0"), now)
            .await,
        Err(StoreError::Invalid(_))
    ));
}

pub async fn fail_stale_pending_only_old_pending(db: &TestDb) {
    let store = db.store();
    let w = store.read().webhooks();
    let now = Timestamp::now();
    let e = endpoint("stale", &install_id(store).await, now);
    w.insert_endpoint(&e).await.unwrap();
    let ev = event("content.created", now);
    w.insert_event(&ev).await.unwrap();
    let mut old = delivery(&e, &ev, now);
    old.next_attempt_at = Some(plus(now, -DAY));
    let mut fresh = delivery(&e, &ev, now);
    fresh.next_attempt_at = Some(plus(now, 60 * SECOND));
    let mut done = delivery(&e, &ev, now);
    done.next_attempt_at = Some(plus(now, -DAY));
    done.status = DeliveryStatus::Delivered;
    for d in [&old, &fresh, &done] {
        w.insert_delivery(d).await.unwrap();
    }
    let cutoff = plus(now, -3600 * SECOND);
    assert_eq!(
        w.fail_stale_pending(cutoff, "gave up", now, 100)
            .await
            .unwrap(),
        1
    );
    let got = w.get_delivery(old.id).await.unwrap().unwrap();
    assert_eq!(got.status, DeliveryStatus::Failed);
    assert_eq!(got.last_error.as_deref(), Some("gave up"));
    assert_eq!(got.next_attempt_at, None);
    assert_eq!(
        w.get_delivery(fresh.id).await.unwrap().unwrap().status,
        DeliveryStatus::Pending
    );
    assert_eq!(
        w.get_delivery(done.id).await.unwrap().unwrap().status,
        DeliveryStatus::Delivered
    );
    assert_eq!(
        w.fail_stale_pending(cutoff, "gave up", now, 100)
            .await
            .unwrap(),
        0
    );
}
