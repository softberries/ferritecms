//! Plugins in page rendering on both profiles (plan ruling P3): loading from rows +
//! artifacts, the store-backed services (published-only `content.read`, Review Focus
//! 1), the rendered head, content filters, and the per-call trace (05 §7a).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod analytics;
mod common;
mod contact_form;
mod egress;
mod install;
mod mail;
mod reload;
mod seo;
mod settings;

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

/// `site` with a host whose render deadline is 5 s, for tests that are not about
/// deadlines but assert a call's success (or its trace): a slow first store read under
/// a loaded full test run must not turn into a deadline failure (final review I3).
/// Call it before installing or loading plugins.
fn with_generous_deadlines(mut site: Site) -> Site {
    let config = ferrite_plugin_host::HostConfig {
        render_deadline: std::time::Duration::from_secs(5),
        ..ferrite_plugin_host::HostConfig::default()
    };
    site.host = std::sync::Arc::new(
        ferrite_plugin_host::PluginHost::new(config, site.services.clone()).expect("host"),
    );
    site
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
            orphaned_at: None,
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
    let site = with_generous_deadlines(site);
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
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
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
    // Core's markup a sanitising filter must keep: heading ids (P54) and code blocks
    // (R-b17).
    let body = json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 2},
         "content": [{"type": "text", "text": "Install the binary"}]},
        {"type": "paragraph", "content": [{"type": "text", "text": "Body of hello"}]},
        {"type": "heading", "attrs": {"level": 3},
         "content": [{"type": "text", "text": "Content"}]},
        {"type": "codeBlock", "attrs": {"language": "rust"},
         "content": [{"type": "text", "text": "fn main() {}"}]}
    ]});
    let id = site
        .post_with_body(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
            body,
        )
        .await;
    site.install("html-inject").await;
    site.load().await;
    let mut item = public_item(&site, id).await;
    let before = item.html.clone();
    assert!(
        before.contains("<h2 id=\"install-the-binary\">")
            && before.contains("<h3 id=\"content-1\">")
            && before.contains("<pre tabindex=\"0\"><code class=\"language-rust\">"),
        "{before}"
    );
    let toc: Vec<&str> = item.toc.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(toc, ["install-the-binary", "content-1"]);
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
    let text = ferrite_plugin_host::SettingSpec {
        kind: ferrite_plugin_host::SettingKind::Text,
        label: "Key".into(),
        default: None,
        options: Vec::new(),
    };
    assert_eq!(
        s.setting_get("seo", "key", &text).await.as_deref(),
        Some("\"v\"")
    );
    assert_eq!(s.setting_get("other", "key", &text).await, None);
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

/// Ruling X1: `options.get` answers `site.url` (the configured URL, no trailing slash)
/// and `site.title` (the option when it is a non-blank string, else the configured
/// title) as JSON strings; settings stay private.
async fn site_options_come_from_the_configuration(site: Site) -> Site {
    let s = ferrite_http::StoreHostServices::new(
        site.store.clone(),
        site.reader.clone(),
        "https://example.com/",
    )
    .with_site_title("Configured \"Title\"".to_owned());
    assert_eq!(
        s.option_get("site.url").await.as_deref(),
        Some("\"https://example.com\"")
    );
    // No option stored: the configured title, JSON-escaped.
    assert_eq!(
        s.option_get("site.title").await.as_deref(),
        Some("\"Configured \\\"Title\\\"\"")
    );
    let options = site.store.read().options();
    options
        .set("site.title", &json!("Stored"), false)
        .await
        .unwrap();
    assert_eq!(
        s.option_get("site.title").await.as_deref(),
        Some("\"Stored\"")
    );
    // A blank or non-string option falls back to the configured title
    // (`settings::site_title`; a theme would show a blank string as it is).
    for unset in [json!("  "), json!(7)] {
        options.set("site.title", &unset, false).await.unwrap();
        assert_eq!(
            s.option_get("site.title").await.as_deref(),
            Some("\"Configured \\\"Title\\\"\""),
            "{unset}"
        );
    }
    // Other keys are read as before; `site.url` is never an option row.
    options
        .set("site.url", &json!("https://evil.test"), false)
        .await
        .unwrap();
    assert_eq!(
        s.option_get("site.url").await.as_deref(),
        Some("\"https://example.com\"")
    );
    assert_eq!(s.option_get("site.description").await, None);
    site
}

/// P23 + Task 11 carry: `post_kind` answers the type of any item whatever its status
/// (the host's type check), while `meta.set`, like `meta.get`, reaches only items the
/// plugin could see (published), so an action cannot annotate a draft.
async fn meta_writes_reach_visible_items_only(site: Site) -> Site {
    let s = site.services.as_ref();
    let author = site.user("ada", Role::Author).await;
    let public = site
        .post(&author, "pub", "Pub", ContentStatus::Published, Some(-HOUR))
        .await;
    let draft = site
        .post(&author, "draft", "Draft", ContentStatus::Draft, None)
        .await;
    assert_eq!(
        s.post_kind(&public.to_string()).await.as_deref(),
        Some("post")
    );
    assert_eq!(
        s.post_kind(&draft.to_string()).await.as_deref(),
        Some("post")
    );
    assert_eq!(s.post_kind(&ContentId::new().to_string()).await, None);
    assert_eq!(s.post_kind("not-a-uuid").await, None);

    let before = site.store.read().events().max_seq().await.unwrap();
    assert_eq!(
        s.meta_set(&draft.to_string(), "seo.title", "\"T\"").await,
        Err("no such post".to_owned())
    );
    assert_eq!(
        site.store
            .read()
            .meta()
            .get(draft, "seo.title")
            .await
            .unwrap(),
        None
    );
    assert!(
        site.store
            .read()
            .events()
            .since(before, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.meta_set(&public.to_string(), "seo.title", "\"T\"").await,
        Ok(())
    );
    site
}

/// Cancel-safety: a `meta.set` whose future is dropped at any point (the host's hard
/// deadline, a client disconnect) either wrote the value and its `invalidate` event
/// or neither (one short `Tx`; a dropped `Tx` rolls back).
///
/// Deterministic (final review I2): attempt `k` polls the write by hand at most `k`
/// times and then drops it, for `k = 1, 2, …` until a write completes, so every await
/// point is a drop point once. Between polls the test sleeps briefly so the store's
/// worker can make progress; that only decides how far a poll gets, never whether the
/// write is dropped.
///
/// A dropped write whose `COMMIT` was already sent may land later (on Postgres nothing
/// orders it before the test's reads), even during a later attempt, and its event cannot
/// be told from another attempt's. So the check is cumulative over every attempt since
/// the start: `S` = `seo.k*` values stored, `E` = events since the start. Each sample
/// reads `S`, then `E`, then `S` again; with all-or-nothing commits that always gives
/// `S1 <= E <= S2`, whatever lands in between (no half-done state is observable). After
/// each attempt the test waits (bounded) for `S == E`; a value without its event, or an
/// event without its value, never converges.
async fn dropped_meta_write_is_all_or_nothing(site: Site) -> Site {
    use std::{future::Future, task::Poll};

    let s = site.services.clone();
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
    let subject = id.to_string();
    let start = site.store.read().events().max_seq().await.unwrap();
    let stored = async || {
        let meta = site.store.read().meta().list(id).await.unwrap();
        meta.iter().filter(|(k, _)| k.starts_with("seo.k")).count()
    };
    // At most one event per attempt, and attempts stop at 1000 (= `MAX_EVENT_BATCH`).
    let events = async || site.store.read().events().since(start, 1000).await.unwrap();
    let (mut completed, mut dropped) = (false, 0);
    for k in 1..=1_000usize {
        let key = format!("seo.k{k}");
        let mut write = Box::pin(s.meta_set(&subject, &key, "1"));
        let mut result = None;
        for _ in 0..k {
            match std::future::poll_fn(|cx| Poll::Ready(write.as_mut().poll(cx))).await {
                Poll::Ready(r) => {
                    result = Some(r);
                    break;
                }
                Poll::Pending => tokio::time::sleep(std::time::Duration::from_millis(1)).await,
            }
        }
        drop(write);
        // Converge: every write so far has landed whole or not at all.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let s1 = stored().await;
            let e = events().await.len();
            let s2 = stored().await;
            assert!(
                s1 <= e && e <= s2,
                "k {k}: half-done write ({s1} <= {e} <= {s2})"
            );
            if s1 == e && e == s2 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "k {k}: values ({s2}) and events ({e}) never matched"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        match result {
            Some(result) => {
                assert_eq!(result, Ok(()), "k {k}");
                assert_eq!(
                    site.store.read().meta().get(id, &key).await.unwrap(),
                    Some(json!(1)),
                    "k {k}"
                );
                completed = true;
                break;
            }
            None => dropped += 1,
        }
    }
    assert!(completed, "the write never finished within 1000 polls");
    // The write awaits the store, so its first poll is always pending.
    assert!(dropped > 0, "no write was dropped mid-way");
    site
}

/// P22: the head filters of a page share its render budget: once it is spent the
/// remaining plugins are skipped and the head is volatile.
async fn render_budget_bounds_the_head(site: Site) -> Site {
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
    let slow = std::sync::Arc::new(common::SlowPosts::new(
        site.services.clone(),
        std::time::Duration::from_secs(5),
    ));
    let host = slow.host(
        ferrite_plugin_host::HostConfig::default(),
        &["slow-a", "slow-b", "slow-c"],
    );
    let item = public_item(&site, id).await;
    let start = std::time::Instant::now();
    let req = req_for(&item).until(start + std::time::Duration::from_millis(300));
    let mut trace = RenderTrace::default();
    let html = page_head(
        &host,
        site.store.as_ref(),
        SITE,
        HeadPage {
            item: Some(&item),
            path: &item.url,
        },
        &req,
        &mut trace,
    )
    .await;
    let took = start.elapsed();
    assert!(
        html.starts_with("<title>Hello – Test &amp; Site</title>"),
        "{html}"
    );
    assert!(trace.volatile);
    assert!(
        took < std::time::Duration::from_millis(500),
        "took {took:?}"
    );
    assert_eq!(slow.calls(), 1, "slow-b and slow-c were skipped");
    site
}

/// A store error that a plugin read swallows (the plugin still succeeds) marks the
/// page volatile, so a transient failure is not cached for long (final review I2).
async fn plugin_read_errors_are_volatile(site: Site) -> Site {
    // Final review I3: the assertions need the reader's call to succeed, so a slow
    // first read must not be a deadline failure; a failure names its real cause.
    let site = with_generous_deadlines(site);
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
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
    assert!(!trace.volatile);

    site.store.close().await;
    // No item: only the plugin's `list_posts` and `options.get` touch the store.
    let (html, trace) = head(&site, None, &req).await;
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
    assert_eq!(meta_value(&html, "reader-list"), Some(""), "{html}");
    assert!(trace.volatile);
    site
}

/// A failed `seo.description` read falls back to the excerpt and marks the page
/// volatile (final review I2).
async fn seo_read_error_is_volatile(site: Site) -> Site {
    let site = with_generous_deadlines(site);
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
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
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
    site_options_come_from_the_configuration,
    plugin_read_errors_are_volatile,
    seo_read_error_is_volatile,
    meta_writes_reach_visible_items_only,
    dropped_meta_write_is_all_or_nothing,
    render_budget_bounds_the_head,
);

#[test]
fn dep_key_formats_match_core_grammar() {
    let id = ContentId::new();
    assert_eq!(dep_keys::post(&id.to_string()), post_key(id));
    assert_eq!(dep_keys::archive("page"), archive_key("page"));
    assert_eq!(dep_keys::option("site.title"), option_key("site.title"));
}
