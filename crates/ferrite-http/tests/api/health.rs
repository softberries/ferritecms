//! M6-4 task 9: `/api/v1/health/*` (design 16 §10): the summary, the findings page with
//! filters, "Run now" and its 409, and the `edit_others` rule.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{Method, StatusCode};
use ferrite_core::quality::HealthRunner;
use ferrite_domain::{HealthTrigger, Role};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
};

api_tests!(
    summary_and_findings_need_edit_others,
    manual_run_202_then_409_while_queued,
    findings_filters_type_locale_severity,
);

async fn editor(api: &Api) -> Bearer {
    let ed = api.user("ed", Role::Editor, false).await;
    api.bearer(&ed, &["*"]).await
}

async fn post(api: &Api, b: &Bearer, kind: &str, body: Value) {
    let r = b
        .send(
            api,
            Method::POST,
            &format!("/api/v1/content/{kind}"),
            Some(&json!({"title": "Hello", "body": body})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let item = r.json();
    let id = item["id"].as_str().unwrap();
    let v = item["version"].as_i64().unwrap();
    let r = b
        .send(
            api,
            Method::POST,
            &format!("/api/v1/content/{kind}/{id}/publish"),
            None,
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
}

async fn run_now(api: &Api) {
    HealthRunner::new(
        api.store.clone(),
        "https://example.com".into(),
        Arc::new(ferrite_http::SiteProbe::new(api.app.clone())),
        None,
        "test".into(),
    )
    .run(
        HealthTrigger::Manual,
        None,
        Instant::now() + Duration::from_secs(60),
    )
    .await
    .unwrap();
}

async fn summary_and_findings_need_edit_others(api: Api) -> Api {
    let au = api.user("au", Role::Author, false).await;
    let b = api.bearer(&au, &["*"]).await;
    for path in ["/api/v1/health/summary", "/api/v1/health/findings"] {
        b.get(&api, path).await.assert_problem(403, "forbidden");
    }
    b.send(&api, Method::POST, "/api/v1/health/runs", None, None)
        .await
        .assert_problem(403, "forbidden");
    // Never run: an empty report.
    let e = editor(&api).await;
    let s = e.get(&api, "/api/v1/health/summary").await;
    assert_eq!(s.status, StatusCode::OK, "{s:?}");
    let s = s.json();
    assert_eq!(s["last"], Value::Null);
    assert_eq!(s["external_links"], false);
    assert!(s["next_at"].as_str().unwrap().contains("T04:00:00"));
    let f = e.get(&api, "/api/v1/health/findings").await.json();
    assert_eq!(f["items"], json!([]));
    // A read-only token may read, not run.
    let ed = api.user("ed2", Role::Editor, false).await;
    let ro = api.bearer(&ed, &["content:read"]).await;
    assert_eq!(
        ro.get(&api, "/api/v1/health/summary").await.status,
        StatusCode::OK
    );
    ro.send(&api, Method::POST, "/api/v1/health/runs", None, None)
        .await
        .assert_problem(403, "forbidden");
    api
}

async fn manual_run_202_then_409_while_queued(api: Api) -> Api {
    let e = editor(&api).await;
    let r = e
        .send(&api, Method::POST, "/api/v1/health/runs", None, None)
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    assert!(r.json()["job_id"].as_str().is_some());
    e.send(&api, Method::POST, "/api/v1/health/runs", None, None)
        .await
        .assert_problem(409, "conflict");
    api
}

async fn findings_filters_type_locale_severity(api: Api) -> Api {
    let e = editor(&api).await;
    post(&api, &e, "post", json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"src": "/a.png"}},
        {"type": "heading", "attrs": {"level": 4}, "content": [{"type": "text", "text": "Deep"}]}
    ]}))
    .await;
    post(
        &api,
        &e,
        "page",
        json!({"type": "doc", "content": [
            {"type": "image", "attrs": {"src": "/b.png", "alt": ""}}
        ]}),
    )
    .await;
    run_now(&api).await;
    let s = e.get(&api, "/api/v1/health/summary").await.json();
    assert_eq!(s["last"]["status"], "done");
    assert_eq!(s["last"]["items_checked"], 2);
    let get = |q: &str| {
        let e = &e;
        let api = &api;
        let q = q.to_owned();
        async move {
            e.get(api, &format!("/api/v1/health/findings{q}"))
                .await
                .json()
        }
    };
    let errors = get("?severity=error").await;
    let rules: Vec<&str> = errors["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["image-alt-missing"]);
    let item = &errors["items"][0]["item"];
    assert_eq!(item["type"], "post");
    assert_eq!(item["locale"], "en");
    assert!(
        item["admin_url"]
            .as_str()
            .unwrap()
            .starts_with("/admin/content/post/")
    );
    let pages = get("?type=page").await;
    assert!(
        pages["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["item"]["type"] == "page")
    );
    let none = get("?locale=de").await;
    assert_eq!(none["items"], json!([]));
    let paged = get("?limit=1").await;
    assert_eq!(paged["items"].as_array().unwrap().len(), 1);
    assert!(paged["next_cursor"].is_string());
    e.get(&api, "/api/v1/health/findings?severity=bad")
        .await
        .assert_problem(422, "validation");
    e.get(&api, "/api/v1/health/findings?cursor=nope")
        .await
        .assert_problem(400, "validation");
    api
}
