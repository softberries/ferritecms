//! Contract tests for the audit-log filters (M2b-3 Task 3): `actor`, an exact `action`
//! or a prefix ending in `.`, `target_kind` with an optional `target_id`, each paged
//! with the cursor.

use std::time::Duration;

use ferrite_domain::{AuditId, AuditQuery, NewAudit, UserId};
use ferrite_store::{AuditRepo, StoreError};
use serde_json::json;

use crate::{TestDb, fixtures::fixture_user};

fn row(actor: UserId, action: &str, kind: &str, id: Option<&str>) -> NewAudit {
    NewAudit {
        actor_id: Some(actor),
        action: action.into(),
        target_kind: kind.into(),
        target_id: id.map(str::to_owned),
        details: json!({}),
    }
}

/// Every page of `q` (one row per page), following `next_cursor`.
async fn paged(log: &dyn AuditRepo, mut q: AuditQuery) -> Vec<AuditId> {
    q.limit = 1;
    let mut ids = Vec::new();
    loop {
        let page = log.list(&q).await.unwrap();
        assert!(page.items.len() <= 1);
        ids.extend(page.items.iter().map(|e| e.id));
        match page.next_cursor {
            Some(c) => q.cursor = Some(c),
            None => return ids,
        }
    }
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

/// Each filter alone and combined, newest first, the same rows when paged with the
/// cursor; a prefix is literal (no LIKE wildcards, case-sensitive) and stops at the
/// `.` (`user.` does not match `users.purge`).
pub async fn audit_filters(db: &TestDb) {
    let store = db.store();
    let log = store.read().audit();
    let a = fixture_user(store).await.id;
    let b = fixture_user(store).await.id;
    let rows = [
        row(a, "user.create", "user", Some("u1")),
        row(a, "user.update", "user", Some("u2")),
        row(b, "user.delete", "user", Some("u1")),
        row(a, "settings.update", "settings", None),
        row(b, "users.purge", "user", Some("u1")),
        row(a, "USER.create", "user", Some("u3")),
        row(a, "u_er.x", "user", Some("u3")),
    ];
    let mut ids = Vec::new();
    for r in &rows {
        ids.push(log.append(r).await.unwrap());
        // Distinct `at` on both backends (ties are broken by id, too).
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // Newest first: indexes into `ids`, in the order a listing returns them.
    let want = |idx: &[usize]| idx.iter().map(|&i| ids[i]).collect::<Vec<_>>();
    let q = |actor: Option<UserId>, action: Option<&str>, kind: Option<&str>, id: Option<&str>| {
        AuditQuery {
            actor,
            action: action.map(str::to_owned),
            target_kind: kind.map(str::to_owned),
            target_id: id.map(str::to_owned),
            limit: 200,
            cursor: None,
        }
    };
    let cases = [
        (q(Some(a), None, None, None), want(&[6, 5, 3, 1, 0])),
        (q(Some(b), None, None, None), want(&[4, 2])),
        (q(None, Some("user.update"), None, None), want(&[1])),
        (q(None, Some("user"), None, None), want(&[])),
        (q(None, Some("user."), None, None), want(&[2, 1, 0])),
        (q(None, Some("u_er."), None, None), want(&[6])),
        (q(None, Some("u%."), None, None), want(&[])),
        (q(None, Some("USER."), None, None), want(&[5])),
        (q(None, None, Some("user"), Some("u1")), want(&[4, 2, 0])),
        (q(None, None, Some("settings"), None), want(&[3])),
        (q(None, None, Some("user"), None), want(&[6, 5, 4, 2, 1, 0])),
        (q(Some(b), Some("user."), None, None), want(&[2])),
        (
            q(Some(a), Some("user."), Some("user"), Some("u2")),
            want(&[1]),
        ),
        (q(Some(UserId::new()), None, None, None), want(&[])),
    ];
    for (query, expected) in cases {
        // Only this test's rows: other rows may exist in a shared schema's log.
        let listed: Vec<AuditId> = log
            .list(&query)
            .await
            .unwrap()
            .items
            .iter()
            .map(|e| e.id)
            .filter(|id| ids.contains(id))
            .collect();
        assert_eq!(listed, expected, "{query:?}");
        let pages: Vec<AuditId> = paged(log, query.clone())
            .await
            .into_iter()
            .filter(|id| ids.contains(id))
            .collect();
        assert_eq!(pages, expected, "paged: {query:?}");
    }

    // The filtered rows come back whole.
    let one = log
        .list(&q(None, None, Some("user"), Some("u2")))
        .await
        .unwrap()
        .items;
    assert_eq!(one.len(), 1);
    assert_eq!(
        (one[0].actor_id, one[0].action.as_str()),
        (Some(a), "user.update")
    );

    // `target_id` needs `target_kind`; NUL in a filter is refused; limits stay checked.
    assert!(is_invalid(log.list(&q(None, None, None, Some("u1"))).await));
    assert!(is_invalid(
        log.list(&q(None, Some("a\0."), None, None)).await
    ));
    assert!(is_invalid(
        log.list(&q(None, None, Some("u\0"), None)).await
    ));
    assert!(is_invalid(
        log.list(&q(None, None, Some("user"), Some("\0"))).await
    ));
    for bad in [0, ferrite_domain::MAX_PAGE + 1] {
        let mut query = AuditQuery::first(1);
        query.limit = bad;
        assert!(is_invalid(log.list(&query).await));
    }
    assert_eq!(AuditQuery::first(7).limit, 7);
}
