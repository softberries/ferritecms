//! M5 Task 7: the media, taxonomy, menu, content type and redirect tools of the MCP
//! endpoint (design 12 §5.1, §5.2): uploads from base64 and from a URL (with the SSRF and
//! size rules), alt-text edits with the stale-version rule, term CRUD, menus as nested
//! trees, content types with field schemas, redirects, per-area scopes, annotations and
//! the audit rows.

use axum::{
    Router,
    body::Body,
    http::{StatusCode, header},
    routing::get,
};
use ferrite_domain::{AuditQuery, Role, User};
use ferrite_http::media::Uploader;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::{
    api_tests,
    harness::{Api, Bearer, MAX_UPLOAD, SITE_URL, send},
    mcp::{mcp_call, rpc, text, tool},
};

api_tests!(
    upload_base64_png_records_media,
    upload_from_url_uses_egress_rules,
    url_fetches_are_limited_and_hold_no_upload_slot,
    oversize_upload_refused,
    upload_takes_one_source_and_authorizes_first,
    alt_text_update,
    media_trash_restore_and_list,
    term_crud,
    menu_set_replaces_tree,
    type_create_with_fields,
    redirect_crud,
    scopes_limit_each_area,
    structure_tools_are_annotated,
    structure_writes_are_audited,
);

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A file that sniffs as PNG, `len` bytes long.
pub(crate) fn png(tag: &str, len: usize) -> Vec<u8> {
    let mut v = PNG_MAGIC.to_vec();
    v.extend_from_slice(b"\0\0\0\rIHDR");
    v.extend_from_slice(tag.as_bytes());
    v.resize(len, 0);
    v
}

pub(crate) fn b64(bytes: &[u8]) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD.encode(bytes)
}

async fn admin(api: &Api) -> (User, Bearer) {
    let u = api.user("ada", Role::Admin, false).await;
    let t = api.bearer(&u, &["*"]).await;
    (u, t)
}

/// The structured content of a successful result.
fn ok(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

/// The problem of a failed result.
fn problem(r: &Value) -> &Value {
    assert_eq!(r["isError"], true, "{r}");
    &r["structuredContent"]
}

/// `name` called as a client of the current protocol on `router`.
async fn tool_on(router: &Router, token: &str, name: &str, args: Value) -> Value {
    let req = rpc(
        Some(token),
        crate::mcp::V_CURRENT,
        "tools/call",
        json!({"name": name, "arguments": args}),
    );
    let r = send(router, req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}

async fn mcp_rows(api: &Api) -> Vec<ferrite_domain::AuditEntry> {
    api.store
        .read()
        .audit()
        .list(&AuditQuery::first(200))
        .await
        .unwrap()
        .items
        .into_iter()
        .filter(|r| r.action.starts_with("mcp."))
        .collect()
}

async fn media_count(api: &Api, t: &Bearer) -> usize {
    let r = tool(api, &t.0, "media_list", json!({})).await;
    ok(&r)["items"].as_array().unwrap().len()
}

async fn upload(api: &Api, t: &Bearer, name: &str, title: &str, alt: &str) -> Value {
    let r = tool(
        api,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&png(name, 1000)), "filename": format!("{name}.png"),
               "title": title, "alt": alt}),
    )
    .await;
    ok(&r).clone()
}

// ---- media -----------------------------------------------------------------------------

async fn upload_base64_png_records_media(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let r = tool(
        &api,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&png("sky", 1000)), "filename": "sky.png",
               "title": "Sky", "alt": "A red sky"}),
    )
    .await;
    let m = ok(&r);
    let id = m["id"].as_str().unwrap();
    assert_eq!(m["mime"], "image/png", "{m}");
    assert_eq!(m["bytes"], 1000, "{m}");
    assert_eq!(m["title"], "Sky", "{m}");
    assert_eq!(m["alt"], "A red sky", "{m}");
    assert_eq!(m["original_name"], "sky.png", "{m}");
    assert_eq!(m["status"], "published", "{m}");
    assert_eq!(m["reference"], format!("ferrite:media/{id}"), "{m}");
    assert_eq!(
        m["markdown"],
        format!("![A red sky](ferrite:media/{id})"),
        "{m}"
    );
    assert!(
        m["url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{SITE_URL}/_media/")),
        "{m}"
    );
    assert!(text(&r).contains(id), "{}", text(&r));

    // REST sees the same item.
    let rest = t.get(&api, &format!("/api/v1/media/{id}")).await;
    assert_eq!(rest.status, StatusCode::OK, "{rest:?}");
    assert_eq!(rest.json()["alt"], "A red sky");

    // The title defaults to the file name; types come from the content, not the name.
    let r = tool(
        &api,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&png("x", 500)), "filename": "no-title.jpeg"}),
    )
    .await;
    let m2 = ok(&r);
    assert_eq!(m2["title"], "no-title", "{m2}");
    assert_eq!(m2["mime"], "image/png", "{m2}");

    // List and get.
    let list = tool(&api, &t.0, "media_list", json!({"kind": "image"})).await;
    assert_eq!(ok(&list)["items"].as_array().unwrap().len(), 2, "{list}");
    let list = tool(&api, &t.0, "media_list", json!({"kind": "video"})).await;
    assert_eq!(ok(&list)["items"].as_array().unwrap().len(), 0, "{list}");
    let got = tool(&api, &t.0, "media_get", json!({"id": id})).await;
    assert_eq!(ok(&got)["id"], id, "{got}");
    let missing = tool(
        &api,
        &t.0,
        "media_get",
        json!({"id": "00000000-0000-4000-8000-000000000000"}),
    )
    .await;
    assert_eq!(problem(&missing)["status"], 404, "{missing}");
    api
}

/// A server on a free loopback port: `/pic.png` a small PNG, `/big.png` over the limit
/// with a `Content-Length`, `/stream.png` over the limit without one, `/gone` a 404.
async fn file_server() -> u16 {
    let app = Router::new()
        .route("/pic.png", get(|| async { png("url", 700) }))
        .route(
            "/big.png",
            get(|| async { png("big", MAX_UPLOAD as usize + 10) }),
        )
        .route(
            "/stream.png",
            get(|| async {
                let first = png("stream", 1000);
                let chunks = (0..=(MAX_UPLOAD / 65_536))
                    .map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![0u8; 65_536])));
                let body = futures::stream::iter(
                    std::iter::once(Ok(bytes::Bytes::from(first))).chain(chunks),
                );
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    Body::from_stream(body),
                )
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

async fn upload_from_url_uses_egress_rules(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let port = file_server().await;

    // The production rules: plain http, loopback and credentials are refused, and
    // nothing is recorded.
    let refused = [
        (format!("http://127.0.0.1:{port}/pic.png"), "only https"),
        (
            format!("https://127.0.0.1:{port}/pic.png"),
            "could not be fetched",
        ),
        (
            format!("https://localhost:{port}/pic.png"),
            "could not be fetched",
        ),
        (
            "https://user:pw@example.org/a.png".to_owned(),
            "credentials",
        ),
        ("ftp://example.org/a.png".to_owned(), "only https"),
        ("not a url".to_owned(), "absolute URL"),
    ];
    for (url, why) in refused {
        let r = tool(&api, &t.0, "media_upload", json!({"url": url})).await;
        let p = problem(&r);
        assert_eq!(p["status"], 422, "{url}: {r}");
        assert_eq!(p["errors"][0]["field"], "url", "{url}: {r}");
        assert!(text(&r).contains(why), "{url}: {}", text(&r));
    }
    assert_eq!(media_count(&api, &t).await, 0);

    // With the test seam (loopback and http allowed) the same server works.
    let (router, _) = api.router_with_uploader(
        Uploader::new(api.api.media.clone(), MAX_UPLOAD).with_private_fetch(),
    );
    let r = tool_on(
        &router,
        &t.0,
        "media_upload",
        json!({"url": format!("http://127.0.0.1:{port}/pic.png"), "alt": "From the web"}),
    )
    .await;
    let m = ok(&r);
    assert_eq!(m["mime"], "image/png", "{m}");
    assert_eq!(m["bytes"], 700, "{m}");
    assert_eq!(m["original_name"], "pic.png", "{m}");
    assert_eq!(m["title"], "pic", "{m}");
    assert_eq!(m["alt"], "From the web", "{m}");

    // An explicit file name wins over the URL's.
    let r = tool_on(
        &router,
        &t.0,
        "media_upload",
        json!({"url": format!("http://127.0.0.1:{port}/pic.png"), "filename": "renamed.png"}),
    )
    .await;
    assert_eq!(ok(&r)["original_name"], "renamed.png", "{r}");

    // Over the size cap: refused early by Content-Length, and while streaming without it.
    for path in ["big.png", "stream.png"] {
        let r = tool_on(
            &router,
            &t.0,
            "media_upload",
            json!({"url": format!("http://127.0.0.1:{port}/{path}")}),
        )
        .await;
        assert_eq!(problem(&r)["status"], 413, "{path}: {r}");
        assert_eq!(problem(&r)["errors"][0]["field"], "url", "{path}: {r}");
    }
    // A 404 is a fetch failure.
    let r = tool_on(
        &router,
        &t.0,
        "media_upload",
        json!({"url": format!("http://127.0.0.1:{port}/gone")}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    assert_eq!(problem(&r)["errors"][0]["field"], "url", "{r}");

    assert_eq!(media_count(&api, &t).await, 2);
    // Only successes are audited, and a URL upload names the host, never the URL.
    let rows = mcp_rows(&api).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].details["source_host"], "127.0.0.1", "{rows:?}");
    api
}

async fn url_fetches_are_limited_and_hold_no_upload_slot(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let app = Router::new().route(
        "/slow.png",
        get(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            png("slow", 300)
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (router, uploader) = api.router_with_uploader(
        Uploader::new(api.api.media.clone(), MAX_UPLOAD).with_private_fetch(),
    );
    let url = format!("http://127.0.0.1:{port}/slow.png");
    let slow = |name: &'static str| {
        let (router, token, url) = (router.clone(), t.0.clone(), url.clone());
        tokio::spawn(async move {
            tool_on(
                &router,
                &token,
                "media_upload",
                json!({"url": url, "filename": name}),
            )
            .await
        })
    };
    let (a, b) = (slow("a.png"), slow("b.png"));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // Two downloads are under way and hold no upload slot.
    assert_eq!(uploader.in_flight(), 0);
    // A third finds no fetch slot and gives up after the short wait: 503, nothing stored.
    let r = tool_on(&router, &t.0, "media_upload", json!({"url": url})).await;
    assert_eq!(problem(&r)["status"], 503, "{r}");
    assert_eq!(uploader.in_flight(), 0);
    // The two that held the fetch slots finish and are recorded.
    for h in [a, b] {
        let r = h.await.unwrap();
        assert_eq!(ok(&r)["bytes"], 300, "{r}");
    }
    assert_eq!(media_count(&api, &t).await, 2);
    // Only the fetch slots were limited: a download can start again.
    let r = tool_on(
        &router,
        &t.0,
        "media_upload",
        json!({"url": format!("http://127.0.0.1:{port}/gone")}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    api
}

async fn oversize_upload_refused(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    // `media.max_upload` (1 MiB in the harness) applies under the tool's own 6 MiB.
    let big = png("big", MAX_UPLOAD as usize + 1);
    let r = tool(
        &api,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&big), "filename": "big.png"}),
    )
    .await;
    let p = problem(&r);
    assert_eq!(p["status"], 413, "{r}");
    assert_eq!(p["errors"][0]["field"], "data_base64", "{r}");
    assert_eq!(media_count(&api, &t).await, 0);
    assert!(api.stored_files().is_empty(), "{:?}", api.stored_files());

    // Exactly the limit is fine.
    let r = tool(
        &api,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&png("ok", MAX_UPLOAD as usize)), "filename": "ok.png"}),
    )
    .await;
    ok(&r);

    // With a larger `media.max_upload` the tool's own 6 MiB cap is the limit: a file of
    // exactly 6 MiB (8 MiB as base64, inside the endpoint's 9 MiB body) is taken end to
    // end, one byte more is refused by the tool.
    let (router, _) = api.router_with_max_upload(16 << 20);
    let big = png("big", 6 << 20);
    let r = tool_on(
        &router,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&big), "filename": "big.png"}),
    )
    .await;
    assert_eq!(ok(&r)["bytes"], big.len(), "{r}");
    let over = png("over", (6 << 20) + 1);
    let r = tool_on(
        &router,
        &t.0,
        "media_upload",
        json!({"data_base64": b64(&over), "filename": "over.png"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 413, "{r}");
    assert_eq!(problem(&r)["errors"][0]["field"], "data_base64", "{r}");
    api
}

async fn upload_takes_one_source_and_authorizes_first(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let data = b64(&png("a", 100));
    for (args, field) in [
        (json!({}), None),
        (
            json!({"data_base64": data, "url": "https://example.org/a.png", "filename": "a.png"}),
            None,
        ),
        (json!({"data_base64": data}), Some("filename")),
        (
            json!({"data_base64": "###", "filename": "a.png"}),
            Some("data_base64"),
        ),
        (
            json!({"data_base64": data, "filename": "../"}),
            Some("filename"),
        ),
        (
            json!({"url": "https://example.org/a.png", "filename": "../"}),
            Some("filename"),
        ),
        (
            json!({"data_base64": data, "filename": "a.png", "title": "x".repeat(201)}),
            Some("title"),
        ),
        (
            json!({"data_base64": data, "filename": "a.png", "bogus": 1}),
            None,
        ),
    ] {
        let r = tool(&api, &t.0, "media_upload", args.clone()).await;
        let p = problem(&r);
        if let Some(field) = field {
            assert_eq!(p["errors"][0]["field"], field, "{args}: {r}");
        }
    }
    // (A refused title is only seen once the file is stored; the blob waits for GC.)
    assert_eq!(media_count(&api, &t).await, 0);

    // A contributor may not upload: refused before the data is even decoded (the invalid
    // base64 would be a 422 otherwise) or the URL fetched.
    let u = api.user("connie", Role::Contributor, false).await;
    let c = api.bearer(&u, &["*"]).await;
    for args in [
        json!({"data_base64": "###", "filename": "a.png"}),
        json!({"url": "https://example.org/a.png"}),
    ] {
        let r = tool(&api, &c.0, "media_upload", args).await;
        assert_eq!(problem(&r)["status"], 403, "{r}");
        assert!(text(&r).contains("upload_media"), "{}", text(&r));
    }
    api
}

async fn alt_text_update(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let m = upload(&api, &t, "dog", "Dog", "").await;
    let id = m["id"].as_str().unwrap();
    // Recorded as a draft and published at once: version 2.
    let v = m["version"].as_i64().unwrap();

    let r = tool(
        &api,
        &t.0,
        "media_update",
        json!({"id": id, "version": v, "alt": "A dog on a beach", "title": "Beach dog"}),
    )
    .await;
    let updated = ok(&r);
    assert_eq!(updated["alt"], "A dog on a beach", "{updated}");
    assert_eq!(updated["title"], "Beach dog", "{updated}");
    assert_eq!(updated["version"], v + 1, "{updated}");
    assert_eq!(
        updated["markdown"],
        format!("![A dog on a beach](ferrite:media/{id})"),
        "{updated}"
    );

    // Only the members sent change.
    let r = tool(
        &api,
        &t.0,
        "media_update",
        json!({"id": id, "version": v + 1, "alt": ""}),
    )
    .await;
    assert_eq!(ok(&r)["title"], "Beach dog", "{r}");
    assert_eq!(ok(&r)["alt"], "", "{r}");

    // A stale version writes nothing and returns the item as stored now.
    let r = tool(
        &api,
        &t.0,
        "media_update",
        json!({"id": id, "version": v, "alt": "Stale"}),
    )
    .await;
    let p = problem(&r);
    assert_eq!(p["status"], 412, "{r}");
    assert_eq!(p["current"]["version"], v + 2, "{r}");
    assert_eq!(p["current"]["title"], "Beach dog", "{r}");
    assert!(text(&r).contains("current"), "{}", text(&r));
    let got = tool(&api, &t.0, "media_get", json!({"id": id})).await;
    assert_eq!(ok(&got)["alt"], "", "{got}");

    // Nothing to change, and a title over the limit.
    let r = tool(
        &api,
        &t.0,
        "media_update",
        json!({"id": id, "version": v + 2}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    let r = tool(
        &api,
        &t.0,
        "media_update",
        json!({"id": id, "version": v + 2, "title": "x".repeat(201)}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "title", "{r}");
    api
}

async fn media_trash_restore_and_list(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let a = upload(&api, &t, "one", "One", "a").await;
    let b = upload(&api, &t, "two", "Two", "b").await;
    let id = a["id"].as_str().unwrap();

    let v = a["version"].as_i64().unwrap();
    let r = tool(&api, &t.0, "media_trash", json!({"id": id, "version": v})).await;
    let trashed = ok(&r);
    assert_eq!(trashed["status"], "trash", "{trashed}");
    // The whole item, as media_restore answers.
    assert_eq!(
        trashed["reference"],
        format!("ferrite:media/{id}"),
        "{trashed}"
    );
    assert!(trashed["mime"].is_string(), "{trashed}");
    assert!(trashed.get("note").is_none(), "{trashed}");
    let version = trashed["version"].as_i64().unwrap();

    // Gone from the default list, found with `trash`.
    let list = tool(&api, &t.0, "media_list", json!({})).await;
    let items = ok(&list)["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["id"], b["id"], "{list}");
    let list = tool(&api, &t.0, "media_list", json!({"trash": true})).await;
    assert_eq!(ok(&list)["items"][0]["id"], id, "{list}");
    // A search cannot be combined with the trash.
    let list = tool(&api, &t.0, "media_list", json!({"trash": true, "q": "one"})).await;
    assert_eq!(problem(&list)["errors"][0]["field"], "q", "{list}");

    // A stale restore carries the current item; the right version restores.
    let r = tool(&api, &t.0, "media_restore", json!({"id": id, "version": v})).await;
    assert_eq!(problem(&r)["status"], 412, "{r}");
    assert_eq!(problem(&r)["current"]["status"], "trash", "{r}");
    let r = tool(
        &api,
        &t.0,
        "media_restore",
        json!({"id": id, "version": version}),
    )
    .await;
    assert_eq!(ok(&r)["status"], "published", "{r}");
    assert_eq!(media_count(&api, &t).await, 2);

    // Paging.
    let first = tool(&api, &t.0, "media_list", json!({"per_page": 1})).await;
    let cursor = ok(&first)["next_cursor"].as_str().unwrap().to_owned();
    let second = tool(
        &api,
        &t.0,
        "media_list",
        json!({"per_page": 1, "cursor": cursor}),
    )
    .await;
    assert_ne!(ok(&first)["items"][0]["id"], ok(&second)["items"][0]["id"]);
    let bad = tool(&api, &t.0, "media_list", json!({"cursor": "garbage"})).await;
    assert_eq!(problem(&bad)["status"], 400, "{bad}");
    let bad = tool(&api, &t.0, "media_list", json!({"per_page": 101})).await;
    assert_eq!(problem(&bad)["errors"][0]["field"], "per_page", "{bad}");
    api
}

// ---- taxonomies ------------------------------------------------------------------------

async fn term_crud(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let taxes = tool(&api, &t.0, "taxonomies_list", json!({})).await;
    let names: Vec<&str> = ok(&taxes)["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"category") && names.contains(&"tag"),
        "{names:?}"
    );

    let r = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "category", "name": "News", "description": "Latest"}),
    )
    .await;
    let news = ok(&r).clone();
    assert_eq!(news["slug"], "news", "{news}");
    assert_eq!(news["description"], "Latest", "{news}");
    assert_eq!(news["parent_id"], Value::Null, "{news}");
    let news_id = news["id"].as_str().unwrap().to_owned();

    let r = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "category", "name": "Local", "parent": news_id}),
    )
    .await;
    let local = ok(&r).clone();
    assert_eq!(local["parent_id"], news_id, "{local}");
    let local_id = local["id"].as_str().unwrap().to_owned();

    // A taken slug gets a suffix; a flat taxonomy takes no parent; an unknown one is 404.
    let r = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "category", "name": "News"}),
    )
    .await;
    assert_eq!(ok(&r)["slug"], "news-2", "{r}");
    let r = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "tag", "name": "Rust", "parent": news_id}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    let r = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "nope", "name": "X"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    let r = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "tag", "name": "  "}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "{r}");

    // Listing: filters and pages with the total.
    let all = tool(&api, &t.0, "terms_list", json!({"taxonomy": "category"})).await;
    let all = ok(&all);
    assert_eq!(all["total"], 3, "{all}"); // News, Local, News 2
    let found = tool(
        &api,
        &t.0,
        "terms_list",
        json!({"taxonomy": "category", "q": "LOC"}),
    )
    .await;
    assert_eq!(ok(&found)["items"][0]["id"], local_id, "{found}");
    assert_eq!(ok(&found)["total"], 1, "{found}");
    let kids = tool(
        &api,
        &t.0,
        "terms_list",
        json!({"taxonomy": "category", "parent": news_id}),
    )
    .await;
    assert_eq!(ok(&kids)["total"], 1, "{kids}");
    let roots = tool(
        &api,
        &t.0,
        "terms_list",
        json!({"taxonomy": "category", "parent": "root"}),
    )
    .await;
    assert_eq!(ok(&roots)["total"], 2, "{roots}");
    let page2 = tool(
        &api,
        &t.0,
        "terms_list",
        json!({"taxonomy": "category", "per_page": 2, "page": 2}),
    )
    .await;
    assert_eq!(ok(&page2)["items"].as_array().unwrap().len(), 1, "{page2}");
    assert_eq!(ok(&page2)["total"], 3, "{page2}");
    let bad = tool(
        &api,
        &t.0,
        "terms_list",
        json!({"taxonomy": "category", "parent": "x"}),
    )
    .await;
    assert_eq!(problem(&bad)["errors"][0]["field"], "parent", "{bad}");

    // Update merges: a rename keeps the parent, `null` moves to the top level.
    let r = tool(
        &api,
        &t.0,
        "term_update",
        json!({"id": local_id, "name": "Local news"}),
    )
    .await;
    assert_eq!(ok(&r)["name"], "Local news", "{r}");
    assert_eq!(ok(&r)["parent_id"], news_id, "{r}");
    assert_eq!(ok(&r)["slug"], "local", "{r}");
    let r = tool(
        &api,
        &t.0,
        "term_update",
        json!({"id": local_id, "parent": null}),
    )
    .await;
    assert_eq!(ok(&r)["parent_id"], Value::Null, "{r}");
    let r = tool(
        &api,
        &t.0,
        "term_update",
        json!({"id": local_id, "parent": news_id}),
    )
    .await;
    assert_eq!(ok(&r)["parent_id"], news_id, "{r}");
    let r = tool(&api, &t.0, "term_update", json!({"id": local_id})).await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    let r = tool(
        &api,
        &t.0,
        "term_update",
        json!({"id": news_id, "parent": local_id}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "a cycle: {r}");

    // A term with children cannot be deleted; children first.
    let r = tool(&api, &t.0, "term_delete", json!({"id": news_id})).await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    let r = tool(&api, &t.0, "term_delete", json!({"id": local_id})).await;
    assert_eq!(ok(&r)["deleted"], true, "{r}");
    let r = tool(&api, &t.0, "term_delete", json!({"id": news_id})).await;
    ok(&r);
    let r = tool(&api, &t.0, "term_delete", json!({"id": news_id})).await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    api
}

// ---- menus -----------------------------------------------------------------------------

async fn menu_set_replaces_tree(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let post = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "About us"}),
    )
    .await;
    let post_id = ok(&post)["id"].as_str().unwrap().to_owned();

    let set = |items: Value| json!({"name": "primary", "label": "Main menu", "items": items});
    let r = tool(
        &api,
        &t.0,
        "menu_set",
        set(json!([
            {"kind": "url", "label": "Home", "url": "/"},
            {"kind": "item", "label": "About", "target_id": post_id, "children": [
                {"kind": "url", "label": "Team", "url": "/team/"},
                {"kind": "url", "label": "Press", "url": "https://example.org/press"}
            ]},
            {"kind": "url", "label": "Contact", "url": "/contact/"}
        ])),
    )
    .await;
    let menu = ok(&r);
    assert_eq!(menu["name"], "primary", "{menu}");
    assert_eq!(menu["label"], "Main menu", "{menu}");
    let top = menu["items"].as_array().unwrap();
    let labels: Vec<&str> = top.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["Home", "About", "Contact"], "{menu}");
    let kids: Vec<&str> = top[1]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert_eq!(kids, ["Team", "Press"], "{menu}");
    assert_eq!(top[1]["kind"], "item", "{menu}");
    assert_eq!(top[1]["target_id"], post_id, "{menu}");
    let about_id = top[1]["id"].as_str().unwrap().to_owned();

    // menu_get returns the same tree; REST sees it as a flat list.
    let got = tool(&api, &t.0, "menu_get", json!({"name": "primary"})).await;
    assert_eq!(ok(&got)["items"], menu["items"], "{got}");
    let rest = t.get(&api, "/api/v1/menus/primary").await;
    let flat = rest.json()["items"].as_array().unwrap().len();
    assert_eq!(flat, 5, "{rest:?}");

    // A second call replaces the whole tree; an entry keeps its id when it is given,
    // and the label stays when it is left out.
    let r = tool(
        &api,
        &t.0,
        "menu_set",
        json!({"name": "primary", "items": [
            {"id": about_id, "kind": "item", "label": "About us", "target_id": post_id},
            {"kind": "url", "label": "Shop", "url": "/shop/"}
        ]}),
    )
    .await;
    let menu = ok(&r);
    assert_eq!(menu["label"], "Main menu", "{menu}");
    let top = menu["items"].as_array().unwrap();
    assert_eq!(top.len(), 2, "{menu}");
    assert_eq!(top[0]["id"], about_id, "{menu}");
    assert_eq!(top[0]["label"], "About us", "{menu}");
    assert!(top[0]["children"].as_array().unwrap().is_empty(), "{menu}");
    let rest = t.get(&api, "/api/v1/menus/primary").await;
    assert_eq!(
        rest.json()["items"].as_array().unwrap().len(),
        2,
        "{rest:?}"
    );

    // Core's validation applies and a refused tree changes nothing.
    for bad in [
        json!([{"kind": "url", "label": "XSS", "url": "javascript:alert(1)"}]),
        json!([{"kind": "item", "label": "Nowhere"}]),
        json!([{"kind": "url", "label": "x".repeat(201), "url": "/x/"}]),
    ] {
        let r = tool(&api, &t.0, "menu_set", set(bad.clone())).await;
        assert_eq!(problem(&r)["status"], 422, "{bad}: {r}");
    }
    let r = tool(
        &api,
        &t.0,
        "menu_set",
        json!({"name": "primary", "items": [{"kind": "url", "bogus": 1}]}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    let got = tool(&api, &t.0, "menu_get", json!({"name": "primary"})).await;
    assert_eq!(ok(&got)["items"].as_array().unwrap().len(), 2, "{got}");

    // An empty list empties the menu; the listing names it with the theme's locations.
    let r = tool(
        &api,
        &t.0,
        "menu_set",
        json!({"name": "footer", "items": []}),
    )
    .await;
    assert!(ok(&r)["items"].as_array().unwrap().is_empty(), "{r}");
    let list = tool(&api, &t.0, "menus_list", json!({})).await;
    let list = ok(&list);
    let menus: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(
        menus.contains(&"primary") && menus.contains(&"footer"),
        "{list}"
    );
    assert!(list["locations"].is_array(), "{list}");
    let missing = tool(&api, &t.0, "menu_get", json!({"name": "nope"})).await;
    assert_eq!(problem(&missing)["status"], 404, "{missing}");
    api
}

// ---- content types ---------------------------------------------------------------------

async fn type_create_with_fields(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let r = tool(
        &api,
        &t.0,
        "content_type_create",
        json!({"name": "case_study", "label": "Case study", "has_archive": true,
        "fields": [
            {"name": "client", "label": "Client", "kind": "text", "required": true},
            {"name": "rating", "kind": "select", "options": ["good", "great"]},
            {"name": "results", "kind": "repeater", "fields": [
                {"name": "metric", "kind": "text"}, {"name": "value", "kind": "number"}]}
        ]}),
    )
    .await;
    let ty = ok(&r);
    assert_eq!(ty["name"], "case_study", "{ty}");
    assert_eq!(ty["has_archive"], true, "{ty}");
    assert_eq!(ty["hierarchical"], false, "{ty}");
    assert_eq!(ty["built_in"], false, "{ty}");
    assert_eq!(ty["fields"].as_array().unwrap().len(), 3, "{ty}");
    assert_eq!(ty["fields"][2]["fields"][1]["kind"], "number", "{ty}");

    // Listed and readable, and the other content tools use its schema.
    let list = tool(&api, &t.0, "content_types_list", json!({})).await;
    let names: Vec<&str> = ok(&list)["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"case_study") && names.contains(&"post"),
        "{names:?}"
    );
    let got = tool(
        &api,
        &t.0,
        "content_type_get",
        json!({"name": "case_study"}),
    )
    .await;
    assert_eq!(ok(&got)["fields"], ty["fields"], "{got}");
    let item = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "case_study", "title": "ACME", "fields": {"client": "ACME Corp"}}),
    )
    .await;
    assert_eq!(ok(&item)["fields"]["client"], "ACME Corp", "{item}");
    let item = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "case_study", "title": "No client"}),
    )
    .await;
    assert_eq!(problem(&item)["status"], 422, "{item}");

    // Update: the field list is replaced as a whole; name and `hierarchical` stay.
    let r = tool(
        &api,
        &t.0,
        "content_type_update",
        json!({"name": "case_study", "label": "Case studies", "has_archive": false,
               "fields": [{"name": "client", "kind": "text"}]}),
    )
    .await;
    let ty = ok(&r);
    assert_eq!(ty["label"], "Case studies", "{ty}");
    assert_eq!(ty["has_archive"], false, "{ty}");
    assert_eq!(ty["fields"].as_array().unwrap().len(), 1, "{ty}");
    let r = tool(
        &api,
        &t.0,
        "content_type_update",
        json!({"name": "case_study", "label": "Cases"}),
    )
    .await;
    assert_eq!(ok(&r)["fields"].as_array().unwrap().len(), 1, "{r}");
    let r = tool(
        &api,
        &t.0,
        "content_type_update",
        json!({"name": "case_study", "fields": null}),
    )
    .await;
    assert_eq!(ok(&r)["fields"], Value::Null, "{r}");
    let r = tool(
        &api,
        &t.0,
        "content_type_update",
        json!({"name": "case_study"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    let r = tool(
        &api,
        &t.0,
        "content_type_update",
        json!({"name": "case_study", "hierarchical": true}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");

    // Refused: bad names, duplicates, bad schemas; deleting is not offered.
    for args in [
        json!({"name": "Bad Name", "label": "x"}),
        json!({"name": "admin", "label": "x"}),
        json!({"name": "case_study", "label": "again"}),
        json!({"name": "thing", "label": "x", "fields": [{"name": "A-b", "kind": "text"}]}),
        json!({"name": "thing", "label": "x", "fields": [{"name": "a", "kind": "select"}]}),
    ] {
        let r = tool(&api, &t.0, "content_type_create", args.clone()).await;
        assert!(
            matches!(problem(&r)["status"].as_u64(), Some(409 | 422)),
            "{args}: {r}"
        );
    }
    let (status, body) = mcp_call(&api, &t.0, "tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.contains("delete") && n.contains("type")),
        "{names:?}"
    );

    // An editor may read types but not change them.
    let u = api.user("eddie", Role::Editor, false).await;
    let e = api.bearer(&u, &["*"]).await;
    ok(&tool(&api, &e.0, "content_types_list", json!({})).await);
    let r = tool(
        &api,
        &e.0,
        "content_type_create",
        json!({"name": "thing", "label": "Thing"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    api
}

// ---- redirects -------------------------------------------------------------------------

async fn redirect_crud(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let r = tool(
        &api,
        &t.0,
        "redirect_create",
        json!({"source": "/old/", "target": "/new/"}),
    )
    .await;
    let red = ok(&r).clone();
    assert_eq!(red["source"], "/old/", "{red}");
    assert_eq!(red["target"], "/new/", "{red}");
    assert_eq!(red["code"], 301, "{red}");
    assert_eq!(red["origin"], "manual", "{red}");
    let id = red["id"].as_str().unwrap().to_owned();

    // The same source again replaces the target and code.
    let r = tool(
        &api,
        &t.0,
        "redirect_create",
        json!({"source": "/old/", "target": "https://example.org/x", "code": 302}),
    )
    .await;
    assert_eq!(ok(&r)["id"], id, "{r}");
    assert_eq!(ok(&r)["code"], 302, "{r}");
    let r = tool(
        &api,
        &t.0,
        "redirect_create",
        json!({"source": "/gone/", "target": "/", "code": 410}),
    )
    .await;
    ok(&r);

    let list = tool(&api, &t.0, "redirects_list", json!({})).await;
    assert_eq!(ok(&list)["items"].as_array().unwrap().len(), 2, "{list}");
    let found = tool(&api, &t.0, "redirects_list", json!({"q": "EXAMPLE"})).await;
    assert_eq!(ok(&found)["items"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(ok(&found)["items"][0]["id"], id, "{found}");
    let first = tool(&api, &t.0, "redirects_list", json!({"per_page": 1})).await;
    let cursor = ok(&first)["next_cursor"].as_str().unwrap().to_owned();
    let second = tool(
        &api,
        &t.0,
        "redirects_list",
        json!({"per_page": 1, "cursor": cursor}),
    )
    .await;
    assert_eq!(
        ok(&second)["items"].as_array().unwrap().len(),
        1,
        "{second}"
    );
    assert_eq!(ok(&second)["next_cursor"], Value::Null, "{second}");
    let bad = tool(&api, &t.0, "redirects_list", json!({"cursor": "garbage"})).await;
    assert_eq!(problem(&bad)["status"], 400, "{bad}");

    // Update merges.
    let r = tool(
        &api,
        &t.0,
        "redirect_update",
        json!({"id": id, "target": "/newer/", "code": 308}),
    )
    .await;
    assert_eq!(ok(&r)["target"], "/newer/", "{r}");
    assert_eq!(ok(&r)["code"], 308, "{r}");
    assert_eq!(ok(&r)["source"], "/old/", "{r}");
    let r = tool(
        &api,
        &t.0,
        "redirect_update",
        json!({"id": id, "source": "/gone/"}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "source", "{r}");
    let r = tool(&api, &t.0, "redirect_update", json!({"id": id})).await;
    assert_eq!(problem(&r)["status"], 422, "{r}");
    // Bad codes, bad sources and loops are refused by core.
    for args in [
        json!({"source": "/a/", "target": "/b/", "code": 200}),
        json!({"source": "nope", "target": "/b/"}),
        json!({"source": "/newer/", "target": "/old/"}),
    ] {
        let r = tool(&api, &t.0, "redirect_create", args.clone()).await;
        assert_eq!(problem(&r)["status"], 422, "{args}: {r}");
    }

    // Delete.
    let r = tool(&api, &t.0, "redirect_delete", json!({"id": id})).await;
    assert_eq!(ok(&r)["deleted"], true, "{r}");
    let r = tool(&api, &t.0, "redirect_delete", json!({"id": id})).await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    let list = tool(&api, &t.0, "redirects_list", json!({})).await;
    assert_eq!(ok(&list)["items"].as_array().unwrap().len(), 1, "{list}");

    // An editor cannot even read redirects.
    let u = api.user("eddie", Role::Editor, false).await;
    let e = api.bearer(&u, &["*"]).await;
    let r = tool(&api, &e.0, "redirects_list", json!({})).await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    api
}

// ---- scopes, annotations and the audit trail -------------------------------------------

/// Scopes, a call they allow, and calls they refuse with the scope each one names.
type Case = (
    &'static [&'static str],
    (&'static str, Value),
    Vec<(&'static str, Value, &'static str)>,
);

async fn scopes_limit_each_area(api: Api) -> Api {
    let (u, full) = admin(&api).await;
    let limited = |scopes: &'static [&'static str]| {
        let api = &api;
        let u = &u;
        async move { api.bearer(u, scopes).await }
    };
    let data = json!({"data_base64": b64(&png("s", 100)), "filename": "s.png"});
    let id = "00000000-0000-4000-8000-000000000000";
    // Terms and redirects are read before they are written, so these need to exist.
    let term = tool(
        &api,
        &full.0,
        "term_create",
        json!({"taxonomy": "tag", "name": "T"}),
    )
    .await;
    let term_id = ok(&term)["id"].as_str().unwrap().to_owned();
    let red = tool(
        &api,
        &full.0,
        "redirect_create",
        json!({"source": "/o/", "target": "/n/"}),
    )
    .await;
    let red_id = ok(&red)["id"].as_str().unwrap().to_owned();

    // (scopes, a call that works, calls refused naming the missing scope)
    let cases: Vec<Case> = vec![
        (
            &["media:read"],
            ("media_list", json!({})),
            vec![
                ("media_upload", data.clone(), "media:write"),
                (
                    "media_update",
                    json!({"id": id, "version": 1, "alt": "x"}),
                    "media:write",
                ),
                (
                    "media_trash",
                    json!({"id": id, "version": 1}),
                    "media:write",
                ),
                (
                    "media_restore",
                    json!({"id": id, "version": 1}),
                    "media:write",
                ),
                ("terms_list", json!({"taxonomy": "tag"}), "terms:read"),
            ],
        ),
        (
            &["terms:read", "content:read"],
            ("terms_list", json!({"taxonomy": "tag"})),
            vec![
                (
                    "term_create",
                    json!({"taxonomy": "tag", "name": "x"}),
                    "terms:write",
                ),
                (
                    "term_update",
                    json!({"id": term_id, "name": "x"}),
                    "terms:write",
                ),
                ("term_delete", json!({"id": term_id}), "terms:write"),
                ("media_list", json!({}), "media:read"),
                ("menus_list", json!({}), "menus:read"),
            ],
        ),
        (
            &["menus:read"],
            ("menu_get", json!({"name": "primary-x"})), // a missing menu is 404, not 403
            vec![
                (
                    "menu_set",
                    json!({"name": "primary", "items": []}),
                    "menus:write",
                ),
                ("redirects_list", json!({}), "redirects:read"),
            ],
        ),
        (
            &["redirects:read"],
            ("redirects_list", json!({})),
            vec![
                (
                    "redirect_create",
                    json!({"source": "/a/", "target": "/b/"}),
                    "redirects:write",
                ),
                (
                    "redirect_update",
                    json!({"id": red_id, "code": 302}),
                    "redirects:write",
                ),
                ("redirect_delete", json!({"id": red_id}), "redirects:write"),
            ],
        ),
        (
            &["content:read", "settings:read"],
            ("content_types_list", json!({})),
            vec![
                (
                    "content_type_create",
                    json!({"name": "thing", "label": "T"}),
                    "settings:write",
                ),
                (
                    "content_type_update",
                    json!({"name": "post", "label": "T"}),
                    "settings:write",
                ),
                ("redirects_list", json!({}), "redirects:read"),
            ],
        ),
        (
            &["media:write"],
            ("media_upload", data.clone()),
            vec![("content_types_list", json!({}), "content:read")],
        ),
    ];
    for (scopes, (works, args), refused) in cases {
        let t = limited(scopes).await;
        let r = tool(&api, &t.0, works, args).await;
        let allowed = r["isError"] == false || problem(&r)["status"] == 404;
        assert!(allowed, "{scopes:?} {works}: {r}");
        for (name, args, scope) in refused {
            let r = tool(&api, &t.0, name, args).await;
            assert_eq!(problem(&r)["status"], 403, "{scopes:?} {name}: {r}");
            assert!(text(&r).contains(scope), "{scopes:?} {name}: {}", text(&r));
        }
    }
    // The refused writes left nothing behind: only the one allowed upload exists.
    assert_eq!(media_count(&api, &full).await, 1);
    api
}

async fn structure_tools_are_annotated(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let (status, body) = mcp_call(&api, &t.0, "tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tools = body["result"]["tools"].as_array().unwrap();
    let by = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing"))
    };
    let reads = [
        "media_list",
        "media_get",
        "taxonomies_list",
        "terms_list",
        "menus_list",
        "menu_get",
        "content_types_list",
        "content_type_get",
        "redirects_list",
    ];
    let safe_writes = [
        "media_update",
        "media_restore",
        "term_create",
        "term_update",
        "content_type_create",
        "redirect_update",
    ];
    // `redirect_create` replaces an existing redirect from the same source.
    let destructive = [
        "media_trash",
        "term_delete",
        "menu_set",
        "content_type_update",
        "redirect_create",
        "redirect_delete",
    ];
    for name in reads {
        let a = &by(name)["annotations"];
        assert_eq!(a["readOnlyHint"], true, "{name}");
        assert_eq!(a["openWorldHint"], false, "{name}");
    }
    for name in safe_writes {
        let a = &by(name)["annotations"];
        assert_eq!(a["readOnlyHint"], false, "{name}");
        assert_eq!(a["destructiveHint"], false, "{name}");
        assert_eq!(a["openWorldHint"], false, "{name}");
    }
    for name in destructive {
        let a = &by(name)["annotations"];
        assert_eq!(a["readOnlyHint"], false, "{name}");
        assert_eq!(a["destructiveHint"], true, "{name}");
        assert_eq!(a["openWorldHint"], false, "{name}");
    }
    // Only the upload reaches out to the open web.
    let a = &by("media_upload")["annotations"];
    assert_eq!(a["readOnlyHint"], false);
    assert_eq!(a["destructiveHint"], false);
    assert_eq!(a["openWorldHint"], true);
    let open: Vec<&str> = tools
        .iter()
        .filter(|t| t["annotations"]["openWorldHint"] == true)
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(open, ["media_upload"]);

    // Descriptions are written for a model, and the schemas refuse unknown members.
    for name in reads
        .iter()
        .chain(&safe_writes)
        .chain(&destructive)
        .chain(&["media_upload"])
    {
        let tool = by(name);
        assert!(tool["description"].as_str().unwrap().len() > 60, "{name}");
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        if schema["properties"]
            .as_object()
            .is_some_and(|p| !p.is_empty())
        {
            assert_eq!(schema["additionalProperties"], false, "{name}: {schema}");
        }
    }
    let upload = &by("media_upload")["inputSchema"]["properties"];
    for field in ["data_base64", "url", "filename", "title", "alt"] {
        assert!(upload[field]["description"].is_string(), "{field}");
    }
    // The menu schema is a recursive tree and the type schema carries the field kinds.
    let menu = serde_json::to_string(&by("menu_set")["inputSchema"]).unwrap();
    assert!(menu.contains("children"), "{menu}");
    let ty = serde_json::to_string(&by("content_type_create")["inputSchema"]).unwrap();
    assert!(ty.contains("repeater") && ty.contains("rich_text"), "{ty}");
    api
}

async fn structure_writes_are_audited(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let m = upload(&api, &t, "a", "A", "").await;
    let id = m["id"].as_str().unwrap();
    let v = m["version"].as_i64().unwrap();
    let term = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "tag", "name": "T"}),
    )
    .await;
    let term_id = ok(&term)["id"].as_str().unwrap().to_owned();
    let red = tool(
        &api,
        &t.0,
        "redirect_create",
        json!({"source": "/a/", "target": "/b/"}),
    )
    .await;
    let red_id = ok(&red)["id"].as_str().unwrap().to_owned();
    for (name, args) in [
        ("media_update", json!({"id": id, "version": v, "alt": "x"})),
        ("media_trash", json!({"id": id, "version": v + 1})),
        ("media_restore", json!({"id": id, "version": v + 2})),
        ("term_update", json!({"id": term_id, "name": "U"})),
        ("redirect_update", json!({"id": red_id, "code": 302})),
        ("menu_set", json!({"name": "m", "items": []})),
        (
            "content_type_create",
            json!({"name": "thing", "label": "Thing"}),
        ),
        (
            "content_type_update",
            json!({"name": "thing", "label": "Things"}),
        ),
        ("redirect_delete", json!({"id": red_id})),
        ("term_delete", json!({"id": term_id})),
    ] {
        ok(&tool(&api, &t.0, name, args).await);
    }
    // Reads and refused writes are not audited.
    tool(&api, &t.0, "media_list", json!({})).await;
    tool(&api, &t.0, "menus_list", json!({})).await;
    let stale = tool(
        &api,
        &t.0,
        "media_update",
        json!({"id": id, "version": v, "alt": "y"}),
    )
    .await;
    problem(&stale);
    let refused = tool(
        &api,
        &t.0,
        "term_create",
        json!({"taxonomy": "nope", "name": "x"}),
    )
    .await;
    problem(&refused);

    let rows = mcp_rows(&api).await;
    let mut actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
    actions.sort_unstable();
    assert_eq!(
        actions,
        [
            "mcp.content_type_create",
            "mcp.content_type_update",
            "mcp.media_restore",
            "mcp.media_trash",
            "mcp.media_update",
            "mcp.media_upload",
            "mcp.menu_set",
            "mcp.redirect_create",
            "mcp.redirect_delete",
            "mcp.redirect_update",
            "mcp.term_create",
            "mcp.term_delete",
            "mcp.term_update",
        ],
        "{rows:?}"
    );
    for r in &rows {
        assert_eq!(r.details["client"], "API token test", "{r:?}");
    }
    let upload_row = rows
        .iter()
        .find(|r| r.action == "mcp.media_upload")
        .unwrap();
    assert_eq!(upload_row.target_id.as_deref(), Some(id), "{upload_row:?}");
    api
}
