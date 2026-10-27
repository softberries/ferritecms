//! M2b-3 Task 1: `GET /auth/sessions`, `DELETE /auth/sessions/{id}` and `POST
//! /auth/sessions/revoke-others` (P45).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client},
};

api_tests!(sessions_list_and_revoke, session_routes_refuse_tokens);

const SESSIONS: &str = "/api/v1/auth/sessions";
const ME: &str = "/api/v1/auth/me";

async fn list(api: &Api, c: &mut Client) -> Vec<Value> {
    let r = c.get(api, SESSIONS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    r.json()["items"].as_array().unwrap().clone()
}

async fn sessions_list_and_revoke(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    api.user("ben", Role::Author, false).await;
    let mut first = api.logged_in("ann").await;
    let mut second = api.logged_in("ann").await;
    let mut bens = api.logged_in("ben").await;

    // Own sessions only, newest first, the requesting one marked.
    let items = list(&api, &mut first).await;
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0]["current"], false);
    assert_eq!(items[1]["current"], true);
    for item in &items {
        let id = item["id"].as_str().unwrap();
        assert_eq!(id.len(), 16);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(item["created_at"].is_string() && item["last_seen_at"].is_string());
        // Neither the cookie value nor anything else secret is in the item.
        let keys: Vec<&String> = item.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 4, "{item}");
        let text = item.to_string();
        for cookie in [&first.cookie, &second.cookie] {
            assert!(!text.contains(cookie.as_deref().unwrap()));
        }
    }
    let second_id = items[0]["id"].as_str().unwrap().to_owned();
    let first_id = items[1]["id"].as_str().unwrap().to_owned();
    // The second client sees the same ids with `current` the other way round.
    let theirs = list(&api, &mut second).await;
    assert_eq!(theirs[0]["id"], second_id.as_str());
    assert_eq!(theirs[0]["current"], true);
    assert_eq!(list(&api, &mut bens).await.len(), 1);

    // The current session is ended with sign-out, not here.
    first
        .send(
            &api,
            Method::DELETE,
            &format!("{SESSIONS}/{first_id}"),
            None,
            None,
        )
        .await
        .assert_problem(422, "validation");
    // Unknown ids and other users' sessions are 404.
    let ben_id = list(&api, &mut bens).await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for id in ["0000000000000000", "not-an-id", ben_id.as_str()] {
        first
            .send(
                &api,
                Method::DELETE,
                &format!("{SESSIONS}/{id}"),
                None,
                None,
            )
            .await
            .assert_problem(404, "not-found");
    }
    assert_eq!(bens.get(&api, ME).await.status, StatusCode::OK);

    let r = first
        .send(
            &api,
            Method::DELETE,
            &format!("{SESSIONS}/{second_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    second
        .get(&api, ME)
        .await
        .assert_problem(401, "unauthenticated");
    assert_eq!(list(&api, &mut first).await.len(), 1);

    // Revoke the others: two more logins end, the requesting session stays.
    let mut third = api.logged_in("ann").await;
    let mut fourth = api.logged_in("ann").await;
    let r = first
        .post(&api, &format!("{SESSIONS}/revoke-others"), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json(), json!({"revoked": 2}));
    for c in [&mut third, &mut fourth] {
        c.get(&api, ME).await.assert_problem(401, "unauthenticated");
    }
    let left = list(&api, &mut first).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["id"], first_id.as_str());
    assert_eq!(left[0]["current"], true);
    // Nothing else to revoke.
    let r = first
        .post_empty(&api, &format!("{SESSIONS}/revoke-others"))
        .await;
    assert_eq!(r.json(), json!({"revoked": 0}));
    assert_eq!(bens.get(&api, ME).await.status, StatusCode::OK);

    // Audited as `session.revoke`.
    let ada = api.user("ada", Role::Admin, false).await;
    let admin = api.bearer(&ada, &["*"]).await;
    let audit = admin.get(&api, "/api/v1/system/audit").await.json();
    let rows: Vec<&Value> = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "session.revoke")
        .collect();
    assert_eq!(rows.len(), 3, "{audit}");
    assert!(rows.iter().all(|e| e["actor"]["login"] == "ann"));
    assert!(rows.iter().any(|e| e["details"] == json!({"count": 2})));
    assert!(
        rows.iter()
            .any(|e| e["details"] == json!({"session": second_id}))
    );
    api
}

async fn session_routes_refuse_tokens(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    let token = api.bearer(&ann, &["*"]).await;
    let mut session = api.logged_in("ann").await;
    // Sessions are a cookie credential's business: a bearer token is refused as by
    // `POST /auth/logout`, whatever its scopes.
    token
        .get(&api, SESSIONS)
        .await
        .assert_problem(400, "bad-request");
    token
        .send(
            &api,
            Method::DELETE,
            &format!("{SESSIONS}/0000000000000000"),
            None,
            None,
        )
        .await
        .assert_problem(400, "bad-request");
    token
        .send(
            &api,
            Method::POST,
            &format!("{SESSIONS}/revoke-others"),
            None,
            None,
        )
        .await
        .assert_problem(400, "bad-request");
    // Nothing was revoked.
    assert_eq!(list(&api, &mut session).await.len(), 1);
    api
}
