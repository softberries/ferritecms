//! M2b-2 Task 3: `GET /redirects?q` (a case-insensitive substring of source or target).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::json;

use crate::{
    api_tests,
    harness::{Api, Bearer},
};

api_tests!(redirect_q_param);

const REDIRECTS: &str = "/api/v1/redirects";

/// The sources of a `200` list at `query`.
async fn sources(api: &Api, b: &Bearer, query: &str) -> Vec<String> {
    let r = b.get(api, &format!("{REDIRECTS}?{query}")).await;
    assert_eq!(r.status, StatusCode::OK, "{query}: {r:?}");
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["source"].as_str().unwrap().to_owned())
        .collect()
}

async fn redirect_q_param(api: Api) -> Api {
    let ada = api.user("ada", Role::Admin, false).await;
    let b = api.bearer(&ada, &["*"]).await;
    for (source, target) in [
        ("/50%_off", "/sale"),
        ("/50xxoff", "/sale"),
        ("/old-news", "https://Example.org/News"),
        ("/about-us", "/about/"),
    ] {
        let body = json!({"source": source, "target": target, "code": 301});
        let r = b
            .send(&api, Method::POST, REDIRECTS, Some(&body), None)
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    }
    assert_eq!(sources(&api, &b, "q=50%25_off").await, ["/50%_off"]);
    assert_eq!(sources(&api, &b, "q=%25").await, ["/50%_off"]);
    // Case-insensitive, on the target too; trimmed.
    assert_eq!(
        sources(&api, &b, "q=EXAMPLE.ORG%2Fnews").await,
        ["/old-news"]
    );
    assert_eq!(sources(&api, &b, "q=%20about%20").await, ["/about-us"]);
    assert_eq!(sources(&api, &b, "q=news&limit=1").await, ["/old-news"]);
    assert!(sources(&api, &b, "q=missing").await.is_empty());
    // Paging within a search keeps the source order.
    let r = b.get(&api, &format!("{REDIRECTS}?q=sale&limit=1")).await;
    let page = r.json();
    assert_eq!(page["items"][0]["source"], "/50%_off");
    let next = page["next_cursor"].as_str().unwrap();
    assert_eq!(
        sources(&api, &b, &format!("q=sale&limit=1&cursor={next}")).await,
        ["/50xxoff"]
    );

    // 1..=200 characters after trimming.
    let p = b
        .get(&api, &format!("{REDIRECTS}?q={}", "a".repeat(201)))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "q");
    let p = b
        .get(&api, &format!("{REDIRECTS}?q=%20%20"))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "q");
    assert_eq!(
        sources(&api, &b, &format!("q={}", "a".repeat(200))).await,
        Vec::<String>::new()
    );
    api
}
