//! M2b-2 Task 3: `GET /menus` lists the menu locations of the theme this node serves.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use ferrite_theme::{Theme, ThemeSource};
use serde_json::json;

use crate::{api_tests, harness::Api};

api_tests!(menu_list_has_theme_locations);

async fn menu_list_has_theme_locations(api: Api) -> Api {
    let ada = api.user("ada", Role::Admin, false).await;
    let b = api.bearer(&ada, &["*"]).await;
    // The fixture theme declares `primary` only.
    let r = b.get(&api, "/api/v1/menus").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.json()["locations"],
        json!([{"name": "primary", "label": "Primary navigation", "menu": false}])
    );

    // The node now serves the embedded `blog` theme.
    api.app.swap_theme(Arc::new(
        Theme::load(&ThemeSource::Embedded("blog")).unwrap(),
    ));
    let r = b.get(&api, "/api/v1/menus").await;
    assert_eq!(
        r.json(),
        json!({
            "items": [],
            "next_cursor": null,
            "locations": [
                {"name": "footer", "label": "Footer", "menu": false},
                {"name": "primary", "label": "Primary navigation", "menu": false},
            ],
        })
    );

    let body = json!({"label": "Main", "items": []});
    let r = b
        .send(
            &api,
            Method::PUT,
            "/api/v1/menus/primary",
            Some(&body),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    // A menu no location names is listed, not located.
    let r = b
        .send(
            &api,
            Method::PUT,
            "/api/v1/menus/sidebar",
            Some(&body),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = b.get(&api, "/api/v1/menus").await.json();
    assert_eq!(
        list["locations"],
        json!([
            {"name": "footer", "label": "Footer", "menu": false},
            {"name": "primary", "label": "Primary navigation", "menu": true},
        ])
    );
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    // Reads need menus:read.
    let no_scope = api.bearer(&ada, &["content:read"]).await;
    no_scope
        .get(&api, "/api/v1/menus")
        .await
        .assert_problem(403, "forbidden");
    api
}
