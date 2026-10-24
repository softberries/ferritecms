//! The public comment form (`POST /_comment`) and comment threads in the `blog` theme
//! (M2a-2b Task 8, P12, P13, Review Focus 3): the redirect after a post, the origin
//! check, the silent honeypot, error pages, closed and draft items, escaping, nesting,
//! and that pending comments never render.
//!
//! Every submit comes from its own client address ([`peer`]) unless a test is about
//! the per-IP limit.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use ferrite_core::{
    OptionService,
    comments::{CommentInput, CommentService, Commenter, Submitted},
};
use ferrite_domain::{Comment, CommentId, CommentQuery, CommentStatus, ContentId};
use ferrite_http::build_app;
use ferrite_theme::{Theme, ThemeSource};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::app_tests;
use crate::common::SITE_URL;
use crate::harness::{App, Resp, request};

app_tests!(
    form_comment_flow,
    foreign_origin_form_is_refused,
    honeypot_drops_silently,
    form_errors_are_html_pages,
    form_on_closed_or_draft_item_is_404,
    theme_renders_threads_escaped,
    pending_comments_never_render,
);

const FORM: &str = "application/x-www-form-urlencoded";

/// A client address no other request of this binary uses.
fn peer() -> SocketAddr {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    SocketAddr::new(Ipv4Addr::from(0xc633_0000 | n).into(), 40_000)
}

/// `app` serving the embedded `blog` theme (fresh state, empty cache).
async fn blog(mut app: App) -> App {
    let theme = Arc::new(Theme::load(&ThemeSource::Embedded("blog")).expect("blog theme"));
    let state = App::build_state(
        &app.site,
        app.site.reader.clone(),
        theme,
        true,
        ferrite_http::HttpConfig::new(SITE_URL, crate::harness::SITE_TITLE, "en"),
    )
    .await;
    app.state = Arc::new(state);
    app.router = build_app(app.state.clone());
    app
}

/// A comment service over the app's store (the same key, so the same address hashes).
fn service(app: &App) -> CommentService {
    CommentService::new(app.site.store.clone(), App::auth(&app.site))
}

fn enc(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// A form body from `pairs`.
fn body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The fields of a guest's comment on `post`.
fn guest(post: ContentId, text: &str) -> Vec<(&'static str, String)> {
    vec![
        ("post", post.to_string()),
        ("author", "Ann Guest".into()),
        ("email", "ann@guest.example".into()),
        ("url", String::new()),
        ("comment", text.into()),
        ("website", String::new()),
    ]
}

fn pairs(fields: &[(&'static str, String)]) -> String {
    let v: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    body(&v)
}

/// `POST /_comment` with `body`, from the site's origin and a fresh client address.
fn form(body: String) -> Request<Body> {
    form_from(body, Some(SITE_URL), None, Some(peer()))
}

fn form_from(
    body: String,
    origin: Option<&str>,
    referer: Option<&str>,
    peer: Option<SocketAddr>,
) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/_comment")
        .header(header::HOST, "example.com")
        .header(header::CONTENT_TYPE, FORM);
    if let Some(o) = origin {
        b = b.header(header::ORIGIN, o);
    }
    if let Some(r) = referer {
        b = b.header(header::REFERER, r);
    }
    if let Some(p) = peer {
        b = b.extension(ConnectInfo(p));
    }
    b.body(Body::from(body)).expect("request")
}

async fn comments_of(app: &App, post: ContentId) -> Vec<Comment> {
    app.site
        .store
        .read()
        .comments()
        .list(&CommentQuery {
            status: None,
            item: Some(post),
            limit: 100,
            cursor: None,
        })
        .await
        .unwrap()
        .items
}

/// Approves comment `id` as the admin.
async fn approve(app: &App, id: CommentId) {
    service(app)
        .moderate(&app.admin, id, CommentStatus::Approved)
        .await
        .unwrap();
}

/// A guest comment on `post` through the service, approved; returns its id.
async fn approved(
    app: &App,
    post: ContentId,
    parent: Option<CommentId>,
    name: &str,
    url: &str,
    text: &str,
) -> CommentId {
    let input = CommentInput {
        item: post,
        kind: "post".into(),
        parent,
        author_name: name.into(),
        author_email: "someone@guest.example".into(),
        author_url: url.into(),
        body: text.into(),
        honeypot: String::new(),
    };
    let Submitted::Accepted(c) = service(app)
        .submit(input, Commenter::Anonymous, peer().ip())
        .await
        .unwrap()
    else {
        panic!("discarded");
    };
    if c.status != CommentStatus::Approved {
        approve(app, c.id).await;
    }
    c.id
}

fn assert_no_store(r: &Resp) {
    assert_eq!(r.header("cache-control"), Some("no-store"), "{r:?}");
}

/// An HTML error page: no-store, the message, a link back.
fn assert_error_page(r: &Resp, status: u16, message: &str, back: &str) {
    assert_eq!(r.status.as_u16(), status, "{r:?}");
    r.assert_html();
    assert_no_store(r);
    assert!(r.body.contains(message), "{}", r.body);
    assert!(r.body.contains(&format!("href=\"{back}\"")), "{}", r.body);
}

/// Waits until the current rate-limit window has at least `need` left.
async fn align_to_window(app: &App, need: Duration) {
    let w = 60_000_000_i64;
    let now = app
        .site
        .store
        .read()
        .nodes()
        .db_now()
        .await
        .unwrap()
        .as_micros();
    let left = w - now.rem_euclid(w);
    if left < i64::try_from(need.as_micros()).unwrap() {
        tokio::time::sleep(Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50))
            .await;
    }
}

/// A guest's first comment is held (`known`, R2): 303 to `#comment-pending`; once
/// approved, the cached page shows it after one poll; the same guest's next comment
/// is approved at once and redirects to its own fragment. A session cookie does not
/// make a form comment the user's (P12).
async fn form_comment_flow(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    let mut poller = app.poller().await;

    let page = app.get("/hello/").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body.contains("action=\"&#x2f;_comment\""),
        "{}",
        page.body
    );
    assert!(
        page.body.contains("id=\"comment-pending\""),
        "{}",
        page.body
    );
    assert!(!page.body.contains("First comment"));

    let r = app
        .send(form(pairs(&guest(post.id, "First comment"))))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    assert_eq!(r.header("location"), Some("/hello/#comment-pending"));
    assert_no_store(&r);
    let stored = comments_of(&app, post.id).await;
    assert_eq!(stored.len(), 1);
    let first = &stored[0];
    assert_eq!(first.status, CommentStatus::Pending);
    assert_eq!(first.user_id, None);

    approve(&app, first.id).await;
    // Cached until the poll.
    assert!(!app.get("/hello/").await.body.contains("First comment"));
    poller.poll_once().await.unwrap();
    let page = app.get("/hello/").await;
    assert!(page.body.contains("First comment"), "{}", page.body);
    assert!(
        page.body.contains(&format!("id=\"comment-{}\"", first.id)),
        "{}",
        page.body
    );

    // Known now: approved at once, redirected to its own fragment.
    let r = app.send(form(pairs(&guest(post.id, "Again")))).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    let second = comments_of(&app, post.id)
        .await
        .into_iter()
        .find(|c| c.body == "Again")
        .unwrap();
    assert_eq!(second.status, CommentStatus::Approved);
    assert_eq!(
        r.header("location"),
        Some(format!("/hello/#comment-{}", second.id).as_str())
    );

    // The admin's session cookie is ignored: anonymous and held, not auto-approved.
    let cookie = app.session(&app.ada_user).await;
    let mut fields = guest(post.id, "Signed in?");
    fields[1].1 = "Other Guest".into();
    fields[2].1 = "other@guest.example".into();
    let mut req = form(pairs(&fields));
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    let r = app.send(req).await;
    assert_eq!(r.header("location"), Some("/hello/#comment-pending"));
    let c = comments_of(&app, post.id)
        .await
        .into_iter()
        .find(|c| c.body == "Signed in?")
        .unwrap();
    assert_eq!((c.user_id, c.status), (None, CommentStatus::Pending));
    app
}

/// Review Focus 3: a form posted from another origin is refused (`Origin`, else the
/// `Referer`'s origin; neither is refused too), and nothing is stored.
async fn foreign_origin_form_is_refused(app: App) -> App {
    let post = app.post("Hello", "hello", &[]).await;
    let fields = pairs(&guest(post.id, "From elsewhere"));
    let cases = [
        (Some("https://evil.example"), None),
        (Some("null"), None),
        (
            Some("https://evil.example"),
            Some("https://example.com/hello/"),
        ),
        (None, Some("https://evil.example/hello/")),
        (None, Some("http://example.com/hello/")),
        (None, None),
    ];
    for (origin, referer) in cases {
        let r = app
            .send(form_from(fields.clone(), origin, referer, Some(peer())))
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{origin:?} {referer:?}");
        assert_no_store(&r);
    }
    assert!(comments_of(&app, post.id).await.is_empty());

    // The site's own Referer (no Origin) is accepted.
    let r = app
        .send(form_from(
            fields,
            None,
            Some("https://example.com/hello/"),
            Some(peer()),
        ))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    assert_eq!(comments_of(&app, post.id).await.len(), 1);
    app
}

/// Review Focus 3: a filled honeypot gets exactly the pending answer, and nothing is
/// stored.
async fn honeypot_drops_silently(app: App) -> App {
    let post = app.post("Hello", "hello", &[]).await;
    let mut bot = guest(post.id, "Buy now");
    bot[5].1 = "http://spam.example".into();
    let dropped = app.send(form(pairs(&bot))).await;
    assert_eq!(dropped.status, StatusCode::SEE_OTHER, "{dropped:?}");
    assert_eq!(dropped.header("location"), Some("/hello/#comment-pending"));
    assert!(comments_of(&app, post.id).await.is_empty());

    let real = app.send(form(pairs(&guest(post.id, "Buy now")))).await;
    assert_eq!(real.status, StatusCode::SEE_OTHER);
    let names = |r: &Resp| {
        let mut n: Vec<String> = r.headers.keys().map(|k| k.to_string()).collect();
        n.sort();
        n
    };
    assert_eq!(names(&dropped), names(&real));
    for (name, value) in &real.headers {
        assert_eq!(dropped.headers.get(name), Some(value), "{name}");
    }
    assert_eq!(dropped.body, real.body);
    assert_eq!(comments_of(&app, post.id).await.len(), 1);

    // Invalid input is refused whether or not the honeypot is filled.
    bot[4].1 = String::new();
    let r = app.send(form(pairs(&bot))).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{r:?}");
    app
}

/// Validation is a `422` HTML page with the message and a link back, the per-IP limit
/// a `429` page with `Retry-After`; a body of the wrong type is `415`, one over 64 KiB
/// `413`, and without a client address the answer is `503`.
async fn form_errors_are_html_pages(app: App) -> App {
    let post = app.post("Hello", "hello", &[]).await;

    let r = app.send(form(String::new())).await;
    assert_error_page(&r, 422, "The form names no post.", "/");

    let mut empty = guest(post.id, "");
    let r = app.send(form(pairs(&empty))).await;
    assert_error_page(
        &r,
        422,
        "the comment must be 1 to 10000 characters",
        "/hello/",
    );
    empty[4].1 = "ok".into();
    empty[3].1 = "javascript:alert(1)".into();
    let r = app.send(form(pairs(&empty))).await;
    assert_error_page(&r, 422, "the website must be an http or https", "/hello/");
    empty[3].1 = String::new();
    empty.push(("parent", "not-a-uuid".into()));
    let r = app.send(form(pairs(&empty))).await;
    assert_error_page(
        &r,
        422,
        "the reply must be to an approved comment",
        "/hello/",
    );
    assert!(comments_of(&app, post.id).await.is_empty());

    // A repeated field or bytes that are not UTF-8 are a bad request.
    let r = app.send(form("post=a&post=b".into())).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app.send(form("comment=%FF".into())).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Wrong type, too large, no client address.
    let fields = pairs(&guest(post.id, "x"));
    let mut req = form(fields.clone());
    req.headers_mut().insert(
        header::CONTENT_TYPE,
        "multipart/form-data; boundary=x".parse().unwrap(),
    );
    assert_eq!(
        app.send(req).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let mut req = form(fields.clone());
    req.headers_mut().insert(
        header::CONTENT_TYPE,
        "Application/X-WWW-Form-Urlencoded; charset=UTF-8"
            .parse()
            .unwrap(),
    );
    assert_eq!(app.send(req).await.status, StatusCode::SEE_OTHER);
    let big = format!("{fields}&comment2={}", "a".repeat(64 * 1024));
    assert_eq!(
        app.send(form(big)).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let r = app
        .send(form_from(fields.clone(), Some(SITE_URL), None, None))
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{r:?}");
    assert_no_store(&r);
    // Only POST.
    let r = app.send(request(Method::GET, "/_comment")).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{r:?}");

    // Flood from one address: 3 a minute, then 429 with Retry-After.
    align_to_window(&app, Duration::from_secs(20)).await;
    let ip = peer();
    for i in 0..3 {
        let r = app
            .send(form_from(
                pairs(&guest(post.id, &format!("flood {i}"))),
                Some(SITE_URL),
                None,
                Some(ip),
            ))
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{i}: {r:?}");
    }
    let r = app
        .send(form_from(
            pairs(&guest(post.id, "flood 3")),
            Some(SITE_URL),
            None,
            Some(ip),
        ))
        .await;
    assert_error_page(&r, 429, "too quickly", "/hello/");
    let retry: u64 = r.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    app
}

/// A draft, a page, an unknown or malformed id and a closed site are `404` plain text;
/// with comments off the existing thread still shows, the form does not.
async fn form_on_closed_or_draft_item_is_404(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    let held = app.draft_post("Held", "held", &[]).await;
    let page = app.page("About", "about", None).await;
    let shown = approved(&app, post.id, None, "Early Bird", "", "Before closing").await;

    for id in [
        held.id.to_string(),
        page.id.to_string(),
        ContentId::new().to_string(),
        "nope".into(),
    ] {
        let mut fields = guest(post.id, "Hi");
        fields[0].1 = id.clone();
        let r = app.send(form(pairs(&fields))).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{id}: {r:?}");
        assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
        assert_no_store(&r);
    }
    // A page shows no thread and no form.
    let about = app.get("/about/").await;
    assert!(
        !about.body.contains("action=\"&#x2f;_comment\""),
        "{}",
        about.body
    );

    let mut poller = app.poller().await;
    assert!(
        app.get("/hello/")
            .await
            .body
            .contains("action=\"&#x2f;_comment\"")
    );
    OptionService::new(app.site.store.clone())
        .set(
            &app.admin,
            ferrite_core::comments::ENABLED_OPTION,
            &serde_json::json!(false),
            true,
        )
        .await
        .unwrap();
    let r = app.send(form(pairs(&guest(post.id, "Closed?")))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{r:?}");
    poller.poll_once().await.unwrap();
    let page = app.get("/hello/").await;
    assert!(page.body.contains("Before closing"), "{}", page.body);
    assert!(page.body.contains(&format!("id=\"comment-{shown}\"")));
    assert!(
        !page.body.contains("action=\"&#x2f;_comment\""),
        "{}",
        page.body
    );
    assert!(!page.body.contains("name=\"website\""));
    app
}

/// Review Focus 3: script in a stored approved comment's body and name renders as
/// text, the author link is the escaped URL with `rel="nofollow ugc noopener"`, and
/// replies nest by depth right after their parent.
async fn theme_renders_threads_escaped(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    let root = approved(
        &app,
        post.id,
        None,
        "<script>alert(1)</script>",
        "https://ann.example/?a=1&b=2",
        "<script>alert(2)</script>\n\n<b>bold</b>",
    )
    .await;
    let reply = approved(&app, post.id, Some(root), "Bea", "", "First reply").await;
    let deep = approved(&app, post.id, Some(reply), "Cy", "", "Deep reply").await;
    let other = approved(&app, post.id, None, "Dee", "", "Second thread").await;

    let page = app.get("/hello/").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = &page.body;
    assert!(!html.contains("<script>alert"), "{html}");
    assert!(!html.contains("<b>bold"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;&#x2f;script&gt;"),
        "{html}"
    );
    assert!(
        html.contains("<p>&lt;script&gt;alert(2)&lt;/script&gt;</p>"),
        "{html}"
    );
    assert!(html.contains("<p>&lt;b&gt;bold&lt;/b&gt;</p>"), "{html}");
    assert!(
        html.contains(
            "href=\"https:&#x2f;&#x2f;ann.example&#x2f;?a=1&amp;b=2\" rel=\"nofollow ugc noopener\""
        ),
        "{html}"
    );
    assert!(html.contains("4 comments"), "{html}");
    let at = |id: CommentId| {
        html.find(&format!("id=\"comment-{id}\""))
            .unwrap_or_else(|| panic!("comment {id} missing: {html}"))
    };
    assert!(at(root) < at(reply) && at(reply) < at(deep) && at(deep) < at(other));
    for (id, depth) in [(root, 0), (reply, 1), (deep, 2), (other, 0)] {
        let tag = &html[..at(id)];
        let open = tag.rfind("<li").unwrap();
        assert!(
            html[open..at(id) + 60].contains(&format!("comment--depth-{depth}")),
            "{id} at depth {depth}: {}",
            &html[open..at(id) + 60]
        );
    }
    // Replies nest in the parent's list.
    let reply_block = &html[at(root)..at(other)];
    assert!(
        reply_block.contains("class=\"comment-replies\""),
        "{reply_block}"
    );
    app
}

/// Pending, spam and trashed comments never render; an approved one that is trashed
/// leaves the page after one poll.
async fn pending_comments_never_render(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    for text in ["Pending text", "Spam text", "Trash text"] {
        let r = app.send(form(pairs(&guest(post.id, text)))).await;
        assert_eq!(
            r.header("location"),
            Some("/hello/#comment-pending"),
            "{r:?}"
        );
    }
    let svc = service(&app);
    for c in comments_of(&app, post.id).await {
        let status = match c.body.as_str() {
            "Spam text" => CommentStatus::Spam,
            "Trash text" => CommentStatus::Trash,
            _ => continue,
        };
        svc.moderate(&app.admin, c.id, status).await.unwrap();
    }
    let shown = approved(&app, post.id, None, "Ok", "", "Approved text").await;
    let mut poller = app.poller().await;
    let page = app.get("/hello/").await;
    assert!(page.body.contains("Approved text"), "{}", page.body);
    for hidden in ["Pending text", "Spam text", "Trash text"] {
        assert!(!page.body.contains(hidden), "{hidden}: {}", page.body);
    }
    // The notice is only the static "awaiting moderation" text.
    assert!(page.body.contains("awaiting moderation"));

    svc.moderate(&app.admin, shown, CommentStatus::Trash)
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    let page = app.get("/hello/").await;
    assert!(!page.body.contains("Approved text"), "{}", page.body);
    app
}
