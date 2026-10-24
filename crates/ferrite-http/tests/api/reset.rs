//! Task 10: password reset by e-mail over HTTP (M2a-2 P19, P28): `POST
//! /auth/password-reset` and `POST /auth/password-reset/confirm`, both origin-checked
//! like login and open without credentials.

use std::time::Duration;

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_domain::Role;
use serde_json::json;

use crate::{
    api_tests,
    harness::{Api, Client, PASSWORD, SITE_URL, builder, json_body, send},
};

api_tests!(
    password_reset_over_http,
    reset_checks_origin,
    reset_without_mailer_is_503,
);

const RESET: &str = "/api/v1/auth/password-reset";
const CONFIRM: &str = "/api/v1/auth/password-reset/confirm";
const NEW_PASSWORD: &str = "a reset passphrase";
const EVIL: &str = "https://evil.example";
const MAIL_WAIT: Duration = Duration::from_secs(5);

/// The token in the fragment of the reset link in `text`.
fn token_in(text: &str) -> String {
    let base = format!("{SITE_URL}/admin/reset-password#token=");
    let link = text
        .split_whitespace()
        .find(|w| w.starts_with(&base))
        .unwrap_or_else(|| panic!("no reset link in {text:?}"));
    link[base.len()..].to_owned()
}

async fn password_reset_over_http(api: Api) -> Api {
    api.user("kim", Role::Author, false).await;
    let mut c = Client::new();
    // Known and unknown accounts get the same answer: 202, no body.
    let known = c
        .post(&api, RESET, json!({"login": "kim@example.com"}))
        .await;
    let unknown = c.post(&api, RESET, json!({"login": "nobody"})).await;
    for r in [&known, &unknown] {
        assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
        assert_eq!(r.body, "", "{r:?}");
        assert!(r.header("set-cookie").is_none(), "{r:?}");
        assert_eq!(r.header("cache-control"), Some("no-store"), "{r:?}");
    }
    assert_eq!(known.header("content-type"), unknown.header("content-type"));
    let sent = api.mailer.wait_for(1, MAIL_WAIT).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].to, ["kim@example.com"]);
    let token = token_in(&sent[0].text);

    // A policy failure names the password and leaves the token usable.
    let r = c
        .post(&api, CONFIRM, json!({"token": token, "password": "short"}))
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "password", "{p}");

    let r = c
        .post(
            &api,
            CONFIRM,
            json!({"token": token, "password": NEW_PASSWORD}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert!(
        r.header("set-cookie").is_none(),
        "no session is issued: {r:?}"
    );

    // Used: refused, naming the token.
    let r = c
        .post(
            &api,
            CONFIRM,
            json!({"token": token, "password": NEW_PASSWORD}),
        )
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "token", "{p}");

    // The new password logs in, the old one does not.
    let r = Client::new().login(&api, "kim", PASSWORD).await;
    r.assert_problem(401, "invalid-credentials");
    let r = Client::new().login(&api, "kim", NEW_PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

async fn reset_checks_origin(api: Api) -> Api {
    api.user("kim", Role::Author, false).await;
    let mut evil = Client::with_origin(EVIL);
    for (path, body) in [
        (RESET, json!({"login": "kim"})),
        (CONFIRM, json!({"token": "v1.x", "password": NEW_PASSWORD})),
    ] {
        let r = evil.post(&api, path, body.clone()).await;
        r.assert_problem(403, "csrf");
        // No `Origin` and no `Referer`: refused too.
        let req = json_body(builder(Method::POST, path), &body);
        send(&api.router, req).await.assert_problem(403, "csrf");
        // The origin is checked before the body is read.
        let req = builder(Method::POST, path)
            .header(header::ORIGIN, EVIL)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{not json"))
            .unwrap();
        send(&api.router, req).await.assert_problem(403, "csrf");
    }
    // Nothing was sent; a same-origin request then is.
    let r = Client::new()
        .post(&api, RESET, json!({"login": "kim"}))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    assert_eq!(api.mailer.wait_for(1, MAIL_WAIT).await.len(), 1);
    assert_eq!(
        api.mailer
            .wait_for(2, Duration::from_millis(200))
            .await
            .len(),
        1
    );
    api
}

async fn reset_without_mailer_is_503(api: Api) -> Api {
    api.user("kim", Role::Author, false).await;
    let router = api.router_without_mail();
    let mut c = Client::new();
    let req = json_body(c.builder(Method::POST, RESET), &json!({"login": "kim"}));
    let r = c.send_on(&router, req).await;
    let p = r.assert_problem(503, "mail-unavailable");
    assert!(p["detail"].as_str().unwrap().contains("mail"), "{p}");
    assert!(
        api.mailer
            .wait_for(1, Duration::from_millis(200))
            .await
            .is_empty()
    );
    api
}
