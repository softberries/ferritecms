//! Plugin routes under `/_p/<plugin>/` (M2a-2 Task 8, P15, P16): the check order,
//! the request the plugin sees, the response the visitor gets (Review Focus 2), and
//! hostile handlers that must not take the site down.

use std::net::IpAddr;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use ferrite_core::Scopes;
use ferrite_http::ClientIp;
use serde_json::Value;

use crate::app_tests;
use crate::common::SITE_URL;
use crate::harness::{App, Resp};

/// `method path` with `headers`, `body` and (when given) the client address.
fn route_request(
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
    ip: Option<&str>,
) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "example.com");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let mut req = b.body(Body::from(body)).expect("request");
    if let Some(ip) = ip {
        let ip: IpAddr = ip.parse().expect("ip");
        req.extensions_mut().insert(ClientIp(ip));
    }
    req
}

async fn get(app: &App, path: &str) -> Resp {
    app.send(route_request(Method::GET, path, &[], Vec::new(), None))
        .await
}

/// Installs and loads example plugins `names` (enabled rows, manifest honoured).
async fn with_plugins(app: &App, names: &[&str]) {
    for name in names {
        app.site.install(name).await;
    }
    let loaded = app.site.load().await;
    for name in names {
        assert!(
            app.site.host.is_enabled(name),
            "{name} not loaded: {loaded:?}"
        );
    }
}

/// The headers every answer from a plugin's handler carries (P15).
fn assert_contained(r: &Resp) {
    assert_eq!(
        r.header("content-security-policy"),
        Some("sandbox allow-forms"),
        "{r:?}"
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.header("x-robots-tag"), Some("noindex"));
    assert!(r.header("set-cookie").is_none(), "{r:?}");
}

fn plain(r: &Resp, status: StatusCode) {
    assert_eq!(r.status, status, "{r:?}");
    assert_eq!(
        r.header("content-type"),
        Some("text/plain; charset=utf-8"),
        "{r:?}"
    );
}

app_tests!(
    route_success_and_user_id,
    route_xss_is_contained,
    undeclared_path_and_method,
    post_needs_site_origin,
    cross_site_location_refused,
    route_response_caps,
    body_over_1_mib_is_413,
    route_rate_limit_answers_429,
    disabled_or_deleted_plugin_is_404_at_once,
    hostile_handlers_do_not_break_the_site,
    paths_stay_inside_the_plugin,
);

async fn route_success_and_user_id(app: App) -> App {
    with_plugins(&app, &["router"]).await;
    let json = |r: &Resp| -> Value { serde_json::from_str(&r.body).expect("json body") };

    // Anonymous: no user; the IP hash is per plugin and keyed (32 hex).
    let anon = app
        .send(route_request(
            Method::GET,
            "/_p/router/hello",
            &[],
            Vec::new(),
            Some("198.51.100.7"),
        ))
        .await;
    assert_eq!(anon.status, StatusCode::OK, "{anon:?}");
    assert_eq!(anon.header("content-type"), Some("application/json"));
    assert_contained(&anon);
    let body = json(&anon);
    assert_eq!(body["user"], Value::Null);
    let ip = body["ip"].as_str().expect("ip hash").to_owned();
    assert_eq!(ip.len(), 32, "{ip}");
    assert!(ip.bytes().all(|b| b.is_ascii_hexdigit()), "{ip}");
    let auth = App::auth(&app.site);
    let site_hash = auth.ip_hash("198.51.100.7".parse().unwrap());
    assert_ne!(
        ip, site_hash,
        "the plugin hash must not be the session hash"
    );
    assert_eq!(
        ip,
        auth.plugin_ip_hash("router", "198.51.100.7".parse().unwrap())
    );

    // With the session cookie: the user id.
    let cookie = app.session(&app.ada_user).await;
    let r = app
        .send(route_request(
            Method::GET,
            "/_p/router/hello",
            &[("cookie", &cookie)],
            Vec::new(),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(json(&r)["user"], app.ada_user.id.to_string());

    // A bearer token alone never identifies the visitor to a plugin.
    let (secret, _) = auth
        .create_token(&app.ada, app.ada_user.id, "t", Scopes::ALL, None)
        .await
        .expect("token");
    let bearer = format!("Bearer {}", secret.expose());
    let r = app
        .send(route_request(
            Method::GET,
            "/_p/router/hello",
            &[("authorization", &bearer)],
            Vec::new(),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(json(&r)["user"], Value::Null);
    // Never cached.
    assert_eq!(app.cached().await, 0);
    app
}

async fn route_xss_is_contained(app: App) -> App {
    with_plugins(&app, &["route-xss"]).await;
    let cookie = app.session(&app.ada_user).await;
    let r = app
        .send(route_request(
            Method::GET,
            "/_p/route-xss/x",
            &[
                ("cookie", &cookie),
                ("authorization", "Bearer abc"),
                ("x-csrf-token", "t"),
                ("accept", "text/html"),
                ("user-agent", "test-agent"),
                ("referer", "https://example.com/"),
            ],
            Vec::new(),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_contained(&r);
    // The plugin's HTML is served, but only inside the sandbox.
    assert_eq!(r.header("content-type"), Some("text/html"));
    assert!(
        r.body.contains("<script>document.cookie</script>"),
        "{}",
        r.body
    );
    // Everything else it set is gone or overridden.
    for name in [
        "authorization",
        "strict-transport-security",
        "access-control-allow-origin",
    ] {
        assert!(r.header(name).is_none(), "{name}: {r:?}");
    }
    // The request it saw: only allowlisted headers, never the cookie or credentials.
    let seen = r
        .body
        .split_once("<!-- headers: ")
        .and_then(|(_, rest)| rest.split_once(" -->"))
        .map(|(names, _)| names.to_owned())
        .expect("header names comment");
    let mut seen: Vec<&str> = seen.split(',').filter(|s| !s.is_empty()).collect();
    seen.sort_unstable();
    assert_eq!(seen, ["accept", "referer", "user-agent"], "{}", r.body);
    app
}

async fn undeclared_path_and_method(app: App) -> App {
    with_plugins(&app, &["router", "hello"]).await;
    // Unknown plugin, plugin without `routes`, undeclared path: plain 404.
    for path in [
        "/_p/nope/hello",
        "/_p/hello/hello",
        "/_p/router/nope",
        "/_p/router/",
        "/_p/router/hello/",
    ] {
        let r = get(&app, path).await;
        plain(&r, StatusCode::NOT_FOUND);
        assert_eq!(r.body, "not found\n", "{path}");
    }
    // A declared path with another method: 405 with the route's methods.
    let r = app
        .send(route_request(
            Method::DELETE,
            "/_p/router/hello",
            &[("origin", SITE_URL)],
            Vec::new(),
            None,
        ))
        .await;
    plain(&r, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.header("allow"), Some("GET"));
    let r = get(&app, "/_p/router/echo").await;
    plain(&r, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.header("allow"), Some("POST"));
    // `/_p/<name>` is the public fallback's: the canonical 301 to `/_p/<name>/`, which
    // is the plugin's route `"/"` (none here: 404).
    let r = get(&app, "/_p/router").await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{r:?}");
    assert_eq!(r.header("location"), Some("/_p/router/"));
    app
}

async fn post_needs_site_origin(app: App) -> App {
    with_plugins(&app, &["router", "route-flood"]).await;
    let post = |origin: Option<&'static str>, path: &'static str| {
        let headers: Vec<(&str, &str)> = origin.map(|o| ("origin", o)).into_iter().collect();
        route_request(Method::POST, path, &headers, b"hi".to_vec(), None)
    };
    for origin in [Some("https://evil.example"), None] {
        let r = app.send(post(origin, "/_p/router/echo")).await;
        plain(&r, StatusCode::FORBIDDEN);
    }
    // A foreign Referer is refused too; the site's own is accepted.
    let r = app
        .send(route_request(
            Method::POST,
            "/_p/router/echo",
            &[("referer", "https://evil.example/form")],
            b"hi".to_vec(),
            None,
        ))
        .await;
    plain(&r, StatusCode::FORBIDDEN);
    let r = app.send(post(Some(SITE_URL), "/_p/router/echo")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.body, "hi");
    assert_contained(&r);
    // `cross_origin = true`: any origin may post.
    let r = app
        .send(post(Some("https://evil.example"), "/_p/route-flood/submit"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    app
}

async fn cross_site_location_refused(app: App) -> App {
    with_plugins(&app, &["router"]).await;
    let r = get(&app, "/_p/router/away").await;
    plain(&r, StatusCode::BAD_GATEWAY);
    assert!(r.header("location").is_none(), "{r:?}");
    let r = get(&app, "/_p/router/go").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    assert_eq!(r.header("location"), Some("/thanks/"));
    assert_contained(&r);
    app
}

async fn route_response_caps(app: App) -> App {
    with_plugins(&app, &["router"]).await;
    let r = get(&app, "/_p/router/big").await;
    plain(&r, StatusCode::BAD_GATEWAY);
    assert!(r.raw.len() < 1024, "{} bytes", r.raw.len());
    // The plugin keeps answering its other routes.
    let r = get(&app, "/_p/router/hello").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    app
}

async fn body_over_1_mib_is_413(app: App) -> App {
    with_plugins(&app, &["router"]).await;
    let big = vec![b'a'; 1024 * 1024 + 1];
    let r = app
        .send(route_request(
            Method::POST,
            "/_p/router/echo",
            &[("origin", SITE_URL)],
            big,
            None,
        ))
        .await;
    plain(&r, StatusCode::PAYLOAD_TOO_LARGE);
    // Exactly 1 MiB passes.
    let ok = vec![b'a'; 1024 * 1024];
    let r = app
        .send(route_request(
            Method::POST,
            "/_p/router/echo",
            &[("origin", SITE_URL)],
            ok,
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.raw.len(), 1024 * 1024);
    app
}

async fn route_rate_limit_answers_429(app: App) -> App {
    with_plugins(&app, &["route-flood"]).await;
    let post = |ip: &str| {
        route_request(
            Method::POST,
            "/_p/route-flood/submit",
            &[],
            Vec::new(),
            Some(ip),
        )
    };
    // The window is a minute: keep the six posts inside one.
    align_to_minute(&app, std::time::Duration::from_secs(10)).await;
    for i in 0..5 {
        let r = app.send(post("203.0.113.1")).await;
        assert_eq!(r.status, StatusCode::OK, "post {i}: {r:?}");
    }
    let r = app.send(post("203.0.113.1")).await;
    plain(&r, StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = r
        .header("retry-after")
        .and_then(|v| v.parse().ok())
        .expect("Retry-After");
    assert!((1..=60).contains(&retry), "{retry}");
    // Another client has its own count.
    let r = app.send(post("203.0.113.2")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    app
}

/// Sleeps into the next one-minute window when fewer than `need` are left of this one.
async fn align_to_minute(app: &App, need: std::time::Duration) {
    let w: i64 = 60_000_000;
    let now = app
        .site
        .store
        .read()
        .nodes()
        .db_now()
        .await
        .expect("db now")
        .as_micros();
    let left = w - now.rem_euclid(w);
    if left < i64::try_from(need.as_micros()).expect("micros") {
        tokio::time::sleep(
            std::time::Duration::from_micros(left.unsigned_abs())
                + std::time::Duration::from_millis(50),
        )
        .await;
    }
}

async fn disabled_or_deleted_plugin_is_404_at_once(app: App) -> App {
    with_plugins(&app, &["router", "route-xss"]).await;
    assert_eq!(get(&app, "/_p/router/hello").await.status, StatusCode::OK);
    let plugins = app.site.store.read().plugins();
    // The row changes; this node has not reloaded yet, and still answers 404 at once.
    plugins
        .set_enabled("router", false, Some("test"))
        .await
        .expect("disable");
    assert!(app.site.host.is_enabled("router"));
    plain(&get(&app, "/_p/router/hello").await, StatusCode::NOT_FOUND);
    plugins.delete("route-xss").await.expect("delete");
    plain(&get(&app, "/_p/route-xss/x").await, StatusCode::NOT_FOUND);
    // Disabled on this host (e.g. after repeated failures) with the row enabled: 404.
    plugins
        .set_enabled("router", true, None)
        .await
        .expect("enable");
    assert_eq!(get(&app, "/_p/router/hello").await.status, StatusCode::OK);
    app.site.host.disable("router", "test");
    plain(&get(&app, "/_p/router/hello").await, StatusCode::NOT_FOUND);
    app
}

async fn hostile_handlers_do_not_break_the_site(app: App) -> App {
    with_plugins(&app, &["route-xss"]).await;
    app.post("Still here", "still-here", &[]).await;
    for path in ["/_p/route-xss/panic", "/_p/route-xss/loop"] {
        let r = get(&app, path).await;
        plain(&r, StatusCode::BAD_GATEWAY);
        assert_eq!(r.body, "bad gateway\n", "{path}");
    }
    assert_eq!(app.site.host.failures().len(), 2);
    // The site, and the plugin's healthy route, keep serving.
    let home = get(&app, "/").await;
    assert_eq!(home.status, StatusCode::OK, "{home:?}");
    let post = get(&app, "/still-here/").await;
    assert_eq!(post.status, StatusCode::OK, "{post:?}");
    let r = get(&app, "/_p/route-xss/x").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    app
}

async fn paths_stay_inside_the_plugin(app: App) -> App {
    with_plugins(&app, &["router", "route-xss"]).await;
    // Dot segments and encoded slashes are never resolved: they name no route of the
    // plugin, and never another plugin's or a core route.
    for path in [
        "/_p/router/../route-xss/x",
        "/_p/router/..%2Froute-xss%2Fx",
        "/_p/router/%2e%2e/route-xss/x",
        "/_p/router/../../api/v1/auth/me",
        "/_p/router%2F..%2Froute-xss/x",
        "/_p/Router/hello",
        "/_p/router//hello",
        "/_p/router/hello%00",
        "/_p/router/%ff",
    ] {
        let r = get(&app, path).await;
        plain(&r, StatusCode::NOT_FOUND);
    }
    // An encoded character of a declared path is the same path.
    assert_eq!(get(&app, "/_p/router/hell%6F").await.status, StatusCode::OK);
    app
}
