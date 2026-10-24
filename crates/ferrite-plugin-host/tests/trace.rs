//! Per-call tracing (design 05 §7a): dependency keys recorded from `content`, `meta` and
//! `options` imports, volatile marking for `kv`, `http`, wall-clock use and failures,
//! and `html` sanitising of filter output (05 §7, M0 ruling R4).
mod common;

use std::sync::Arc;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    FilterOutcome, FilterValue, Head, HostConfig, InMemoryServices, Manifest, PluginHost, Post,
    RequestInfo, dep_keys,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn post(id: &str, slug: &str) -> Post {
    Post {
        id: id.into(),
        kind: "post".into(),
        slug: slug.into(),
        title: format!("Title {slug}"),
        excerpt: String::new(),
        status: "published".into(),
        published: None,
        url: format!("/{slug}/"),
        author: "Ada".into(),
        terms: Vec::new(),
    }
}

fn host_with(services: Arc<InMemoryServices>, plugins: &[&str]) -> TestResult<PluginHost> {
    let host = PluginHost::new(HostConfig::default(), services)?;
    for name in plugins {
        host.install(Manifest::parse(&plugin_manifest(name))?, &plugin_wasm(name))?;
    }
    Ok(host)
}

fn req(post: Option<&str>) -> RequestInfo {
    RequestInfo {
        path: "/".into(),
        locale: "en".into(),
        is_logged_in: false,
        post: post.map(Into::into),
    }
}

async fn head_outcome(host: &PluginHost, post: Option<&str>) -> FilterOutcome {
    host.apply_filter_traced(
        "render.head",
        FilterValue::Head(Head::default()),
        &req(post),
    )
    .await
}

fn meta<'a>(v: &'a FilterValue, name: &str) -> Option<&'a str> {
    let FilterValue::Head(h) = v else {
        return None;
    };
    h.metas
        .iter()
        .find(|m| m.name.as_deref() == Some(name))
        .map(|m| m.content.as_str())
}

#[tokio::test]
async fn content_meta_and_option_reads_record_keys() -> TestResult {
    let services = Arc::new(InMemoryServices::new());
    services.insert_post(post("p1", "one"));
    services.insert_post(post("p2", "two"));
    services.set_option("site.title", "\"Site\"");
    let host = host_with(services, &["reader"])?;

    let out = head_outcome(&host, Some("p1")).await;
    assert_eq!(meta(&out.value, "reader-current"), Some("Title one"));
    assert_eq!(meta(&out.value, "reader-list"), Some("one,two"));
    assert_eq!(meta(&out.value, "reader-option"), Some("\"Site\""));
    let want = [
        dep_keys::archive("post"),
        dep_keys::option("site.title"),
        dep_keys::post("p1"),
        dep_keys::post("p2"),
    ];
    assert_eq!(out.deps, want, "sorted, deduplicated");
    assert!(!out.volatile);
    Ok(())
}

#[tokio::test]
async fn missing_post_still_records_its_key() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["reader"])?;
    let out = head_outcome(&host, Some("gone")).await;
    assert_eq!(meta(&out.value, "reader-current"), Some("-"));
    // Publishing `gone` later must refresh the page.
    assert!(out.deps.contains(&dep_keys::post("gone")), "{:?}", out.deps);
    assert!(out.deps.contains(&dep_keys::archive("post")));
    Ok(())
}

#[tokio::test]
async fn pure_plugin_records_nothing() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["hello"])?;
    let out = head_outcome(&host, None).await;
    assert_eq!(meta(&out.value, "generator"), Some("ferrite-hello"));
    assert!(out.deps.is_empty(), "{:?}", out.deps);
    assert!(!out.volatile, "hello reads no clock, kv or http");
    Ok(())
}

#[tokio::test]
async fn kv_use_is_volatile() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["kv-budget"])?;
    let out = head_outcome(&host, None).await;
    assert_eq!(meta(&out.value, "kv-ok"), Some("100"));
    assert!(out.volatile);
    Ok(())
}

#[tokio::test]
async fn wall_clock_use_is_volatile() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["clock"])?;
    let out = head_outcome(&host, None).await;
    let secs: u64 = meta(&out.value, "clock").ok_or("no clock meta")?.parse()?;
    assert!(secs > 1_700_000_000, "real wall clock: {secs}");
    assert!(out.volatile);
    Ok(())
}

#[tokio::test]
async fn failed_call_is_volatile_and_keeps_input() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["panic"])?;
    let out = head_outcome(&host, None).await;
    assert_eq!(out.value, FilterValue::Head(Head::default()));
    // A failure may be transient: the page must not be cached without bound.
    assert!(out.volatile);
    Ok(())
}

#[tokio::test]
async fn apply_filter_is_the_untraced_value() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["hello"])?;
    let input = FilterValue::Head(Head::default());
    let plain = host
        .apply_filter("render.head", input.clone(), &req(None))
        .await;
    let traced = host
        .apply_filter_traced("render.head", input, &req(None))
        .await;
    assert_eq!(plain, traced.value);
    Ok(())
}

const BODY: &str = "<p>Hello</p>";

#[tokio::test]
async fn sanitized_html_output_is_cleaned() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["html-inject"])?;
    let out = host
        .apply_filter("content.body", FilterValue::Html(BODY.into()), &req(None))
        .await;
    let FilterValue::Html(html) = out else {
        panic!("expected html, got {out:?}");
    };
    assert!(html.starts_with(BODY), "{html}");
    for bad in ["<script", "alert(1)", "onerror", "javascript:"] {
        assert!(!html.contains(bad), "{bad} survived: {html}");
    }
    assert!(html.contains(r#"<img src="/x.png">"#), "{html}");
    assert!(
        html.contains(r#"<a href="https://example.org/" rel="noopener noreferrer">ok</a>"#),
        "{html}"
    );
    Ok(())
}

#[tokio::test]
async fn raw_html_output_passes_through() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    let manifest = plugin_manifest("html-inject")
        .replace(r#"render.html = "sanitized""#, r#"render.html = "raw""#);
    host.install(Manifest::parse(&manifest)?, &plugin_wasm("html-inject"))?;
    let out = host
        .apply_filter("content.body", FilterValue::Html(BODY.into()), &req(None))
        .await;
    let FilterValue::Html(html) = out else {
        panic!("expected html, got {out:?}");
    };
    assert!(html.contains("<script>alert(1)</script>"), "{html}");
    assert!(html.contains(r#"onerror="alert(2)""#), "{html}");
    Ok(())
}

#[tokio::test]
async fn text_output_is_not_html_sanitised() -> TestResult {
    let host = host_with(Arc::new(InMemoryServices::new()), &["html-inject"])?;
    let out = host
        .apply_filter(
            "content.excerpt",
            FilterValue::Text("a < b".into()),
            &req(None),
        )
        .await;
    // Text is escaped by whoever prints it; the host leaves it alone.
    assert_eq!(out, FilterValue::Text("a < b [filtered]".into()));
    Ok(())
}

/// Ruling R20: no storage writes on the render path; actions may write.
#[tokio::test]
async fn meta_writes_are_refused_in_render_filters() -> TestResult {
    use ferrite_plugin_host::{ActionEvent, HostServices};
    let services = Arc::new(InMemoryServices::new());
    // A `post`: meta-writer's `content.read` type (P23).
    services.insert_post(post("p1", "p1"));
    let host = host_with(services.clone(), &["meta-writer"])?;
    let out = head_outcome(&host, Some("p1")).await;
    // The guest got an error for each try and still returned its head.
    for name in ["meta-set-1", "meta-set-2"] {
        assert_eq!(
            meta(&out.value, name),
            Some("meta.set is not allowed during render filters"),
            "{name}"
        );
    }
    assert!(
        host.failures().is_empty(),
        "a refused write is not a plugin failure"
    );
    assert_eq!(services.meta_get("p1", "seo.seen").await, None);

    let ev = ActionEvent {
        subject: Some("p1".into()),
        payload: serde_json::Value::Null,
    };
    host.dispatch_action("meta-writer", "post.saved", ev)
        .await?;
    assert_eq!(
        services.meta_get("p1", "seo.seen").await.as_deref(),
        Some("\"action\"")
    );
    Ok(())
}
