//! Comments through the API (M2a-2b Task 7): the public submit (`SameOrigin` unless a
//! bearer token, an optional session with CSRF or a token), the moderation queue,
//! `PATCH` and `DELETE`, and what each response shows (Review Focus 3, F18, F22).
//!
//! Every submit comes from its own client address ([`fresh`], [`peer`]), so the per-IP
//! limit (3 a minute) never interferes; the limit itself is tested in core and across
//! nodes.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::atomic::{AtomicU32, Ordering},
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, header},
};
use ferrite_core::{Actor, ContentDraft};
use ferrite_domain::{CommentId, CommentStatus, ContentId, Role, User};
use ferrite_http::build_site;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer, Client, Resp, SITE_URL, builder, json_body, send},
};

api_tests!(
    comment_api_flow,
    comments_on_closed_items_are_404,
    comment_xss_is_escaped,
    cookie_without_csrf_is_403,
    foreign_origin_api_submit_is_403,
    bearer_submit_uses_the_account,
);

/// A client address no other request of this binary uses.
fn peer() -> SocketAddr {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    SocketAddr::new(Ipv4Addr::from(0xc612_0000 | n).into(), 40_000)
}

/// `c` with a fresh client address for its next request.
fn fresh(c: &mut Client) -> &mut Client {
    c.peer = Some(peer());
    c
}

/// `POST path` with a JSON `body` as `token`, from a fresh client address.
async fn bearer_submit(api: &Api, token: &Bearer, path: &str, body: &Value) -> Resp {
    let req = token
        .builder(Method::POST, path)
        .extension(ConnectInfo(peer()));
    api.send(json_body(req, body)).await
}

/// A published post or page by a fresh admin.
async fn published(api: &Api, kind: &str, slug: &str) -> ContentId {
    let admin = api.user(&format!("owner-{slug}"), Role::Admin, false).await;
    let actor = Actor::new(admin.id, admin.role);
    let item = api
        .api
        .content
        .create(
            &actor,
            ContentDraft {
                kind: kind.into(),
                title: format!("Title {slug}"),
                slug: Some(slug.into()),
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
            },
        )
        .await
        .unwrap();
    api.api
        .content
        .publish(&actor, item.id, item.version)
        .await
        .unwrap()
        .id
}

fn path(kind: &str, id: ContentId) -> String {
    format!("/api/v1/content/{kind}/{id}/comments")
}

fn guest(body: &str) -> Value {
    json!({
        "body": body,
        "author_name": "Ann",
        "author_email": "ann@example.com",
    })
}

/// The public comment body: never the e-mail or the address hash.
fn assert_public(v: &Value) {
    let obj = v.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "author_name",
            "author_url",
            "date",
            "html",
            "id",
            "parent_id",
            "status"
        ],
        "{v}"
    );
}

async fn stored(api: &Api, id: &str) -> Option<ferrite_domain::Comment> {
    let id = CommentId::from_uuid(id.parse().unwrap());
    api.store.read().comments().get(id).await.unwrap()
}

async fn comment_api_flow(api: Api) -> Api {
    let post = published(&api, "post", "flow").await;
    let mut visitor = Client::new();

    // A guest's first comment is held (`known`, R2): 202 with the public shape.
    let r = fresh(&mut visitor)
        .post(&api, &path("post", post), guest("First!"))
        .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let first = r.json();
    assert_public(&first);
    assert_eq!(first["status"], "pending");
    assert_eq!(first["author_name"], "Ann");
    assert_eq!(first["author_url"], Value::Null);
    assert_eq!(first["parent_id"], Value::Null);
    assert_eq!(first["html"], "<p>First!</p>");
    let first_id = first["id"].as_str().unwrap().to_owned();
    assert!(stored(&api, &first_id).await.is_some());

    // The editor's queue shows it with the e-mail and the keyed address hash.
    api.user("ed", Role::Editor, false).await;
    let mut editor = api.logged_in("ed").await;
    let r = editor.get(&api, "/api/v1/comments?status=pending").await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let list = r.json();
    assert_eq!(list["next_cursor"], Value::Null);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    let row = &items[0];
    assert_eq!(row["id"], first_id.as_str());
    assert_eq!(row["item_id"], post.to_string());
    assert_eq!(row["author_email"], "ann@example.com");
    assert_eq!(row["body"], "First!");
    assert_eq!(row["user_id"], Value::Null);
    let hash = row["ip_hash"].as_str().unwrap();
    assert_eq!(hash.len(), 32);
    assert!(!hash.contains("203.0.113.7"));
    assert_eq!(
        editor
            .get(
                &api,
                &format!("/api/v1/comments?item={post}&status=approved")
            )
            .await
            .json()["items"],
        json!([])
    );
    editor
        .get(&api, "/api/v1/comments?status=bogus")
        .await
        .assert_problem(400, "validation");
    editor
        .get(&api, "/api/v1/comments?cursor=nope")
        .await
        .assert_problem(400, "validation");
    editor
        .get(&api, "/api/v1/comments?limit=0")
        .await
        .assert_problem(400, "validation");

    // An author cannot moderate or read the queue.
    api.user("au", Role::Author, false).await;
    let mut author = api.logged_in("au").await;
    let p = author
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/comments/{first_id}"),
            Some(&json!({"status": "approved"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("moderate_comments"));
    author
        .get(&api, "/api/v1/comments")
        .await
        .assert_problem(403, "forbidden");
    author
        .send(
            &api,
            Method::DELETE,
            &format!("/api/v1/comments/{first_id}"),
            None,
            None,
        )
        .await
        .assert_problem(403, "forbidden");

    // The editor approves it.
    let comment_url = format!("/api/v1/comments/{first_id}");
    let r = editor
        .send(
            &api,
            Method::PATCH,
            &comment_url,
            Some(&json!({"status": "approved"})),
            None,
        )
        .await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.json()["status"], "approved");
    editor
        .send(
            &api,
            Method::PATCH,
            &comment_url,
            Some(&json!({"status": "published"})),
            None,
        )
        .await
        .assert_problem(422, "validation");

    // Now known: the same guest posts directly (201), replying to the first.
    let mut reply = guest("Me again");
    reply["parent_id"] = json!(first_id);
    reply["author_url"] = json!("https://ann.example/");
    let r = fresh(&mut visitor)
        .post(&api, &path("post", post), reply)
        .await;
    assert_eq!(r.status.as_u16(), 201, "{r:?}");
    let second = r.json();
    assert_public(&second);
    assert_eq!(second["status"], "approved");
    assert_eq!(second["parent_id"], first_id.as_str());
    assert_eq!(second["author_url"], "https://ann.example/");

    // A bot that fills the honeypot gets the pending answer; nothing is stored (F18).
    let mut bot = guest("Buy now");
    bot["website"] = json!("http://spam.example");
    let r = fresh(&mut visitor)
        .post(&api, &path("post", post), bot)
        .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    // The same headers as a real pending comment's (M1).
    let mut held = guest("Buy now");
    held["author_name"] = json!("Bea");
    let real = fresh(&mut visitor)
        .post(&api, &path("post", post), held)
        .await;
    assert_eq!(real.status.as_u16(), 202, "{real:?}");
    let names = |r: &Resp| {
        let mut n: Vec<String> = r.headers.keys().map(|k| k.to_string()).collect();
        n.sort();
        n
    };
    assert_eq!(names(&r), names(&real));
    for h in ["content-type", "cache-control"] {
        assert_eq!(r.header(h), real.header(h), "{h}");
    }
    assert!(r.header("location").is_none());
    let dropped = r.json();
    assert_public(&dropped);
    assert_eq!(dropped["status"], "pending");
    assert_eq!(dropped["html"], "<p>Buy now</p>");
    assert!(
        stored(&api, dropped["id"].as_str().unwrap())
            .await
            .is_none()
    );

    // Only spam or trash is deleted; delete answers 204.
    editor
        .send(&api, Method::DELETE, &comment_url, None, None)
        .await
        .assert_problem(422, "validation");
    editor
        .send(
            &api,
            Method::PATCH,
            &comment_url,
            Some(&json!({"status": "trash"})),
            None,
        )
        .await;
    let r = editor
        .send(&api, Method::DELETE, &comment_url, None, None)
        .await;
    assert_eq!(r.status.as_u16(), 204, "{r:?}");
    assert!(stored(&api, &first_id).await.is_none());
    editor
        .send(&api, Method::DELETE, &comment_url, None, None)
        .await
        .assert_problem(404, "not-found");
    api
}

async fn comments_on_closed_items_are_404(api: Api) -> Api {
    let post = published(&api, "post", "closed").await;
    let page = published(&api, "page", "a-page").await;
    let mut visitor = Client::new();
    // A malformed id is refused before any rate-limit hit.
    fresh(&mut visitor)
        .post(&api, "/api/v1/content/post/not-a-uuid/comments", guest("x"))
        .await
        .assert_problem(404, "not-found");
    fresh(&mut visitor)
        .post(&api, &path("page", page), guest("x"))
        .await
        .assert_problem(404, "not-found");
    fresh(&mut visitor)
        .post(&api, &path("page", post), guest("x"))
        .await
        .assert_problem(404, "not-found");
    api.store
        .read()
        .options()
        .set("comments.enabled", &json!(false), true)
        .await
        .unwrap();
    fresh(&mut visitor)
        .post(&api, &path("post", post), guest("x"))
        .await
        .assert_problem(404, "not-found");
    api
}

async fn comment_xss_is_escaped(api: Api) -> Api {
    let post = published(&api, "post", "xss").await;
    let mut visitor = Client::new();
    let r = fresh(&mut visitor)
        .post(
            &api,
            &path("post", post),
            json!({
                "body": "<script>alert(1)</script>\nok",
                "author_name": "<img onerror=x>",
                "author_email": "x@example.com",
            }),
        )
        .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    let got = r.json();
    assert_eq!(
        got["html"],
        "<p>&lt;script&gt;alert(1)&lt;/script&gt;<br>ok</p>"
    );
    // The name is plain text in JSON; it is stored as typed, never as HTML.
    assert_eq!(got["author_name"], "<img onerror=x>");
    let row = stored(&api, got["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(row.body, "<script>alert(1)</script>\nok");
    assert_eq!(row.author_name, "<img onerror=x>");

    // `javascript:` (and any other non-http URL) is refused.
    let mut js = guest("hi");
    js["author_url"] = json!("javascript:alert(1)");
    let p = fresh(&mut visitor)
        .post(&api, &path("post", post), js)
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "author_url");
    // A control character is refused, as is a NUL anywhere in the document.
    let p = fresh(&mut visitor)
        .post(&api, &path("post", post), guest("bell \u{7}"))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "body");
    api
}

async fn cookie_without_csrf_is_403(api: Api) -> Api {
    let post = published(&api, "post", "csrf").await;
    api.user("carol", Role::Contributor, false).await;
    let carol = api.logged_in("carol").await;
    let cookie = carol.cookie.clone().unwrap();

    // A resolved session without `X-CSRF-Token`, or with a wrong one: 403 csrf.
    let mut no_token = Client::new();
    no_token.cookie = Some(cookie.clone());
    let p = fresh(&mut no_token)
        .post(&api, &path("post", post), guest("x"))
        .await
        .assert_problem(403, "csrf");
    assert!(p["detail"].as_str().unwrap().contains("X-CSRF-Token"));
    no_token.csrf = Some("wrong".into());
    fresh(&mut no_token)
        .post(&api, &path("post", post), guest("x"))
        .await
        .assert_problem(403, "csrf");

    // A cookie that does not resolve is a guest (F22).
    let mut stale = Client::new();
    stale.cookie = Some("A".repeat(43));
    let r = fresh(&mut stale)
        .post(&api, &path("post", post), guest("as guest"))
        .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    let row = stored(&api, r.json()["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(row.user_id, None);
    assert_eq!(row.author_name, "Ann");

    // With the token the session's account comments; its inputs are ignored.
    let mut carol = carol;
    let r = fresh(&mut carol)
        .post(
            &api,
            &path("post", post),
            json!({"body": "signed", "author_name": "Mallory", "author_email": "m@evil.example"}),
        )
        .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    assert_eq!(r.json()["author_name"], "carol");
    let row = stored(&api, r.json()["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(row.author_email, "carol@example.com");
    assert!(row.user_id.is_some());
    api
}

async fn foreign_origin_api_submit_is_403(api: Api) -> Api {
    let post = published(&api, "post", "origin").await;
    let mut evil = Client::with_origin("https://evil.example");
    fresh(&mut evil)
        .post(&api, &path("post", post), guest("x"))
        .await
        .assert_problem(403, "csrf");
    // No `Origin`: the `Referer` decides; neither header is refused too.
    let send_with = |headers: Vec<(&'static str, &'static str)>| {
        let mut b = builder(Method::POST, &path("post", post)).extension(ConnectInfo(peer()));
        for (k, v) in headers {
            b = b.header(k, v);
        }
        json_body(b, &guest("x"))
    };
    api.send(send_with(vec![("referer", "https://evil.example/post/")]))
        .await
        .assert_problem(403, "csrf");
    api.send(send_with(vec![]))
        .await
        .assert_problem(403, "csrf");
    api.send(send_with(vec![("origin", "null")]))
        .await
        .assert_problem(403, "csrf");
    // The site's own `Referer` passes.
    let r = api
        .send(send_with(vec![("referer", "https://example.com/origin/")]))
        .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");

    // A bearer request needs no origin: a browser never attaches one by itself.
    let user = api.user("bot", Role::Contributor, false).await;
    let token = api.bearer(&user, &["comments:write"]).await;
    let req = token
        .builder(Method::POST, &path("post", post))
        .extension(ConnectInfo(peer()))
        .header(header::ORIGIN, "https://evil.example");
    let r = api.send(json_body(req, &guest("by token"))).await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");

    // Without a client address the per-IP limit cannot apply: 503 busy.
    let bare = build_site(api.app.clone(), api.api.clone());
    let req = json_body(
        builder(Method::POST, &path("post", post)).header(header::ORIGIN, SITE_URL),
        &guest("x"),
    );
    let r = send(&bare, req).await;
    r.assert_problem(503, "busy");
    assert!(r.header("retry-after").is_some());
    api
}

async fn bearer_submit_uses_the_account(api: Api) -> Api {
    let post = published(&api, "post", "bearer").await;
    let dana: User = api.user("dana", Role::Contributor, false).await;
    let token = api.bearer(&dana, &["comments:write"]).await;
    let r = bearer_submit(
        &api,
        &token,
        &path("post", post),
        &json!({"body": "via token", "author_name": "Not Dana"}),
    )
    .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    let got = r.json();
    assert_public(&got);
    assert_eq!(got["author_name"], "dana");
    let row = stored(&api, got["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(row.user_id, Some(dana.id));
    assert_eq!(row.author_email, "dana@example.com");
    assert_eq!(row.status, CommentStatus::Pending);
    // A signed-in discard echoes the account's name, as a real comment would.
    let r = bearer_submit(
        &api,
        &token,
        &path("post", post),
        &json!({"body": "spam", "author_name": "Not Dana", "website": "x"}),
    )
    .await;
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    let dropped = r.json();
    assert_public(&dropped);
    assert_eq!(dropped["author_name"], "dana");
    assert!(
        stored(&api, dropped["id"].as_str().unwrap())
            .await
            .is_none()
    );

    // A token without `comments:write` cannot comment; a bad token is 401.
    let narrow = api.bearer(&dana, &["content:write"]).await;
    let p = bearer_submit(&api, &narrow, &path("post", post), &json!({"body": "x"}))
        .await
        .assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("comments:write"));
    let r = api
        .send(
            builder(Method::POST, &path("post", post))
                .extension(ConnectInfo(peer()))
                .header(header::AUTHORIZATION, "Bearer nope")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"body": "x"}).to_string()))
                .unwrap(),
        )
        .await;
    r.assert_problem(401, "unauthenticated");

    // A moderator's token: approved at once (201).
    let ed = api.user("eddie", Role::Editor, false).await;
    let token = api.bearer(&ed, &["comments:write"]).await;
    let r = bearer_submit(
        &api,
        &token,
        &path("post", post),
        &json!({"body": "from the editor"}),
    )
    .await;
    assert_eq!(r.status.as_u16(), 201, "{r:?}");
    assert_eq!(r.json()["status"], "approved");
    // A token with only `comments:read` lists the queue, cannot moderate.
    let reader = api.bearer(&ed, &["comments:read"]).await;
    let r = reader.get(&api, "/api/v1/comments").await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.json()["items"].as_array().unwrap().len(), 2);
    let id = got["id"].as_str().unwrap();
    reader
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/comments/{id}"),
            Some(&json!({"status": "spam"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    api
}
