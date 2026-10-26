//! M2b-2 Task 3: the moderation queue's item reference (`item`) and
//! `GET /comments/counts?item`.

use axum::http::{Method, StatusCode};
use ferrite_core::{Actor, ContentDraft};
use ferrite_domain::{Comment, CommentId, CommentStatus, ContentId, Role, Timestamp, User};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
};

api_tests!(comment_list_has_item_ref, comment_counts_endpoint);

async fn admin(api: &Api) -> (User, Bearer) {
    let ada = api.user("ada", Role::Admin, false).await;
    let token = api.bearer(&ada, &["*"]).await;
    (ada, token)
}

/// A post titled `title` with slug `slug` by `owner`, published when `publish`.
async fn post(api: &Api, owner: &User, title: &str, slug: &str, publish: bool) -> ContentId {
    let actor = Actor::new(owner.id, owner.role);
    let item = api
        .api
        .content
        .create(
            &actor,
            ContentDraft {
                kind: "post".into(),
                title: title.into(),
                slug: Some(slug.into()),
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    if !publish {
        return item.id;
    }
    api.api
        .content
        .publish(&actor, item.id, item.version)
        .await
        .unwrap()
        .id
}

/// Stores a guest comment on `item` with `status` (the store directly: a held comment
/// on a draft cannot come through the public submit).
async fn comment(api: &Api, item: ContentId, status: CommentStatus) -> CommentId {
    let c = Comment {
        id: CommentId::new(),
        item_id: item,
        parent_id: None,
        user_id: None,
        author_name: "Ann".into(),
        author_email: "ann@example.com".into(),
        author_url: String::new(),
        body: "Hello".into(),
        status,
        ip_hash: None,
        created_at: Timestamp::now(),
    };
    api.store.read().comments().insert(&c).await.unwrap();
    c.id
}

/// The `200` list at `query`, as rows by comment id.
async fn rows(api: &Api, b: &Bearer, query: &str) -> Vec<Value> {
    let r = b.get(api, &format!("/api/v1/comments{query}")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    r.json()["items"].as_array().unwrap().clone()
}

async fn comment_list_has_item_ref(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let live = post(&api, &ada, "Live post", "live-post", true).await;
    let draft = post(&api, &ada, "Draft post", "draft-post", false).await;
    let on_live = comment(&api, live, CommentStatus::Approved).await;
    let on_draft = comment(&api, draft, CommentStatus::Pending).await;

    let all = rows(&api, &b, "").await;
    assert_eq!(all.len(), 2, "{all:?}");
    let row = |id: CommentId| {
        all.iter()
            .find(|r| r["id"] == id.to_string())
            .unwrap()
            .clone()
    };
    assert_eq!(
        row(on_live)["item"],
        json!({
            "id": live.to_string(),
            "type": "post",
            "title": "Live post",
            "permalink": "/live-post/",
            "status": "published",
        })
    );
    // A draft item has no public address: `permalink` is null.
    assert_eq!(
        row(on_draft)["item"],
        json!({
            "id": draft.to_string(),
            "type": "post",
            "title": "Draft post",
            "permalink": null,
            "status": "draft",
        })
    );

    // PATCH answers with the item reference too.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/comments/{on_draft}"),
            Some(&json!({"status": "spam"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let moderated = r.json();
    assert_eq!(moderated["status"], "spam");
    assert_eq!(moderated["item"]["title"], "Draft post");
    assert_eq!(moderated["item"]["permalink"], Value::Null);

    // Filtered by item, the reference is the same.
    let only = rows(&api, &b, &format!("?item={live}")).await;
    assert_eq!(only.len(), 1);
    assert_eq!(only[0]["item"]["permalink"], "/live-post/");
    api
}

async fn comment_counts_endpoint(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let counts = b.get(&api, "/api/v1/comments/counts").await;
    assert_eq!(counts.status, StatusCode::OK, "{counts:?}");
    assert_eq!(
        counts.json(),
        json!({"pending": 0, "approved": 0, "spam": 0, "trash": 0})
    );

    let one = post(&api, &ada, "One", "one", true).await;
    let two = post(&api, &ada, "Two", "two", true).await;
    for (item, status) in [
        (one, CommentStatus::Pending),
        (one, CommentStatus::Pending),
        (one, CommentStatus::Spam),
        (two, CommentStatus::Approved),
        (two, CommentStatus::Trash),
    ] {
        comment(&api, item, status).await;
    }
    assert_eq!(
        b.get(&api, "/api/v1/comments/counts").await.json(),
        json!({"pending": 2, "approved": 1, "spam": 1, "trash": 1})
    );
    assert_eq!(
        b.get(&api, &format!("/api/v1/comments/counts?item={one}"))
            .await
            .json(),
        json!({"pending": 2, "approved": 0, "spam": 1, "trash": 0})
    );
    b.get(&api, "/api/v1/comments/counts?item=nope")
        .await
        .assert_problem(400, "malformed-query");

    // An editor moderates; an author cannot read the counts; a token needs comments:read.
    let ed = api.user("ed", Role::Editor, false).await;
    let ed_token = api.bearer(&ed, &["comments:read"]).await;
    let r = ed_token.get(&api, "/api/v1/comments/counts").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let no_scope = api.bearer(&ed, &["content:read"]).await;
    no_scope
        .get(&api, "/api/v1/comments/counts")
        .await
        .assert_problem(403, "forbidden");
    let au = api.user("au", Role::Author, false).await;
    let au_token = api.bearer(&au, &["*"]).await;
    let p = au_token
        .get(&api, "/api/v1/comments/counts")
        .await
        .assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("moderate_comments"));
    api
}
