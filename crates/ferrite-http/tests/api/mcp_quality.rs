//! M6-4 task 10: the MCP tools `content_check` and `health_report` (design 16 §10), the
//! findings of a refused `content_publish`, and the writing guide.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{Method, StatusCode};
use ferrite_core::quality::HealthRunner;
use ferrite_domain::{HealthTrigger, Role};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
    mcp::{mcp_call, text, tool},
};

api_tests!(
    content_check_markdown_draft_finds_missing_alt,
    content_check_by_id_applies_suppressions,
    content_publish_blocked_lists_findings,
    health_report_needs_edit_others,
    health_report_pages,
    tools_list_has_both_with_annotations,
    writing_guide_mentions_content_check,
);

async fn editor(api: &Api) -> Bearer {
    let u = api.user("eddie", Role::Editor, false).await;
    api.bearer(&u, &["*"]).await
}

fn ok(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

async fn create(api: &Api, b: &Bearer, markdown: &str) -> Value {
    let r = tool(
        api,
        &b.0,
        "content_create",
        json!({"type": "post", "title": "Lakes", "body_markdown": markdown}),
    )
    .await;
    ok(&r).clone()
}

async fn content_check_markdown_draft_finds_missing_alt(api: Api) -> Api {
    let b = editor(&api).await;
    let r = tool(
        &api,
        &b.0,
        "content_check",
        json!({"type": "post", "title": "T", "body_markdown": "Intro.\n\n![](/a.png)\n\n[click here](/x/)\n"}),
    )
    .await;
    let out = ok(&r);
    let rules: Vec<&str> = out["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(rules.contains(&"image-alt-empty"), "{rules:?}");
    assert!(rules.contains(&"link-text-vague"), "{rules:?}");
    let f = &out["findings"][0];
    assert!(f["message"].as_str().unwrap().len() > 5);
    assert!(f["fix"].as_str().unwrap().len() > 5);
    assert!(text(&r).contains("warnings"), "{}", text(&r));
    // Exactly one of id and type.
    let r = tool(&api, &b.0, "content_check", json!({})).await;
    assert_eq!(r["isError"], true);
    api
}

async fn content_check_by_id_applies_suppressions(api: Api) -> Api {
    let b = editor(&api).await;
    let item = create(&api, &b, "![A lake](/a.png)\n\n![](/b.png)\n").await;
    let id = item["id"].as_str().unwrap();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{id}/suppressions"),
            Some(&json!({"rule": "image-alt-empty", "key": "/b.png", "reason": "divider"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = tool(&api, &b.0, "content_check", json!({"id": id})).await;
    let out = ok(&r);
    assert_eq!(out["counts"]["suppressed"], 1);
    assert_eq!(out["findings"][0]["suppressed"]["reason"], "divider");
    api
}

async fn content_publish_blocked_lists_findings(api: Api) -> Api {
    let admin = api.user("ada", Role::Admin, false).await;
    let a = api.bearer(&admin, &["*"]).await;
    let r = a
        .send(
            &api,
            Method::PATCH,
            "/api/v1/types/post",
            Some(&json!({"quality_gate": "errors"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let b = editor(&api).await;
    let item = create(
        &api,
        &b,
        "Text.\n\n```ferrite-block\n{\"type\":\"image\",\"attrs\":{\"src\":\"/a.png\"}}\n```\n",
    )
    .await;
    let r = tool(
        &api,
        &b.0,
        "content_publish",
        json!({"type": "post", "id": item["id"], "version": item["version"]}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    assert_eq!(
        r["structuredContent"]["findings"][0]["rule"],
        "image-alt-missing"
    );
    assert!(text(&r).contains("image-alt-missing"), "{}", text(&r));
    api
}

async fn health_report_needs_edit_others(api: Api) -> Api {
    let u = api.user("au", Role::Author, false).await;
    let t = api.bearer(&u, &["*"]).await;
    let r = tool(&api, &t.0, "health_report", json!({})).await;
    assert_eq!(r["isError"], true, "{r}");
    assert!(text(&r).contains("edit_others"), "{}", text(&r));
    api
}

async fn health_report_pages(api: Api) -> Api {
    let b = editor(&api).await;
    let r = tool(&api, &b.0, "health_report", json!({})).await;
    assert!(text(&r).contains("No content health run"), "{}", text(&r));
    for i in 0..3 {
        let item = create(&api, &b, "![](/a.png)\n").await;
        let r = tool(
            &api,
            &b.0,
            "content_publish",
            json!({"type": "post", "id": item["id"], "version": item["version"]}),
        )
        .await;
        assert_eq!(r["isError"], false, "{i}: {r}");
    }
    HealthRunner::new(
        api.store.clone(),
        "https://example.com".into(),
        Arc::new(ferrite_http::SiteProbe::new(api.app.clone())),
        None,
        "t".into(),
    )
    .run(
        HealthTrigger::Manual,
        None,
        Instant::now() + Duration::from_secs(60),
    )
    .await
    .unwrap();
    let r = tool(
        &api,
        &b.0,
        "health_report",
        json!({"per_page": 2, "rule": "image-alt-empty"}),
    )
    .await;
    let out = ok(&r);
    assert_eq!(out["findings"].as_array().unwrap().len(), 2);
    assert!(out["next_cursor"].is_string());
    assert_eq!(out["summary"]["last"]["status"], "done");
    assert!(out["findings"][0]["message"].is_string());
    let r = tool(
        &api,
        &b.0,
        "health_report",
        json!({"per_page": 2, "rule": "image-alt-empty", "cursor": out["next_cursor"]}),
    )
    .await;
    assert_eq!(ok(&r)["findings"].as_array().unwrap().len(), 1);
    api
}

async fn tools_list_has_both_with_annotations(api: Api) -> Api {
    let b = editor(&api).await;
    let (_, body) = mcp_call(&api, &b.0, "tools/list", json!({})).await;
    let tools = body["result"]["tools"].as_array().unwrap();
    for name in ["content_check", "health_report"] {
        let t = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{name}");
    }
    let desc = tools.iter().find(|t| t["name"] == "content_check").unwrap()["description"]
        .as_str()
        .unwrap();
    assert!(desc.contains("content_publish"), "{desc}");
    api
}

async fn writing_guide_mentions_content_check(api: Api) -> Api {
    let b = editor(&api).await;
    let (_, body) = mcp_call(
        &api,
        &b.0,
        "resources/read",
        json!({"uri": "ferrite://guide/writing"}),
    )
    .await;
    let guide = body["result"]["contents"][0]["text"].as_str().unwrap();
    for needle in ["content_check", "alt text", "## Accessibility"] {
        assert!(guide.contains(needle), "{needle}: {guide}");
    }
    api
}
