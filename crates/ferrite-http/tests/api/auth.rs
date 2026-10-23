//! Task 7: session cookie and bearer authentication, CSRF and `Origin` checks, and the
//! `/auth` endpoints (P7, P8, Review Focus 1 and 4).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_core::{Actor, Scopes};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, PASSWORD, Resp, SITE_URL, builder, json_body, send},
};

api_tests!(
    login_sets_cookie_and_me_works,
    csrf_required_for_cookie_mutations,
    foreign_origin_is_refused,
    login_checks_origin,
    bearer_needs_no_csrf,
    login_429_has_retry_after,
    must_reset_user_is_limited,
    password_change_keeps_only_this_session,
    password_check_is_rate_limited,
    totp_login_flow_over_http,
    logout_clears_cookie_and_session,
);

const NEW_PASSWORD: &str = "another long passphrase";
const ME: &str = "/api/v1/auth/me";
const LOGIN: &str = "/api/v1/auth/login";
const TOTP_SETUP: &str = "/api/v1/auth/totp/setup";
const PASSWORD_PATH: &str = "/api/v1/auth/password";
const POSTS: &str = "/api/v1/content/post";
const EVIL: &str = "https://evil.example";

/// A request with only the given cookie (no `Origin`, no CSRF token).
async fn me_with_cookie(api: &Api, cookie: &str) -> Resp {
    let req = builder(Method::GET, ME)
        .header(header::COOKIE, format!("ferrite_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    api.send(req).await
}

fn assert_unauthenticated(r: &Resp) {
    r.assert_problem(401, "unauthenticated");
    assert!(
        r.header("www-authenticate")
            .is_some_and(|v| v.starts_with("Bearer")),
        "{r:?}"
    );
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn authenticator(setup: &Value) -> totp_rs::Totp {
    let secret =
        totp_rs::Secret::try_from_base32(setup["secret_base32"].as_str().unwrap()).unwrap();
    totp_rs::Builder::new().with_secret(secret).build().unwrap()
}

async fn login_sets_cookie_and_me_works(api: Api) -> Api {
    let eddie = api.user("eddie", Role::Editor, false).await;

    let mut c = Client::new();
    let r = c.login(&api, "eddie", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let body = r.json();
    assert_eq!(body["status"], "session", "{body}");
    assert_eq!(body["user"]["login"], "eddie");
    let csrf = body["csrf_token"].as_str().unwrap().to_owned();
    assert_eq!(body["user"]["csrf_token"], csrf.as_str());
    let token = c.cookie.clone().unwrap();
    assert_eq!(token.len(), 43, "32 bytes base64url");
    // https site: `Secure`.
    assert_eq!(
        r.header("set-cookie").unwrap(),
        format!("ferrite_session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000; Secure")
    );

    let r = c.get(&api, ME).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let all_scopes = Scopes::ALL.to_strings();
    assert_eq!(
        r.json(),
        json!({
            "id": eddie.id.to_string(),
            "login": "eddie",
            "email": "eddie@example.com",
            "display_name": "eddie",
            "role": "editor",
            "must_reset": false,
            "totp_enabled": false,
            "capabilities": [
                "read_admin",
                "edit_own",
                "publish_own",
                "edit_others",
                "publish_others",
                "upload_media",
                "manage_terms",
                "manage_menus",
                "moderate_comments"
            ],
            "scopes": all_scopes,
            "csrf_token": csrf,
        })
    );

    // No credentials: 401 with a challenge.
    assert_unauthenticated(&api.get(ME).await);
    assert_unauthenticated(&me_with_cookie(&api, "not-a-session").await);

    // Wrong password: `invalid-credentials`, with a challenge too.
    let r = Client::new().login(&api, "eddie", "wrong password!!").await;
    r.assert_problem(401, "invalid-credentials");
    assert!(r.header("www-authenticate").is_some(), "{r:?}");
    assert!(r.header("set-cookie").is_none());

    // Logging in again from the same browser replaces its previous session (fixation).
    let old = c.cookie.clone().unwrap();
    let r = c.login(&api, "eddie", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let new = c.cookie.clone().unwrap();
    assert_ne!(old, new);
    assert_unauthenticated(&me_with_cookie(&api, &old).await);
    assert_eq!(me_with_cookie(&api, &new).await.status, StatusCode::OK);

    // The JSON body rules hold through the whole site (no-store is checked too).
    let req = builder(Method::POST, LOGIN)
        .header(header::ORIGIN, SITE_URL)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("{}"))
        .unwrap();
    api.send(req)
        .await
        .assert_problem(415, "unsupported-media-type");
    let req = builder(Method::POST, LOGIN)
        .header(header::ORIGIN, SITE_URL)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{\"login\":"))
        .unwrap();
    api.send(req).await.assert_problem(400, "malformed-json");

    // An http site: no `Secure`.
    let http = api.router_for_site("http://example.com");
    let mut c = Client::with_origin("http://example.com");
    let req = json_body(
        c.builder(Method::POST, LOGIN),
        &json!({"login": "eddie", "password": PASSWORD}),
    );
    let r = c.send_on(&http, req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.header("set-cookie").unwrap(),
        format!(
            "ferrite_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000",
            c.cookie.as_deref().unwrap()
        )
    );
    // The https origin is foreign to the http site.
    let req = json_body(
        Client::new().builder(Method::POST, LOGIN),
        &json!({"login": "eddie", "password": PASSWORD}),
    );
    send(&http, req).await.assert_problem(403, "csrf");
    api
}

async fn csrf_required_for_cookie_mutations(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    let mut c = api.logged_in("ann").await;
    let good = c.csrf.take().unwrap();

    // No CSRF header.
    c.post_empty(&api, TOTP_SETUP)
        .await
        .assert_problem(403, "csrf");
    // A wrong one.
    c.csrf = Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
    c.post_empty(&api, TOTP_SETUP)
        .await
        .assert_problem(403, "csrf");
    // Another session's token.
    let other = api.logged_in("ann").await;
    c.csrf = other.csrf.clone();
    c.post_empty(&api, TOTP_SETUP)
        .await
        .assert_problem(403, "csrf");

    // Nothing is written by a refused mutation: the password stays.
    c.csrf = None;
    let r = c
        .post(
            &api,
            PASSWORD_PATH,
            json!({"current": PASSWORD, "new": NEW_PASSWORD}),
        )
        .await;
    r.assert_problem(403, "csrf");
    assert!(r.header("set-cookie").is_none());
    let r = Client::new().login(&api, "ann", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // The right token.
    c.csrf = Some(good);
    let r = c.post_empty(&api, TOTP_SETUP).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    assert!(
        body["otpauth_url"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/")
    );
    assert!(body["enrollment"].is_string() && body["secret_base32"].is_string());
    // GET needs no token.
    c.csrf = None;
    assert_eq!(c.get(&api, ME).await.status, StatusCode::OK);
    api
}

async fn foreign_origin_is_refused(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    let c = api.logged_in("ann").await;
    let cookie = format!("ferrite_session={}", c.cookie.as_deref().unwrap());
    let csrf = c.csrf.clone().unwrap();
    let setup = |origin: Option<&str>, referer: Option<&str>| {
        let mut b = builder(Method::POST, TOTP_SETUP)
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        if let Some(r) = referer {
            b = b.header(header::REFERER, r);
        }
        b.body(Body::empty()).unwrap()
    };

    // Correct token, foreign origin.
    api.send(setup(Some(EVIL), None))
        .await
        .assert_problem(403, "csrf");
    api.send(setup(Some("null"), None))
        .await
        .assert_problem(403, "csrf");
    // A foreign Origin is not rescued by a same-site Referer.
    api.send(setup(Some(EVIL), Some(&format!("{SITE_URL}/admin"))))
        .await
        .assert_problem(403, "csrf");
    // No Origin: the Referer's origin decides.
    let r = api
        .send(setup(None, Some(&format!("{SITE_URL}/admin"))))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api.send(setup(None, Some(&format!("{EVIL}/admin"))))
        .await
        .assert_problem(403, "csrf");
    // Neither.
    api.send(setup(None, None))
        .await
        .assert_problem(403, "csrf");
    // Same origin.
    let r = api.send(setup(Some(SITE_URL), None)).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

async fn login_checks_origin(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    let body = json!({"login": "ann", "password": "wrong password!!"});
    // More attempts than either limit allows (10/min per IP, 5/min per login).
    for i in 0..11 {
        let r = Client::with_origin(EVIL)
            .login(&api, "ann", "wrong password!!")
            .await;
        r.assert_problem(403, "csrf");
        assert!(r.header("set-cookie").is_none(), "attempt {i}");
    }
    let req = json_body(builder(Method::POST, LOGIN), &body);
    api.send(req).await.assert_problem(403, "csrf");
    let req = json_body(
        builder(Method::POST, "/api/v1/auth/login/totp"),
        &json!({"challenge": "x", "code": "123456"}),
    );
    api.send(req).await.assert_problem(403, "csrf");
    // None of them counted: the right password still gets in.
    let r = Client::new().login(&api, "ann", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

async fn bearer_needs_no_csrf(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    let (bearer, _) = api
        .api
        .auth
        .create_token(
            &Actor::new(ann.id, ann.role),
            ann.id,
            "ci",
            Scopes::ALL,
            None,
        )
        .await
        .unwrap();
    let auth = format!("Bearer {}", bearer.expose());

    // POST with only `Authorization`: no Origin, no CSRF token.
    let req = builder(Method::POST, TOTP_SETUP)
        .header(header::AUTHORIZATION, &auth)
        .body(Body::empty())
        .unwrap();
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    let req = builder(Method::GET, ME)
        .header(header::AUTHORIZATION, &auth)
        .body(Body::empty())
        .unwrap();
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let me = r.json();
    assert_eq!(me["login"], "ann");
    assert_eq!(me["csrf_token"], Value::Null);

    // Logout is for sessions.
    let req = builder(Method::POST, "/api/v1/auth/logout")
        .header(header::AUTHORIZATION, &auth)
        .body(Body::empty())
        .unwrap();
    api.send(req).await.assert_problem(400, "bad-request");

    // A bearer request never falls back to the cookie.
    let c = api.logged_in("ann").await;
    let cookie = format!("ferrite_session={}", c.cookie.as_deref().unwrap());
    for value in [
        "Bearer fer_unknown",
        "Bearer",
        "Basic YW5uOnB3",
        "bearer-ish",
    ] {
        let req = builder(Method::GET, ME)
            .header(header::AUTHORIZATION, value)
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        assert_unauthenticated(&api.send(req).await);
    }

    // A valid bearer with someone else's valid cookie on an unsafe method and no CSRF
    // token: the bearer wins, CSRF is not asked for, and the request acts as its user.
    api.user("bob", Role::Author, false).await;
    let bob = api.logged_in("bob").await;
    let req = builder(Method::POST, TOTP_SETUP)
        .header(header::AUTHORIZATION, &auth)
        .header(
            header::COOKIE,
            format!("ferrite_session={}", bob.cookie.as_deref().unwrap()),
        )
        .header(header::ORIGIN, EVIL)
        .body(Body::empty())
        .unwrap();
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let url = r.json()["otpauth_url"].as_str().unwrap().to_owned();
    assert!(url.contains("ann") && !url.contains("bob"), "{url}");
    api
}

async fn login_429_has_retry_after(api: Api) -> Api {
    api.user("dan", Role::Author, false).await;
    api.align_to_window(Duration::from_secs(40)).await;
    // Ten failures from one address, over three logins so the per-login limit (5/min)
    // never applies (F9).
    let logins = [
        "ann", "ann", "ann", "ann", "ben", "ben", "ben", "ben", "cat", "cat",
    ];
    for (i, login) in logins.iter().enumerate() {
        let r = Client::new().login(&api, login, "wrong password!!").await;
        r.assert_problem(401, "invalid-credentials");
        assert_eq!(r.header("retry-after"), None, "attempt {}", i + 1);
    }
    // The 11th is refused, even with the right password.
    let r = Client::new().login(&api, "dan", PASSWORD).await;
    r.assert_problem(429, "rate-limited");
    assert!(r.header("set-cookie").is_none());
    let retry: u64 = r.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    api
}

async fn must_reset_user_is_limited(api: Api) -> Api {
    let newbie = api.user("newbie", Role::Editor, true).await;
    let token = api.bearer(&newbie, &["*"]).await;
    let mut c = api.logged_in("newbie").await;
    let old = c.cookie.clone().unwrap();

    let r = c.get(&api, ME).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["must_reset"], true);
    // Every other endpoint is closed (F13: an `Authed` endpoint of this task).
    let body = c
        .post_empty(&api, TOTP_SETUP)
        .await
        .assert_problem(403, "password-reset-required");
    assert!(body["detail"].is_string());
    c.get(&api, POSTS)
        .await
        .assert_problem(403, "password-reset-required");
    // R26 (M1): `must_reset` restricts sessions only; the account's API tokens are
    // independent credentials (R22) and keep working.
    let r = token.get(&api, POSTS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = token.get(&api, ME).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["must_reset"], true);

    let r = c
        .post(
            &api,
            PASSWORD_PATH,
            json!({"current": PASSWORD, "new": NEW_PASSWORD}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let new = c.cookie.clone().unwrap();
    assert_ne!(old, new, "a new session is issued");
    assert_unauthenticated(&me_with_cookie(&api, &old).await);
    // The response carries the new session's CSRF token: no `me` call needed.
    let csrf = r.json()["csrf_token"].as_str().unwrap().to_owned();
    assert_eq!(c.csrf.as_deref(), Some(csrf.as_str()));
    let r = c.post_empty(&api, TOTP_SETUP).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    let r = c.get(&api, ME).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["must_reset"], false);
    assert_eq!(r.json()["csrf_token"], csrf.as_str());
    let r = c.post_empty(&api, TOTP_SETUP).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

async fn password_change_keeps_only_this_session(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    let mut a = api.logged_in("ann").await;
    let b = api.logged_in("ann").await;
    let a_old = a.cookie.clone().unwrap();

    let r = a
        .post(
            &api,
            PASSWORD_PATH,
            json!({"current": PASSWORD, "new": NEW_PASSWORD}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.json()["csrf_token"].is_string(), "{r:?}");
    let cookie = r.header("set-cookie").unwrap();
    assert!(cookie.ends_with("; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000; Secure"));

    // This browser stays logged in with the new cookie; every other session is gone.
    assert_eq!(a.get(&api, ME).await.status, StatusCode::OK);
    assert_unauthenticated(&me_with_cookie(&api, &a_old).await);
    assert_unauthenticated(&me_with_cookie(&api, b.cookie.as_deref().unwrap()).await);
    // The new password is the one that works (by e-mail: the login's own bucket is
    // nearly used up by the logins and the change, R11).
    Client::new()
        .login(&api, "ann@example.com", PASSWORD)
        .await
        .assert_problem(401, "invalid-credentials");
    let r = Client::new()
        .login(&api, "ann@example.com", NEW_PASSWORD)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // A wrong current password is refused and changes nothing.
    let r = a
        .post(
            &api,
            PASSWORD_PATH,
            json!({"current": "not the password", "new": PASSWORD}),
        )
        .await;
    r.assert_problem(403, "forbidden");
    assert!(r.header("set-cookie").is_none());
    assert_eq!(a.get(&api, ME).await.status, StatusCode::OK);
    api
}

/// R11: the current-password check of a self password change counts on the login's
/// per-login bucket (5/min).
async fn password_check_is_rate_limited(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    api.align_to_window(Duration::from_secs(40)).await;
    // The login counts once on the bucket.
    let mut c = api.logged_in("ann").await;
    // Refusals before the password check take no slot: a new password breaking the
    // policy, and a token without `users:write`.
    for _ in 0..6 {
        c.post(
            &api,
            PASSWORD_PATH,
            json!({"current": "not the password", "new": "short"}),
        )
        .await
        .assert_problem(422, "validation");
    }
    let (read_only, _) = api
        .api
        .auth
        .create_token(
            &Actor::new(ann.id, ann.role),
            ann.id,
            "read",
            Scopes::from_strs(&["users:read".to_owned()]).unwrap(),
            None,
        )
        .await
        .unwrap();
    for _ in 0..6 {
        let req = json_body(
            builder(Method::POST, PASSWORD_PATH).header(
                header::AUTHORIZATION,
                format!("Bearer {}", read_only.expose()),
            ),
            &json!({"current": "not the password", "new": NEW_PASSWORD}),
        );
        api.send(req).await.assert_problem(403, "forbidden");
    }
    for i in 0..4 {
        let r = c
            .post(
                &api,
                PASSWORD_PATH,
                json!({"current": "not the password", "new": NEW_PASSWORD}),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "attempt {}: {r:?}", i + 1);
    }
    // The 6th guess on the bucket is refused, even with the right password.
    let r = c
        .post(
            &api,
            PASSWORD_PATH,
            json!({"current": PASSWORD, "new": NEW_PASSWORD}),
        )
        .await;
    r.assert_problem(429, "rate-limited");
    let retry: u64 = r.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    // Nothing changed: the session and the old password still work.
    assert_eq!(c.get(&api, ME).await.status, StatusCode::OK);
    api
}

async fn totp_login_flow_over_http(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    // Logins by e-mail here: the password re-checks below count on the login's own
    // bucket (5/min, R11).
    let mut c = api.logged_in("ann@example.com").await;
    // A second session, ended by the enrollment (R26).
    let other = api.logged_in("ann@example.com").await;

    let r = c.post_empty(&api, TOTP_SETUP).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let setup = r.json();
    let totp = authenticator(&setup);
    let r = c
        .post(
            &api,
            "/api/v1/auth/totp/confirm",
            json!({"enrollment": setup["enrollment"], "code": "000000x", "password": PASSWORD}),
        )
        .await;
    r.assert_problem(422, "validation");
    // The right code without the password: a stolen session cannot enroll.
    let r = c
        .post(
            &api,
            "/api/v1/auth/totp/confirm",
            json!({
                "enrollment": setup["enrollment"],
                "code": totp.generate(now_secs()).to_string(),
                "password": "not the password",
            }),
        )
        .await;
    r.assert_problem(403, "forbidden");
    assert_eq!(c.get(&api, ME).await.json()["totp_enabled"], false);
    let r = c
        .post(
            &api,
            "/api/v1/auth/totp/confirm",
            json!({
                "enrollment": setup["enrollment"],
                "code": totp.generate(now_secs()).to_string(),
                "password": PASSWORD,
            }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_eq!(c.get(&api, ME).await.json()["totp_enabled"], true);
    assert_unauthenticated(&me_with_cookie(&api, other.cookie.as_deref().unwrap()).await);

    // The password alone gives a challenge, no cookie.
    let mut d = Client::new();
    let r = d.login(&api, "ann@example.com", PASSWORD).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.header("set-cookie").is_none());
    let body = r.json();
    assert_eq!(body["status"], "totp_required", "{body}");
    let challenge = body["challenge"].as_str().unwrap().to_owned();
    assert!(d.cookie.is_none());

    // The second step checks the origin too.
    let mut evil = Client::with_origin(EVIL);
    evil.post(
        &api,
        "/api/v1/auth/login/totp",
        json!({"challenge": challenge, "code": totp.generate(now_secs()).to_string()}),
    )
    .await
    .assert_problem(403, "csrf");
    // A wrong code.
    let r = d
        .post(
            &api,
            "/api/v1/auth/login/totp",
            json!({"challenge": challenge, "code": "12345x"}),
        )
        .await;
    r.assert_problem(401, "invalid-credentials");
    // The next step's code (the confirm used this step's; replays are refused), within
    // the one-step skew.
    let r = d
        .post(
            &api,
            "/api/v1/auth/login/totp",
            json!({"challenge": challenge, "code": totp.generate(now_secs() + 30).to_string()}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    assert_eq!(body["status"], "session", "{body}");
    assert_eq!(body["user"]["totp_enabled"], true);
    assert!(d.cookie.is_some() && d.csrf.is_some());
    assert_eq!(d.get(&api, ME).await.status, StatusCode::OK);

    // Disable needs the password.
    d.post(
        &api,
        "/api/v1/auth/totp/disable",
        json!({"password": "not the password"}),
    )
    .await
    .assert_problem(403, "forbidden");
    let r = d
        .post(
            &api,
            "/api/v1/auth/totp/disable",
            json!({"password": PASSWORD}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_eq!(d.get(&api, ME).await.json()["totp_enabled"], false);
    api
}

async fn logout_clears_cookie_and_session(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    let mut c = api.logged_in("ann").await;
    let cookie = c.cookie.clone().unwrap();
    let other = api.logged_in("ann").await;

    // Logout is a mutation: it needs the CSRF token.
    let csrf = c.csrf.take();
    c.post_empty(&api, "/api/v1/auth/logout")
        .await
        .assert_problem(403, "csrf");
    c.csrf = csrf;
    let r = c.post_empty(&api, "/api/v1/auth/logout").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_eq!(
        r.header("set-cookie"),
        Some("ferrite_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure")
    );
    assert!(c.cookie.is_none());
    assert_unauthenticated(&me_with_cookie(&api, &cookie).await);
    // Other sessions stay.
    assert_eq!(
        me_with_cookie(&api, other.cookie.as_deref().unwrap())
            .await
            .status,
        StatusCode::OK
    );
    api
}
