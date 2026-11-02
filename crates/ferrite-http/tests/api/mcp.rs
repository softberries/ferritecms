//! M5 Task 5: the MCP endpoint `/mcp` (design 12 §2, §3.1, §5.1, §5.5, §6): stateless
//! Streamable HTTP, bearer-only authentication with the 401 challenge, the Origin check,
//! the off switch, the per-user tool-call limit, the site tools, resources and prompts.
//!
//! The helpers ([`rpc`], [`mcp_call`], [`mcp_call_v`], [`tool`]) are for the later MCP
//! test files too.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use std::sync::Arc;

use ferrite_core::{Actor, ContentDraft};
use ferrite_domain::{Role, User};
use ferrite_http::mcp::{Caller, FerriteMcp};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer, SITE_TITLE, SITE_URL, builder, send},
    oauth::connected,
};

api_tests!(
    no_token_is_401_with_resource_metadata,
    session_cookie_is_not_accepted,
    foreign_origin_is_403,
    mcp_disabled_is_404,
    initialize_reports_ferritecms_and_instructions,
    tools_list_has_annotations_and_schemas,
    site_overview_reflects_role_and_scopes,
    site_search_finds_only_readable_content,
    get_is_405,
    oversized_body_is_413,
    just_under_the_body_limit_is_accepted,
    answers_are_no_store,
    rate_limit_returns_tool_error,
    bad_arguments_are_tool_errors,
    oauth_and_personal_tokens_both_work,
    resources_and_prompts,
    audit_helper_names_the_client,
);

// ---- helpers ---------------------------------------------------------------------------

/// A client of the protocol revision before the stateless one (an `initialize`
/// handshake, the `MCP-Protocol-Version` header, no per-request metadata).
pub(crate) const V_LEGACY: &str = "2025-06-18";
/// The current revision: per-request `_meta`, the standard `Mcp-Method`/`Mcp-Name`
/// headers, `server/discover` instead of `initialize`.
pub(crate) const V_CURRENT: &str = "2026-07-28";

const RESOURCE_METADATA: &str = "https://example.com/.well-known/oauth-protected-resource/mcp";

/// `POST /mcp` with one JSON-RPC request as protocol `version` speaks it, with the bearer
/// `token` if given.
pub(crate) fn rpc(
    token: Option<&str>,
    version: &str,
    method: &str,
    params: Value,
) -> Request<Body> {
    let mut params = match params {
        Value::Object(map) => map,
        Value::Null => serde_json::Map::new(),
        other => panic!("params must be an object: {other}"),
    };
    let mut b = builder(Method::POST, "/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if method != "initialize" {
        b = b.header("mcp-protocol-version", version);
    }
    if version >= V_CURRENT {
        params.insert(
            "_meta".into(),
            json!({
                "io.modelcontextprotocol/protocolVersion": version,
                "io.modelcontextprotocol/clientInfo": {"name": "test-client", "version": "1.0.0"},
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
        b = b
            .header("mcp-protocol-version", version)
            .header("mcp-method", method);
        let name = match method {
            "tools/call" | "prompts/get" => params.get("name"),
            "resources/read" => params.get("uri"),
            _ => None,
        };
        if let Some(name) = name.and_then(Value::as_str) {
            b = b.header("mcp-name", name);
        }
    }
    if let Some(token) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    b.body(Body::from(body.to_string())).unwrap()
}

/// `method` with `params` as a [`V_CURRENT`] client; the status and the JSON body.
pub(crate) async fn mcp_call(
    api: &Api,
    token: &str,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    mcp_call_v(api, token, V_CURRENT, method, params).await
}

/// [`mcp_call`] as a client of protocol `version`.
pub(crate) async fn mcp_call_v(
    api: &Api,
    token: &str,
    version: &str,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let r = api.send(rpc(Some(token), version, method, params)).await;
    assert!(
        r.header("content-type")
            .is_some_and(|t| t.starts_with("application/json")),
        "{r:?}"
    );
    (r.status, r.json())
}

/// The result of tool `name` called with `args` (a JSON-RPC success; the tool result may
/// still be `isError`).
pub(crate) async fn tool(api: &Api, token: &str, name: &str, args: Value) -> Value {
    let (status, body) = mcp_call(
        api,
        token,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}

/// The text of a tool result's first content block.
pub(crate) fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or("")
}

async fn editor(api: &Api, scopes: &[&str]) -> (User, Bearer) {
    let u = api.user("eddie", Role::Editor, false).await;
    let t = api.bearer(&u, scopes).await;
    (u, t)
}

fn draft(kind: &str, title: &str, text: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}
        ]}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

// ---- transport and authentication ------------------------------------------------------

async fn no_token_is_401_with_resource_metadata(api: Api) -> Api {
    let init = json!({"protocolVersion": V_LEGACY, "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"}});
    let r = api
        .send(rpc(None, V_LEGACY, "initialize", init.clone()))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{r:?}");
    assert_eq!(
        r.header("www-authenticate"),
        Some(format!("Bearer resource_metadata=\"{RESOURCE_METADATA}\"").as_str()),
        "{r:?}"
    );
    assert_eq!(r.header("cache-control"), Some("no-store"), "{r:?}");
    // A bad token: the same challenge with `error="invalid_token"`.
    let r = api
        .send(rpc(
            Some("fer_notarealtoken"),
            V_LEGACY,
            "initialize",
            init.clone(),
        ))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{r:?}");
    assert_eq!(
        r.header("www-authenticate"),
        Some(
            format!("Bearer resource_metadata=\"{RESOURCE_METADATA}\", error=\"invalid_token\"")
                .as_str()
        ),
        "{r:?}"
    );
    // Two Authorization headers, or another scheme: `invalid_request`.
    let (_, good) = editor(&api, &["*"]).await;
    let mut req = rpc(Some(&good.0), V_LEGACY, "initialize", init.clone());
    req.headers_mut().append(
        header::AUTHORIZATION,
        format!("Bearer {}", good.0).parse().unwrap(),
    );
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{r:?}");
    assert!(
        r.header("www-authenticate")
            .is_some_and(|v| v.ends_with("error=\"invalid_request\"")),
        "{r:?}"
    );
    let mut req = rpc(None, V_LEGACY, "initialize", init);
    req.headers_mut()
        .insert(header::AUTHORIZATION, "Basic dXNlcjpwYXNz".parse().unwrap());
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{r:?}");
    api
}

async fn session_cookie_is_not_accepted(api: Api) -> Api {
    api.user("ada", Role::Admin, false).await;
    let c = api.logged_in("ada").await;
    // A signed-in browser with its cookie, CSRF token and the site's origin.
    let req = rpc(None, V_LEGACY, "tools/list", json!({}));
    let (parts, body) = req.into_parts();
    let mut b = c.builder(Method::POST, "/mcp");
    for (name, value) in &parts.headers {
        b = b.header(name, value);
    }
    let r = api.send(b.body(body).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{r:?}");
    assert_eq!(
        r.header("www-authenticate"),
        Some(format!("Bearer resource_metadata=\"{RESOURCE_METADATA}\"").as_str()),
        "{r:?}"
    );
    api
}

async fn foreign_origin_is_403(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let mut req = rpc(Some(&t.0), V_LEGACY, "tools/list", json!({}));
    req.headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{r:?}");
    // `Origin: null` is foreign too, and so is a second Origin header.
    let mut req = rpc(Some(&t.0), V_LEGACY, "tools/list", json!({}));
    req.headers_mut()
        .insert(header::ORIGIN, "null".parse().unwrap());
    assert_eq!(api.send(req).await.status, StatusCode::FORBIDDEN);
    let mut req = rpc(Some(&t.0), V_LEGACY, "tools/list", json!({}));
    req.headers_mut()
        .append(header::ORIGIN, SITE_URL.parse().unwrap());
    req.headers_mut()
        .append(header::ORIGIN, SITE_URL.parse().unwrap());
    assert_eq!(api.send(req).await.status, StatusCode::FORBIDDEN);
    // Checked before the token: a foreign page learns nothing about it.
    let mut req = rpc(None, V_LEGACY, "tools/list", json!({}));
    req.headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(api.send(req).await.status, StatusCode::FORBIDDEN);
    // The site's own origin (any case) and no Origin at all pass.
    let mut req = rpc(Some(&t.0), V_LEGACY, "tools/list", json!({}));
    req.headers_mut()
        .insert(header::ORIGIN, "https://EXAMPLE.com".parse().unwrap());
    let r = api.send(req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let (status, _) = mcp_call_v(&api, &t.0, V_LEGACY, "tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    api
}

async fn mcp_disabled_is_404(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let admin = api.user("ada", Role::Admin, false).await;
    let set = |on: bool| {
        let api = &api;
        async move {
            api.api
                .options
                .set(
                    &Actor::new(admin.id, Role::Admin),
                    "mcp.enabled",
                    &json!(on),
                    true,
                )
                .await
                .unwrap();
        }
    };
    set(false).await;
    for req in [
        rpc(Some(&t.0), V_LEGACY, "tools/list", json!({})),
        rpc(None, V_LEGACY, "tools/list", json!({})),
        builder(Method::GET, "/mcp").body(Body::empty()).unwrap(),
    ] {
        let r = api.send(req).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{r:?}");
    }
    set(true).await;
    let (status, _) = mcp_call(&api, &t.0, "tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    api
}

async fn get_is_405(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    for method in [Method::GET, Method::DELETE, Method::PUT] {
        let r = api
            .send(
                builder(method.clone(), "/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {}", t.0))
                    .header(header::ACCEPT, "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {r:?}");
        assert_eq!(r.header("allow"), Some("POST"), "{r:?}");
    }
    api
}

async fn oversized_body_is_413(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let filler = "x".repeat(ferrite_http::mcp::MAX_MCP_BODY);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"pad": filler}});
    let r = api
        .send(
            builder(Method::POST, "/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-protocol-version", V_LEGACY)
                .header(header::AUTHORIZATION, format!("Bearer {}", t.0))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.status);
    api
}

async fn just_under_the_body_limit_is_accepted(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    // A tool call whose argument fills the body to just under the limit reaches the
    // tool (an unusable cursor: a tool error, not a 413).
    let envelope = rpc(
        Some(&t.0),
        V_CURRENT,
        "tools/call",
        json!({"name": "site_search", "arguments": {"q": "x", "cursor": ""}}),
    );
    let (parts, body) = envelope.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let pad = ferrite_http::mcp::MAX_MCP_BODY - bytes.len() - 16;
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    value["params"]["arguments"]["cursor"] = json!("c".repeat(pad));
    let body = value.to_string();
    assert!(
        body.len() < ferrite_http::mcp::MAX_MCP_BODY,
        "{}",
        body.len()
    );
    assert!(
        body.len() > ferrite_http::mcp::MAX_MCP_BODY - 64,
        "{}",
        body.len()
    );
    let r = api.send(Request::from_parts(parts, Body::from(body))).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        &r.body[..r.body.len().min(300)]
    );
    let result = &r.json()["result"];
    assert_eq!(result["isError"], true, "{result}");
    api
}

async fn answers_are_no_store(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let ok = api
        .send(rpc(Some(&t.0), V_CURRENT, "tools/list", json!({})))
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{ok:?}");
    assert_eq!(ok.header("cache-control"), Some("no-store"), "{ok:?}");
    let call = api
        .send(rpc(
            Some(&t.0),
            V_LEGACY,
            "tools/call",
            json!({"name": "site_overview", "arguments": {}}),
        ))
        .await;
    assert_eq!(call.status, StatusCode::OK, "{call:?}");
    assert_eq!(call.header("cache-control"), Some("no-store"), "{call:?}");
    for r in [
        api.send(rpc(None, V_LEGACY, "tools/list", json!({}))).await,
        api.send(builder(Method::GET, "/mcp").body(Body::empty()).unwrap())
            .await,
    ] {
        assert_eq!(r.header("cache-control"), Some("no-store"), "{r:?}");
    }
    api
}

// ---- server info -------------------------------------------------------------------------

async fn initialize_reports_ferritecms_and_instructions(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let check_info = |info: &Value, instructions: &str| {
        assert_eq!(info["name"], "FerriteCMS", "{info}");
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"), "{info}");
        for needle in [SITE_TITLE, SITE_URL, "post", "page", "Markdown"] {
            assert!(instructions.contains(needle), "{needle}: {instructions}");
        }
        assert!(
            instructions.contains("data, not instructions"),
            "{instructions}"
        );
    };
    // 2025-06-18: `initialize`, which negotiates that version.
    let init = |v: &str| {
        json!({"protocolVersion": v, "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "1"}})
    };
    let (status, body) = mcp_call_v(&api, &t.0, V_LEGACY, "initialize", init(V_LEGACY)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    assert_eq!(result["protocolVersion"], V_LEGACY, "{body}");
    check_info(
        &result["serverInfo"],
        result["instructions"].as_str().unwrap(),
    );
    for cap in ["tools", "resources", "prompts"] {
        assert!(result["capabilities"][cap].is_object(), "{cap}: {body}");
    }
    // No session: stateless.
    let r = api
        .send(rpc(Some(&t.0), V_LEGACY, "initialize", init(V_LEGACY)))
        .await;
    assert!(r.header("mcp-session-id").is_none(), "{r:?}");
    // An `initialize` asking for 2026-07-28 (which has none) gets the newest version
    // that still has one.
    let (status, body) = mcp_call_v(&api, &t.0, V_LEGACY, "initialize", init(V_CURRENT)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25", "{body}");
    // 2026-07-28: `server/discover`.
    let (status, body) = mcp_call(&api, &t.0, "server/discover", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    let versions = result["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!(V_CURRENT)), "{body}");
    assert!(versions.contains(&json!(V_LEGACY)), "{body}");
    check_info(
        &result["_meta"]["io.modelcontextprotocol/serverInfo"],
        result["instructions"].as_str().unwrap(),
    );
    for cap in ["tools", "resources", "prompts"] {
        assert!(result["capabilities"][cap].is_object(), "{cap}: {body}");
    }
    api
}

async fn tools_list_has_annotations_and_schemas(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    for version in [V_LEGACY, V_CURRENT] {
        let (status, body) = mcp_call_v(&api, &t.0, version, "tools/list", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let tools = body["result"]["tools"].as_array().unwrap();
        let by_name = |name: &str| {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} missing: {body}"))
        };
        for name in ["site_overview", "site_search"] {
            let t = by_name(name);
            assert!(
                t["description"].as_str().is_some_and(|d| d.len() > 40),
                "{t}"
            );
            assert_eq!(t["inputSchema"]["type"], "object", "{t}");
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{t}");
            assert_eq!(t["annotations"]["openWorldHint"], false, "{t}");
            assert!(t["annotations"]["title"].is_string(), "{t}");
        }
        let search = by_name("site_search");
        assert_eq!(
            search["inputSchema"]["properties"]["q"]["type"], "string",
            "{search}"
        );
        assert_eq!(search["inputSchema"]["required"], json!(["q"]), "{search}");
        for name in tools.iter().map(|t| t["name"].as_str().unwrap()) {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name}"
            );
        }
    }
    api
}

// ---- site tools --------------------------------------------------------------------------

async fn site_overview_reflects_role_and_scopes(api: Api) -> Api {
    let (ed, narrow) = editor(&api, &["content:read"]).await;
    let r = tool(&api, &narrow.0, "site_overview", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    let o = &r["structuredContent"];
    assert_eq!(o["site"]["title"], SITE_TITLE, "{o}");
    assert_eq!(o["site"]["url"], SITE_URL, "{o}");
    assert_eq!(o["site"]["admin_url"], format!("{SITE_URL}/admin/"), "{o}");
    assert_eq!(o["site"]["version"], env!("CARGO_PKG_VERSION"), "{o}");
    assert!(o["site"]["theme"].is_string(), "{o}");
    assert_eq!(o["caller"]["role"], "editor", "{o}");
    assert_eq!(o["caller"]["login"], "eddie", "{o}");
    assert_eq!(o["caller"]["scopes"], json!(["content:read"]), "{o}");
    assert_eq!(o["caller"]["connection"], "api_token", "{o}");
    assert_eq!(o["caller"]["client"], "API token test", "{o}");
    let types: Vec<&str> = o["content_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"post") && types.contains(&"page"), "{o}");
    assert!(!types.contains(&"media"), "{o}");
    let post = o["content_types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "post")
        .unwrap();
    assert_eq!(post["counts"]["draft"], 0, "{post}");
    assert!(o["taxonomies"].is_array(), "{o}");
    // No `menus:read`: the section is left out and says why.
    assert!(o["menus"].is_null(), "{o}");
    assert!(
        o["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["section"] == "menus"
                && u["reason"]
                    .as_str()
                    .is_some_and(|r| r.contains("menus:read"))),
        "{o}"
    );
    assert!(text(&r).contains(SITE_TITLE), "{r}");
    // Every scope: menus too, and a draft counted.
    let wide = api.bearer(&ed, &["*"]).await;
    api.api
        .content
        .create(
            &Actor::new(ed.id, Role::Editor),
            draft("post", "Draft one", "x"),
        )
        .await
        .unwrap();
    let r = tool(&api, &wide.0, "site_overview", json!({})).await;
    let o = &r["structuredContent"];
    assert!(o["menus"].is_array(), "{o}");
    assert_eq!(o["unavailable"], json!([]), "{o}");
    let post = o["content_types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "post")
        .unwrap();
    assert_eq!(post["counts"]["draft"], 1, "{post}");
    assert_eq!(o["caller"]["scopes"].as_array().unwrap().len(), 24, "{o}");
    // A token without `content:read` still gets the site and caller sections.
    let none = api.bearer(&ed, &["media:read"]).await;
    let r = tool(&api, &none.0, "site_overview", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    let o = &r["structuredContent"];
    assert!(o["content_types"].is_null(), "{o}");
    assert_eq!(o["caller"]["scopes"], json!(["media:read"]), "{o}");
    api
}

async fn site_search_finds_only_readable_content(api: Api) -> Api {
    let boss = api.user("boss", Role::Admin, false).await;
    let boss_actor = Actor::new(boss.id, Role::Admin);
    let secret = api
        .api
        .content
        .create(
            &boss_actor,
            draft("post", "Zebra secret", "a hidden zebra plan"),
        )
        .await
        .unwrap();
    let public = api
        .api
        .content
        .create(
            &boss_actor,
            draft("page", "Zebra news", "zebras everywhere"),
        )
        .await
        .unwrap();
    api.api
        .content
        .publish(&boss_actor, public.id, public.version)
        .await
        .unwrap();
    let boss_token = api.bearer(&boss, &["*"]).await;
    let r = tool(&api, &boss_token.0, "site_search", json!({"q": "zebra"})).await;
    assert_eq!(r["isError"], false, "{r}");
    let ids: Vec<&str> = r["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "{r}");
    assert!(ids.contains(&secret.id.to_string().as_str()), "{r}");
    let hit = r["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == public.id.to_string())
        .unwrap();
    assert_eq!(hit["type"], "page", "{hit}");
    assert_eq!(hit["status"], "published", "{hit}");
    assert!(
        hit["url"].as_str().is_some_and(|u| u.starts_with(SITE_URL)),
        "{hit}"
    );
    // The same fields as a content_list summary: version, public, admin_url.
    assert_eq!(hit["public"], true, "{hit}");
    assert_eq!(hit["version"], public.version + 1, "{hit}");
    assert_eq!(
        hit["admin_url"],
        format!("{SITE_URL}/admin/content/page/{}", public.id),
        "{hit}"
    );
    let draft_hit = r["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == secret.id.to_string())
        .unwrap();
    assert_eq!(draft_hit["public"], false, "{draft_hit}");
    assert_eq!(
        draft_hit["url"],
        format!("{SITE_URL}/_preview/{}", secret.id),
        "{draft_hit}"
    );
    // An author (no `edit_others`) does not find the admin's draft.
    let author = api.user("ann", Role::Author, false).await;
    let t = api.bearer(&author, &["*"]).await;
    let r = tool(&api, &t.0, "site_search", json!({"q": "zebra"})).await;
    let items = r["structuredContent"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{r}");
    assert_eq!(items[0]["id"], public.id.to_string(), "{r}");
    // Limited to a type.
    let r = tool(
        &api,
        &boss_token.0,
        "site_search",
        json!({"q": "zebra", "types": ["post"]}),
    )
    .await;
    assert_eq!(
        r["structuredContent"]["items"].as_array().unwrap().len(),
        1,
        "{r}"
    );
    // No `content:read`: a tool error naming the scope.
    let none = api.bearer(&author, &["media:read"]).await;
    let r = tool(&api, &none.0, "site_search", json!({"q": "zebra"})).await;
    assert_eq!(r["isError"], true, "{r}");
    assert!(text(&r).contains("content:read"), "{r}");
    assert_eq!(r["structuredContent"]["status"], 403, "{r}");
    api
}

async fn rate_limit_returns_tool_error(api: Api) -> Api {
    let (ed, t) = editor(&api, &["*"]).await;
    api.in_one_window(std::time::Duration::from_secs(20), |_| {
        let (api, t) = (&api, &t);
        async move {
            // Spend the minute's calls directly, then one more through MCP.
            for _ in 0..120 {
                api.api.auth.hit_mcp_limit(ed.id).await.unwrap();
            }
            let r = tool(api, &t.0, "site_overview", json!({})).await;
            assert_eq!(r["isError"], true, "{r}");
            assert!(text(&r).to_lowercase().contains("too many"), "{r}");
            assert_eq!(r["structuredContent"]["status"], 429, "{r}");
            // Listing is not a tool call.
            let (status, _) = mcp_call(api, &t.0, "tools/list", json!({})).await;
            assert_eq!(status, StatusCode::OK);
            // Reading a resource and getting a prompt count too: their results have no
            // isError, so they are JSON-RPC errors.
            for (method, params) in [
                ("resources/read", json!({"uri": "ferrite://site"})),
                (
                    "prompts/get",
                    json!({"name": "site_health", "arguments": {}}),
                ),
            ] {
                for version in [V_LEGACY, V_CURRENT] {
                    let (status, body) =
                        mcp_call_v(api, &t.0, version, method, params.clone()).await;
                    assert_eq!(status, StatusCode::OK, "{method} {body}");
                    let e = &body["error"];
                    assert_eq!(e["code"], -32029, "{method} {body}");
                    assert_eq!(e["data"]["status"], 429, "{method} {body}");
                    assert!(e["data"]["retry_after_secs"].is_u64(), "{method} {body}");
                    assert!(
                        e["message"]
                            .as_str()
                            .unwrap()
                            .starts_with("Too many requests"),
                        "{body}"
                    );
                }
            }
        }
    })
    .await;
    api
}

async fn bad_arguments_are_tool_errors(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let r = tool(&api, &t.0, "site_search", json!({"q": 7})).await;
    assert_eq!(r["isError"], true, "{r}");
    assert!(!text(&r).is_empty(), "{r}");
    // An unknown tool stays a protocol error.
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "tools/call",
        json!({"name": "no_such_tool", "arguments": {}}),
    )
    .await;
    assert!(body["error"].is_object(), "{body}");
    api
}

async fn oauth_and_personal_tokens_both_work(api: Api) -> Api {
    let (_, _, access, _) = connected(&api, &["content:read", "users:read"]).await;
    for version in [V_LEGACY, V_CURRENT] {
        let (status, body) = mcp_call_v(
            &api,
            &access,
            version,
            "tools/call",
            json!({"name": "site_overview", "arguments": {}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let o = &body["result"]["structuredContent"];
        assert_eq!(o["caller"]["connection"], "oauth", "{o}");
        assert_eq!(o["caller"]["client"], "Test Assistant", "{o}");
        assert_eq!(o["caller"]["role"], "admin", "{o}");
        assert_eq!(
            o["caller"]["scopes"],
            json!(["content:read", "users:read"]),
            "{o}"
        );
    }
    let (_, personal) = editor(&api, &["*"]).await;
    let r = tool(&api, &personal.0, "site_overview", json!({})).await;
    assert_eq!(r["structuredContent"]["caller"]["connection"], "api_token");
    // The OAuth token stays refused by the REST API.
    let r = send(
        &api.router,
        builder(Method::GET, "/api/v1/auth/me")
            .header(header::AUTHORIZATION, format!("Bearer {access}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    api
}

// ---- resources and prompts ---------------------------------------------------------------

async fn resources_and_prompts(api: Api) -> Api {
    let (_, t) = editor(&api, &["*"]).await;
    let (_, body) = mcp_call(&api, &t.0, "resources/list", json!({})).await;
    let uris: Vec<&str> = body["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("{body}"))
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert!(uris.contains(&"ferrite://site"), "{body}");
    assert!(uris.contains(&"ferrite://guide/writing"), "{body}");
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "resources/read",
        json!({"uri": "ferrite://site"}),
    )
    .await;
    let c = &body["result"]["contents"][0];
    assert_eq!(c["mimeType"], "text/markdown", "{body}");
    assert!(
        c["text"]
            .as_str()
            .is_some_and(|s| s.starts_with("# ") && s.contains(SITE_TITLE)),
        "{body}"
    );
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "resources/read",
        json!({"uri": "ferrite://guide/writing"}),
    )
    .await;
    let guide = body["result"]["contents"][0]["text"].as_str().unwrap();
    for needle in ["ferrite:media/", "ferrite-block", "seo", "# "] {
        assert!(guide.contains(needle), "{needle}: {guide}");
    }
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "resources/read",
        json!({"uri": "ferrite://nope"}),
    )
    .await;
    assert!(body["error"].is_object(), "{body}");

    let (_, body) = mcp_call(&api, &t.0, "prompts/list", json!({})).await;
    let prompts = body["result"]["prompts"].as_array().unwrap();
    let names: Vec<&str> = prompts
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    for name in ["write_post", "review_seo", "site_health", "tidy_media"] {
        assert!(names.contains(&name), "{name}: {body}");
    }
    let write = prompts.iter().find(|p| p["name"] == "write_post").unwrap();
    let args: Vec<&str> = write["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(args.contains(&"topic") && args.contains(&"tone"), "{write}");
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "prompts/get",
        json!({"name": "write_post", "arguments": {"topic": "spring opening hours"}}),
    )
    .await;
    let msg = &body["result"]["messages"][0];
    assert_eq!(msg["role"], "user", "{body}");
    assert!(
        msg["content"]["text"]
            .as_str()
            .is_some_and(|s| s.contains("spring opening hours") && s.contains("content_create")),
        "{body}"
    );
    // Prompts have no side effects: nothing was written.
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "prompts/get",
        json!({"name": "site_health", "arguments": {}}),
    )
    .await;
    assert!(body["result"]["messages"].is_array(), "{body}");
    api
}

// ---- helpers for the write tools ---------------------------------------------------------

/// The audit helper the write tools end with: `mcp.<tool>`, the caller as actor, the
/// client's name in `details`.
async fn audit_helper_names_the_client(api: Api) -> Api {
    let (ed, t) = editor(&api, &["content:read"]).await;
    let who = api
        .api
        .auth
        .resolve_token(&t.0, None)
        .await
        .unwrap()
        .unwrap();
    let caller = Caller {
        who,
        client: "API token test".into(),
        ip: None,
    };
    let server = FerriteMcp::new(api.api.clone(), Arc::new(FerriteMcp::routers()));
    server
        .audit(
            &caller,
            "content_create",
            "content",
            Some("x1".into()),
            json!({"type": "post"}),
        )
        .await;
    // A failing append (here core refuses the action name) is logged, never returned:
    // the helper has nothing to return, and nothing is written.
    server
        .audit(&caller, "Not.A.Tool", "content", None, json!({}))
        .await;
    let rows = api
        .store
        .read()
        .audit()
        .list(&ferrite_domain::AuditQuery::first(10))
        .await
        .unwrap()
        .items;
    let row = rows
        .iter()
        .find(|r| r.action == "mcp.content_create")
        .unwrap();
    assert_eq!(row.actor_id, Some(ed.id));
    assert_eq!(row.target_id.as_deref(), Some("x1"));
    assert_eq!(
        row.details,
        json!({"type": "post", "client": "API token test"})
    );
    assert_eq!(
        rows.iter().filter(|r| r.action.starts_with("mcp.")).count(),
        1
    );
    api
}
