//! Contract tests for the auth tables of `0005_auth` (M2a-1 task 1): sessions, API
//! tokens, rate-limit counters and the audit log.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use ferrite_domain::{
    ApiToken, ApiTokenId, AuditQuery, NewAudit, Secret, Session, Timestamp, User, UserId,
};
use ferrite_store::{MAX_PRUNE_BATCH, Store, StoreError, USER_CURSOR_TAG, encode_text_cursor};
use serde_json::json;

use crate::{TestDb, fixtures::fixture_user};

const DAY: i64 = 24 * 3600 * 1_000_000;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

fn plus(t: Timestamp, micros: i64) -> Timestamp {
    at(t.as_micros() + micros)
}

fn session(user: &User, id_hash: &str, created_at: Timestamp) -> Session {
    Session {
        id_hash: id_hash.into(),
        user_id: user.id,
        csrf_secret: Secret::new(format!("csrf-{id_hash}")),
        created_at,
        last_seen_at: created_at,
        expires_at: plus(created_at, 30 * DAY),
        ip_hash: Some(format!("ip-{id_hash}")),
        user_agent: Some("Mozilla/5.0 (ü)".into()),
    }
}

fn token(user: &User, hash: &str, created_at: Timestamp) -> ApiToken {
    ApiToken {
        id: ApiTokenId::new(),
        user_id: user.id,
        name: format!("token {hash}"),
        token_hash: hash.into(),
        scopes: vec!["content:read".into(), "users:write".into()],
        created_at,
        last_used_at: None,
        expires_at: Some(plus(created_at, 90 * DAY)),
    }
}

fn audit(actor: Option<UserId>, i: usize) -> NewAudit {
    NewAudit {
        actor_id: actor,
        action: "user.update".into(),
        target_kind: "user".into(),
        target_id: Some(format!("target-{i}")),
        details: json!({"i": i, "fields": ["role"]}),
    }
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

pub async fn sessions_roundtrip_touch_delete(db: &TestDb) {
    let store = db.store();
    let sessions = store.read().sessions();
    let user = fixture_user(store).await;
    let other = fixture_user(store).await;
    let t0 = Timestamp::now();

    let a = session(&user, "a", t0);
    sessions.insert(&a).await.unwrap();
    let got = sessions.get("a").await.unwrap().expect("stored");
    assert_eq!(got, a);
    assert_eq!(got.csrf_secret.expose(), "csrf-a");
    assert_eq!(sessions.get("missing").await.unwrap(), None);

    let mut b = session(&user, "b", plus(t0, 1_000));
    b.ip_hash = None;
    b.user_agent = None;
    sessions.insert(&b).await.unwrap();
    assert_eq!(sessions.get("b").await.unwrap(), Some(b.clone()));
    let c = session(&user, "c", plus(t0, 2_000));
    sessions.insert(&c).await.unwrap();
    let theirs = session(&other, "theirs", t0);
    sessions.insert(&theirs).await.unwrap();

    assert!(matches!(
        sessions.insert(&a).await,
        Err(StoreError::UniqueViolation { constraint }) if constraint == "sessions_pkey"
    ));
    let mut orphan = session(&user, "orphan", t0);
    orphan.user_id = UserId::new();
    assert!(matches!(
        sessions.insert(&orphan).await,
        Err(StoreError::ForeignKey { .. })
    ));

    // touch moves last_seen_at (never backwards); a missing id is false.
    let later = plus(t0, 60_000_000);
    assert!(sessions.touch("a", later).await.unwrap());
    let touched = sessions.get("a").await.unwrap().unwrap();
    assert_eq!(touched.last_seen_at, later);
    assert_eq!(
        (touched.created_at, touched.expires_at),
        (a.created_at, a.expires_at)
    );
    assert!(sessions.touch("a", t0).await.unwrap(), "row exists");
    assert_eq!(
        sessions.get("a").await.unwrap().unwrap().last_seen_at,
        later,
        "an older touch does not move it back"
    );
    assert!(!sessions.touch("missing", later).await.unwrap());

    // Newest first, only this user's.
    let listed = sessions.list_for_user(user.id).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|s| s.id_hash.as_str())
            .collect::<Vec<_>>(),
        ["c", "b", "a"]
    );

    // Everything but the kept one; other users untouched.
    assert_eq!(
        sessions.delete_for_user(user.id, Some("a")).await.unwrap(),
        2
    );
    assert_eq!(
        sessions
            .list_for_user(user.id)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id_hash.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert!(sessions.get("theirs").await.unwrap().is_some());
    assert_eq!(sessions.delete_for_user(other.id, None).await.unwrap(), 1);
    assert_eq!(sessions.get("theirs").await.unwrap(), None);

    // delete is idempotent.
    sessions.insert(&b).await.unwrap();
    sessions.delete("b").await.unwrap();
    sessions.delete("b").await.unwrap();
    assert_eq!(sessions.get("b").await.unwrap(), None);

    // Deleting the user cascades.
    store.read().users().delete(user.id).await.unwrap();
    assert_eq!(sessions.get("a").await.unwrap(), None);
    assert!(sessions.list_for_user(user.id).await.unwrap().is_empty());
}

pub async fn sessions_prune_by_expiry_and_idle(db: &TestDb) {
    let store = db.store();
    let sessions = store.read().sessions();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let idle_before = plus(now, -7 * DAY);

    for i in 0..3 {
        // Expired: absolute end passed, recently seen.
        let mut s = session(&user, &format!("expired-{i}"), plus(now, -31 * DAY));
        s.last_seen_at = plus(now, -1_000_000);
        s.expires_at = plus(now, -1);
        sessions.insert(&s).await.unwrap();
    }
    for i in 0..2 {
        // Idle: not seen for 8 days, absolute end ahead.
        let mut s = session(&user, &format!("idle-{i}"), plus(now, -10 * DAY));
        s.last_seen_at = plus(now, -8 * DAY);
        s.expires_at = plus(now, 20 * DAY);
        sessions.insert(&s).await.unwrap();
    }
    // Fresh, including the boundaries (`<` on both).
    let mut fresh = session(&user, "fresh", plus(now, -7 * DAY));
    fresh.last_seen_at = idle_before;
    fresh.expires_at = now;
    sessions.insert(&fresh).await.unwrap();

    for bad in [0, MAX_PRUNE_BATCH + 1] {
        assert!(is_invalid(sessions.prune(now, idle_before, bad).await));
    }
    assert_eq!(sessions.prune(now, idle_before, 2).await.unwrap(), 2);
    assert_eq!(sessions.prune(now, idle_before, 10).await.unwrap(), 3);
    assert_eq!(sessions.prune(now, idle_before, 10).await.unwrap(), 0);
    assert_eq!(
        sessions
            .list_for_user(user.id)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id_hash.as_str())
            .collect::<Vec<_>>(),
        ["fresh"]
    );
}

pub async fn api_tokens_unique_hash_and_owner_scoped_delete(db: &TestDb) {
    let store = db.store();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let other = fixture_user(store).await;
    let t0 = Timestamp::now();

    let first = token(&user, "h1", t0);
    tokens.insert(&first).await.unwrap();
    assert_eq!(tokens.get_by_hash("h1").await.unwrap(), Some(first.clone()));
    assert_eq!(tokens.get_by_hash("nope").await.unwrap(), None);

    let mut dup = token(&user, "h1", t0);
    dup.name = "dup".into();
    assert!(matches!(
        tokens.insert(&dup).await,
        Err(StoreError::UniqueViolation { constraint }) if constraint == "api_tokens_hash_uq"
    ));
    let mut same_id = token(&user, "h-other", t0);
    same_id.id = first.id;
    assert!(matches!(
        tokens.insert(&same_id).await,
        Err(StoreError::UniqueViolation { constraint }) if constraint == "api_tokens_pkey"
    ));
    let mut orphan = token(&user, "h-orphan", t0);
    orphan.user_id = UserId::new();
    assert!(matches!(
        tokens.insert(&orphan).await,
        Err(StoreError::ForeignKey { .. })
    ));

    let mut second = token(&user, "h2", plus(t0, 1_000));
    second.scopes = Vec::new();
    second.expires_at = None;
    tokens.insert(&second).await.unwrap();
    let mut third = token(&user, "h3", plus(t0, 2_000));
    third.scopes = vec!["settings:write".into()];
    tokens.insert(&third).await.unwrap();
    let theirs = token(&other, "h-theirs", t0);
    tokens.insert(&theirs).await.unwrap();

    let listed = tokens.list_for_user(user.id).await.unwrap();
    assert_eq!(listed, vec![first.clone(), second.clone(), third.clone()]);
    assert_eq!(tokens.count_for_user(user.id).await.unwrap(), 3);
    assert_eq!(tokens.count_for_user(other.id).await.unwrap(), 1);
    assert_eq!(tokens.count_for_user(UserId::new()).await.unwrap(), 0);

    // touch sets last_used_at, never backwards; a missing id is false.
    let used = plus(t0, 5_000_000);
    assert!(tokens.touch(first.id, used).await.unwrap());
    assert!(tokens.touch(first.id, t0).await.unwrap());
    assert_eq!(
        tokens
            .get_by_hash("h1")
            .await
            .unwrap()
            .unwrap()
            .last_used_at,
        Some(used)
    );
    assert!(!tokens.touch(ApiTokenId::new(), used).await.unwrap());

    // Owner-scoped delete.
    assert!(!tokens.delete(other.id, first.id).await.unwrap());
    assert!(tokens.get_by_hash("h1").await.unwrap().is_some());
    assert!(tokens.delete(user.id, first.id).await.unwrap());
    assert!(!tokens.delete(user.id, first.id).await.unwrap());
    assert_eq!(tokens.get_by_hash("h1").await.unwrap(), None);
    assert_eq!(
        tokens.count_for_user(user.id).await.unwrap(),
        tokens.list_for_user(user.id).await.unwrap().len() as u64
    );

    assert_eq!(tokens.delete_for_user(user.id).await.unwrap(), 2);
    assert_eq!(tokens.count_for_user(user.id).await.unwrap(), 0);
    assert!(tokens.get_by_hash("h-theirs").await.unwrap().is_some());

    // Deleting the user cascades.
    store.read().users().delete(other.id).await.unwrap();
    assert_eq!(tokens.get_by_hash("h-theirs").await.unwrap(), None);
}

/// `prune` deletes tokens whose `expires_at` passed (`<`, so `now` itself stays), never
/// those without an expiry, in batches of `limit` (R5).
pub async fn api_tokens_prune_expired(db: &TestDb) {
    let store = db.store();
    let tokens = store.read().api_tokens();
    let user = fixture_user(store).await;
    let now = Timestamp::now();

    for i in 0..3 {
        let mut t = token(&user, &format!("expired-{i}"), plus(now, -10 * DAY));
        t.expires_at = Some(plus(now, -1));
        tokens.insert(&t).await.unwrap();
    }
    let mut boundary = token(&user, "boundary", plus(now, -DAY));
    boundary.expires_at = Some(now);
    tokens.insert(&boundary).await.unwrap();
    let mut forever = token(&user, "forever", plus(now, -400 * DAY));
    forever.expires_at = None;
    tokens.insert(&forever).await.unwrap();
    let ahead = token(&user, "ahead", now);
    tokens.insert(&ahead).await.unwrap();

    for bad in [0, MAX_PRUNE_BATCH + 1] {
        assert!(is_invalid(tokens.prune(now, bad).await));
    }
    assert_eq!(tokens.prune(now, 2).await.unwrap(), 2);
    assert_eq!(tokens.prune(now, 10).await.unwrap(), 1);
    assert_eq!(tokens.prune(now, 10).await.unwrap(), 0);
    let left: BTreeSet<String> = tokens
        .list_for_user(user.id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.token_hash)
        .collect();
    assert_eq!(
        left,
        ["ahead", "boundary", "forever"]
            .map(String::from)
            .into_iter()
            .collect()
    );
}

/// Sleeps until 50 ms past the next `window` boundary of the database clock when fewer
/// than `need` remain in the current window, so a test's hits share one window (F10).
pub async fn align_to_window(store: &dyn Store, window: Duration, need: Duration) {
    let w = i64::try_from(window.as_micros()).unwrap();
    let now = store.read().nodes().db_now().await.unwrap().as_micros();
    let left = w - now.rem_euclid(w);
    if left < i64::try_from(need.as_micros()).unwrap() {
        tokio::time::sleep(Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50))
            .await;
    }
}

pub async fn rate_limit_counts_within_window_and_resets(db: &TestDb) {
    let store = db.store();
    let limits = store.read().rate_limits();
    let window = Duration::from_secs(2);
    let w = 2_000_000;

    // Validation, before any SQL.
    assert!(is_invalid(limits.hit("", window).await));
    assert!(is_invalid(limits.hit(&"x".repeat(201), window).await));
    assert!(is_invalid(limits.hit("a\0", window).await));
    assert!(is_invalid(limits.hit("a", Duration::ZERO).await));
    assert_eq!(limits.hit(&"é".repeat(200), window).await.unwrap().count, 1);

    align_to_window(store, window, window).await;
    let first = limits.hit("login:ip:1", window).await.unwrap();
    let other = limits.hit("login:ip:2", window).await.unwrap();
    let second = limits.hit("login:ip:1", window).await.unwrap();
    let third = limits.hit("login:ip:1", window).await.unwrap();
    assert_eq!((first.count, second.count, third.count), (1, 2, 3));
    assert_eq!(other.count, 1, "buckets are independent");
    assert_eq!(first.reset_at, third.reset_at);
    assert_eq!(first.reset_at, other.reset_at);
    let start = first.reset_at.as_micros() - w;
    assert_eq!(
        start.rem_euclid(w),
        0,
        "window_start is floored to the window"
    );
    for hit in [first, second, third] {
        assert!(
            hit.now.as_micros() >= start && hit.now < hit.reset_at,
            "{hit:?}"
        );
    }

    tokio::time::sleep(Duration::from_millis(2100)).await;
    let reset = limits.hit("login:ip:1", window).await.unwrap();
    assert_eq!(reset.count, 1, "a new window starts over");
    assert_eq!(reset.reset_at, plus(first.reset_at, w));

    // prune removes windows that started before the cut only.
    for bad in [0, MAX_PRUNE_BATCH + 1] {
        assert!(is_invalid(limits.prune(first.reset_at, bad).await));
    }
    assert_eq!(limits.prune(first.reset_at, 1).await.unwrap(), 1);
    // "login:ip:2" and the 200-char bucket (an earlier window) remain before the cut.
    assert_eq!(limits.prune(first.reset_at, 10).await.unwrap(), 1);
    assert_eq!(limits.prune(first.reset_at, 10).await.unwrap(), 0);
    assert_eq!(
        limits.hit("login:ip:1", window).await.unwrap().count,
        2,
        "the current window survived prune"
    );
    assert_eq!(
        limits.hit("login:ip:2", window).await.unwrap().count,
        1,
        "the pruned bucket starts over"
    );
}

/// R7: a hit stamped in an older window than the stored one (it read its clock before
/// waiting for the row) counts in the stored window and never rolls it back. The repo
/// takes its stamp from the database clock, so the newer stored window (W+1) is written
/// at the SQL level and the repo's hit then arrives stamped in W.
pub async fn rate_limit_window_never_moves_back(db: &TestDb) {
    let store = db.store();
    let window = Duration::from_secs(3600);
    let w: i64 = 3_600_000_000;
    // At least 30 s left in W, so the repo's hit below is stamped in W, not W+1.
    align_to_window(store, window, Duration::from_secs(30)).await;
    let now = store.read().nodes().db_now().await.unwrap().as_micros();
    let next = now - now.rem_euclid(w) + w;
    let stamp = if db.is_sqlite() {
        next.to_string()
    } else {
        format!("'{}'::timestamptz", at(next).to_rfc3339())
    };
    db.exec_raw(&format!(
        "INSERT INTO rate_limits (bucket, window_start, count) VALUES ('late', {stamp}, 1)"
    ))
    .await;
    let hit = store
        .read()
        .rate_limits()
        .hit("late", window)
        .await
        .unwrap();
    assert_eq!(hit.count, 2, "the late hit counts in the stored window");
    assert_eq!(
        hit.reset_at,
        at(next + w),
        "the stored window W+1 is kept, not rolled back to W"
    );
    // A hit in a strictly newer window still starts over (covered with real windows in
    // `rate_limit_counts_within_window_and_resets`).
}

pub async fn rate_limit_is_atomic_under_concurrency(db: Arc<TestDb>) {
    const TASKS: usize = 8;
    const HITS: usize = 25;
    let window = Duration::from_secs(3600);
    let store = db.shared_store();
    align_to_window(store.as_ref(), window, Duration::from_secs(30)).await;
    let mut handles = Vec::new();
    for _ in 0..TASKS {
        let node: Arc<dyn Store> = db.peer().await.unwrap_or_else(|| store.clone());
        handles.push(tokio::spawn(async move {
            let mut counts = Vec::new();
            for _ in 0..HITS {
                let hit = node
                    .read()
                    .rate_limits()
                    .hit("shared", window)
                    .await
                    .unwrap();
                counts.push(hit.count);
            }
            counts
        }));
    }
    let mut seen = BTreeSet::new();
    for h in handles {
        for count in h.await.unwrap() {
            assert!(seen.insert(count), "count {count} returned twice");
        }
    }
    assert_eq!(seen, (1..=(TASKS * HITS) as u32).collect::<BTreeSet<_>>());
}

pub async fn audit_append_and_list_newest_first(db: &TestDb) {
    let store = db.store();
    let log = store.read().audit();
    let actor = fixture_user(store).await;

    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(log.append(&audit(Some(actor.id), i)).await.unwrap());
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let mut system = audit(None, 5);
    system.target_id = None;
    system.details = json!({});
    ids.push(log.append(&system).await.unwrap());
    assert!(ids.iter().all(|id| id.as_uuid().get_version_num() == 7));

    for bad in [0, ferrite_domain::MAX_PAGE + 1] {
        assert!(is_invalid(log.list(&AuditQuery::first(bad)).await));
    }
    let mut pages = Vec::new();
    let mut cursor = None;
    loop {
        let page = log
            .list(&AuditQuery {
                cursor: cursor.clone(),
                ..AuditQuery::first(4)
            })
            .await
            .unwrap();
        assert!(page.items.len() <= 4);
        pages.push(page.items);
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(pages.len(), 2);
    let all: Vec<_> = pages.concat();
    assert_eq!(
        all.iter().map(|e| e.id).collect::<Vec<_>>(),
        ids.iter().rev().copied().collect::<Vec<_>>(),
        "newest first, pages disjoint and complete"
    );
    assert!(all.windows(2).all(|p| p[0].at >= p[1].at));
    let oldest = all.last().unwrap();
    assert_eq!(oldest.actor_id, Some(actor.id));
    assert_eq!(
        (
            oldest.action.as_str(),
            oldest.target_kind.as_str(),
            oldest.target_id.as_deref()
        ),
        ("user.update", "user", Some("target-0"))
    );
    assert_eq!(oldest.details, json!({"i": 0, "fields": ["role"]}));
    assert_eq!(oldest.node, None);
    let newest = &all[0];
    assert_eq!((newest.actor_id, newest.target_id.as_deref()), (None, None));
    assert_eq!(newest.details, json!({}));
    let skew = (newest.at.as_micros() - Timestamp::now().as_micros()).abs();
    assert!(skew < 5_000_000, "at is now ({skew} µs off)");

    // A cursor of another listing is rejected.
    let foreign = encode_text_cursor(USER_CURSOR_TAG, "x", ids[0].as_uuid());
    assert!(is_invalid(
        log.list(&AuditQuery {
            cursor: Some(foreign.to_owned()),
            ..AuditQuery::first(4)
        })
        .await
    ));
    assert!(is_invalid(
        log.list(&AuditQuery {
            cursor: Some("garbage".to_owned()),
            ..AuditQuery::first(4)
        })
        .await
    ));

    // Non-object or NUL details are Invalid and do not poison a Tx.
    let tx = store.begin().await.unwrap();
    for details in [json!([1]), json!("x"), json!(null), json!({"k": "a\0"})] {
        let mut bad = audit(Some(actor.id), 9);
        bad.details = details;
        assert!(is_invalid(tx.audit().append(&bad).await));
    }
    let in_tx = tx.audit().append(&audit(Some(actor.id), 7)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        log.list(&AuditQuery::first(1)).await.unwrap().items[0].id,
        in_tx
    );

    // Deleting the actor keeps the rows with a NULL actor.
    store.read().users().delete(actor.id).await.unwrap();
    let after = log.list(&AuditQuery::first(200)).await.unwrap().items;
    assert_eq!(after.len(), 7);
    assert!(after.iter().all(|e| e.actor_id.is_none()));
}

pub async fn auth_text_binds_reject_nul(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let window = Duration::from_secs(60);

    assert!(is_invalid(repos.rate_limits().hit("b\0", window).await));

    let sessions = repos.sessions();
    let mut s = session(&user, "s\0", now);
    assert!(is_invalid(sessions.insert(&s).await));
    s.id_hash = "s".into();
    s.csrf_secret = Secret::new("c\0");
    assert!(is_invalid(sessions.insert(&s).await));
    s.csrf_secret = Secret::new("c");
    s.ip_hash = Some("i\0".into());
    assert!(is_invalid(sessions.insert(&s).await));
    s.ip_hash = None;
    s.user_agent = Some("u\0".into());
    assert!(is_invalid(sessions.insert(&s).await));
    assert!(is_invalid(sessions.get("s\0").await));
    assert!(is_invalid(sessions.touch("s\0", now).await));
    assert!(is_invalid(sessions.delete("s\0").await));
    assert!(is_invalid(
        sessions.delete_for_user(user.id, Some("s\0")).await
    ));

    let tokens = repos.api_tokens();
    let mut t = token(&user, "t\0", now);
    assert!(is_invalid(tokens.insert(&t).await));
    t.token_hash = "t".into();
    t.name = "n\0".into();
    assert!(is_invalid(tokens.insert(&t).await));
    t.name = "n".into();
    t.scopes = vec!["content:read".into(), "x\0".into()];
    assert!(is_invalid(tokens.insert(&t).await));
    assert!(is_invalid(tokens.get_by_hash("t\0").await));

    let log = repos.audit();
    for field in 0..3 {
        let mut a = audit(Some(user.id), 0);
        match field {
            0 => a.action = "a\0".into(),
            1 => a.target_kind = "k\0".into(),
            _ => a.target_id = Some("t\0".into()),
        }
        assert!(is_invalid(log.append(&a).await));
    }

    // Nothing was written.
    assert_eq!(sessions.list_for_user(user.id).await.unwrap(), Vec::new());
    assert_eq!(tokens.count_for_user(user.id).await.unwrap(), 0);
    assert!(
        log.list(&AuditQuery::first(10))
            .await
            .unwrap()
            .items
            .is_empty()
    );
}
