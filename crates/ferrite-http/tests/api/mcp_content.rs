//! M5 Task 6: the content tools of the MCP endpoint (design 12 §5.1 to §5.3, §5.5):
//! Markdown bodies in and out (the leading-H1 rule, the size cap, `ferrite:media` checks),
//! optimistic concurrency with the current item in the error, the lifecycle, revisions,
//! preview links, paging, the audit rows and the `ferrite://content/{type}/{id}` resource.

use axum::http::StatusCode;
use ferrite_domain::{AuditQuery, Role, User};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    api_tests,
    harness::{Api, Bearer, Part, SITE_URL},
    mcp::{V_CURRENT, V_LEGACY, mcp_call, mcp_call_v, text, tool},
};

api_tests!(
    create_draft_from_markdown_then_publish,
    markdown_title_rules,
    markdown_limits_and_media_checks,
    blocks_in_blocks_out,
    stale_version_returns_current_item,
    contributor_submits_but_cannot_publish,
    schedule_in_future,
    trash_and_restore,
    revisions_and_restore_revision,
    preview_link_works_for_draft,
    list_filters_by_status_and_q,
    update_merges_fields_and_sets_terms,
    audit_rows_name_the_client,
    content_resource_reads_an_item,
    content_tools_are_annotated,
    other_types_and_missing_items,
);

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn png(tag: &str) -> Vec<u8> {
    let mut v = PNG_MAGIC.to_vec();
    v.extend_from_slice(b"\0\0\0\rIHDR");
    v.extend_from_slice(tag.as_bytes());
    v.resize(1000, 0);
    v
}

async fn editor(api: &Api) -> (User, Bearer) {
    let u = api.user("eddie", Role::Editor, false).await;
    let t = api.bearer(&u, &["*"]).await;
    (u, t)
}

/// An uploaded image's media id.
async fn upload(api: &Api, tag: &str) -> String {
    let admin = api.user(&format!("media-{tag}"), Role::Admin, false).await;
    let t = api.bearer(&admin, &["*"]).await;
    let bytes = png(tag);
    let r = t
        .upload(
            api,
            &[
                Part::text("title", tag),
                Part::file("pic.png", "image/png", &bytes),
                Part::text("alt", "A picture"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()["id"].as_str().unwrap().to_owned()
}

/// The structured content of a successful result.
fn item(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

/// The problem of a failed result.
fn problem(r: &Value) -> &Value {
    assert_eq!(r["isError"], true, "{r}");
    &r["structuredContent"]
}

async fn create(api: &Api, t: &Bearer, args: Value) -> Value {
    let r = tool(api, &t.0, "content_create", args).await;
    item(&r).clone()
}

async fn rows(api: &Api) -> Vec<ferrite_domain::AuditEntry> {
    api.store
        .read()
        .audit()
        .list(&AuditQuery::first(100))
        .await
        .unwrap()
        .items
}

// ---- Markdown in and out ---------------------------------------------------------------

async fn create_draft_from_markdown_then_publish(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let mut poller = api.poller().await;
    let media = upload(&api, "sky").await;
    let md = format!(
        "## Intro\n\nSome **bold** text and a [link](https://example.com).\n\n\
         - one\n- two\n\n![A red sky](ferrite:media/{media} \"Sunset\")\n"
    );
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Hello MCP", "body_markdown": md, "excerpt": "Short"}),
    )
    .await;
    let it = item(&r);
    let id = it["id"].as_str().unwrap().to_owned();
    assert_eq!(it["type"], "post", "{it}");
    assert_eq!(it["title"], "Hello MCP", "{it}");
    assert_eq!(it["slug"], "hello-mcp", "{it}");
    assert_eq!(it["status"], "draft", "{it}");
    assert_eq!(it["version"], 1, "{it}");
    assert_eq!(it["excerpt"], "Short", "{it}");
    assert_eq!(it["author"]["login"], "eddie", "{it}");
    assert_eq!(
        it["admin_url"],
        format!("{SITE_URL}/admin/content/post/{id}"),
        "{it}"
    );
    // A draft has no public page: its url is the preview link.
    assert_eq!(it["url"], format!("{SITE_URL}/_preview/{id}"), "{it}");
    let body = it["body_markdown"].as_str().unwrap();
    for needle in [
        "## Intro",
        "**bold**",
        "[link](https://example.com)",
        "- one",
        &format!("![A red sky](ferrite:media/{media} \"Sunset\")"),
    ] {
        assert!(body.contains(needle), "{needle:?} missing from {body:?}");
    }
    assert!(!body.contains("# Hello MCP"), "{body}");
    assert!(
        text(&r).contains("Hello MCP"),
        "the summary names the item: {}",
        text(&r)
    );

    // The same item on a read.
    let g = tool(&api, &t.0, "content_get", json!({"type": "post", "id": id})).await;
    assert_eq!(item(&g)["body_markdown"], it["body_markdown"], "{g}");
    assert_eq!(item(&g)["version"], 1, "{g}");
    assert_eq!(item(&g)["allowed"]["publish"], true, "{g}");

    // Publish with the version read: published, live, its url the public page.
    let p = tool(
        &api,
        &t.0,
        "content_publish",
        json!({"type": "post", "id": id, "version": 1}),
    )
    .await;
    poller.poll_once().await.unwrap();
    let pub_item = item(&p);
    assert_eq!(pub_item["status"], "published", "{pub_item}");
    assert_eq!(pub_item["version"], 2, "{pub_item}");
    assert_eq!(
        pub_item["url"],
        format!("{SITE_URL}/hello-mcp/"),
        "{pub_item}"
    );
    assert!(pub_item["published_at"].is_string(), "{pub_item}");
    let page = api.get("/hello-mcp/").await;
    assert_eq!(page.status, StatusCode::OK, "{page:?}");
    assert!(page.body.contains("Hello MCP"), "{}", page.body);

    // Unpublish takes it down again.
    let u = tool(
        &api,
        &t.0,
        "content_unpublish",
        json!({"type": "post", "id": id, "version": 2}),
    )
    .await;
    assert_eq!(item(&u)["status"], "draft", "{u}");
    poller.poll_once().await.unwrap();
    assert_eq!(api.get("/hello-mcp/").await.status, StatusCode::NOT_FOUND);
    api
}

async fn markdown_title_rules(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let new = |args: Value| {
        let (api, t) = (&api, &t);
        async move { tool(api, &t.0, "content_create", args).await }
    };
    // No title: the leading H1 becomes it, and the body does not repeat it.
    let r = new(json!({"type": "post", "body_markdown": "# Lifted title\n\nText.\n"})).await;
    let it = item(&r);
    assert_eq!(it["title"], "Lifted title", "{it}");
    assert!(
        !it["body_markdown"].as_str().unwrap().contains("Lifted"),
        "{it}"
    );
    assert!(
        it["body_markdown"].as_str().unwrap().contains("Text."),
        "{it}"
    );

    // The same H1 as the given title (trimmed, any case): dropped.
    let r = new(json!({"type": "post", "title": " Given Title ",
        "body_markdown": "# given title\n\nText.\n"}))
    .await;
    let it = item(&r);
    assert_eq!(it["title"].as_str().unwrap().trim(), "Given Title", "{it}");
    assert!(
        !it["body_markdown"].as_str().unwrap().contains("given"),
        "{it}"
    );

    // A different H1: the model is asked to move it.
    let r =
        new(json!({"type": "post", "title": "Given", "body_markdown": "# Other\n\nText.\n"})).await;
    let p = problem(&r);
    assert_eq!(p["status"], 422, "{p}");
    let msg = text(&r);
    assert!(
        msg.contains("title") && msg.contains("##") && msg.contains("Other"),
        "{msg}"
    );

    // Neither a title nor an H1.
    let r = new(json!({"type": "post", "body_markdown": "Just text.\n"})).await;
    assert_eq!(problem(&r)["errors"][0]["field"], "title", "{r}");

    // Updates follow the same rule: the H1 renames when no title is sent...
    let made = create(&api, &t, json!({"type": "post", "title": "Old"})).await;
    let id = made["id"].as_str().unwrap();
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 1,
            "body_markdown": "# New name\n\nBody.\n"}),
    )
    .await;
    assert_eq!(item(&r)["title"], "New name", "{r}");
    // ...and a different one next to a title is refused, writing nothing.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 2, "title": "Kept",
            "body_markdown": "# Different\n\nBody.\n"}),
    )
    .await;
    problem(&r);
    let g = tool(&api, &t.0, "content_get", json!({"type": "post", "id": id})).await;
    assert_eq!(item(&g)["title"], "New name", "{g}");
    assert_eq!(item(&g)["version"], 2, "{g}");

    // What the converter changed is reported.
    let r = new(json!({"type": "post", "title": "Notes",
        "body_markdown": "Text.\n\n![local](pictures/x.png)\n"}))
    .await;
    let notes = item(&r)["notes"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"));
    assert!(!notes.is_empty(), "{r}");
    assert!(
        text(&r).contains("Notes from the Markdown conversion"),
        "{r}"
    );
    api
}

async fn markdown_limits_and_media_checks(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let media = upload(&api, "ok").await;
    // Over 1 MiB of Markdown.
    let big = format!("{}\n", "a".repeat((1 << 20) + 1));
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Big", "body_markdown": big}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "body_markdown", "{r}");
    // Exactly 1 MiB passes.
    let fits = "a".repeat(1 << 20);
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Fits", "body_markdown": fits}),
    )
    .await;
    item(&r);

    // Markdown and blocks are exclusive.
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Both", "body_markdown": "x",
            "body_blocks": {"type": "doc", "content": []}}),
    )
    .await;
    let msg = text(&r);
    assert!(
        msg.contains("body_markdown") && msg.contains("body_blocks"),
        "{msg}"
    );
    problem(&r);

    // An image of media that does not exist is refused, naming the id.
    let ghost = Uuid::now_v7();
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Ghost",
            "body_markdown": format!("![x](ferrite:media/{ghost})\n\n![y](ferrite:media/{media})")}),
    )
    .await;
    problem(&r);
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    assert!(
        !text(&r).contains(&media),
        "only the missing id: {}",
        text(&r)
    );
    // The same for the featured image, and for an id that is not media.
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Ghost", "featured_image": ghost}),
    )
    .await;
    problem(&r);
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    let post = create(&api, &t, json!({"type": "post", "title": "A post"})).await;
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Not media", "featured_image": post["id"]}),
    )
    .await;
    problem(&r);
    assert!(
        text(&r).contains(post["id"].as_str().unwrap()),
        "{}",
        text(&r)
    );

    // Real media is accepted as inline and featured image.
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Pretty", "featured_image": media,
            "body_markdown": format!("![x](ferrite:media/{media})")}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["featured_image"], media, "{it}");
    assert_eq!(it["fields"]["featured_image"], media, "{it}");

    // `fields.featured_image` is checked like the argument, on create and on update.
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Ghost field", "fields": {"featured_image": ghost}}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "featured_image", "{r}");
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": post["id"], "version": 1,
            "fields": {"featured_image": ghost}}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "featured_image", "{r}");
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    // Real media inside `fields` is accepted and shows as the featured image.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": post["id"], "version": 1,
            "fields": {"featured_image": media}}),
    )
    .await;
    assert_eq!(item(&r)["featured_image"], media, "{r}");

    // A caller who cannot read media cannot use it either.
    let ed = api.user("narrow", Role::Editor, false).await;
    let narrow = api.bearer(&ed, &["content:read", "content:write"]).await;
    let r = tool(
        &api,
        &narrow.0,
        "content_create",
        json!({"type": "post", "title": "No media scope", "featured_image": media}),
    )
    .await;
    problem(&r);
    assert!(text(&r).contains(&media), "{}", text(&r));
    api
}

async fn blocks_in_blocks_out(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let ghost = Uuid::now_v7();
    let image =
        json!({"type": "image", "attrs": {"src": format!("ferrite:media/{ghost}"), "alt": "x"}});
    // A missing image in body_blocks is refused, naming the id and the argument sent.
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Ghost blocks",
            "body_blocks": {"type": "doc", "content": [image.clone()]}}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "body_blocks", "{r}");
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    // ...also 40 block levels down (deeper than the walk once looked).
    let mut nested = image.clone();
    for _ in 0..40 {
        nested = json!({"type": "blockquote", "content": [nested]});
    }
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Deep ghost",
            "body_blocks": {"type": "doc", "content": [nested]}}),
    )
    .await;
    problem(&r);
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    // The same for an image inside a ferrite-block fence of body_markdown.
    let fence = format!("```ferrite-block\n{image}\n```\n");
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Ghost fence", "body_markdown": fence}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "body_markdown", "{r}");
    assert!(text(&r).contains(&ghost.to_string()), "{}", text(&r));
    // And on update.
    let made = create(&api, &t, json!({"type": "post", "title": "Update ghost"})).await;
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": made["id"], "version": 1,
            "body_blocks": {"type": "doc", "content": [image]}}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "body_blocks", "{r}");
    let blocks = json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Exact"}]},
        {"type": "paragraph", "content": [{"type": "text", "text": "Control."}]}
    ]});
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Blocks", "body_blocks": blocks}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["body_markdown"], "## Exact\n\nControl.\n", "{it}");
    // Not a document: refused.
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Bad", "body_blocks": {"type": "paragraph"}}),
    )
    .await;
    problem(&r);
    // Updating with blocks replaces the body.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": it["id"], "version": 1, "body_blocks": {"type": "doc",
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Swapped"}]}]}}),
    )
    .await;
    assert_eq!(item(&r)["body_markdown"], "Swapped\n", "{r}");
    api
}

// ---- concurrency and roles ---------------------------------------------------------------

async fn stale_version_returns_current_item(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let made = create(
        &api,
        &t,
        json!({"type": "post", "title": "Draft", "body_markdown": "First."}),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 1, "title": "Newer title",
            "body_markdown": "Second."}),
    )
    .await;
    assert_eq!(item(&r)["version"], 2, "{r}");

    // A write on version 1 loses: nothing is written, the current item comes back.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 1, "title": "Stale title"}),
    )
    .await;
    let p = problem(&r);
    assert_eq!(p["status"], 412, "{p}");
    assert_eq!(p["current"]["version"], 2, "{p}");
    assert_eq!(p["current"]["title"], "Newer title", "{p}");
    assert_eq!(p["current"]["body_markdown"], "Second.\n", "{p}");
    // The text names the version and carries the item for clients that read only text.
    assert!(text(&r).contains("version 2"), "{}", text(&r));
    let all: String = r["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect();
    assert!(
        all.contains("Newer title") && all.contains("Second."),
        "{all}"
    );

    // The same for every transition.
    for (name, args) in [
        ("content_publish", json!({})),
        ("content_trash", json!({})),
        ("content_submit_for_review", json!({})),
        ("content_schedule", json!({"at": "2999-01-01T00:00:00Z"})),
    ] {
        let mut a = json!({"type": "post", "id": id, "version": 1});
        a.as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        let r = tool(&api, &t.0, name, a).await;
        assert_eq!(problem(&r)["status"], 412, "{name}: {r}");
        assert_eq!(problem(&r)["current"]["version"], 2, "{name}: {r}");
    }
    let g = tool(&api, &t.0, "content_get", json!({"type": "post", "id": id})).await;
    assert_eq!(item(&g)["title"], "Newer title", "{g}");
    assert_eq!(item(&g)["status"], "draft", "{g}");
    // Re-applying on the version returned works.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 2, "title": "Merged"}),
    )
    .await;
    assert_eq!(item(&r)["version"], 3, "{r}");
    api
}

async fn contributor_submits_but_cannot_publish(api: Api) -> Api {
    let cora = api.user("cora", Role::Contributor, false).await;
    let ct = api.bearer(&cora, &["*"]).await;
    let (_, et) = editor(&api).await;
    let made = create(
        &api,
        &ct,
        json!({"type": "post", "title": "Pitch", "body_markdown": "An idea."}),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    assert_eq!(made["allowed"]["publish"], false, "{made}");
    assert_eq!(made["allowed"]["submit"], true, "{made}");

    // Publishing is refused and says what is missing; nothing changed.
    let r = tool(
        &api,
        &ct.0,
        "content_publish",
        json!({"type": "post", "id": id, "version": 1}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    // It names the capability that is missing.
    assert!(
        text(&r).starts_with("Forbidden: missing publish"),
        "{}",
        text(&r)
    );

    let r = tool(
        &api,
        &ct.0,
        "content_submit_for_review",
        json!({"type": "post", "id": id, "version": 1}),
    )
    .await;
    assert_eq!(item(&r)["status"], "pending", "{r}");
    assert_eq!(item(&r)["version"], 2, "{r}");

    // The editor sees it pending and publishes it.
    let r = tool(
        &api,
        &et.0,
        "content_publish",
        json!({"type": "post", "id": id, "version": 2}),
    )
    .await;
    assert_eq!(item(&r)["status"], "published", "{r}");

    // A token without the write scope cannot create at all.
    let ro = api.bearer(&cora, &["content:read"]).await;
    let r = tool(
        &api,
        &ro.0,
        "content_create",
        json!({"type": "post", "title": "Nope"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    api
}

async fn schedule_in_future(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let made = create(&api, &t, json!({"type": "post", "title": "Later"})).await;
    let id = made["id"].as_str().unwrap();
    let at = (chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339();
    let r = tool(
        &api,
        &t.0,
        "content_schedule",
        json!({"type": "post", "id": id, "version": 1, "at": at}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["status"], "scheduled", "{it}");
    assert!(it["scheduled_at"].is_string(), "{it}");
    assert_eq!(it["version"], 2, "{it}");
    // Not public yet: the url is the preview link.
    assert_eq!(it["url"], format!("{SITE_URL}/_preview/{id}"), "{it}");
    assert_eq!(api.get("/later/").await.status, StatusCode::NOT_FOUND);

    // The past, and a malformed time, are refused.
    let other = create(&api, &t, json!({"type": "post", "title": "Past"})).await;
    let oid = other["id"].as_str().unwrap();
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let r = tool(
        &api,
        &t.0,
        "content_schedule",
        json!({"type": "post", "id": oid, "version": 1, "at": past}),
    )
    .await;
    problem(&r);
    let r = tool(
        &api,
        &t.0,
        "content_schedule",
        json!({"type": "post", "id": oid, "version": 1, "at": "tomorrow"}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "at", "{r}");
    api
}

async fn trash_and_restore(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let mut poller = api.poller().await;
    let made = create(&api, &t, json!({"type": "post", "title": "Doomed"})).await;
    let id = made["id"].as_str().unwrap();
    let p = tool(
        &api,
        &t.0,
        "content_publish",
        json!({"type": "post", "id": id, "version": 1}),
    )
    .await;
    assert_eq!(item(&p)["status"], "published", "{p}");
    poller.poll_once().await.unwrap();
    assert_eq!(api.get("/doomed/").await.status, StatusCode::OK);

    let r = tool(
        &api,
        &t.0,
        "content_trash",
        json!({"type": "post", "id": id, "version": 2}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["status"], "trash", "{it}");
    assert_eq!(it["version"], 3, "{it}");
    assert!(it["url"].is_null(), "a trashed item has no page: {it}");
    assert_eq!(it["allowed"]["restore"], true, "{it}");
    poller.poll_once().await.unwrap();
    assert_eq!(api.get("/doomed/").await.status, StatusCode::NOT_FOUND);
    // The list leaves it out unless asked.
    let l = tool(&api, &t.0, "content_list", json!({"type": "post"})).await;
    assert_eq!(item(&l)["items"].as_array().unwrap().len(), 0, "{l}");
    let l = tool(
        &api,
        &t.0,
        "content_list",
        json!({"type": "post", "status": ["trash"]}),
    )
    .await;
    assert_eq!(item(&l)["items"].as_array().unwrap().len(), 1, "{l}");

    let r = tool(
        &api,
        &t.0,
        "content_restore",
        json!({"type": "post", "id": id, "version": 3}),
    )
    .await;
    assert_eq!(item(&r)["status"], "draft", "{r}");
    assert_eq!(item(&r)["version"], 4, "{r}");
    // Restoring what is not trashed is an error, not a no-op.
    let r = tool(
        &api,
        &t.0,
        "content_restore",
        json!({"type": "post", "id": id, "version": 4}),
    )
    .await;
    problem(&r);
    api
}

// ---- revisions and previews ----------------------------------------------------------------

async fn revisions_and_restore_revision(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let made = create(
        &api,
        &t,
        json!({"type": "post", "title": "Versions", "body_markdown": "First draft."}),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 1, "body_markdown": "Second draft."}),
    )
    .await;
    assert_eq!(item(&r)["version"], 2, "{r}");
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 2, "title": "Versions 3",
            "body_markdown": "Third draft."}),
    )
    .await;
    assert_eq!(item(&r)["version"], 3, "{r}");

    let l = tool(
        &api,
        &t.0,
        "content_revisions",
        json!({"type": "post", "id": id}),
    )
    .await;
    let revs = item(&l)["items"].as_array().unwrap();
    assert!(revs.len() >= 3, "{l}");
    // Newest first, each with who and when, no bodies in the list.
    assert_eq!(revs[0]["title"], "Versions 3", "{l}");
    assert_eq!(revs[0]["author"]["login"], "eddie", "{l}");
    assert!(revs[0].get("body_markdown").is_none(), "{l}");
    let oldest = revs.last().unwrap()["id"].as_str().unwrap().to_owned();

    // One revision in full, as Markdown.
    let one = tool(
        &api,
        &t.0,
        "content_revisions",
        json!({"type": "post", "id": id, "revision": oldest}),
    )
    .await;
    assert_eq!(item(&one)["body_markdown"], "First draft.\n", "{one}");
    assert_eq!(item(&one)["title"], "Versions", "{one}");

    // Restoring it is a new version with the old text, kept in the history.
    let r = tool(
        &api,
        &t.0,
        "content_restore_revision",
        json!({"type": "post", "id": id, "revision": oldest, "version": 3}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["body_markdown"], "First draft.\n", "{it}");
    assert_eq!(it["title"], "Versions", "{it}");
    assert_eq!(it["version"], 4, "{it}");
    let l = tool(
        &api,
        &t.0,
        "content_revisions",
        json!({"type": "post", "id": id}),
    )
    .await;
    assert!(
        item(&l)["items"][0]["note"]
            .as_str()
            .is_some_and(|n| n.starts_with("restored from")),
        "{l}"
    );
    // A stale version is the 412 with the current item.
    let r = tool(
        &api,
        &t.0,
        "content_restore_revision",
        json!({"type": "post", "id": id, "revision": oldest, "version": 3}),
    )
    .await;
    assert_eq!(problem(&r)["current"]["version"], 4, "{r}");
    // A revision of nothing is not found.
    let r = tool(
        &api,
        &t.0,
        "content_revisions",
        json!({"type": "post", "id": id, "revision": Uuid::now_v7()}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    api
}

async fn preview_link_works_for_draft(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let made = create(
        &api,
        &t,
        json!({"type": "post", "title": "Secret draft", "body_markdown": "Not yet."}),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let r = tool(
        &api,
        &t.0,
        "content_preview_link",
        json!({"type": "post", "id": id}),
    )
    .await;
    let link = item(&r);
    let url = link["url"].as_str().unwrap();
    assert_eq!(url, format!("{SITE_URL}/_preview/{id}"), "{link}");
    // The preview needs the signed-in browser session: there is no expiry to name.
    assert!(link["expires_at"].is_null(), "{link}");
    assert_eq!(link["requires_sign_in"], true, "{link}");
    assert!(
        link["note"].as_str().is_some_and(|n| n.contains("sign")),
        "{link}"
    );

    let path = url.strip_prefix(SITE_URL).unwrap();
    // The editor, in a browser, sees the draft...
    let mut browser = api.logged_in("eddie").await;
    let page = browser.get(&api, path).await;
    assert_eq!(page.status, StatusCode::OK, "{page:?}");
    assert!(page.body.contains("Secret draft"), "{}", page.body);
    // ...nobody else does: not an anonymous visitor, not the bearer token.
    assert_eq!(api.get(path).await.status, StatusCode::NOT_FOUND);
    assert_eq!(t.get(&api, path).await.status, StatusCode::NOT_FOUND);

    // A trashed item has no preview.
    let trashed = tool(
        &api,
        &t.0,
        "content_trash",
        json!({"type": "post", "id": id, "version": 1}),
    )
    .await;
    item(&trashed);
    let r = tool(
        &api,
        &t.0,
        "content_preview_link",
        json!({"type": "post", "id": id}),
    )
    .await;
    problem(&r);
    assert!(text(&r).to_lowercase().contains("trash"), "{}", text(&r));

    // Someone who may not read the draft is told it does not exist.
    let ann = api.user("ann", Role::Author, false).await;
    let at = api.bearer(&ann, &["*"]).await;
    let other = create(&api, &t, json!({"type": "post", "title": "Editor draft"})).await;
    let r = tool(
        &api,
        &at.0,
        "content_preview_link",
        json!({"type": "post", "id": other["id"]}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    api
}

// ---- lists ---------------------------------------------------------------------------------

async fn list_filters_by_status_and_q(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let alpha = create(
        &api,
        &t,
        json!({"type": "post", "title": "Alpha apples", "body_markdown": "Red apples."}),
    )
    .await;
    let beta = create(
        &api,
        &t,
        json!({"type": "post", "title": "Beta bananas", "body_markdown": "Yellow."}),
    )
    .await;
    let gamma = create(
        &api,
        &t,
        json!({"type": "post", "title": "Gamma apples", "body_markdown": "Green apples."}),
    )
    .await;
    let page = create(&api, &t, json!({"type": "page", "title": "About"})).await;
    tool(
        &api,
        &t.0,
        "content_publish",
        json!({"type": "post", "id": beta["id"], "version": 1}),
    )
    .await;
    let ids = |r: &Value| -> Vec<String> {
        item(r)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect()
    };

    let all = tool(&api, &t.0, "content_list", json!({"type": "post"})).await;
    assert_eq!(ids(&all).len(), 3, "{all}");
    assert!(
        !ids(&all).contains(&page["id"].as_str().unwrap().to_owned()),
        "{all}"
    );
    // Summaries: no body, but what is needed to choose and then fetch.
    let first = &item(&all)["items"][0];
    for key in [
        "id", "type", "title", "slug", "status", "version", "excerpt", "author",
    ] {
        assert!(first.get(key).is_some(), "{key}: {first}");
    }
    assert!(
        first.get("body_markdown").is_none() && first.get("body").is_none(),
        "{first}"
    );
    assert!(
        first["admin_url"].as_str().unwrap().starts_with(SITE_URL),
        "{first}"
    );
    assert!(item(&all)["next_cursor"].is_null(), "{all}");

    let published = tool(
        &api,
        &t.0,
        "content_list",
        json!({"type": "post", "status": ["published"]}),
    )
    .await;
    assert_eq!(
        ids(&published),
        vec![beta["id"].as_str().unwrap().to_owned()],
        "{published}"
    );
    assert_eq!(
        item(&published)["items"][0]["url"],
        format!("{SITE_URL}/beta-bananas/"),
        "{published}"
    );
    let drafts = tool(
        &api,
        &t.0,
        "content_list",
        json!({"type": "post", "status": ["draft", "pending"]}),
    )
    .await;
    assert_eq!(ids(&drafts).len(), 2, "{drafts}");
    // No public page yet: the url is the preview link, and `public` says so.
    assert_eq!(item(&drafts)["items"][0]["public"], false, "{drafts}");
    assert!(
        item(&drafts)["items"][0]["url"]
            .as_str()
            .unwrap()
            .contains("/_preview/"),
        "{drafts}"
    );
    assert_eq!(item(&published)["items"][0]["public"], true, "{published}");

    // Full text.
    let q = tool(
        &api,
        &t.0,
        "content_list",
        json!({"type": "post", "q": "apples"}),
    )
    .await;
    let found = ids(&q);
    assert_eq!(found.len(), 2, "{q}");
    for want in [&alpha, &gamma] {
        assert!(
            found.contains(&want["id"].as_str().unwrap().to_owned()),
            "{q}"
        );
    }

    // Paging by cursor.
    let p1 = tool(
        &api,
        &t.0,
        "content_list",
        json!({"type": "post", "per_page": 2, "order": "title"}),
    )
    .await;
    assert_eq!(ids(&p1).len(), 2, "{p1}");
    let cursor = item(&p1)["next_cursor"].as_str().unwrap().to_owned();
    let p2 = tool(
        &api,
        &t.0,
        "content_list",
        json!({"type": "post", "per_page": 2, "order": "title", "cursor": cursor}),
    )
    .await;
    assert_eq!(
        ids(&p2),
        vec![gamma["id"].as_str().unwrap().to_owned()],
        "{p2}"
    );
    assert!(item(&p2)["next_cursor"].is_null(), "{p2}");

    // Bad input is a tool error that says which field.
    for (args, field) in [
        (json!({"type": "post", "per_page": 0}), "per_page"),
        (json!({"type": "post", "per_page": 101}), "per_page"),
        (json!({"type": "post", "status": ["nope"]}), "status"),
        (json!({"type": "post", "cursor": "garbage"}), "cursor"),
    ] {
        let r = tool(&api, &t.0, "content_list", args.clone()).await;
        let p = problem(&r);
        let named = p["errors"].to_string().contains(field) || text(&r).contains(field);
        assert!(named, "{args}: {r}");
    }
    let r = tool(&api, &t.0, "content_list", json!({"type": "nosuchtype"})).await;
    assert_eq!(problem(&r)["status"], 404, "{r}");

    // An author sees the published post and their own drafts only.
    let ann = api.user("ann", Role::Author, false).await;
    let at = api.bearer(&ann, &["*"]).await;
    let l = tool(&api, &at.0, "content_list", json!({"type": "post"})).await;
    assert_eq!(
        ids(&l),
        vec![beta["id"].as_str().unwrap().to_owned()],
        "{l}"
    );
    api
}

async fn update_merges_fields_and_sets_terms(api: Api) -> Api {
    let (ed, t) = editor(&api).await;
    let media = upload(&api, "feat").await;
    let news = api
        .api
        .taxonomy
        .create_term(
            &ferrite_core::Actor::new(ed.id, ed.role),
            "category",
            "News",
            None,
            None,
            "",
        )
        .await
        .unwrap();
    let made = create(
        &api,
        &t,
        json!({"type": "post", "title": "Fields", "fields": {"a": 1},
            "terms": [news.id], "seo": {"title": "SEO T", "description": "SEO D"}}),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    assert_eq!(made["fields"], json!({"a": 1}), "{made}");
    assert_eq!(made["terms"], json!([news.id]), "{made}");
    assert_eq!(made["seo"]["title"], "SEO T", "{made}");
    assert!(made["featured_image"].is_null(), "{made}");

    // Fields sent replace those members; the others stay, and so does the featured
    // image, which has its own argument.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 1, "fields": {"b": 2},
            "featured_image": media, "seo": {"description": null, "canonical": "https://example.org/x"}}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["fields"]["a"], 1, "{it}");
    assert_eq!(it["fields"]["b"], 2, "{it}");
    assert_eq!(it["featured_image"], media, "{it}");
    assert_eq!(it["terms"], json!([news.id]), "terms not sent: kept: {it}");
    assert_eq!(it["seo"]["title"], "SEO T", "{it}");
    assert!(it["seo"]["description"].is_null(), "{it}");
    assert_eq!(it["seo"]["canonical"], "https://example.org/x", "{it}");

    // null clears the featured image, the terms are replaced by [], the excerpt
    // is derived again.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 2, "featured_image": null,
            "terms": [], "excerpt": null, "body_markdown": "Derived excerpt text."}),
    )
    .await;
    let it = item(&r);
    assert!(it["featured_image"].is_null(), "{it}");
    assert_eq!(it["terms"], json!([]), "{it}");
    assert_eq!(it["excerpt"], "Derived excerpt text.", "{it}");
    assert_eq!(it["fields"]["b"], 2, "{it}");

    // A member sent as null is cleared.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 3, "fields": {"b": null}}),
    )
    .await;
    let it = item(&r);
    assert_eq!(it["fields"], json!({"a": 1}), "{it}");

    // Pages take a parent; null moves back to the top level.
    let parent = create(&api, &t, json!({"type": "page", "title": "Parent"})).await;
    let child = create(
        &api,
        &t,
        json!({"type": "page", "title": "Child", "parent": parent["id"]}),
    )
    .await;
    assert_eq!(child["parent"], parent["id"], "{child}");
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "page", "id": child["id"], "version": 1, "parent": null}),
    )
    .await;
    assert!(item(&r)["parent"].is_null(), "{r}");
    // Unknown members are refused, not ignored.
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 4, "status": "published"}),
    )
    .await;
    problem(&r);
    api
}

// ---- audit, resources, annotations ---------------------------------------------------------

async fn audit_rows_name_the_client(api: Api) -> Api {
    let (ed, t) = editor(&api).await;
    let made = create(&api, &t, json!({"type": "post", "title": "Audited"})).await;
    let id = made["id"].as_str().unwrap();
    let steps = [
        (
            "content_update",
            json!({"version": 1, "title": "Audited 2"}),
        ),
        ("content_publish", json!({"version": 2})),
        ("content_unpublish", json!({"version": 3})),
        ("content_trash", json!({"version": 4})),
        ("content_restore", json!({"version": 5})),
        (
            "content_schedule",
            json!({"version": 6, "at": "2999-01-01T00:00:00Z"}),
        ),
    ];
    for (name, extra) in steps {
        let mut args = json!({"type": "post", "id": id});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let r = tool(&api, &t.0, name, args).await;
        item(&r);
    }
    // Reads and refused writes are not audited.
    tool(&api, &t.0, "content_get", json!({"type": "post", "id": id})).await;
    tool(&api, &t.0, "content_list", json!({"type": "post"})).await;
    let stale = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": 1, "title": "Stale"}),
    )
    .await;
    problem(&stale);

    let rows = rows(&api).await;
    let mcp: Vec<_> = rows
        .iter()
        .filter(|r| r.action.starts_with("mcp."))
        .collect();
    let mut actions: Vec<&str> = mcp.iter().map(|r| r.action.as_str()).collect();
    actions.sort_unstable();
    assert_eq!(
        actions,
        vec![
            "mcp.content_create",
            "mcp.content_publish",
            "mcp.content_restore",
            "mcp.content_schedule",
            "mcp.content_trash",
            "mcp.content_unpublish",
            "mcp.content_update",
        ],
        "{mcp:?}"
    );
    for row in &mcp {
        assert_eq!(row.actor_id, Some(ed.id), "{row:?}");
        assert_eq!(row.target_kind, "content", "{row:?}");
        assert_eq!(row.target_id.as_deref(), Some(id), "{row:?}");
        assert_eq!(row.details["client"], "API token test", "{row:?}");
        assert_eq!(row.details["type"], "post", "{row:?}");
    }
    api
}

async fn content_resource_reads_an_item(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let made = create(
        &api,
        &t,
        json!({"type": "post", "title": "Resource \"quoted\"", "body_markdown": "Body **text**."}),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let uri = format!("ferrite://content/post/{id}");

    for version in [V_LEGACY, V_CURRENT] {
        let (status, body) =
            mcp_call_v(&api, &t.0, version, "resources/read", json!({"uri": uri})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let c = &body["result"]["contents"][0];
        assert_eq!(c["uri"], uri, "{body}");
        assert_eq!(c["mimeType"], "text/markdown", "{body}");
        let md = c["text"].as_str().unwrap();
        // TOML front matter between `+++` lines, as the Markdown importer reads it.
        assert!(md.starts_with("+++\n"), "{md}");
        let (front, rest) = md[4..]
            .split_once("\n+++\n")
            .unwrap_or_else(|| panic!("{md}"));
        let meta: toml::Table = front.parse().unwrap_or_else(|e| panic!("{e}: {front}"));
        assert_eq!(meta["id"].as_str(), Some(id), "{front}");
        assert_eq!(meta["type"].as_str(), Some("post"), "{front}");
        assert_eq!(
            meta["title"].as_str(),
            Some("Resource \"quoted\""),
            "{front}"
        );
        assert_eq!(meta["status"].as_str(), Some("draft"), "{front}");
        assert_eq!(meta["version"].as_integer(), Some(1), "{front}");
        assert_eq!(meta["public"].as_bool(), Some(false), "{front}");
        assert_eq!(rest.trim(), "Body **text**.", "{md}");
    }

    // The template is listed.
    let (status, body) = mcp_call(&api, &t.0, "resources/templates/list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let templates = body["result"]["resourceTemplates"].as_array().unwrap();
    assert!(
        templates
            .iter()
            .any(|t| t["uriTemplate"] == "ferrite://content/{type}/{id}"),
        "{body}"
    );

    // Missing, wrong type, malformed and unreadable all read as not found.
    let ann = api.user("ann", Role::Author, false).await;
    let at = api.bearer(&ann, &["*"]).await;
    for (token, uri) in [
        (&t.0, format!("ferrite://content/post/{}", Uuid::now_v7())),
        (&t.0, format!("ferrite://content/page/{id}")),
        (&t.0, "ferrite://content/post/not-a-uuid".to_owned()),
        (&t.0, "ferrite://content/post".to_owned()),
        (&at.0, uri.clone()),
    ] {
        // The code is -32002 up to 2025-11-25 and -32602 from 2026-07-28 (SEP-2164).
        for (version, code) in [(V_LEGACY, -32002), (V_CURRENT, -32602)] {
            let (_, body) =
                mcp_call_v(&api, token, version, "resources/read", json!({"uri": uri})).await;
            assert_eq!(body["error"]["code"], code, "{uri}: {body}");
            let msg = body["error"]["message"].as_str().unwrap();
            assert!(msg.starts_with("no resource"), "{uri}: {body}");
        }
    }
    api
}

async fn content_tools_are_annotated(api: Api) -> Api {
    let (_, t) = editor(&api).await;
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
        "content_list",
        "content_get",
        "content_revisions",
        "content_preview_link",
    ];
    let safe_writes = [
        "content_create",
        "content_update",
        "content_publish",
        "content_schedule",
        "content_submit_for_review",
        "content_restore",
        "content_restore_revision",
    ];
    let destructive = ["content_unpublish", "content_trash"];
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
    }
    assert_eq!(by("content_create")["annotations"]["idempotentHint"], false);
    assert_eq!(by("content_restore")["annotations"]["idempotentHint"], true);
    // Written for a model: a paragraph, the Markdown conventions, the guide.
    for name in ["content_create", "content_update"] {
        let t = by(name);
        let d = t["description"].as_str().unwrap();
        for needle in [
            "ferrite:media",
            "ferrite-block",
            "ferrite://guide/writing",
            "H1",
        ] {
            assert!(d.contains(needle), "{name}: {needle}: {d}");
        }
        let props = &t["inputSchema"]["properties"];
        for key in [
            "type",
            "body_markdown",
            "body_blocks",
            "fields",
            "terms",
            "seo",
            "title",
        ] {
            assert!(props.get(key).is_some(), "{name}: {key}");
            assert!(
                props[key]["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{name}.{key} has no description: {}",
                props[key]
            );
        }
        assert_eq!(t["inputSchema"]["additionalProperties"], false, "{name}");
    }
    for name in safe_writes.iter().chain(&destructive) {
        if *name == "content_create" {
            continue;
        }
        let req = by(name)["inputSchema"]["required"].to_string();
        assert!(
            req.contains("version") && req.contains("\"id\""),
            "{name}: {req}"
        );
    }
    api
}

async fn other_types_and_missing_items(api: Api) -> Api {
    let (_, t) = editor(&api).await;
    let made = create(&api, &t, json!({"type": "post", "title": "A post"})).await;
    let id = made["id"].as_str().unwrap();
    // A page id under /post is not found; media is not served here; unknown types too.
    for (name, args) in [
        ("content_get", json!({"type": "page", "id": id})),
        ("content_get", json!({"type": "media", "id": id})),
        ("content_get", json!({"type": "post", "id": Uuid::now_v7()})),
        ("content_create", json!({"type": "media", "title": "x"})),
        (
            "content_create",
            json!({"type": "nosuchtype", "title": "x"}),
        ),
        (
            "content_publish",
            json!({"type": "page", "id": id, "version": 1}),
        ),
    ] {
        let r = tool(&api, &t.0, name, args.clone()).await;
        assert_eq!(problem(&r)["status"], 404, "{name} {args}: {r}");
    }
    // A malformed id is a tool error from the schema, not a protocol error.
    let (status, body) = mcp_call(
        &api,
        &t.0,
        "tools/call",
        json!({"name": "content_get", "arguments": {"type": "post", "id": "nope"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["isError"], true, "{body}");
    // Reading needs content:read; the error names it.
    let ann = api.user("ann", Role::Author, false).await;
    let none = api.bearer(&ann, &["media:read"]).await;
    let r = tool(&api, &none.0, "content_list", json!({"type": "post"})).await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    assert!(text(&r).contains("content:read"), "{}", text(&r));
    api
}
