//! M2b-3 Task 1: `GET /users` filters (P46), `POST /users/{id}/totp/disable` (P43) and the
//! password an own e-mail change needs (P44).

use std::time::Duration;

use axum::http::{Method, StatusCode};
use ferrite_domain::{Role, Secret};
use serde_json::json;

use crate::{
    api_tests,
    harness::{Api, Bearer, PASSWORD},
};

api_tests!(
    users_filter_params,
    admin_resets_another_users_totp,
    own_email_change_needs_password
);

const USERS: &str = "/api/v1/users";

/// The logins of a `200` list at `query`.
async fn logins(api: &Api, b: &Bearer, query: &str) -> Vec<String> {
    let r = b.get(api, &format!("{USERS}?{query}")).await;
    assert_eq!(r.status, StatusCode::OK, "{query}: {r:?}");
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["login"].as_str().unwrap().to_owned())
        .collect()
}

async fn users_filter_params(api: Api) -> Api {
    let ada = api.user("ada", Role::Admin, false).await;
    let b = api.bearer(&ada, &["*"]).await;
    api.user("bob", Role::Editor, false).await;
    let cy = api.user("cy", Role::Editor, false).await;
    api.user("dee", Role::Author, false).await;
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("{USERS}/{}", cy.id),
            Some(&json!({"status": "disabled", "display_name": "Cy 50%_off"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // `q` over login, display name and e-mail, case-insensitive, wildcards literal.
    assert_eq!(logins(&api, &b, "q=BOB").await, ["bob"]);
    assert_eq!(logins(&api, &b, "q=50%25_off").await, ["cy"]);
    assert_eq!(logins(&api, &b, "q=%25").await, ["cy"]);
    assert_eq!(
        logins(&api, &b, "q=%20example.com%20").await,
        ["ada", "bob", "cy", "dee"]
    );
    assert_eq!(logins(&api, &b, "role=editor").await, ["bob", "cy"]);
    assert_eq!(logins(&api, &b, "status=disabled").await, ["cy"]);
    assert_eq!(
        logins(&api, &b, "role=editor&status=active&q=b").await,
        ["bob"]
    );
    // The cursor carries through a filtered list.
    let r = b.get(&api, &format!("{USERS}?role=editor&limit=1")).await;
    let page = r.json();
    assert_eq!(page["items"][0]["login"], "bob");
    let next = page["next_cursor"].as_str().unwrap();
    assert_eq!(
        logins(&api, &b, &format!("role=editor&limit=1&cursor={next}")).await,
        ["cy"]
    );

    // `q`: 1..=100 characters after trimming (422 on `q`).
    for bad in ["q=%20%20".to_owned(), format!("q={}", "a".repeat(101))] {
        let p = b
            .get(&api, &format!("{USERS}?{bad}"))
            .await
            .assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], "q", "{bad}");
    }
    assert!(
        logins(&api, &b, &format!("q={}", "a".repeat(100)))
            .await
            .is_empty()
    );
    // An unknown role or status is a malformed query.
    b.get(&api, &format!("{USERS}?role=owner"))
        .await
        .assert_problem(400, "malformed-query");
    b.get(&api, &format!("{USERS}?status=gone"))
        .await
        .assert_problem(400, "malformed-query");
    api
}

async fn admin_resets_another_users_totp(api: Api) -> Api {
    let ada = api.user("ada", Role::Admin, false).await;
    let tess = api.user("tess", Role::Author, false).await;
    let ed = api.user("ed", Role::Editor, false).await;
    // Tess logs in before TOTP is on, and stays logged in through the reset (P43).
    let mut tess_c = api.logged_in("tess").await;
    api.store
        .read()
        .users()
        .set_totp(tess.id, Some(&Secret::new("sealed")))
        .await
        .unwrap();
    let path = format!("{USERS}/{}/totp/disable", tess.id);

    // An editor may not; a token without `users:write` may not.
    let ed_token = api.bearer(&ed, &["*"]).await;
    ed_token
        .send(&api, Method::POST, &path, None, None)
        .await
        .assert_problem(403, "forbidden");
    let read_only = api.bearer(&ada, &["users:read"]).await;
    read_only
        .send(&api, Method::POST, &path, None, None)
        .await
        .assert_problem(403, "forbidden");

    let mut c = api.logged_in("ada").await;
    let r = c.send(&api, Method::POST, &path, None, None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    let r = c.get(&api, &format!("{USERS}/{}", tess.id)).await;
    assert_eq!(r.json()["totp_enabled"], false);
    assert_eq!(
        tess_c.get(&api, "/api/v1/auth/me").await.status,
        StatusCode::OK
    );
    // Again: TOTP is not on.
    c.send(&api, Method::POST, &path, None, None)
        .await
        .assert_problem(422, "validation");
    // Oneself: 422 on `id`.
    let p = c
        .send(
            &api,
            Method::POST,
            &format!("{USERS}/{}/totp/disable", ada.id),
            None,
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "id");
    c.send(
        &api,
        Method::POST,
        &format!("{USERS}/0192b7a0-0000-7000-8000-000000000000/totp/disable"),
        None,
        None,
    )
    .await
    .assert_problem(404, "not-found");

    let r = c.get(&api, "/api/v1/system/audit").await;
    let audit = r.json();
    let row = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "user.totp_disable")
        .unwrap()
        .clone();
    assert_eq!(row["actor"]["login"], "ada");
    assert_eq!(row["target_id"], tess.id.to_string());
    api
}

async fn own_email_change_needs_password(api: Api) -> Api {
    let ada = api.user("ada", Role::Admin, false).await;
    let ann = api.user("ann", Role::Author, false).await;
    api.align_to_window(Duration::from_secs(40)).await;
    // The login takes one slot of ann's per-login bucket (5/min).
    let mut c = api.logged_in("ann").await;
    let me = format!("{USERS}/{}", ann.id);

    let p = c
        .send(
            &api,
            Method::PATCH,
            &me,
            Some(&json!({"email": "new@example.com"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "current_password");
    // The current e-mail and a display name need no password.
    let r = c
        .send(
            &api,
            Method::PATCH,
            &me,
            Some(&json!({"email": "ann@example.com", "display_name": "Ann B"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = c
        .send(
            &api,
            Method::PATCH,
            &me,
            Some(&json!({"email": "new@example.com", "current_password": PASSWORD})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["email"], "new@example.com");

    // An admin changes another user's e-mail without a password.
    let admin = api.bearer(&ada, &["*"]).await;
    let r = admin
        .send(
            &api,
            Method::PATCH,
            &me,
            Some(&json!({"email": "third@example.com"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // Wrong passwords are 403 and share the per-login bucket with /auth/password: the
    // login and the right password above took two slots, three more are left.
    for _ in 0..3 {
        c.send(
            &api,
            Method::PATCH,
            &me,
            Some(&json!({"email": "x@example.com", "current_password": "wrong password!!"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    }
    c.send(
        &api,
        Method::PATCH,
        &me,
        Some(&json!({"email": "x@example.com", "current_password": PASSWORD})),
        None,
    )
    .await
    .assert_problem(429, "rate-limited");
    c.post(
        &api,
        "/api/v1/auth/password",
        json!({"current": PASSWORD, "new": "another long password"}),
    )
    .await
    .assert_problem(429, "rate-limited");
    let r = admin.get(&api, &me).await;
    assert_eq!(r.json()["email"], "third@example.com");
    api
}
