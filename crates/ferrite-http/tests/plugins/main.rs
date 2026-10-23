//! Plugins in page rendering on both profiles (plan ruling P3): loading from rows +
//! artifacts, the store-backed services (published-only `content.read`, Review Focus
//! 1), the rendered head, content filters, and the per-call trace (05 §7a).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod reload;

use common::{HOUR, SITE_URL, Site};
use ferrite_core::{
    PageItem,
    events::{archive_key, option_key, post_key, user_key},
};
use ferrite_domain::{ContentId, ContentStatus, Role};
use ferrite_http::{HeadPage, HeadSite, PluginRequest, RenderTrace, filter_item, page_head};
use ferrite_plugin_host::{HostServices, dep_keys};
use serde_json::json;

const SITE: HeadSite<'static> = HeadSite {
    title: "Test & Site",
    url: SITE_URL,
    description: "Tagline",
};

async fn public_item(site: &Site, id: ContentId) -> PageItem {
    site.reader.by_id(id).await.unwrap().value.unwrap()
}

async fn head(site: &Site, item: Option<&PageItem>, req: &PluginRequest) -> (String, RenderTrace) {
    let mut trace = RenderTrace::default();
    let path = item.map_or("/", |i| i.url.as_str());
    let html = page_head(
        &site.host,
        site.store.as_ref(),
        SITE,
        HeadPage { item, path },
        req,
        &mut trace,
    )
    .await;
    (html, trace)
}

fn req_for(item: &PageItem) -> PluginRequest {
    PluginRequest::public(&item.url, "en", Some(item.item.id))
}

fn meta_tag(name: &str, content: &str) -> String {
    format!("<meta name=\"{name}\" content=\"{content}\">")
}

async fn hello_head_from_rows_and_artifact(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello <World>",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    site.install("hello").await;
    let summary = site.load().await;
    assert_eq!(summary.loaded, ["hello"]);
    assert!(summary.skipped.is_empty(), "{:?}", summary.skipped);

    let item = public_item(&site, id).await;
    let (html, trace) = head(&site, Some(&item), &req_for(&item)).await;
    assert!(
        html.contains(&meta_tag("generator", "ferrite-hello")),
        "{html}"
    );
    assert!(
        html.starts_with("<title>Hello &lt;World&gt; – Test &amp; Site</title>\n"),
        "{html}"
    );
    assert!(html.contains("<link rel=\"canonical\" href=\"https://example.com/hello/\">"));
    assert!(
        html.contains(&meta_tag("description", "Excerpt of hello.")),
        "{html}"
    );
    assert!(!trace.volatile);
    site
}

async fn hostile_plugins_do_not_break_the_page(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    for name in ["panic", "big-output", "hello"] {
        site.install(name).await;
    }
    // Rows that cannot load: artifact row without a blob, a manifest naming another
    // plugin, invalid manifest, and a disabled row (ignored, not skipped).
    let hello_hash = site.put_artifact(common::plugin_wasm("hello")).await;
    let orphan = site.put_artifact(b"not wasm".to_vec()).await;
    site.row(
        "garbage",
        &common::plugin_manifest("hello").replace("hello", "garbage"),
        orphan,
        true,
    )
    .await;
    site.row(
        "imposter",
        &common::plugin_manifest("hello"),
        hello_hash.clone(),
        true,
    )
    .await;
    site.row(
        "bad-manifest",
        "[plugin]\nname = \"bad-manifest\"\n",
        hello_hash.clone(),
        true,
    )
    .await;
    site.row(
        "off",
        &common::plugin_manifest("hello").replace("\"hello\"", "\"off\""),
        hello_hash,
        false,
    )
    .await;
    let summary = site.load().await;
    assert_eq!(summary.loaded, ["big-output", "hello", "panic"]);
    let skipped: Vec<&str> = summary.skipped.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(skipped, ["bad-manifest", "garbage", "imposter"]);

    let mut item = public_item(&site, id).await;
    let original_html = item.html.clone();
    let req = req_for(&item);
    let mut trace = RenderTrace::default();
    filter_item(&site.host, &mut item, &req, &mut trace).await;
    assert_eq!(
        item.html, original_html,
        "big-output's 3 MiB body was rejected"
    );
    let (html, head_trace) = head(&site, Some(&item), &req).await;
    assert!(
        html.starts_with("<title>Hello – Test &amp; Site</title>"),
        "{html}"
    );
    assert!(html.contains("<link rel=\"canonical\""));
    assert!(
        html.contains(&meta_tag("generator", "ferrite-hello")),
        "{html}"
    );
    // Failed calls may be transient: the page gets the short TTL.
    assert!(trace.volatile && head_trace.volatile);
    site
}

async fn loader_rejects_tampered_missing_and_ungranted(site: Site) -> Site {
    // Tampered blob: the bytes no longer hash to the row's artifact hash.
    site.install("hello").await;
    let hello = site
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    site.tamper(&hello.artifact_hash, b"\0asm tampered");
    // Missing blob: the artifacts row exists, the object does not.
    let missing = ferrite_domain::ArtifactHash::parse(&"ab".repeat(32)).unwrap();
    site.store
        .read()
        .artifacts()
        .insert_if_absent(&ferrite_domain::Artifact {
            hash: missing.clone(),
            kind: ferrite_domain::ArtifactKind::Plugin,
            bytes: 10,
            mime: "application/wasm".into(),
            created_at: ferrite_domain::Timestamp::now(),
        })
        .await
        .unwrap();
    site.row(
        "order-a",
        &common::plugin_manifest("order-a"),
        missing,
        true,
    )
    .await;
    // Grants (R21): reader within its grant loads, kv-budget asking for kv does not.
    let reader = site.put_artifact(common::plugin_wasm("reader")).await;
    site.row_granted(
        "reader",
        &common::plugin_manifest("reader"),
        reader,
        true,
        json!({
            "content": { "read": ["*"], "meta": { "read": ["seo.*"] } },
            "options": { "read": ["site.*"] },
            "render": { "head": true },
        }),
    )
    .await;
    let kv = site.put_artifact(common::plugin_wasm("kv-budget")).await;
    site.row_granted(
        "kv-budget",
        &common::plugin_manifest("kv-budget"),
        kv,
        true,
        json!({ "render": { "head": true } }),
    )
    .await;

    let summary = site.load().await;
    assert_eq!(summary.loaded, ["reader"]);
    let reasons: Vec<(&str, &str)> = summary
        .skipped
        .iter()
        .map(|(n, r)| (n.as_str(), r.as_str()))
        .collect();
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(
        reasons[0].0 == "hello" && reasons[0].1.contains("does not match"),
        "{reasons:?}"
    );
    assert!(
        reasons[1].0 == "kv-budget" && reasons[1].1.contains("beyond the approved grant"),
        "{reasons:?}"
    );
    assert!(
        reasons[2].0 == "order-a" && reasons[2].1.contains("artifact"),
        "{reasons:?}"
    );
    assert!(!site.host.is_enabled("hello"));
    site
}

/// Hidden statuses, each with a publish time that would otherwise make it public.
const HIDDEN: [(&str, ContentStatus, i64); 6] = [
    ("draft", ContentStatus::Draft, -HOUR),
    ("pending", ContentStatus::Pending, -HOUR),
    ("scheduled", ContentStatus::Scheduled, HOUR),
    ("private", ContentStatus::Private, -HOUR),
    ("trash", ContentStatus::Trash, -HOUR),
    ("future", ContentStatus::Published, HOUR),
];

fn meta_value<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let start = format!("<meta name=\"{name}\" content=\"");
    let rest = &html[html.find(&start)? + start.len()..];
    rest.split('"').next()
}

async fn content_read_never_returns_drafts(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    let visible = site
        .post(
            &author,
            "visible",
            "Visible",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    let mut hidden = Vec::new();
    for (slug, status, at) in HIDDEN {
        let id = site.post(&author, slug, slug, status, Some(at)).await;
        site.store
            .read()
            .meta()
            .set(id, "seo.description", &json!("secret"))
            .await
            .unwrap();
        hidden.push((slug, id));
    }
    site.install("reader").await;
    site.load().await;

    for (slug, id) in &hidden {
        // A request naming a hidden item (not a preview) sees nothing of it.
        let req = PluginRequest::public("/x/", "en", Some(*id));
        let (html, _) = head(&site, None, &req).await;
        assert_eq!(
            meta_value(&html, "reader-current"),
            Some("-"),
            "{slug}: {html}"
        );
        assert_eq!(
            meta_value(&html, "reader-list"),
            Some("visible"),
            "{slug}: {html}"
        );
        let s = site.services.as_ref();
        assert!(
            s.get_post(&id.to_string(), &|_| true).await.is_none(),
            "{slug}"
        );
        assert!(s.get_post_by_slug("post", slug).await.is_none(), "{slug}");
        assert!(
            s.meta_get(&id.to_string(), "seo.description")
                .await
                .is_none(),
            "{slug}"
        );
    }
    let posts = site.services.list_posts("post", 100, 0).await;
    let slugs: Vec<&str> = posts.iter().map(|p| p.slug.as_str()).collect();
    assert_eq!(slugs, ["visible"]);
    assert_eq!(posts[0].id, visible.to_string());
    assert_eq!(posts[0].status, "published");
    assert_eq!(posts[0].url, "https://example.com/visible/");
    assert!(
        site.services
            .get_post_by_slug("post", "visible")
            .await
            .is_some()
    );
    // The type grant is applied before loading (refused reads cost one lookup).
    let v = visible.to_string();
    assert!(site.services.get_post(&v, &|k| k == "page").await.is_none());
    assert!(site.services.get_post(&v, &|k| k == "post").await.is_some());
    site
}

async fn preview_shows_only_the_previewed_item(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    site.post(
        &author,
        "visible",
        "Visible",
        ContentStatus::Published,
        Some(-HOUR),
    )
    .await;
    let draft = site
        .post(
            &author,
            "wip",
            "Work in progress",
            ContentStatus::Draft,
            None,
        )
        .await;
    let other = site
        .post(&author, "other-draft", "Other", ContentStatus::Draft, None)
        .await;
    site.install("reader").await;
    site.load().await;

    let previewed = site
        .reader
        .preview(&Site::actor(&author), draft)
        .await
        .unwrap()
        .value;
    assert_eq!(previewed.status(), ContentStatus::Draft);
    let item = previewed.item().clone();
    let req = PluginRequest::preview("/wip/", "en", previewed);
    let (html, _) = head(&site, Some(&item), &req).await;
    assert_eq!(
        meta_value(&html, "reader-current"),
        Some("Work in progress"),
        "{html}"
    );
    assert_eq!(meta_value(&html, "reader-list"), Some("visible"), "{html}");
    // Outside the preview scope the draft is hidden again, and so is any other draft.
    assert!(
        site.services
            .get_post(&draft.to_string(), &|_| true)
            .await
            .is_none()
    );
    let mut req_other = req.clone();
    req_other.info.post = Some(other.to_string());
    let (html, _) = head(&site, None, &req_other).await;
    assert_eq!(meta_value(&html, "reader-current"), Some("-"), "{html}");
    site
}

async fn plugin_reads_reach_the_trace(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    site.store
        .read()
        .meta()
        .set(id, "seo.description", &json!("SEO text"))
        .await
        .unwrap();
    site.store
        .read()
        .options()
        .set("site.title", &json!("Site"), false)
        .await
        .unwrap();
    site.install("reader").await;
    site.load().await;

    let item = public_item(&site, id).await;
    let (html, trace) = head(&site, Some(&item), &req_for(&item)).await;
    assert_eq!(meta_value(&html, "reader-current"), Some("Hello"));
    assert_eq!(
        meta_value(&html, "reader-meta"),
        Some("&quot;SEO text&quot;"),
        "{html}"
    );
    assert_eq!(meta_value(&html, "reader-option"), Some("&quot;Site&quot;"));
    // Display name only: never the login or e-mail.
    assert_eq!(meta_value(&html, "reader-author"), Some("ada display"));
    assert_eq!(
        meta_value(&html, "description"),
        Some("SEO text"),
        "core uses seo meta"
    );
    for key in [
        post_key(id),
        archive_key("post"),
        option_key("site.title"),
        // From SiteReader through the request scope, not from the plugin host.
        user_key(author.id),
    ] {
        assert!(trace.deps.contains(&key), "{key} missing: {:?}", trace.deps);
    }
    assert!(!trace.volatile);
    site
}

async fn kv_use_is_volatile(site: Site) -> Site {
    site.install("kv-budget").await;
    site.load().await;
    let (html, trace) = head(&site, None, &PluginRequest::public("/", "en", None)).await;
    // The point is volatility: kv use marks the page volatile, and so does a call cut
    // off by the render deadline (R19). How many of the 100 writes land before the
    // deadline depends on machine load, so only a successful run checks the count
    // (the per-call kv budget itself is tested in ferrite-plugin-host).
    assert!(trace.volatile);
    let usage = site
        .store
        .read()
        .plugin_kv()
        .usage("kv-budget")
        .await
        .unwrap();
    assert!(usage.0 >= 1, "at least one kv write landed: {usage:?}");
    if let Some(ok) = meta_value(&html, "kv-ok") {
        assert_eq!(ok, "100", "{html}");
        assert_eq!(usage, (100, 100), "100 one-byte values");
    }
    site
}

async fn sanitised_content_filters(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    site.install("html-inject").await;
    site.load().await;
    let mut item = public_item(&site, id).await;
    let before = item.html.clone();
    let mut trace = RenderTrace::default();
    let req = req_for(&item);
    filter_item(&site.host, &mut item, &req, &mut trace).await;
    assert!(item.html.starts_with(&before), "{}", item.html);
    for bad in ["<script", "onerror", "javascript:"] {
        assert!(!item.html.contains(bad), "{bad}: {}", item.html);
    }
    assert!(item.html.contains("rel=\"noopener noreferrer\""));
    assert_eq!(item.item.excerpt, "Excerpt of hello. [filtered]");
    assert!(!trace.volatile);
    site
}

async fn services_kv_settings_options_meta(site: Site) -> Site {
    let s = site.services.as_ref();
    // kv quota: 10 MiB of values per plugin.
    assert!(
        s.kv_set("p", "big", vec![0; 10 * 1024 * 1024 + 1])
            .await
            .is_err()
    );
    assert_eq!(s.kv_set("p", "k1", b"v".to_vec()).await, Ok(()));
    assert_eq!(s.kv_get("p", "k1").await, Some(b"v".to_vec()));
    assert_eq!(s.kv_get("other", "k1").await, None);
    assert_eq!(s.kv_list("p", "k", 10).await, ["k1"]);
    s.kv_delete("p", "k1").await;
    assert_eq!(s.kv_get("p", "k1").await, None);

    let options = site.store.read().options();
    options
        .set("plugin.seo.settings.key", &json!("v"), false)
        .await
        .unwrap();
    options.set("site.title", &json!("T"), false).await.unwrap();
    assert_eq!(s.setting_get("seo", "key").await.as_deref(), Some("\"v\""));
    assert_eq!(s.setting_get("other", "key").await, None);
    assert_eq!(s.option_get("site.title").await.as_deref(), Some("\"T\""));
    assert_eq!(
        s.option_get("plugin.seo.settings.key").await,
        None,
        "settings stay private"
    );

    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    let before = site.store.read().events().max_seq().await.unwrap();
    assert_eq!(
        s.meta_set(&id.to_string(), "seo.title", "\"T\"").await,
        Ok(())
    );
    assert_eq!(
        site.store.read().meta().get(id, "seo.title").await.unwrap(),
        Some(json!("T"))
    );
    assert_eq!(
        s.meta_get(&id.to_string(), "seo.title").await.as_deref(),
        Some("\"T\"")
    );
    let events = site.store.read().events().since(before, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "invalidate");
    assert_eq!(events[0].keys, [post_key(id)]);
    assert!(
        s.meta_set(&id.to_string(), "seo.title", "not json")
            .await
            .is_err()
    );
    assert!(
        s.meta_set(&ContentId::new().to_string(), "seo.title", "1")
            .await
            .is_err()
    );
    assert!(s.meta_set("not-a-uuid", "seo.title", "1").await.is_err());
    site
}

/// A store error that a plugin read swallows (the plugin still succeeds) marks the
/// page volatile, so a transient failure is not cached for long (final review I2).
async fn plugin_read_errors_are_volatile(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    site.post(
        &author,
        "hello",
        "Hello",
        ContentStatus::Published,
        Some(-HOUR),
    )
    .await;
    site.install("reader").await;
    site.load().await;
    let req = PluginRequest::public("/", "en", None);
    let (_, trace) = head(&site, None, &req).await;
    assert!(!trace.volatile);

    site.store.close().await;
    // No item: only the plugin's `list_posts` and `options.get` touch the store.
    let (html, trace) = head(&site, None, &req).await;
    assert_eq!(meta_value(&html, "reader-list"), Some(""), "{html}");
    assert!(trace.volatile);
    site
}

/// A failed `seo.description` read falls back to the excerpt and marks the page
/// volatile (final review I2).
async fn seo_read_error_is_volatile(site: Site) -> Site {
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    site.install("hello").await;
    site.load().await;
    let item = public_item(&site, id).await;
    let (_, trace) = head(&site, Some(&item), &req_for(&item)).await;
    assert!(!trace.volatile);

    site.store.close().await;
    let (html, trace) = head(&site, Some(&item), &req_for(&item)).await;
    assert!(
        html.contains(&meta_tag("description", "Excerpt of hello.")),
        "{html}"
    );
    assert!(trace.volatile);
    site
}

profile_tests!(
    hello_head_from_rows_and_artifact,
    hostile_plugins_do_not_break_the_page,
    loader_rejects_tampered_missing_and_ungranted,
    content_read_never_returns_drafts,
    preview_shows_only_the_previewed_item,
    plugin_reads_reach_the_trace,
    kv_use_is_volatile,
    sanitised_content_filters,
    services_kv_settings_options_meta,
    plugin_read_errors_are_volatile,
    seo_read_error_is_volatile,
);

#[test]
fn dep_key_formats_match_core_grammar() {
    let id = ContentId::new();
    assert_eq!(dep_keys::post(&id.to_string()), post_key(id));
    assert_eq!(dep_keys::archive("page"), archive_key("page"));
    assert_eq!(dep_keys::option("site.title"), option_key("site.title"));
}
