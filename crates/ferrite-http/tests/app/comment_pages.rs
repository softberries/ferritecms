//! Public comment pages (M2b-2 Task 4, P34) in the `blog` theme: the heading counts
//! every approved comment, `?cpage=N` pages the top-level threads and is part of the
//! cache key, a page past the end shows the last one, and the form's redirect lands on
//! the page that holds the new comment.

use axum::http::StatusCode;
use ferrite_core::{OptionService, comments::PER_PAGE_OPTION};
use ferrite_domain::{Comment, CommentId, CommentQuery, CommentStatus, ContentId, Timestamp};
use serde_json::json;

use crate::app_tests;
use crate::comments::{blog, form, guest, pairs};
use crate::harness::App;

app_tests!(
    count_is_the_real_total,
    form_redirect_lands_on_the_comments_page,
    cpage_is_part_of_the_cache_key,
    cpage_past_the_end_shows_the_last_page,
);

/// `n` approved top-level comments on `post` by the form's guest (`guest()`), oldest
/// first, a millisecond apart and all in the past; one `Tx`, no invalidation (call it
/// before the page is cached).
async fn roots(app: &App, post: ContentId, n: i64) -> Vec<CommentId> {
    let base = Timestamp::now().as_micros() - 60_000_000;
    let tx = app.site.store.begin().await.unwrap();
    let mut ids = Vec::new();
    for i in 0..n {
        let c = Comment {
            id: CommentId::new(),
            item_id: post,
            parent_id: None,
            user_id: None,
            author_name: "Ann Guest".into(),
            author_email: "ann@guest.example".into(),
            author_url: String::new(),
            body: format!("Root number {i}."),
            status: CommentStatus::Approved,
            ip_hash: None,
            created_at: Timestamp::from_micros(base + i * 1000).unwrap(),
        };
        tx.comments().insert(&c).await.unwrap();
        ids.push(c.id);
    }
    tx.commit().await.unwrap();
    ids
}

async fn per_page(app: &App, n: u32) {
    OptionService::new(app.site.store.clone())
        .set(&app.admin, PER_PAGE_OPTION, &json!(n), true)
        .await
        .unwrap();
}

fn anchor(id: CommentId) -> String {
    format!("id=\"comment-{id}\"")
}

/// 600 approved comments: the heading says 600 (not the old cap of 500), and the
/// first page links to the next.
async fn count_is_the_real_total(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    let ids = roots(&app, post.id, 600).await;
    let page = app.get("/hello/").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = &page.body;
    assert!(html.contains(">600 comments</h2>"), "{html}");
    assert!(html.contains("Page 1 of 12"), "{html}");
    assert!(
        html.contains("rel=\"next\" href=\"&#x2f;hello&#x2f;?cpage=2\""),
        "{html}"
    );
    assert!(!html.contains("rel=\"prev\""), "{html}");
    assert!(html.contains(&anchor(ids[49])) && !html.contains(&anchor(ids[50])));
    let last = app.get("/hello/?cpage=12").await;
    assert!(last.body.contains(&anchor(ids[599])), "{}", last.body);
    assert!(
        last.body
            .contains("rel=\"prev\" href=\"&#x2f;hello&#x2f;?cpage=11\"")
    );
    app
}

/// With 2 threads a page and 5 existing roots, a new approved comment is the sixth root:
/// the 303 goes to `?cpage=3#comment-<id>`, and that page shows it.
async fn form_redirect_lands_on_the_comments_page(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    per_page(&app, 2).await;
    roots(&app, post.id, 5).await;

    // A known guest: approved at once.
    let r = app.send(form(pairs(&guest(post.id, "Sixth")))).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    let sixth = app
        .site
        .store
        .read()
        .comments()
        .list(&CommentQuery {
            status: Some(CommentStatus::Approved),
            item: Some(post.id),
            limit: 1,
            cursor: None,
        })
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(sixth.body, "Sixth");
    assert_eq!(
        r.header("location"),
        Some(format!("/hello/?cpage=3#comment-{}", sixth.id).as_str())
    );
    let page = app.get("/hello/?cpage=3").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(&anchor(sixth.id)), "{}", page.body);

    // On page 1 the redirect has no query.
    let other = app.post("Other", "other", &[]).await;
    let r = app.send(form(pairs(&guest(other.id, "First")))).await;
    let location = r.header("location").unwrap();
    assert!(location.starts_with("/other/#comment-"), "{location}");
    app
}

/// Page 1 and page 2 are cached apart; `?cpage=abc` and `?cpage=1` are page 1 (the same
/// entry).
async fn cpage_is_part_of_the_cache_key(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    per_page(&app, 2).await;
    let ids = roots(&app, post.id, 3).await;

    let one = app.get("/hello/").await;
    let two = app.get("/hello/?cpage=2").await;
    assert_eq!(app.cached().await, 2);
    assert_ne!(one.body, two.body);
    assert!(one.body.contains(&anchor(ids[0])) && !one.body.contains(&anchor(ids[2])));
    assert!(two.body.contains(&anchor(ids[2])) && !two.body.contains(&anchor(ids[0])));
    // Served from the cache: the same bodies.
    assert_eq!(app.get("/hello/").await.body, one.body);
    assert_eq!(app.get("/hello/?cpage=2").await.body, two.body);
    for same in ["/hello/?cpage=abc", "/hello/?cpage=1", "/hello/?cpage=0"] {
        assert_eq!(app.get(same).await.body, one.body, "{same}");
    }
    assert_eq!(app.cached().await, 2);
    app
}

/// A page past the end shows the last page (200, not 404).
async fn cpage_past_the_end_shows_the_last_page(app: App) -> App {
    let app = blog(app).await;
    let post = app.post("Hello", "hello", &[]).await;
    per_page(&app, 2).await;
    let ids = roots(&app, post.id, 3).await;
    let page = app.get("/hello/?cpage=9").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(&anchor(ids[2])), "{}", page.body);
    assert!(!page.body.contains(&anchor(ids[0])), "{}", page.body);
    assert!(page.body.contains("Page 2 of 2"), "{}", page.body);
    assert!(page.body.contains(">3 comments</h2>"), "{}", page.body);
    app
}
