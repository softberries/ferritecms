//! M5 Task 3: the OAuth endpoints (`/.well-known/*`, `/oauth/*`), the consent API under
//! `/api/v1/oauth`, the REST audience rule and the metadata-document fetcher (design 12
//! §2, §3, §6, §7).

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_core::oauth::{ClientMetadataFetcher, FetchError};
use ferrite_domain::Role;
use ferrite_http::{
    oauth_fetch::OAuthFetcher,
    plugins::egress::{EgressOverrides, is_public_ip},
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::{
    api_tests,
    harness::{Api, Client, Resp, SITE_TITLE, SITE_URL, builder, json_body, request, send},
};

api_tests!(
    protected_resource_metadata_names_mcp,
    as_metadata_lists_endpoints_and_cimd,
    dcr_registers_public_client_and_rate_limits,
    authorize_redirects_to_admin_consent,
    full_flow_code_token_refresh_revoke,
    oauth_token_refused_by_rest_api,
    bad_client_shows_error_page_not_redirect,
    mcp_disabled_hides_everything,
    cimd_client_fetched_from_local_server,
);

/// RFC 7636 Appendix B.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const REDIRECT: &str = "http://127.0.0.1:33418/callback";
const FORM: &str = "application/x-www-form-urlencoded";

// ---- helpers ---------------------------------------------------------------------------

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

async fn post_form(api: &Api, path: &str, body: &str) -> Resp {
    api.send(
        builder(Method::POST, path)
            .header(header::CONTENT_TYPE, FORM)
            .body(Body::from(body.to_owned()))
            .unwrap(),
    )
    .await
}

async fn register(api: &Api, body: &Value) -> Resp {
    api.send(json_body(builder(Method::POST, "/oauth/register"), body))
        .await
}

/// A registered client's id.
async fn registered(api: &Api) -> String {
    let r = register(
        api,
        &json!({"client_name": "Test Assistant", "redirect_uris": [REDIRECT]}),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()["client_id"].as_str().unwrap().to_owned()
}

fn authorize_path(client_id: &str, extra: &str) -> String {
    format!(
        "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256&state=st-1{extra}",
        enc(client_id),
        enc(REDIRECT),
    )
}

/// `GET /oauth/authorize` for `client_id`; returns the pending request's id.
async fn begin(api: &Api, client_id: &str, extra: &str) -> String {
    let r = api.get(&authorize_path(client_id, extra)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    let location = r.header("location").unwrap();
    let prefix = format!("{SITE_URL}/admin/connect?request=");
    let id = location
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{location}"));
    uuid::Uuid::parse_str(id).unwrap();
    id.to_owned()
}

fn query(url: &str, key: &str) -> Option<String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// Approves `request` as the signed-in `client` with `scopes`; returns the code.
async fn approve(api: &Api, c: &mut Client, request: &str, scopes: &[&str]) -> String {
    let r = c
        .post(
            api,
            &format!("/api/v1/oauth/requests/{request}/approve"),
            json!({ "scopes": scopes }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let to = r.json()["redirect_to"].as_str().unwrap().to_owned();
    assert!(to.starts_with(REDIRECT), "{to}");
    assert_eq!(query(&to, "state").as_deref(), Some("st-1"));
    assert_eq!(query(&to, "iss").as_deref(), Some(SITE_URL));
    query(&to, "code").unwrap()
}

async fn exchange_code(api: &Api, client_id: &str, code: &str) -> Resp {
    post_form(
        api,
        "/oauth/token",
        &form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT),
            ("client_id", client_id),
            ("code_verifier", VERIFIER),
        ]),
    )
    .await
}

pub(crate) async fn refresh(api: &Api, client_id: &str, token: &str) -> Resp {
    post_form(
        api,
        "/oauth/token",
        &form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", client_id),
        ]),
    )
    .await
}

/// An OAuth error answer: `status`, JSON `error`, `no-store`.
fn assert_oauth_error(r: &Resp, status: u16, error: &str) {
    assert_eq!(r.status.as_u16(), status, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"), "{r:?}");
    assert!(
        r.header("content-type")
            .is_some_and(|t| t.starts_with("application/json")),
        "{r:?}"
    );
    let body = r.json();
    assert_eq!(body["error"], error, "{body}");
    assert!(body["error_description"].is_string(), "{body}");
}

/// A signed-in admin, a registered client and its tokens (`access`, `refresh`).
pub(crate) async fn connected(api: &Api, scopes: &[&str]) -> (Client, String, String, String) {
    api.user("boss", Role::Admin, false).await;
    let mut c = api.logged_in("boss").await;
    let client_id = registered(api).await;
    let request = begin(api, &client_id, "").await;
    let code = approve(api, &mut c, &request, scopes).await;
    let r = exchange_code(api, &client_id, &code).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    (
        c,
        client_id,
        body["access_token"].as_str().unwrap().to_owned(),
        body["refresh_token"].as_str().unwrap().to_owned(),
    )
}

async fn options(api: &Api, path: &str) -> Resp {
    api.send(
        builder(Method::OPTIONS, path)
            .header(header::ORIGIN, "https://assistant.example")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

fn assert_cors(r: &Resp) {
    assert_eq!(r.header("access-control-allow-origin"), Some("*"), "{r:?}");
}

// ---- metadata documents ------------------------------------------------------------------

async fn protected_resource_metadata_names_mcp(api: Api) -> Api {
    for path in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
    ] {
        let r = api.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {r:?}");
        assert_eq!(r.header("cache-control"), Some("public, max-age=3600"));
        assert!(
            r.header("content-type")
                .unwrap()
                .starts_with("application/json")
        );
        assert_cors(&r);
        let doc = r.json();
        assert_eq!(doc["resource"], format!("{SITE_URL}/mcp"), "{doc}");
        assert_eq!(doc["authorization_servers"], json!([SITE_URL]));
        assert_eq!(doc["bearer_methods_supported"], json!(["header"]));
        // Only what a grant can hold: no `users:write`, `import:write`, `system:write`.
        assert_eq!(doc["scopes_supported"].as_array().unwrap().len(), 19);
        assert!(
            doc["scopes_supported"]
                .as_array()
                .unwrap()
                .contains(&json!("content:write"))
        );
        for unused in ["users:write", "import:write", "system:write"] {
            assert!(
                !doc["scopes_supported"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(unused)),
                "{unused}"
            );
        }
        assert_eq!(doc["resource_name"], SITE_TITLE);
        assert!(
            doc["resource_documentation"]
                .as_str()
                .unwrap()
                .starts_with("https://")
        );
        let pre = options(&api, path).await;
        assert_eq!(pre.status, StatusCode::NO_CONTENT, "{pre:?}");
        assert_cors(&pre);
        assert!(
            pre.header("access-control-allow-methods")
                .unwrap()
                .contains("GET")
        );
    }
    // The resource name follows the `site.title` option.
    api.store
        .read()
        .options()
        .set("site.title", &json!("Renamed"), true)
        .await
        .unwrap();
    let doc = api
        .get("/.well-known/oauth-protected-resource/mcp")
        .await
        .json();
    assert_eq!(doc["resource_name"], "Renamed");
    api
}

async fn as_metadata_lists_endpoints_and_cimd(api: Api) -> Api {
    let r = api.get("/.well-known/oauth-authorization-server").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("public, max-age=3600"));
    assert_cors(&r);
    let doc = r.json();
    assert_eq!(doc["issuer"], SITE_URL);
    for (key, path) in [
        ("authorization_endpoint", "/oauth/authorize"),
        ("token_endpoint", "/oauth/token"),
        ("registration_endpoint", "/oauth/register"),
        ("revocation_endpoint", "/oauth/revoke"),
    ] {
        assert_eq!(doc[key], format!("{SITE_URL}{path}"), "{key}");
    }
    assert_eq!(doc["response_types_supported"], json!(["code"]));
    assert_eq!(
        doc["grant_types_supported"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(doc["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        doc["token_endpoint_auth_methods_supported"],
        json!(["none"])
    );
    assert_eq!(doc["client_id_metadata_document_supported"], true);
    assert_eq!(doc["authorization_response_iss_parameter_supported"], true);
    assert_eq!(doc["scopes_supported"].as_array().unwrap().len(), 19);
    // No path in site.url: no path-inserted copy.
    assert_eq!(
        api.get("/.well-known/oauth-authorization-server/blog")
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // A site.url with a path: the issuer keeps it, and the RFC 8414 path-inserted
    // address serves the same document.
    let blog = api.router_for_oauth_site("https://example.com/blog/");
    for path in [
        "/.well-known/oauth-authorization-server/blog",
        "/.well-known/oauth-authorization-server",
    ] {
        let r = send(&blog, request(Method::GET, path)).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {r:?}");
        let doc = r.json();
        assert_eq!(doc["issuer"], "https://example.com/blog");
        assert_eq!(
            doc["token_endpoint"],
            "https://example.com/blog/oauth/token"
        );
    }
    let r = send(
        &blog,
        request(Method::GET, "/.well-known/oauth-protected-resource/mcp"),
    )
    .await;
    assert_eq!(r.json()["resource"], "https://example.com/blog/mcp");
    api
}

// ---- registration ------------------------------------------------------------------------

async fn dcr_registers_public_client_and_rate_limits(api: Api) -> Api {
    api.in_one_window_of(Duration::from_secs(3600), Duration::from_secs(30), |_| async {
        let r = register(
            &api,
            &json!({
                "client_name": "Claude",
                "redirect_uris": [REDIRECT, "https://claude.example/cb"],
                "client_uri": "https://claude.example/",
                "logo_uri": "https://claude.example/logo.png",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
                "scope": "content:read",
            }),
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
        assert_eq!(r.header("cache-control"), Some("no-store"));
        assert_cors(&r);
        let body = r.json();
        let id = body["client_id"].as_str().unwrap();
        assert!(id.starts_with("ferc_") && id.len() == 27, "{id}");
        assert!(body["client_id_issued_at"].as_i64().unwrap() > 1_700_000_000);
        assert_eq!(body["client_name"], "Claude");
        assert_eq!(
            body["redirect_uris"],
            json!([REDIRECT, "https://claude.example/cb"])
        );
        assert_eq!(
            body["grant_types"],
            json!(["authorization_code", "refresh_token"])
        );
        assert_eq!(body["response_types"], json!(["code"]));
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert_eq!(body["client_uri"], "https://claude.example/");
        assert_eq!(body["logo_uri"], "https://claude.example/logo.png");
        assert!(body.get("client_secret").is_none(), "{body}");

        // Refused registrations (they count on the limit too).
        let secret = register(
            &api,
            &json!({"client_name": "A", "redirect_uris": [REDIRECT], "token_endpoint_auth_method": "client_secret_basic"}),
        )
        .await;
        assert_oauth_error(&secret, 400, "invalid_client_metadata");
        let js = register(
            &api,
            &json!({"client_name": "A", "redirect_uris": ["javascript:alert(1)"]}),
        )
        .await;
        assert_oauth_error(&js, 400, "invalid_redirect_uri");
        let shape = register(&api, &json!({"client_name": "A", "redirect_uris": "x"})).await;
        assert_oauth_error(&shape, 400, "invalid_client_metadata");
        // Not JSON at all: refused before core (not counted).
        let text = api
            .send(
                builder(Method::POST, "/oauth/register")
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await;
        assert_oauth_error(&text, 400, "invalid_client_metadata");
        // Over the 64 KiB body limit.
        let big = api
            .send(
                builder(Method::POST, "/oauth/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(vec![b' '; 65 * 1024]))
                    .unwrap(),
            )
            .await;
        assert_eq!(big.status, StatusCode::PAYLOAD_TOO_LARGE, "{big:?}");

        // Three hits so far (a body of the wrong shape never reaches the limit); the
        // limit is 10 an hour per IP.
        for _ in 0..7 {
            registered(&api).await;
        }
        let limited = register(
            &api,
            &json!({"client_name": "Late", "redirect_uris": [REDIRECT]}),
        )
        .await;
        assert_oauth_error(&limited, 429, "temporarily_unavailable");
        assert!(limited.header("retry-after").is_some(), "{limited:?}");
        let pre = options(&api, "/oauth/register").await;
        assert_eq!(pre.status, StatusCode::NO_CONTENT);
        assert_cors(&pre);
    })
    .await;
    api
}

// ---- authorization ------------------------------------------------------------------------

async fn authorize_redirects_to_admin_consent(api: Api) -> Api {
    api.in_one_window(Duration::from_secs(20), |_| async {
        let client_id = registered(&api).await;
        let r = api
            .get(&authorize_path(
                &client_id,
                "&scope=content%3Aread+media%3Awrite",
            ))
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
        assert_eq!(r.header("cache-control"), Some("no-store"));
        let request = r
            .header("location")
            .unwrap()
            .strip_prefix(&format!("{SITE_URL}/admin/connect?request="))
            .unwrap()
            .to_owned();

        // The consent API: sessions only.
        let author = api.user("writer", Role::Author, false).await;
        let mut c = api.logged_in("writer").await;
        let path = format!("/api/v1/oauth/requests/{request}");
        let r = c.get(&api, &path).await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        let view = r.json();
        assert_eq!(view["id"], request.as_str());
        assert_eq!(view["client"]["id"], client_id.as_str());
        assert_eq!(view["client"]["name"], "Test Assistant");
        assert_eq!(view["client"]["kind"], "registered");
        assert_eq!(view["verified"], false);
        assert_eq!(view["redirect_host"], "127.0.0.1:33418");
        assert_eq!(view["requested"], json!(["content:read", "media:write"]));
        // An author can upload media: both are grantable.
        assert_eq!(view["grantable"], json!(["content:read", "media:write"]));
        let token = api.bearer(&author, &["*"]).await;
        token
            .get(&api, &path)
            .await
            .assert_problem(403, "forbidden");
        let approve_by_token = token
            .send(
                &api,
                Method::POST,
                &format!("{path}/approve"),
                Some(&json!({"scopes": ["content:read"]})),
                None,
            )
            .await;
        approve_by_token.assert_problem(403, "forbidden");
        // Without the CSRF token: refused.
        let mut no_csrf = Client {
            csrf: None,
            ..Client::new()
        };
        no_csrf.cookie = c.cookie.clone();
        no_csrf
            .post(
                &api,
                &format!("{path}/approve"),
                json!({"scopes": ["content:read"]}),
            )
            .await
            .assert_problem(403, "csrf");
        c.get(
            &api,
            "/api/v1/oauth/requests/0196f6b0-0000-7000-8000-000000000000",
        )
        .await
        .assert_problem(404, "not-found");
        // Beyond the request: forbidden, and the request stays.
        c.post(
            &api,
            &format!("{path}/approve"),
            json!({"scopes": ["users:write"]}),
        )
        .await
        .assert_problem(403, "forbidden");
        let code = approve(&api, &mut c, &request, &["content:read"]).await;
        assert!(!code.is_empty());
        // Single use.
        c.post(
            &api,
            &format!("{path}/approve"),
            json!({"scopes": ["content:read"]}),
        )
        .await
        .assert_problem(404, "not-found");

        // Deny answers access_denied back to the client.
        let second = begin(&api, &client_id, "").await;
        let r = c
            .post_empty(&api, &format!("/api/v1/oauth/requests/{second}/deny"))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        let to = r.json()["redirect_to"].as_str().unwrap().to_owned();
        assert_eq!(query(&to, "error").as_deref(), Some("access_denied"));
        assert_eq!(query(&to, "state").as_deref(), Some("st-1"));

        // A valid client with a bad request goes back to the client with the error.
        let r = api
            .get(&format!(
                "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&state=s2",
                enc(&client_id),
                enc(REDIRECT)
            ))
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
        let to = r.header("location").unwrap();
        assert!(to.starts_with(REDIRECT), "{to}");
        assert_eq!(query(to, "error").as_deref(), Some("invalid_request"));
        assert_eq!(query(to, "state").as_deref(), Some("s2"));
        assert_eq!(query(to, "iss").as_deref(), Some(SITE_URL));

        // 30 authorization requests a minute per IP: 3 so far.
        for _ in 0..27 {
            let r = api.get(&authorize_path(&client_id, "")).await;
            assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
        }
        let r = api.get(&authorize_path(&client_id, "")).await;
        assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{r:?}");
        assert!(r.header("retry-after").is_some());
        assert!(r.header("location").is_none());
        assert!(r.header("content-type").unwrap().starts_with("text/html"));
    })
    .await;
    api
}

async fn full_flow_code_token_refresh_revoke(api: Api) -> Api {
    let (mut c, client_id, access, refresh_token) =
        connected(&api, &["content:read", "content:write"]).await;
    // The token answer.
    // OAuth access tokens have their own prefix (12 §3.1).
    assert!(access.starts_with("fera_"));
    // A personal token's list does not show it.
    let me = c.get(&api, "/api/v1/auth/me").await.json();
    let user_id = me["id"].as_str().unwrap().to_owned();
    let tokens = c
        .get(&api, &format!("/api/v1/users/{user_id}/tokens"))
        .await
        .json();
    assert_eq!(tokens["items"], json!([]));

    // Refresh rotates; the answer has the OAuth headers.
    let r = refresh(&api, &client_id, &refresh_token).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.header("pragma"), Some("no-cache"));
    assert_cors(&r);
    let body = r.json();
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], 3600);
    assert_eq!(body["scope"], "content:read content:write");
    let refresh_2 = body["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(refresh_2, refresh_token);

    // The connected app shows up.
    let grants = c.get(&api, "/api/v1/oauth/grants").await;
    assert_eq!(grants.status, StatusCode::OK, "{grants:?}");
    let items = grants.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["client"]["name"], "Test Assistant");
    assert_eq!(items[0]["client"]["id"], client_id.as_str());
    assert_eq!(items[0]["user_id"], user_id.as_str());
    assert_eq!(items[0]["scopes"], json!(["content:read", "content:write"]));
    for key in ["created_at", "last_used_at", "expires_at"] {
        assert!(items[0][key].is_string(), "{key}");
    }

    // Revoking the refresh token ends the grant; unknown tokens are fine too.
    for token in [refresh_2.as_str(), "nope", "fer_unknown"] {
        let r = post_form(&api, "/oauth/revoke", &form(&[("token", token)])).await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        assert_cors(&r);
    }
    assert_oauth_error(
        &refresh(&api, &client_id, &refresh_2).await,
        400,
        "invalid_grant",
    );
    let items = c.get(&api, "/api/v1/oauth/grants").await.json();
    assert_eq!(items["items"], json!([]));
    let r = post_form(&api, "/oauth/revoke", "").await;
    assert_oauth_error(&r, 400, "invalid_request");

    // Token endpoint errors.
    let json_body_token = api
        .send(json_body(
            builder(Method::POST, "/oauth/token"),
            &json!({"grant_type": "refresh_token"}),
        ))
        .await;
    assert_oauth_error(&json_body_token, 400, "invalid_request");
    let r = post_form(&api, "/oauth/token", "grant_type=password").await;
    assert_oauth_error(&r, 400, "unsupported_grant_type");
    let r = post_form(
        &api,
        "/oauth/token",
        "grant_type=refresh_token&grant_type=refresh_token",
    )
    .await;
    assert_oauth_error(&r, 400, "invalid_request");
    let pre = options(&api, "/oauth/token").await;
    assert_eq!(pre.status, StatusCode::NO_CONTENT);
    assert_cors(&pre);

    // Connect again, then a reused code revokes the grant made from it.
    let request = begin(&api, &client_id, "").await;
    let code = approve(&api, &mut c, &request, &["content:read"]).await;
    let first = exchange_code(&api, &client_id, &code).await;
    assert_eq!(first.status, StatusCode::OK, "{first:?}");
    let reused = exchange_code(&api, &client_id, &code).await;
    assert_oauth_error(&reused, 400, "invalid_grant");
    let items = c.get(&api, "/api/v1/oauth/grants").await.json();
    assert_eq!(items["items"], json!([]));

    // Disconnect from the account.
    let request = begin(&api, &client_id, "").await;
    let code = approve(&api, &mut c, &request, &["content:read"]).await;
    let tokens = exchange_code(&api, &client_id, &code).await.json();
    let items = c.get(&api, "/api/v1/oauth/grants").await.json();
    let grant = items["items"][0]["id"].as_str().unwrap().to_owned();
    let r = c
        .send(
            &api,
            Method::DELETE,
            &format!("/api/v1/oauth/grants/{grant}"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_oauth_error(
        &refresh(&api, &client_id, tokens["refresh_token"].as_str().unwrap()).await,
        400,
        "invalid_grant",
    );
    c.send(
        &api,
        Method::DELETE,
        &format!("/api/v1/oauth/grants/{grant}"),
        None,
        None,
    )
    .await
    .assert_problem(404, "not-found");

    // An admin sees another user's grants with `?user=`; others do not.
    let other = api.user("other", Role::Editor, false).await;
    let mut o = api.logged_in("other").await;
    let request = begin(&api, &client_id, "").await;
    approve(&api, &mut o, &request, &["content:read"]).await;
    let theirs = c
        .get(&api, &format!("/api/v1/oauth/grants?user={}", other.id))
        .await;
    assert_eq!(theirs.status, StatusCode::OK, "{theirs:?}");
    // The code was never exchanged: no grant yet.
    assert_eq!(theirs.json()["items"], json!([]));
    o.get(&api, &format!("/api/v1/oauth/grants?user={user_id}"))
        .await
        .assert_problem(403, "forbidden");
    api
}

async fn oauth_token_refused_by_rest_api(api: Api) -> Api {
    let (mut c, client_id, access, _) = connected(&api, &["content:read", "users:read"]).await;
    assert!(access.starts_with("fera_"));
    for path in ["/api/v1/auth/me", "/api/v1/users"] {
        let r = api
            .send(
                builder(Method::GET, path)
                    .header(header::AUTHORIZATION, format!("Bearer {access}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        r.assert_problem(401, "unauthenticated");
        assert_eq!(
            r.header("www-authenticate"),
            Some("Bearer error=\"invalid_token\""),
            "{r:?}"
        );
    }
    // The public comment submit (optional credentials) refuses it too.
    let r = api
        .send(json_body(
            builder(
                Method::POST,
                "/api/v1/content/post/0196f6b0-0000-7000-8000-000000000000/comments",
            )
            .header(header::AUTHORIZATION, format!("Bearer {access}")),
            &json!({"body": "hi", "author_name": "A", "author_email": "a@example.com"}),
        ))
        .await;
    r.assert_problem(401, "unauthenticated");
    assert_eq!(
        r.header("www-authenticate"),
        Some("Bearer error=\"invalid_token\""),
        "{r:?}"
    );
    // A personal token still works.
    let plain = api.user("plain", Role::Editor, false).await;
    let personal = api.bearer(&plain, &["*"]).await;
    let r = personal.get(&api, "/api/v1/auth/me").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // Defence in depth: even an administrator grants users, imports and the system
    // read-only, since no MCP tool writes them (12 §3.3).
    let request = begin(&api, &client_id, "").await;
    let path = format!("/api/v1/oauth/requests/{request}");
    let view = c.get(&api, &path).await.json();
    let grantable = view["grantable"].as_array().unwrap().clone();
    assert_eq!(grantable.len(), 11, "{grantable:?}");
    for unused in ["users:write", "import:write", "system:write"] {
        assert!(!grantable.contains(&json!(unused)), "{unused}");
        c.post(
            &api,
            &format!("{path}/approve"),
            json!({"scopes": [unused]}),
        )
        .await
        .assert_problem(403, "forbidden");
    }
    for read in ["users:read", "import:read", "system:read"] {
        assert!(grantable.contains(&json!(read)), "{read}");
    }
    api
}

async fn bad_client_shows_error_page_not_redirect(api: Api) -> Api {
    let client_id = registered(&api).await;
    let evil = "https://evil.example/cb";
    for path in [
        // Unknown client.
        format!(
            "/oauth/authorize?response_type=code&client_id=ferc_unknownunknownunknow&redirect_uri={}&state=x",
            enc(evil)
        ),
        // Not a client id at all.
        format!(
            "/oauth/authorize?client_id={}&redirect_uri={}",
            enc("<script>alert(1)</script>"),
            enc(evil)
        ),
        // A redirect URI the client did not register.
        format!(
            "/oauth/authorize?response_type=code&client_id={}&redirect_uri={}",
            enc(&client_id),
            enc(evil)
        ),
        // No client id, no redirect URI.
        "/oauth/authorize".to_owned(),
        // A repeated parameter.
        format!(
            "/oauth/authorize?client_id={0}&client_id={0}&redirect_uri={1}",
            enc(&client_id),
            enc(REDIRECT)
        ),
    ] {
        let r = api.get(&path).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}: {r:?}");
        assert!(r.header("location").is_none(), "{path}");
        assert!(r.header("content-type").unwrap().starts_with("text/html"));
        assert_eq!(r.header("cache-control"), Some("no-store"));
        assert_eq!(r.header("x-frame-options"), Some("DENY"));
        assert_eq!(
            r.header("content-security-policy"),
            Some("default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'")
        );
        assert!(!r.body.contains("<script>"), "{}", r.body);
        assert!(r.body.contains("<!doctype html>"), "{}", r.body);
    }
    api
}

async fn mcp_disabled_hides_everything(api: Api) -> Api {
    let (mut c, client_id, _, _) = connected(&api, &["content:read"]).await;
    let request = begin(&api, &client_id, "").await;
    let r = c
        .send(
            &api,
            Method::PATCH,
            "/api/v1/settings",
            Some(&json!({"mcp.enabled": false})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["mcp.enabled"], false);
    let gets = [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
        &authorize_path(&client_id, ""),
    ];
    for path in gets {
        assert_eq!(api.get(path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    for path in ["/oauth/token", "/oauth/register", "/oauth/revoke"] {
        assert_eq!(
            post_form(&api, path, "token=x").await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        assert_eq!(options(&api, path).await.status, StatusCode::NOT_FOUND);
        // Another method is a 404 too, not a 405.
        assert_eq!(api.get(path).await.status, StatusCode::NOT_FOUND);
    }
    let consent = format!("/api/v1/oauth/requests/{request}");
    c.get(&api, &consent).await.assert_problem(404, "not-found");
    c.post(
        &api,
        &format!("{consent}/approve"),
        json!({"scopes": ["content:read"]}),
    )
    .await
    .assert_problem(404, "not-found");
    c.post_empty(&api, &format!("{consent}/deny"))
        .await
        .assert_problem(404, "not-found");
    // Connected apps stay listed (and can be disconnected).
    let grants = c.get(&api, "/api/v1/oauth/grants").await;
    assert_eq!(grants.status, StatusCode::OK);
    assert_eq!(grants.json()["items"].as_array().unwrap().len(), 1);

    // On again: everything answers again, the pending request included.
    let r = c
        .send(
            &api,
            Method::PATCH,
            "/api/v1/settings",
            Some(&json!({"mcp.enabled": true})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        api.get("/.well-known/oauth-authorization-server")
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(c.get(&api, &consent).await.status, StatusCode::OK);
    api
}

// ---- metadata documents from a local TLS server ------------------------------------------

const DOC_HOST: &str = "api.example.test";
const CERT: &[u8] = include_bytes!("../fixtures/tls/cert.pem");
const KEY: &[u8] = include_bytes!("../fixtures/tls/key.pem");

/// One request the test server read: path and headers.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A TLS server on `127.0.0.1` (certificate for `api.example.test`) answering by path.
struct DocServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for DocServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn doc_url(path: &str) -> String {
    format!("https://{DOC_HOST}{path}")
}

fn client_doc(path: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "client_id": doc_url(path),
        "client_name": "Local App",
        "redirect_uris": [REDIRECT],
        "logo_uri": "https://api.example.test/logo.png",
    }))
    .unwrap()
}

fn http_response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

impl DocServer {
    async fn start() -> Self {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT)
            .collect::<Result<_, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, seen) = (acceptor.clone(), s.clone());
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(tcp).await {
                        let Some(req) = read_head(&mut tls).await else {
                            return;
                        };
                        seen.lock().unwrap().push(req.clone());
                        answer(&mut tls, &req.path).await;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });
        Self { addr, seen, task }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// A fetcher whose resolver points `api.example.test` here, trusting the test
    /// certificate, with the address rule `allow_ip`.
    fn fetcher(&self, allow_ip: fn(IpAddr) -> bool) -> OAuthFetcher {
        let resolver = Arc::new(FixedResolver(self.addr));
        OAuthFetcher::for_tests(EgressOverrides {
            resolver,
            roots: vec![reqwest::Certificate::from_pem(CERT).unwrap()],
            allow_ip,
        })
        .unwrap()
    }
}

async fn read_head<S: tokio::io::AsyncRead + Unpin>(io: &mut S) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match io.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_owned();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(Seen { path, headers })
}

async fn answer<S: tokio::io::AsyncWrite + Unpin>(io: &mut S, path: &str) {
    let ok = |body: Vec<u8>| http_response(200, &[("Content-Type", "application/json")], &body);
    let bytes = match path {
        "/oauth/client.json" | "/hop/0" | "/landing.json" => ok(client_doc(path)),
        "/big-cl" => ok(vec![b' '; 16 * 1024 + 1]),
        "/to-http" => http_response(
            302,
            &[("Location", "http://api.example.test/landing.json")],
            b"",
        ),
        "/to-landing" => http_response(302, &[("Location", "/landing.json")], b""),
        "/multiple" => http_response(300, &[("Location", "/landing.json")], b""),
        "/not-modified" => http_response(304, &[("Location", "/landing.json")], b""),
        "/big" => {
            // No Content-Length: 1 MiB of spaces, then the connection closes. The
            // fetcher must stop reading at 16 KiB.
            if io
                .write_all(b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            let block = vec![b' '; 64 * 1024];
            for _ in 0..16 {
                if io.write_all(&block).await.is_err() {
                    return;
                }
            }
            return;
        }
        p if p.starts_with("/hop/") => match p["/hop/".len()..].parse::<u32>() {
            Ok(n) => http_response(302, &[("Location", &format!("/hop/{}", n - 1))], b""),
            Err(_) => http_response(404, &[], b""),
        },
        _ => http_response(404, &[], b"missing"),
    };
    let _ = io.write_all(&bytes).await;
}

/// Answers `addr` for every name.
struct FixedResolver(SocketAddr);

impl Resolve for FixedResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        let addr = self.0;
        Box::pin(async move { Ok(Box::new(std::iter::once(addr)) as Addrs) })
    }
}

async fn cimd_client_fetched_from_local_server(api: Api) -> Api {
    let server = DocServer::start().await;
    let router = api.router_with_fetcher(Arc::new(server.fetcher(|_| true)));
    let client_id = doc_url("/oauth/client.json");
    let r = send(
        &router,
        request(Method::GET, &authorize_path(&client_id, "")),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    let request_id = r
        .header("location")
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_owned();
    // The fetch: JSON wanted, no credentials of any kind sent.
    let seen = server.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].path, "/oauth/client.json");
    assert_eq!(seen[0].header("accept"), Some("application/json"));
    for name in ["cookie", "authorization", "referer"] {
        assert!(seen[0].header(name).is_none(), "{name}: {seen:?}");
    }
    assert!(
        seen[0]
            .header("user-agent")
            .unwrap()
            .starts_with("ferrite/")
    );

    api.user("boss", Role::Admin, false).await;
    let mut c = api.logged_in("boss").await;
    let req = c
        .builder(Method::GET, &format!("/api/v1/oauth/requests/{request_id}"))
        .body(Body::empty())
        .unwrap();
    let view = c.send_on(&router, req).await;
    assert_eq!(view.status, StatusCode::OK, "{view:?}");
    let view = view.json();
    assert_eq!(view["verified"], true);
    assert_eq!(view["client"]["kind"], "metadata");
    assert_eq!(view["client"]["name"], "Local App");
    assert_eq!(
        view["client"]["logo_uri"],
        "https://api.example.test/logo.png"
    );
    // Cached: a second authorization request does not fetch again.
    let r = send(
        &router,
        request(Method::GET, &authorize_path(&client_id, "")),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    assert_eq!(server.seen().len(), 1);
    // With the production address rule, the loopback server is refused: an error page.
    let strict = api.router_with_fetcher(Arc::new(server.fetcher(is_public_ip)));
    let other = doc_url("/landing.json");
    let r = send(&strict, request(Method::GET, &authorize_path(&other, ""))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{r:?}");
    assert!(r.header("location").is_none());
    api
}

/// The fetcher's egress rules, straight (no site needed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_fetcher_enforces_egress_rules() {
    let server = DocServer::start().await;
    let fetcher = server.fetcher(|_| true);
    let fetch = |path: &str| {
        let url = Url::parse(&doc_url(path)).unwrap();
        let fetcher = &fetcher;
        async move { fetcher.fetch(&url).await }
    };
    assert_eq!(
        fetch("/oauth/client.json").await.unwrap(),
        client_doc("/oauth/client.json")
    );
    // https redirects are followed, at most three.
    assert_eq!(
        fetch("/to-landing").await.unwrap(),
        client_doc("/landing.json")
    );
    assert!(fetch("/hop/3").await.is_ok());
    assert!(matches!(fetch("/hop/4").await, Err(FetchError::Refused(_))));
    // Never to http.
    assert!(matches!(
        fetch("/to-http").await,
        Err(FetchError::Refused(_))
    ));
    // 16 KiB at most, declared or streamed.
    assert!(matches!(fetch("/big-cl").await, Err(FetchError::TooLarge)));
    assert!(matches!(fetch("/big").await, Err(FetchError::TooLarge)));
    assert!(matches!(
        fetch("/missing").await,
        Err(FetchError::Failed(_))
    ));
    // Only 301, 302, 303, 307 and 308 are followed; another 3xx is a failure.
    for path in ["/multiple", "/not-modified"] {
        assert!(
            matches!(fetch(path).await, Err(FetchError::Failed(_))),
            "{path}"
        );
    }
    // Not https at all, credentials, IP literals off the address rule.
    let strict = server.fetcher(is_public_ip);
    for url in [
        "http://api.example.test/oauth/client.json",
        "https://user:pw@api.example.test/oauth/client.json",
        "https://127.0.0.1/oauth/client.json",
    ] {
        let got = strict.fetch(&Url::parse(url).unwrap()).await;
        assert!(matches!(got, Err(FetchError::Refused(_))), "{url}: {got:?}");
    }
    // A name resolving to loopback is refused by the production rule.
    let got = strict
        .fetch(&Url::parse(&doc_url("/oauth/client.json")).unwrap())
        .await;
    assert!(matches!(got, Err(FetchError::Refused(_))), "{got:?}");
}
