//! Failing plugins never break a page: bad outputs are rejected, failures are classified
//! and recorded (and reported to a failure sink), and repeat offenders are disabled.
//! Hot reload (`prepare` + `commit`) never drops a call in flight.
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    FailureKind, FailureSink, FilterValue, Head, HostConfig, HostServices, InMemoryServices,
    KindOk, Manifest, PluginError, PluginHost, Post, RequestInfo, RouteRequest,
};
use tokio::sync::Notify;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn host_with(cfg: HostConfig, plugins: &[&str]) -> TestResult<PluginHost> {
    let host = PluginHost::new(cfg, Arc::new(InMemoryServices::new()))?;
    for name in plugins {
        host.install(Manifest::parse(&plugin_manifest(name))?, &plugin_wasm(name))?;
    }
    Ok(host)
}

fn req() -> RequestInfo {
    RequestInfo {
        path: "/".into(),
        locale: "en".into(),
        is_logged_in: false,
        post: None,
    }
}

fn head() -> FilterValue {
    FilterValue::Head(Head {
        title: Some("unchanged".into()),
        ..Head::default()
    })
}

fn last_kind(host: &PluginHost) -> Option<FailureKind> {
    host.failures().last().map(|f| f.kind)
}

#[tokio::test]
async fn big_output_rejected() -> TestResult {
    let host = host_with(HostConfig::default(), &["big-output"])?;
    let input = FilterValue::Html("<p>a</p>".into());
    let out = host
        .apply_filter("content.body", input.clone(), &req())
        .await;
    assert_eq!(out, input);
    // 3 MiB fits in the 32 MiB memory cap, so only the 2 MiB output cap can reject it.
    assert_eq!(last_kind(&host), Some(FailureKind::InvalidOutput));
    Ok(())
}

#[tokio::test]
async fn wrong_variant_ignored() -> TestResult {
    let host = host_with(HostConfig::default(), &["wrong-variant"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head());
    assert_eq!(last_kind(&host), Some(FailureKind::InvalidOutput));
    Ok(())
}

#[tokio::test]
async fn wrong_variant_does_not_poison_later_plugins() -> TestResult {
    let host = host_with(HostConfig::default(), &["wrong-variant", "hello"])?;
    let FilterValue::Head(h) = host.apply_filter("render.head", head(), &req()).await else {
        panic!("expected head");
    };
    assert_eq!(h.title.as_deref(), Some("unchanged"));
    assert!(h.metas.iter().any(|m| m.content == "ferrite-hello"));
    Ok(())
}

#[tokio::test]
async fn flapper_auto_disabled() -> TestResult {
    let cfg = HostConfig {
        disable_after_failures: 5,
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["flapper"])?;
    for _ in 0..4 {
        host.apply_filter("render.head", head(), &req()).await;
        assert!(host.is_enabled("flapper"));
    }
    host.apply_filter("render.head", head(), &req()).await;
    assert!(!host.is_enabled("flapper"));
    assert_eq!(
        host.disabled_reason("flapper").as_deref(),
        Some("auto: 5 failures in window")
    );
    assert_eq!(host.failures().len(), 5);
    assert!(
        host.failures()
            .iter()
            .all(|f| f.kind == FailureKind::GuestError && f.plugin == "flapper")
    );

    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head());
    assert_eq!(host.failures().len(), 5, "disabled plugin is not called");
    Ok(())
}

#[tokio::test]
async fn failures_outside_the_window_do_not_disable() -> TestResult {
    let cfg = HostConfig {
        disable_after_failures: 3,
        failure_window: Duration::from_millis(100),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["flapper"])?;
    for _ in 0..6 {
        host.apply_filter("render.head", head(), &req()).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(host.is_enabled("flapper"));
    assert_eq!(host.failures().len(), 6);
    Ok(())
}

#[tokio::test]
async fn oom_classified_as_memory() -> TestResult {
    // Filling 32 MiB can take longer than the 200 ms render deadline while the other
    // hostile tests load the machine; this test checks classification, not timing.
    let cfg = HostConfig {
        render_deadline: Duration::from_secs(5),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["oom"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head());
    assert_eq!(last_kind(&host), Some(FailureKind::Memory));
    Ok(())
}

#[tokio::test]
async fn panic_classified_as_trap() -> TestResult {
    let host = host_with(HostConfig::default(), &["panic"])?;
    host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(last_kind(&host), Some(FailureKind::Trap));
    Ok(())
}

#[tokio::test]
async fn loop_classified() -> TestResult {
    let cfg = HostConfig {
        render_deadline: Duration::from_secs(5),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["loop"])?;
    host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(last_kind(&host), Some(FailureKind::OutOfFuel));

    let cfg = HostConfig {
        fuel: u64::MAX / 2,
        render_deadline: Duration::from_millis(50),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["spin-host-fuel"])?;
    host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(last_kind(&host), Some(FailureKind::Deadline));
    Ok(())
}

/// What `tracing` events from the host carried, for the fields we assert on.
#[derive(Debug, Default, Clone)]
struct Captured {
    plugin: String,
    stream: String,
    truncated_bytes: u64,
    message: String,
    error: String,
}

#[derive(Default)]
struct FieldVisitor(Captured);

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "plugin" => self.0.plugin = value.into(),
            "stream" => self.0.stream = value.into(),
            "error" => self.0.error = value.into(),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "truncated_bytes" {
            self.0.truncated_bytes = value;
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.0.message = format!("{value:?}"),
            // `plugin = %name` arrives as a Display wrapper, whose Debug is its Display.
            "plugin" => self.0.plugin = format!("{value:?}"),
            _ => {}
        }
    }
}

/// Minimal subscriber that records every event.
#[derive(Clone, Default)]
struct Recorder(Arc<std::sync::Mutex<Vec<Captured>>>);

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut v = FieldVisitor::default();
        event.record(&mut v);
        if let Ok(mut events) = self.0.lock() {
            events.push(v.0);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Install the recorder as the global subscriber (once). A scoped `set_default` is
/// unreliable here: other tests in this binary emit the same callsites concurrently, and
/// tracing caches a callsite as disabled if it is first hit while no subscriber is set.
fn recorder() -> Recorder {
    static RECORDER: std::sync::OnceLock<Recorder> = std::sync::OnceLock::new();
    RECORDER
        .get_or_init(|| {
            let recorder = Recorder::default();
            // Fails only if another subscriber is already installed; none is in this binary.
            let _ = tracing::subscriber::set_global_default(recorder.clone());
            recorder
        })
        .clone()
}

fn meta_content(head: &FilterValue, name: &str) -> Option<String> {
    let FilterValue::Head(h) = head else {
        return None;
    };
    h.metas
        .iter()
        .find(|m| m.name.as_deref() == Some(name))
        .map(|m| m.content.clone())
}

#[tokio::test]
async fn fs_escape_sees_no_files() -> TestResult {
    let host = host_with(HostConfig::default(), &["fs-escape"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    let seen = meta_content(&out, "fs-escape").ok_or("fs-escape meta missing")?;
    assert!(seen.starts_with("error:"), "{seen}");
    assert!(!seen.contains("root:"), "{seen}");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    Ok(())
}

#[tokio::test]
async fn noisy_output_is_capped() -> TestResult {
    // Pushing 11 MiB through std's line-buffered stdout costs the guest far more than the
    // default 50 M fuel; the point here is the host's output handling, not the budget.
    let cfg = HostConfig {
        fuel: 20_000_000_000,
        render_deadline: Duration::from_secs(30),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["noisy"])?;
    let recorder = recorder();
    let out = host.apply_filter("render.head", head(), &req()).await;

    assert_eq!(out, head(), "call succeeds and returns its input");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    let events = recorder.0.lock().map_err(|e| e.to_string())?.clone();
    for (stream, total) in [("stdout", 10usize << 20), ("stderr", 1 << 20)] {
        let e = events
            .iter()
            .find(|e| e.plugin == "noisy" && e.stream == stream)
            .ok_or(format!("no {stream} event"))?;
        assert_eq!(e.plugin, "noisy");
        assert!(!e.message.is_empty());
        // The logged text is escaped (newline -> `\n`); undo that to count raw bytes.
        let raw = e.message.replace("\\n", "\n").len();
        assert!(raw <= 64 * 1024, "{stream}: {raw}");
        assert_eq!(
            raw as u64 + e.truncated_bytes,
            total as u64,
            "{stream}: captured + truncated == written"
        );
    }
    Ok(())
}

#[tokio::test]
async fn fat_init_fails_cleanly() -> TestResult {
    let host = host_with(HostConfig::default(), &["fat-init"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head(), "input returned unchanged");
    let kind = last_kind(&host);
    assert!(
        matches!(kind, Some(FailureKind::Memory | FailureKind::Instantiate)),
        "{kind:?}"
    );
    // The host carries on: another plugin still works.
    host.install(
        Manifest::parse(&plugin_manifest("hello"))?,
        &plugin_wasm("hello"),
    )?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert!(meta_content(&out, "generator").is_some(), "{out:?}");
    Ok(())
}

#[tokio::test]
async fn kv_budget_enforced() -> TestResult {
    let host = host_with(HostConfig::default(), &["kv-budget"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(meta_content(&out, "kv-ok").as_deref(), Some("100"));
    Ok(())
}

#[tokio::test]
async fn long_guest_error_is_capped_and_escaped() -> TestResult {
    let host = host_with(HostConfig::default(), &["long-error"])?;
    let recorder = recorder();
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head(), "page value unchanged");
    assert_eq!(last_kind(&host), Some(FailureKind::GuestError));

    let events = recorder.0.lock().map_err(|e| e.to_string())?.clone();
    let e = events
        .iter()
        .find(|e| e.plugin == "long-error" && !e.error.is_empty())
        .ok_or("no failure event for long-error")?;
    assert!(
        !e.error.contains(['\n', '\u{1b}']),
        "unescaped: {:?}",
        e.error
    );
    let body = e
        .error
        .strip_prefix("plugin error: ")
        .and_then(|s| s.strip_suffix("…[truncated]"))
        .ok_or("expected prefix and truncation marker")?;
    // Undo the escaping to count what was kept of the guest's text.
    let raw = body.replace("\\n", "\n").replace("\\u{1b}", "\u{1b}");
    assert!(raw.len() <= 1024, "{} bytes kept", raw.len());
    assert!(raw.starts_with("FORGED\n\u{1b}[31mred\n"), "{raw:?}");
    Ok(())
}

#[tokio::test]
async fn resource_flood_is_contained() -> TestResult {
    // A generous deadline, so only the resource-table cap can stop the flood.
    let cfg = HostConfig {
        render_deadline: Duration::from_secs(10),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["resource-flood"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head(), "page value unchanged");
    assert_eq!(meta_content(&out, "resource-flood"), None);
    // A full resource table surfaces as a host error inside the call (not a wasm trap),
    // which the host classifies as `Trap`.
    assert_eq!(last_kind(&host), Some(FailureKind::Trap));
    host.install(
        Manifest::parse(&plugin_manifest("hello"))?,
        &plugin_wasm("hello"),
    )?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert!(meta_content(&out, "generator").is_some(), "{out:?}");
    Ok(())
}

/// Services whose `get_post` signals `entered`, then waits for `release`; everything
/// else is the empty in-memory services.
#[derive(Default)]
struct GatedServices {
    entered: Notify,
    release: Notify,
    rest: InMemoryServices,
}

#[async_trait::async_trait]
impl HostServices for GatedServices {
    async fn get_post(&self, id: &str, _kind_ok: &KindOk) -> Option<Post> {
        self.entered.notify_one();
        self.release.notified().await;
        Some(Post {
            id: id.into(),
            kind: "post".into(),
            slug: id.into(),
            title: String::new(),
            excerpt: String::new(),
            status: "publish".into(),
            published: None,
            url: String::new(),
            author: String::new(),
            terms: Vec::new(),
        })
    }
    async fn post_kind(&self, id: &str) -> Option<String> {
        self.rest.post_kind(id).await
    }
    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post> {
        self.rest.get_post_by_slug(kind, slug).await
    }
    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post> {
        self.rest.list_posts(kind, limit, offset).await
    }
    async fn meta_get(&self, post: &str, key: &str) -> Option<String> {
        self.rest.meta_get(post, key).await
    }
    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String> {
        self.rest.meta_set(post, key, value).await
    }
    async fn option_get(&self, key: &str) -> Option<String> {
        self.rest.option_get(key).await
    }
    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>> {
        self.rest.kv_get(plugin, key).await
    }
    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.rest.kv_set(plugin, key, value).await
    }
    async fn kv_delete(&self, plugin: &str, key: &str) {
        self.rest.kv_delete(plugin, key).await;
    }
    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String> {
        self.rest.kv_list(plugin, prefix, limit).await
    }
    async fn setting_get(
        &self,
        plugin: &str,
        key: &str,
        spec: &ferrite_plugin_host::SettingSpec,
    ) -> Option<String> {
        self.rest.setting_get(plugin, key, spec).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_commit_swaps_atomically() -> TestResult {
    let services = Arc::new(GatedServices::default());
    let cfg = HostConfig {
        render_deadline: Duration::from_secs(20),
        ..HostConfig::default()
    };
    let host = Arc::new(PluginHost::new(cfg, services.clone())?);
    let manifest = || Manifest::parse(&plugin_manifest("slow-import"));
    host.install(manifest()?, &plugin_wasm("slow-import"))?;

    // The old instance's call parks inside its `get_post` import.
    let req = RequestInfo {
        post: Some("p1".into()),
        ..req()
    };
    let in_flight = tokio::spawn({
        let host = host.clone();
        let req = req.clone();
        async move { host.apply_filter("render.head", head(), &req).await }
    });
    tokio::time::timeout(Duration::from_secs(10), services.entered.notified()).await?;

    // New version of `slow-import`: hello's component (never calls `get_post`).
    let next = host.prepare(manifest()?, &plugin_wasm("hello"))?;
    assert_eq!(next.name(), "slow-import");
    host.commit(vec![next], &[]);
    assert_eq!(host.loaded(), ["slow-import"]);

    services.release.notify_one();
    let old = tokio::time::timeout(Duration::from_secs(10), in_flight).await??;
    assert_eq!(
        meta_content(&old, "slow-import").as_deref(),
        Some("returned")
    );
    assert_eq!(meta_content(&old, "generator"), None, "{old:?}");

    let new = tokio::time::timeout(
        Duration::from_secs(10),
        host.apply_filter("render.head", head(), &req),
    )
    .await?;
    assert_eq!(
        meta_content(&new, "generator").as_deref(),
        Some("ferrite-hello")
    );
    assert_eq!(meta_content(&new, "slow-import"), None, "{new:?}");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    Ok(())
}

#[tokio::test]
async fn commit_keep_preserves_untouched_plugins() -> TestResult {
    let host = host_with(HostConfig::default(), &["hello", "clock"])?;
    assert!(host.disable("hello", "operator"));
    let order_a = host.prepare(
        Manifest::parse(&plugin_manifest("order-a"))?,
        &plugin_wasm("order-a"),
    )?;
    host.commit(vec![order_a], &["hello"]);

    assert_eq!(host.loaded(), ["hello", "order-a"], "clock dropped");
    // `hello` is the same instance, so it is still disabled.
    assert_eq!(host.disabled_reason("hello").as_deref(), Some("operator"));
    assert!(host.is_enabled("order-a"));
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(meta_content(&out, "generator"), None, "{out:?}");
    assert_eq!(meta_content(&out, "clock"), None, "{out:?}");

    // Preparing `hello` again replaces the kept instance with an enabled one.
    let hello = host.prepare(
        Manifest::parse(&plugin_manifest("hello"))?,
        &plugin_wasm("hello"),
    )?;
    host.commit(vec![hello], &["hello", "order-a"]);
    assert_eq!(host.loaded(), ["hello", "order-a"]);
    assert!(host.is_enabled("hello"));
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(
        meta_content(&out, "generator").as_deref(),
        Some("ferrite-hello")
    );

    host.commit(Vec::new(), &[]);
    assert!(host.loaded().is_empty());
    Ok(())
}

#[tokio::test]
async fn disable_stops_calls_immediately() -> TestResult {
    let host = host_with(HostConfig::default(), &["hello"])?;
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(
        meta_content(&out, "generator").as_deref(),
        Some("ferrite-hello")
    );

    assert!(host.disable("hello", "auto: 20 failures in 10m"));
    assert!(!host.is_enabled("hello"));
    let out = host.apply_filter("render.head", head(), &req()).await;
    assert_eq!(out, head(), "the next call skips it");
    assert!(!host.disable("hello", "again"), "already disabled");
    assert_eq!(
        host.disabled_reason("hello").as_deref(),
        Some("auto: 20 failures in 10m"),
        "the first reason is kept"
    );
    assert!(!host.disable("nope", "x"), "not loaded");
    assert_eq!(host.loaded(), ["hello"], "disabled plugins stay loaded");
    Ok(())
}

#[derive(Default)]
struct SinkLog(std::sync::Mutex<Vec<(String, String, FailureKind)>>);

impl FailureSink for SinkLog {
    fn record(&self, plugin: &str, hook: &str, kind: FailureKind) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((plugin.into(), hook.into(), kind));
    }
}

#[tokio::test]
async fn failure_sink_receives_every_failure() -> TestResult {
    let host = host_with(HostConfig::default(), &["panic", "hello"])?;
    let sink = Arc::new(SinkLog::default());
    host.set_failure_sink(sink.clone());
    for _ in 0..3 {
        host.apply_filter("render.head", head(), &req()).await;
    }
    let seen = sink.0.lock().map_err(|e| e.to_string())?.clone();
    assert_eq!(
        seen,
        vec![
            (
                "panic".to_owned(),
                "render.head".to_owned(),
                FailureKind::Trap
            );
            3
        ],
        "one record per failed call, none for hello"
    );
    assert_eq!(host.failures().len(), 3);
    Ok(())
}

fn route_req(path: &str) -> RouteRequest {
    RouteRequest {
        method: "GET".into(),
        path: path.into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: String::new(),
        user: None,
    }
}

/// A panicking, a spinning and an oversized route handler each fail only their own
/// call (classified and recorded); the plugin's other routes keep answering.
#[tokio::test]
async fn route_failures_are_contained() -> TestResult {
    let host = host_with(HostConfig::default(), &["route-xss", "router"])?;
    let failed = |r: Result<_, PluginError>| matches!(r, Err(PluginError::CallFailed { .. }));
    assert!(failed(
        host.handle_route("route-xss", route_req("/panic")).await
    ));
    assert_eq!(last_kind(&host), Some(FailureKind::Trap));
    assert!(failed(
        host.handle_route("route-xss", route_req("/loop")).await
    ));
    assert_eq!(last_kind(&host), Some(FailureKind::OutOfFuel));
    // 2 MiB body: over the 1 MiB route response cap.
    assert!(failed(host.handle_route("router", route_req("/big")).await));
    assert_eq!(last_kind(&host), Some(FailureKind::InvalidOutput));
    let ok = host.handle_route("route-xss", route_req("/x")).await?;
    assert_eq!(ok.status, 200);
    assert_eq!(host.failures().len(), 3);
    Ok(())
}

/// With fuel to spare, only the route deadline stops a spinning handler.
#[tokio::test]
async fn route_deadline_stops_spinning_handler() -> TestResult {
    let cfg = HostConfig {
        fuel: u64::MAX / 2,
        route_deadline: Duration::from_millis(50),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["route-xss"])?;
    let started = std::time::Instant::now();
    let res = host.handle_route("route-xss", route_req("/loop")).await;
    assert!(
        matches!(res, Err(PluginError::CallFailed { .. })),
        "{res:?}"
    );
    assert_eq!(last_kind(&host), Some(FailureKind::Deadline));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    Ok(())
}
