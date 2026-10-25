//! Task 11: users and their API tokens under `/api/v1/users` (P11, P14, P16, Review
//! Focus 3).

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_core::events::user_key;
use ferrite_domain::{ContentId, Role, User};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    api_tests,
    harness::{Api, Bearer, Client, PASSWORD, Resp, builder, send},
};

api_tests!(
    users_crud_over_http,
    token_shown_once_and_usable,
    role_change_logs_the_user_out,
    editor_cannot_manage_users,
    delete_with_reassign_over_http,
);

const USERS: &str = "/api/v1/users";
const ME: &str = "/api/v1/auth/me";
const NEW_PASSWORD: &str = "a brand new passphrase";

async fn admin(api: &Api) -> (User, Bearer) {
    let ada = api.user("ada", Role::Admin, false).await;
    let token = api.bearer(&ada, &["*"]).await;
    (ada, token)
}

fn user_path(id: impl std::fmt::Display) -> String {
    format!("{USERS}/{id}")
}

/// No secret column ever leaves the API.
fn assert_no_secrets(r: &Resp) {
    for needle in ["password_hash", "totp_secret", "$argon2", "\"password\""] {
        assert!(!r.body.contains(needle), "{needle} in {r:?}");
    }
}

fn logins(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["login"].as_str().unwrap().to_owned())
        .collect()
}

async fn users_crud_over_http(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let mut seen = Vec::new();

    // Create: 201 + Location; the login is normalised.
    let body = json!({
        "login": " Bob ", "email": "Bob@Example.com", "display_name": "Bob B",
        "role": "editor", "password": PASSWORD
    });
    let r = b.send(&api, Method::POST, USERS, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let bob = r.json();
    let bob_id = bob["id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.header("location"),
        Some(user_path(&bob_id).as_str()),
        "{r:?}"
    );
    assert_eq!(bob["login"], "bob");
    assert_eq!(bob["email"], "Bob@Example.com");
    assert_eq!(bob["display_name"], "Bob B");
    assert_eq!(bob["role"], "editor");
    assert_eq!(bob["status"], "active");
    assert_eq!(bob["must_reset"], false);
    assert_eq!(bob["totp_enabled"], false);
    assert!(bob["created_at"].is_string());
    assert_eq!(bob["last_login_at"], Value::Null);
    seen.push(r);

    // Unique login and e-mail (ASCII case-insensitive) are field errors; no constraint
    // names leak.
    for (body, field) in [
        (
            json!({"login": "BOB", "email": "x@example.com", "display_name": "X", "role": "author"}),
            "login",
        ),
        (
            json!({"login": "bob2", "email": "bob@EXAMPLE.com", "display_name": "X", "role": "author"}),
            "email",
        ),
        (
            json!({"login": "-bad", "email": "y@example.com", "display_name": "X", "role": "author"}),
            "login",
        ),
        (
            json!({"login": "ok", "email": "nope", "display_name": "X", "role": "author"}),
            "email",
        ),
        (
            json!({"login": "ok", "email": "ok@example.com", "display_name": " ", "role": "author"}),
            "display_name",
        ),
        (
            json!({"login": "ok", "email": "ok@example.com", "display_name": "Ok", "role": "author", "password": "short"}),
            "password",
        ),
    ] {
        let r = b.send(&api, Method::POST, USERS, Some(&body), None).await;
        let p = r.assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{p}");
        assert!(!r.body.contains("_uq"), "{r:?}");
    }
    let r = b
        .send(
            &api,
            Method::POST,
            USERS,
            Some(&json!({"login": "ok", "email": "ok@example.com", "display_name": "Ok", "role": "root"})),
            None,
        )
        .await;
    r.assert_problem(400, "malformed-json");

    // A user created without a password must reset it.
    let r = b
        .send(
            &api,
            Method::POST,
            USERS,
            Some(&json!({"login": "cy", "email": "cy@example.com", "display_name": "Cy", "role": "author", "must_reset": true})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["must_reset"], true);
    seen.push(r);

    // List by login, with paging and cursor checks.
    let r = b.get(&api, USERS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(logins(&r.json()), ["ada", "bob", "cy"]);
    assert_eq!(r.json()["next_cursor"], Value::Null);
    seen.push(r);
    let r = b.get(&api, &format!("{USERS}?limit=2")).await;
    let page = r.json();
    assert_eq!(logins(&page), ["ada", "bob"]);
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    let r = b
        .get(&api, &format!("{USERS}?limit=2&cursor={cursor}"))
        .await;
    assert_eq!(logins(&r.json()), ["cy"]);
    let p = b
        .get(&api, &format!("{USERS}?cursor=garbage"))
        .await
        .assert_problem(400, "validation");
    assert_eq!(p["errors"][0]["field"], "cursor");
    b.get(&api, &format!("{USERS}?limit=0"))
        .await
        .assert_problem(400, "validation");

    // Get.
    let r = b.get(&api, &user_path(&bob_id)).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["login"], "bob");
    seen.push(r);
    b.get(&api, &user_path(Uuid::now_v7()))
        .await
        .assert_problem(404, "not-found");
    b.get(&api, &user_path("not-a-uuid"))
        .await
        .assert_problem(404, "not-found");

    // Patch: merge; the login never changes (sending it unchanged is fine); null is
    // refused.
    let patch = json!({"display_name": "Bobby", "email": "bobby@example.com", "login": "BOB"});
    let r = b
        .send(&api, Method::PATCH, &user_path(&bob_id), Some(&patch), None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let bob = r.json();
    assert_eq!(bob["display_name"], "Bobby");
    assert_eq!(bob["email"], "bobby@example.com");
    assert_eq!(bob["role"], "editor");
    seen.push(r);
    let r = b
        .send(
            &api,
            Method::PATCH,
            &user_path(&bob_id),
            Some(&json!({"login": "robert"})),
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "login"
    );
    let r = b
        .send(
            &api,
            Method::PATCH,
            &user_path(&bob_id),
            Some(&json!({"email": "ADA@example.com"})),
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "email"
    );
    b.send(
        &api,
        Method::PATCH,
        &user_path(&bob_id),
        Some(&json!({"role": null})),
        None,
    )
    .await
    .assert_problem(400, "malformed-json");
    // Nobody changes their own role.
    b.send(
        &api,
        Method::PATCH,
        &user_path(ada.id),
        Some(&json!({"role": "editor"})),
        None,
    )
    .await
    .assert_problem(422, "validation");

    // Admin password reset: 204, the old password stops working, the new one logs in
    // into a must_reset account.
    let mut bob_client = api.logged_in("bob").await;
    let path = format!("{}/password", user_path(&bob_id));
    let r = b
        .send(
            &api,
            Method::POST,
            &path,
            Some(&json!({"password": NEW_PASSWORD})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_eq!(
        bob_client.get(&api, ME).await.status,
        StatusCode::UNAUTHORIZED
    );
    let mut c = Client::new();
    assert_eq!(
        c.login(&api, "bob", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = c.login(&api, "bob", NEW_PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["user"]["must_reset"], true);
    b.send(
        &api,
        Method::POST,
        &path,
        Some(&json!({"password": "short"})),
        None,
    )
    .await
    .assert_problem(422, "validation");
    // Not for oneself: that is POST /auth/password, with the current password.
    b.send(
        &api,
        Method::POST,
        &format!("{}/password", user_path(ada.id)),
        Some(&json!({"password": NEW_PASSWORD})),
        None,
    )
    .await
    .assert_problem(400, "bad-request");

    // Delete: 204, then 404; deleting oneself is refused.
    let r = b
        .send(&api, Method::DELETE, &user_path(&bob_id), None, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    b.get(&api, &user_path(&bob_id))
        .await
        .assert_problem(404, "not-found");
    b.send(&api, Method::DELETE, &user_path(&bob_id), None, None)
        .await
        .assert_problem(404, "not-found");
    b.send(&api, Method::DELETE, &user_path(ada.id), None, None)
        .await
        .assert_problem(422, "validation");

    for r in &seen {
        assert_no_secrets(r);
    }
    api
}

async fn token_shown_once_and_usable(api: Api) -> Api {
    let al = api.user("al", Role::Author, false).await;
    let mut c = api.logged_in("al").await;
    let tokens = format!("{}/tokens", user_path(al.id));

    let r = c
        .send(
            &api,
            Method::POST,
            &tokens,
            Some(&json!({"name": " ci ", "scopes": ["content:read"]})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let created = r.json();
    let secret = created["token"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("fer_"), "{created}");
    let tid = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.header("location"),
        Some(format!("{tokens}/{tid}").as_str())
    );
    assert_eq!(created["name"], "ci");
    assert_eq!(created["scopes"], json!(["content:read"]));
    assert_eq!(created["expires_at"], Value::Null);
    assert_eq!(created["last_used_at"], Value::Null);
    assert!(created["created_at"].is_string());

    // The token works, narrowed to its scope.
    let bearer = Bearer(secret.clone());
    let r = bearer.get(&api, ME).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["scopes"], json!(["content:read"]));
    // It cannot mint a wider token for itself (no users:write), nor read its own list.
    bearer
        .send(
            &api,
            Method::POST,
            &tokens,
            Some(&json!({"name": "wider", "scopes": ["content:write"]})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    bearer
        .get(&api, &tokens)
        .await
        .assert_problem(403, "forbidden");

    // The list shows metadata, never the secret or its hash.
    let r = c.get(&api, &tokens).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = r.json();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], tid.as_str());
    assert_eq!(list["items"][0]["name"], "ci");
    assert!(list["items"][0].get("token").is_none(), "{list}");
    assert!(list["items"][0].get("token_hash").is_none(), "{list}");
    assert_eq!(list["next_cursor"], Value::Null);
    assert!(!r.body.contains(&secret), "{r:?}");
    assert!(!r.body.contains(&secret[4..]), "{r:?}");

    // Bad inputs.
    for (body, status, slug) in [
        (json!({"name": "x", "scopes": []}), 422, "validation"),
        (
            json!({"name": "x", "scopes": ["nope:read"]}),
            422,
            "validation",
        ),
        (
            json!({"name": "", "scopes": ["content:read"]}),
            422,
            "validation",
        ),
        (
            json!({"name": "x", "scopes": ["content:read"], "expires_at": "2001-01-01T00:00:00Z"}),
            422,
            "validation",
        ),
        (json!({"name": "x"}), 400, "malformed-json"),
    ] {
        c.send(&api, Method::POST, &tokens, Some(&body), None)
            .await
            .assert_problem(status, slug);
    }

    // Revoke: 204, the token is rejected at once, a second revoke is 404.
    let r = c
        .send(&api, Method::DELETE, &format!("{tokens}/{tid}"), None, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    let r = bearer.get(&api, ME).await;
    r.assert_problem(401, "unauthenticated");
    c.send(&api, Method::DELETE, &format!("{tokens}/{tid}"), None, None)
        .await
        .assert_problem(404, "not-found");
    let r = c.get(&api, &tokens).await;
    assert_eq!(r.json()["items"], json!([]));
    api
}

/// Review Focus 3: a role change deletes the user's sessions, so their cookie is
/// refused on the next request on every node. With Postgres the change is made through
/// a second node (the peer store) and checked on both.
async fn role_change_logs_the_user_out(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let ed = api.user("ed", Role::Editor, false).await;
    let mut c = api.logged_in("ed").await;
    assert_eq!(c.get(&api, ME).await.status, StatusCode::OK);
    let peer = match api.db.peer().await {
        Some(store) => Some(api.router_on(store).await),
        None => None,
    };
    let other = peer.as_ref().unwrap_or(&api.router);
    let cookie = c.cookie.clone().unwrap();
    let me_on = |router| {
        let req = builder(Method::GET, ME)
            .header(header::COOKIE, format!("__Host-ferrite_session={cookie}"))
            .body(Body::empty())
            .unwrap();
        send(router, req)
    };
    assert_eq!(me_on(other).await.status, StatusCode::OK);

    // A display-name change leaves the session alone.
    let req = crate::harness::json_body(
        b.builder(Method::PATCH, &user_path(ed.id)),
        &json!({"display_name": "Edward"}),
    );
    assert_eq!(send(other, req).await.status, StatusCode::OK);
    assert_eq!(me_on(&api.router).await.status, StatusCode::OK);

    let req = crate::harness::json_body(
        b.builder(Method::PATCH, &user_path(ed.id)),
        &json!({"role": "author"}),
    );
    let r = send(other, req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["role"], "author");
    me_on(&api.router)
        .await
        .assert_problem(401, "unauthenticated");
    me_on(other).await.assert_problem(401, "unauthenticated");
    // Logging in again gives the new role.
    let r = Client::new().login(&api, "ed", PASSWORD).await;
    assert_eq!(r.json()["user"]["role"], "author");
    api
}

async fn editor_cannot_manage_users(api: Api) -> Api {
    let (ada, admin_token) = admin(&api).await;
    let ed = api.user("ed", Role::Editor, false).await;
    let b = api.bearer(&ed, &["*"]).await;
    let other = user_path(ada.id);
    let forbidden = |r: Resp| {
        let p = r.assert_problem(403, "forbidden");
        assert_eq!(p["detail"], "missing manage_users", "{p}");
    };
    forbidden(b.get(&api, USERS).await);
    forbidden(
        b.send(
            &api,
            Method::POST,
            USERS,
            Some(&json!({"login": "x", "email": "x@example.com", "display_name": "X", "role": "admin"})),
            None,
        )
        .await,
    );
    forbidden(b.get(&api, &other).await);
    forbidden(
        b.send(
            &api,
            Method::PATCH,
            &other,
            Some(&json!({"display_name": "Hacked"})),
            None,
        )
        .await,
    );
    forbidden(b.send(&api, Method::DELETE, &other, None, None).await);
    forbidden(
        b.send(
            &api,
            Method::POST,
            &format!("{other}/password"),
            Some(&json!({"password": NEW_PASSWORD})),
            None,
        )
        .await,
    );
    forbidden(b.get(&api, &format!("{other}/tokens")).await);
    forbidden(
        b.send(
            &api,
            Method::POST,
            &format!("{other}/tokens"),
            Some(&json!({"name": "steal", "scopes": ["content:read"]})),
            None,
        )
        .await,
    );
    let ada_tokens = admin_token.get(&api, &format!("{other}/tokens")).await;
    let tid = ada_tokens.json()["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    forbidden(
        b.send(
            &api,
            Method::DELETE,
            &format!("{other}/tokens/{tid}"),
            None,
            None,
        )
        .await,
    );
    assert_eq!(
        admin_token.get(&api, ME).await.status,
        StatusCode::OK,
        "the admin's token survived"
    );

    // Oneself: read (with e-mail), edit name and e-mail, but not the role.
    let me = user_path(ed.id);
    let r = b.get(&api, &me).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["email"], "ed@example.com");
    let r = b
        .send(
            &api,
            Method::PATCH,
            &me,
            Some(&json!({"display_name": "Ed E", "role": "editor"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["display_name"], "Ed E");
    b.send(
        &api,
        Method::PATCH,
        &me,
        Some(&json!({"role": "admin"})),
        None,
    )
    .await
    .assert_problem(422, "validation");

    // A narrow admin token cannot read users either.
    let narrow = api.bearer(&ada, &["content:read"]).await;
    let p = narrow
        .get(&api, USERS)
        .await
        .assert_problem(403, "forbidden");
    assert_eq!(p["detail"], "missing users:read");
    api
}

async fn delete_with_reassign_over_http(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let al = api.user("al", Role::Author, false).await;
    let al_token = api.bearer(&al, &["*"]).await;
    let r = al_token
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post",
            Some(&json!({"title": "Al's post"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let post = ContentId::from_uuid(r.json()["id"].as_str().unwrap().parse().unwrap());

    // Content without a reassign target is refused; a malformed or unknown target too.
    let path = user_path(al.id);
    b.send(&api, Method::DELETE, &path, None, None)
        .await
        .assert_problem(422, "validation");
    b.send(
        &api,
        Method::DELETE,
        &format!("{path}?reassign=nope"),
        None,
        None,
    )
    .await
    .assert_problem(400, "malformed-query");
    b.send(
        &api,
        Method::DELETE,
        &format!("{path}?reassign={}", Uuid::now_v7()),
        None,
        None,
    )
    .await
    .assert_problem(422, "validation");
    b.send(
        &api,
        Method::DELETE,
        &format!("{path}?reassign={}", al.id),
        None,
        None,
    )
    .await
    .assert_problem(422, "validation");

    let before = api.store.read().events().max_seq().await.unwrap();
    let r = b
        .send(
            &api,
            Method::DELETE,
            &format!("{path}?reassign={}", ada.id),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    let item = api.store.read().content().get(post).await.unwrap().unwrap();
    assert_eq!(item.author_id, ada.id);
    // Both authors' pages are invalidated (F22).
    let keys: Vec<String> = api
        .store
        .read()
        .events()
        .since(before, 10)
        .await
        .unwrap()
        .into_iter()
        .flat_map(|e| e.keys)
        .collect();
    assert!(keys.contains(&user_key(al.id)), "{keys:?}");
    assert!(keys.contains(&user_key(ada.id)), "{keys:?}");
    b.get(&api, &path).await.assert_problem(404, "not-found");
    al_token
        .get(&api, ME)
        .await
        .assert_problem(401, "unauthenticated");
    api
}
