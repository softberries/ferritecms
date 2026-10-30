//! `GET /api/v1/system/metrics/series` (M4-2 P35): the `node_metrics` rows summed over
//! nodes into dense minute, hour or day buckets, for `system:read` only.

use axum::http::StatusCode;
use ferrite_domain::{MINUTE_MICROS, MetricCounts, NodeId, Role, Timestamp};
use serde_json::Value;

use crate::{api_tests, harness::Api};

api_tests!(
    series_requires_system_read,
    series_step_limits_422,
    series_empty_range,
);

const SERIES: &str = "/api/v1/system/metrics/series";
const HOUR: i64 = 60 * MINUTE_MICROS;
const DAY: i64 = 24 * HOUR;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

/// RFC 3339 in UTC with a `Z` (no `+` to escape in a query).
fn rfc(micros: i64) -> String {
    at(micros)
        .as_datetime()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn query(from: i64, to: i64, step: &str) -> String {
    format!("{SERIES}?from={}&to={}&step={step}", rfc(from), rfc(to))
}

/// A whole day a day ago, so every minute below is in the past and in one day bucket.
fn day_start() -> i64 {
    Timestamp::now().as_micros() / DAY * DAY - DAY
}

/// Admins and `system:read` tokens read the series; anonymous callers, editors and
/// tokens without the scope do not.
async fn series_requires_system_read(api: Api) -> Api {
    let t0 = day_start();
    let path = query(t0, t0 + HOUR, "minute");
    api.get(&path).await.assert_problem(401, "unauthenticated");
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    let p = ed.get(&api, &path).await.assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_system"),
        "{p}"
    );
    let root = api.user("root", Role::Admin, false).await;
    let content = api.bearer(&root, &["content:read"]).await;
    let p = content
        .get(&api, &path)
        .await
        .assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("system:read"), "{p}");
    let reader = api.bearer(&root, &["system:read"]).await;
    let r = reader.get(&api, &path).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let mut admin = api.logged_in("root").await;
    let r = admin.get(&api, &path).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

/// Minute series span at most 24 h, hour and day series 7 days (`422`, field `step`);
/// `from` must be before `to` (`422`, field `to`); malformed values are `400`.
async fn series_step_limits_422(api: Api) -> Api {
    api.user("root", Role::Admin, false).await;
    let mut c = api.logged_in("root").await;
    let t0 = day_start() - 7 * DAY;
    for (to, step, ok) in [
        (t0 + DAY, "minute", true),
        (t0 + DAY + MINUTE_MICROS, "minute", false),
        (t0 + 7 * DAY, "hour", true),
        (t0 + 7 * DAY + HOUR, "hour", false),
        (t0 + 7 * DAY, "day", true),
        (t0 + 8 * DAY, "day", false),
    ] {
        let r = c.get(&api, &query(t0, to, step)).await;
        if ok {
            assert_eq!(r.status, StatusCode::OK, "{step}: {r:?}");
        } else {
            let p = r.assert_problem(422, "validation");
            assert_eq!(p["errors"][0]["field"], "step", "{step}: {p}");
        }
    }
    for (from, to) in [(t0, t0), (t0 + HOUR, t0)] {
        let p = c
            .get(&api, &query(from, to, "hour"))
            .await
            .assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], "to", "{p}");
    }
    let good = (rfc(t0), rfc(t0 + HOUR));
    for (q, field) in [
        (
            format!("{SERIES}?from={}&to={}&step=week", good.0, good.1),
            "step",
        ),
        (
            format!("{SERIES}?from=yesterday&to={}&step=hour", good.1),
            "from",
        ),
        (format!("{SERIES}?from={}&to=now&step=hour", good.0), "to"),
        (
            format!("{SERIES}?from={}&to={}&step=hour&node=nope", good.0, good.1),
            "node",
        ),
    ] {
        let p = c.get(&api, &q).await.assert_problem(400, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{q}: {p}");
    }
    c.get(&api, &format!("{SERIES}?from={}&step=hour", good.0))
        .await
        .assert_problem(400, "malformed-query");
    api
}

fn sums(body: &Value, key: &str) -> Vec<u64> {
    body["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b[key].as_u64().unwrap())
        .collect()
}

/// A range without rows is a dense series of zeros; rows of two nodes are summed into
/// their buckets (the others stay zero), `p95_ms` is the larger, `node` filters, and
/// the range is widened to whole buckets.
async fn series_empty_range(api: Api) -> Api {
    api.user("root", Role::Admin, false).await;
    let mut c = api.logged_in("root").await;
    let t0 = day_start();

    let r = c
        .get(&api, &query(t0, t0 + 10 * MINUTE_MICROS, "minute"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    assert_eq!(body["step"], "minute", "{body}");
    let buckets = body["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 10, "{body}");
    for (i, b) in buckets.iter().enumerate() {
        let start = chrono::DateTime::parse_from_rfc3339(b["start"].as_str().unwrap()).unwrap();
        assert_eq!(
            start.timestamp_micros(),
            t0 + i64::try_from(i).unwrap() * MINUTE_MICROS
        );
        for key in [
            "requests",
            "errors_5xx",
            "page_views",
            "cache_hits",
            "cache_misses",
            "plugin_calls",
            "plugin_failures",
            "jobs_done",
            "jobs_failed",
            "p95_ms",
        ] {
            assert_eq!(b[key], 0, "{key}: {b}");
        }
    }

    let m = api.store.read().node_metrics();
    let (a, b) = (NodeId::new(), NodeId::new());
    let row = |n: u64, p95_ms: u32| MetricCounts::from_counters([n; 9], p95_ms);
    m.insert(a, at(t0 + 2 * MINUTE_MICROS), &row(3, 25))
        .await
        .unwrap();
    m.insert(b, at(t0 + 2 * MINUTE_MICROS), &row(4, 100))
        .await
        .unwrap();
    m.insert(b, at(t0 + 5 * MINUTE_MICROS), &row(1, 5))
        .await
        .unwrap();
    let body = c
        .get(&api, &query(t0, t0 + 10 * MINUTE_MICROS, "minute"))
        .await
        .json();
    assert_eq!(sums(&body, "requests"), [0, 0, 7, 0, 0, 1, 0, 0, 0, 0]);
    assert_eq!(sums(&body, "p95_ms"), [0, 0, 100, 0, 0, 5, 0, 0, 0, 0]);
    let only_a = c
        .get(
            &api,
            &format!("{}&node={a}", query(t0, t0 + 10 * MINUTE_MICROS, "minute")),
        )
        .await
        .json();
    assert_eq!(sums(&only_a, "page_views"), [0, 0, 3, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(only_a["node"], a.to_string(), "{only_a}");

    // An unaligned range covers the whole buckets it touches.
    let hours = c
        .get(&api, &query(t0 + 30 * MINUTE_MICROS, t0 + HOUR + 1, "hour"))
        .await
        .json();
    assert_eq!(sums(&hours, "requests"), [8, 0], "{hours}");
    let from = chrono::DateTime::parse_from_rfc3339(hours["from"].as_str().unwrap()).unwrap();
    let to = chrono::DateTime::parse_from_rfc3339(hours["to"].as_str().unwrap()).unwrap();
    assert_eq!(
        (from.timestamp_micros(), to.timestamp_micros()),
        (t0, t0 + 2 * HOUR)
    );
    api
}
