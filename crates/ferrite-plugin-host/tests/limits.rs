//! Hostile plugins are stopped by fuel, epoch deadline, memory limit and trap handling,
//! and the host keeps serving afterwards.
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    ActionEvent, FailureKind, FilterValue, Head, HostConfig, HostServices, InMemoryServices,
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

fn input() -> FilterValue {
    FilterValue::Head(Head {
        title: Some("unchanged".into()),
        ..Head::default()
    })
}

/// Kind of the most recent recorded failure.
fn last_kind(host: &PluginHost) -> Option<FailureKind> {
    host.failures().last().map(|f| f.kind)
}

/// Run `render.head` once and return the output with the elapsed wall time.
async fn timed(host: &PluginHost) -> (FilterValue, Duration) {
    let start = Instant::now();
    let out = host.apply_filter("render.head", input(), &req()).await;
    (out, start.elapsed())
}

#[tokio::test]
async fn infinite_loop_is_killed_by_fuel() -> TestResult {
    // Default limits, except a render deadline far beyond the bound below, so only fuel
    // exhaustion can explain the call ending in time.
    let cfg = HostConfig {
        render_deadline: Duration::from_secs(5),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["loop"])?;
    let (out, elapsed) = timed(&host).await;
    assert_eq!(out, input());
    // The kind proves fuel ended the call; the bound only shows it ended long before the
    // 5 s deadline (kept loose for slow CI runners).
    assert_eq!(last_kind(&host), Some(FailureKind::OutOfFuel));
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn infinite_loop_is_killed_by_epoch() -> TestResult {
    let cfg = HostConfig {
        fuel: u64::MAX / 2,
        render_deadline: Duration::from_millis(50),
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["spin-host-fuel"])?;
    let (out, elapsed) = timed(&host).await;
    assert_eq!(out, input());
    assert_eq!(last_kind(&host), Some(FailureKind::Deadline));
    assert!(elapsed >= Duration::from_millis(40), "took {elapsed:?}");
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn oom_is_contained() -> TestResult {
    let host = host_with(HostConfig::default(), &["oom"])?;
    let (out, _) = timed(&host).await;
    assert_eq!(out, input());

    host.install(
        Manifest::parse(&plugin_manifest("hello"))?,
        &plugin_wasm("hello"),
    )?;
    let FilterValue::Head(head) = host.apply_filter("render.head", input(), &req()).await else {
        panic!("expected head");
    };
    assert_eq!(head.title.as_deref(), Some("unchanged"));
    assert!(
        head.metas
            .iter()
            .any(|m| m.name.as_deref() == Some("generator") && m.content == "ferrite-hello"),
        "hello still runs after oom: {head:?}"
    );
    Ok(())
}

#[tokio::test]
async fn panic_is_contained() -> TestResult {
    let host = host_with(HostConfig::default(), &["panic"])?;
    let (out, _) = timed(&host).await;
    assert_eq!(out, input());
    Ok(())
}

/// Services whose `get_post` takes `delay`; everything else is empty.
struct SlowServices {
    delay: Duration,
}

#[async_trait::async_trait]
impl HostServices for SlowServices {
    async fn get_post(&self, id: &str, _kind_ok: &KindOk) -> Option<Post> {
        tokio::time::sleep(self.delay).await;
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
    async fn post_kind(&self, _: &str) -> Option<String> {
        None
    }
    async fn get_post_by_slug(&self, _: &str, _: &str) -> Option<Post> {
        None
    }
    async fn list_posts(&self, _: &str, _: u32, _: u32) -> Vec<Post> {
        Vec::new()
    }
    async fn meta_get(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    async fn meta_set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn option_get(&self, _: &str) -> Option<String> {
        None
    }
    async fn kv_get(&self, _: &str, _: &str) -> Option<Vec<u8>> {
        None
    }
    async fn kv_set(&self, _: &str, _: &str, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn kv_delete(&self, _: &str, _: &str) {}
    async fn kv_list(&self, _: &str, _: &str, _: u32) -> Vec<String> {
        Vec::new()
    }
    async fn setting_get(
        &self,
        _: &str,
        _: &str,
        _: &ferrite_plugin_host::SettingSpec,
    ) -> Option<String> {
        None
    }
}

async fn slow_import_call(delay: Duration) -> TestResult<(FilterValue, Duration)> {
    let host = PluginHost::new(HostConfig::default(), Arc::new(SlowServices { delay }))?;
    host.install(
        Manifest::parse(&plugin_manifest("slow-import"))?,
        &plugin_wasm("slow-import"),
    )?;
    let req = RequestInfo {
        post: Some("p1".into()),
        ..req()
    };
    let start = Instant::now();
    let out = host.apply_filter("render.head", input(), &req).await;
    Ok((out, start.elapsed()))
}

#[tokio::test]
async fn slow_host_import_is_cut_by_hard_deadline() -> TestResult {
    // Control: with a fast host the plugin's import returns and it adds its meta.
    let (out, _) = slow_import_call(Duration::ZERO).await?;
    let FilterValue::Head(head) = out else {
        panic!("expected head");
    };
    assert!(
        head.metas.iter().any(|m| m.content == "returned"),
        "{head:?}"
    );

    // `get_post` sleeps 2 s; epochs don't interrupt host code, so only the hard deadline
    // (200 ms × 1.5 = 300 ms) can end the call.
    let (out, elapsed) = slow_import_call(Duration::from_secs(2)).await?;
    assert_eq!(out, input());
    assert!(elapsed >= Duration::from_millis(290), "took {elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    // Well under the 2 s the host import sleeps, so the hard deadline must have fired.
    assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
    Ok(())
}

/// Services whose `get_post` counts its callers and parks until [`open`](Self::open);
/// everything else is the in-memory services.
#[derive(Default)]
struct HeldServices {
    entered: AtomicUsize,
    open: AtomicBool,
    opened: Notify,
    rest: InMemoryServices,
}

impl HeldServices {
    fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    /// Wait (at most 10 s) until `n` calls are parked in `get_post`.
    async fn wait_entered(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.entered() < n {
            assert!(Instant::now() < deadline, "only {} entered", self.entered());
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        self.opened.notify_waiters();
    }
}

#[async_trait::async_trait]
impl HostServices for HeldServices {
    async fn get_post(&self, id: &str, _kind_ok: &KindOk) -> Option<Post> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        loop {
            let opened = self.opened.notified();
            if self.open.load(Ordering::SeqCst) {
                break;
            }
            opened.await;
        }
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

/// A host over [`HeldServices`] with `slow-import` installed under each of `names`
/// (F34: one component, several manifests), plus the plugins in `extra`.
fn held_host(
    cfg: HostConfig,
    names: &[&str],
    extra: &[&str],
) -> TestResult<(Arc<PluginHost>, Arc<HeldServices>)> {
    let services = Arc::new(HeldServices::default());
    let host = Arc::new(PluginHost::new(cfg, services.clone())?);
    for name in names {
        let manifest = plugin_manifest("slow-import")
            .replace("name      = \"slow-import\"", &format!("name = \"{name}\""));
        host.install(Manifest::parse(&manifest)?, &plugin_wasm("slow-import"))?;
    }
    for name in extra {
        host.install(Manifest::parse(&plugin_manifest(name))?, &plugin_wasm(name))?;
    }
    Ok((host, services))
}

fn saved(id: &str) -> ActionEvent {
    ActionEvent {
        subject: Some(id.into()),
        payload: serde_json::json!({}),
    }
}

/// P22: with every call slot taken, a call waits at most its deadline for one, then is
/// skipped as `Busy`; nothing counts as a plugin failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrency_cap_skips_without_failure() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 2,
        action_deadline: Duration::from_secs(1),
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &[])?;
    let call = |host: Arc<PluginHost>| {
        tokio::spawn(async move {
            host.dispatch_action("slow-import", "post.saved", saved("p1"))
                .await
        })
    };
    let (a, b) = (call(host.clone()), call(host.clone()));
    services.wait_entered(2).await;
    let start = Instant::now();
    let third = host
        .dispatch_action("slow-import", "post.saved", saved("p1"))
        .await;
    let waited = start.elapsed();
    assert!(
        matches!(&third, Err(PluginError::Busy { plugin }) if plugin == "slow-import"),
        "{third:?}"
    );
    assert!(waited >= Duration::from_millis(900), "waited {waited:?}");
    assert_eq!(services.entered(), 2, "the busy call never ran");
    services.open();
    a.await??;
    b.await??;
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    assert!(host.is_enabled("slow-import"));
    Ok(())
}

/// Task 8 m7: plugin routes take the same call slots, so a flood of route requests
/// cannot exhaust instances, and a busy route is not a plugin failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_calls_share_the_cap() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        route_deadline: Duration::from_millis(100),
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &["router"])?;
    let held = tokio::spawn({
        let host = host.clone();
        async move {
            host.dispatch_action("slow-import", "post.saved", saved("p1"))
                .await
        }
    });
    services.wait_entered(1).await;
    let route = || RouteRequest {
        method: "GET".into(),
        path: "/hello".into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "ab".repeat(16),
        user: None,
    };
    let start = Instant::now();
    let busy = host.handle_route("router", route()).await;
    assert!(
        matches!(&busy, Err(PluginError::Busy { plugin }) if plugin == "router"),
        "{busy:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    services.open();
    held.await??;
    assert_eq!(host.handle_route("router", route()).await?.status, 200);
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    Ok(())
}

/// Final review M5: the wait for a call slot counts toward an action's deadline. The
/// call waits ~400 ms, then hangs in a host import: it is cut at the hard deadline
/// (1.5 s) measured from when it began waiting, not 400 ms later. The plugin itself ran
/// ~1.1 s, more than half the deadline, so the cut is its failure (R24 N2), not `Busy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slot_wait_counts_toward_the_action_deadline() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        action_deadline: Duration::from_secs(1),
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &[])?;
    let dispatch = |host: Arc<PluginHost>| {
        tokio::spawn(async move {
            let start = Instant::now();
            let r = host
                .dispatch_action("slow-import", "post.saved", saved("p1"))
                .await;
            (r, start.elapsed())
        })
    };
    let holder = dispatch(host.clone());
    services.wait_entered(1).await;
    let waiting = dispatch(host.clone());
    tokio::time::sleep(Duration::from_millis(400)).await;
    holder.abort();
    let (result, took) = waiting.await?;
    assert!(
        matches!(&result, Err(PluginError::CallFailed { plugin, .. }) if plugin == "slow-import"),
        "{result:?}"
    );
    assert_eq!(services.entered(), 2, "the waiting call ran");
    assert!(took >= Duration::from_millis(1_400), "took {took:?}");
    assert!(
        took < Duration::from_millis(1_800),
        "wait + full hard deadline: {took:?}"
    );
    assert_eq!(host.failures().len(), 1, "{:?}", host.failures());
    assert_eq!(last_kind(&host), Some(FailureKind::Deadline));
    services.open();
    Ok(())
}

/// Final review M5, for routes: a looping route that waited ~700 ms for its slot (more
/// than half the 1 s route deadline) is interrupted 800 ms later, at the 1.5 s hard
/// deadline from when it began waiting. It ran 800 ms, more than half its deadline, so
/// it is a failure (R24 N2), as the same loop is without a wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slot_wait_counts_toward_the_route_deadline() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        route_deadline: Duration::from_secs(1),
        // Enough fuel that the epoch deadline, not fuel, stops the loop.
        fuel: u64::MAX / 2,
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &["route-xss"])?;
    let looping = || RouteRequest {
        method: "GET".into(),
        path: "/loop".into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "ab".repeat(16),
        user: None,
    };
    let holder = tokio::spawn({
        let host = host.clone();
        async move {
            host.dispatch_action("slow-import", "post.saved", saved("p1"))
                .await
        }
    });
    services.wait_entered(1).await;
    let waiting = tokio::spawn({
        let host = host.clone();
        async move {
            let start = Instant::now();
            let r = host.handle_route("route-xss", looping()).await;
            (r, start.elapsed())
        }
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    holder.abort();
    let (result, took) = waiting.await?;
    assert!(
        matches!(&result, Err(PluginError::CallFailed { plugin, .. }) if plugin == "route-xss"),
        "{result:?}"
    );
    assert!(took >= Duration::from_millis(1_400), "took {took:?}");
    assert!(
        took < Duration::from_millis(1_650),
        "wait + full deadline: {took:?}"
    );
    assert_eq!(host.failures().len(), 1, "{:?}", host.failures());
    assert_eq!(last_kind(&host), Some(FailureKind::Deadline));
    // No wait: the loop gets its full deadline and is the plugin's failure.
    let start = Instant::now();
    let result = host.handle_route("route-xss", looping()).await;
    assert!(
        matches!(&result, Err(PluginError::CallFailed { .. })),
        "{result:?}"
    );
    assert!(start.elapsed() >= Duration::from_millis(950));
    assert_eq!(host.failures().len(), 2, "{:?}", host.failures());
    services.open();
    Ok(())
}

/// R24 N2, regression guard: under contention a looping plugin is still counted. With
/// one call slot, a looping route waits ~400 ms (40 % of its 1 s deadline) for the slot,
/// then runs out of time: one recorded failure, not `Busy`. At 40 % the wait does not
/// cut the epoch deadline, so this passed before N2 too; it guards against a change that
/// would make such a call `Busy`. The N2 change itself is pinned by the two
/// `slot_wait_counts_toward_*` tests (wait cuts that ran at least half are failures),
/// `call.rs`'s `wait_shortened_call_that_ran_half_is_a_failure`, and the opposite branch
/// (a wait that leaves less than half, which the host never produces) by the unit test
/// `wait_that_left_under_half_is_busy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_shortened_slow_plugin_is_counted() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        route_deadline: Duration::from_secs(1),
        fuel: u64::MAX / 2,
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &["route-xss"])?;
    let holder = tokio::spawn({
        let host = host.clone();
        async move {
            host.dispatch_action("slow-import", "post.saved", saved("p1"))
                .await
        }
    });
    services.wait_entered(1).await;
    let waiting = tokio::spawn({
        let host = host.clone();
        async move {
            host.handle_route(
                "route-xss",
                RouteRequest {
                    method: "GET".into(),
                    path: "/loop".into(),
                    query: Vec::new(),
                    headers: Vec::new(),
                    body: Vec::new(),
                    client_ip_hash: "ab".repeat(16),
                    user: None,
                },
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    holder.abort();
    let result = waiting.await?;
    assert!(
        matches!(&result, Err(PluginError::CallFailed { plugin, .. }) if plugin == "route-xss"),
        "{result:?}"
    );
    let failures = host.failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].plugin, "route-xss");
    assert_eq!(failures[0].kind, FailureKind::Deadline);
    services.open();
    Ok(())
}

/// P22 / F37: filters stop once the render budget is spent, a call never outlives the
/// budget (its hard deadline is capped at it), and the outcome is volatile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_budget_bounds_a_filter_chain() -> TestResult {
    let (host, services) = held_host(HostConfig::default(), &["slow-a", "slow-b", "slow-c"], &[])?;
    let req = RequestInfo {
        post: Some("p1".into()),
        ..req()
    };
    let start = Instant::now();
    let until = start + Duration::from_millis(250);
    let out = host
        .apply_filter_until("render.head", input(), &req, until)
        .await;
    let elapsed = start.elapsed();
    assert_eq!(out.value, input());
    assert!(out.volatile);
    assert!(elapsed >= Duration::from_millis(240), "took {elapsed:?}");
    assert!(elapsed < Duration::from_millis(450), "took {elapsed:?}");
    assert_eq!(services.entered(), 1, "slow-b and slow-c were skipped");
    // slow-a's hard deadline was cut from 300 ms to the 250 ms left (review m1): its
    // timeout is the budget's, so nothing is recorded, and the skipped plugins are not
    // either.
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    services.open();
    Ok(())
}

/// A call cut short by the budget before its own deadline is skipped, not counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_cut_is_not_a_failure() -> TestResult {
    let (host, services) = held_host(HostConfig::default(), &["slow-a"], &[])?;
    let req = RequestInfo {
        post: Some("p1".into()),
        ..req()
    };
    let start = Instant::now();
    let out = host
        .apply_filter_until(
            "render.head",
            input(),
            &req,
            start + Duration::from_millis(80),
        )
        .await;
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "{:?}",
        start.elapsed()
    );
    assert!(out.volatile);
    assert_eq!(out.value, input());
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    // A spent budget calls nothing.
    let out = host
        .apply_filter_until("render.head", input(), &req, Instant::now())
        .await;
    assert!(out.volatile);
    assert_eq!(services.entered(), 1);
    services.open();
    Ok(())
}

/// Cancel-safety: a call whose future is dropped mid-call (a client disconnect) frees
/// its instance and its call slot and counts no failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_call_frees_its_slot_and_instance() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        action_deadline: Duration::from_secs(1),
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &[])?;
    let idle = Arc::strong_count(&services);
    let call = tokio::spawn({
        let host = host.clone();
        async move {
            host.dispatch_action("slow-import", "post.saved", saved("p1"))
                .await
        }
    });
    services.wait_entered(1).await;
    assert_eq!(
        Arc::strong_count(&services),
        idle + 1,
        "the call's store holds one"
    );
    call.abort();
    assert!(call.await.is_err_and(|e| e.is_cancelled()));
    assert_eq!(
        Arc::strong_count(&services),
        idle,
        "instance dropped with the future"
    );
    services.open();
    let start = Instant::now();
    host.dispatch_action("slow-import", "post.saved", saved("p1"))
        .await?;
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "slot was free"
    );
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    Ok(())
}

/// Review m6: a looping plugin is trapped by its epoch deadline, which the budget
/// shortened (80 ms of budget, a 200 ms render deadline): skipped, not counted. With
/// its full deadline the same loop is a failure.
#[tokio::test]
async fn budget_cut_epoch_trap_is_not_a_failure() -> TestResult {
    // Fuel far beyond what the loop burns in a second: only the epoch can stop it.
    let cfg = HostConfig {
        fuel: 1_000_000_000_000,
        ..HostConfig::default()
    };
    let host = host_with(cfg, &["loop"])?;
    let start = Instant::now();
    let out = host
        .apply_filter_until(
            "render.head",
            input(),
            &req(),
            start + Duration::from_millis(80),
        )
        .await;
    let elapsed = start.elapsed();
    assert_eq!(out.value, input());
    assert!(out.volatile);
    assert!(elapsed < Duration::from_millis(180), "took {elapsed:?}");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    // Control: no budget, the loop runs into its own 200 ms deadline and is counted.
    let (out, elapsed) = timed(&host).await;
    assert_eq!(out, input());
    assert!(elapsed >= Duration::from_millis(190), "took {elapsed:?}");
    assert_eq!(last_kind(&host), Some(FailureKind::Deadline));
    assert_eq!(host.failures().len(), 1);
    Ok(())
}

/// M2a-2b P20: every outcome lands in its counter of the plugin's hook: a success
/// (`hello`), a failure (`panic`), a budget cut (`slow-import`, cut by the render
/// budget), a `Busy` skip (a route with no free slot); calls that ran are bucketed by
/// duration, and the totals count calls that ran and failures.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_stats_count_outcomes() -> TestResult {
    let cfg = HostConfig {
        max_concurrent_calls: 1,
        action_deadline: Duration::from_secs(5),
        route_deadline: Duration::from_millis(100),
        ..HostConfig::default()
    };
    let (host, services) = held_host(cfg, &["slow-import"], &["hello", "panic", "router"])?;
    assert!(host.call_stats("hello").is_empty());
    assert_eq!(host.totals(), (0, 0));

    // hello, panic, slow-import (name order at equal priority); slow-import parks in
    // `get_post` until the budget cuts it.
    let req = RequestInfo {
        post: Some("p1".into()),
        ..req()
    };
    let until = Instant::now() + Duration::from_millis(150);
    let out = host
        .apply_filter_until("render.head", input(), &req, until)
        .await;
    assert!(out.volatile);
    let one = |plugin: &str| {
        let mut stats = host.call_stats(plugin);
        assert_eq!(stats.len(), 1, "{plugin}: {stats:?}");
        stats.remove(0)
    };
    let hello = one("hello");
    assert_eq!(hello.hook, "render.head");
    assert_eq!(
        (hello.calls, hello.failures, hello.busy, hello.budget_cut),
        (1, 0, 0, 0)
    );
    assert_eq!(hello.buckets.iter().sum::<u64>(), 1);
    assert!(hello.max_micros <= hello.total_micros);
    let panic = one("panic");
    assert_eq!(
        (panic.calls, panic.failures, panic.busy, panic.budget_cut),
        (1, 1, 0, 0)
    );
    let cut = one("slow-import");
    assert_eq!(
        (cut.calls, cut.failures, cut.busy, cut.budget_cut),
        (1, 0, 0, 1)
    );
    assert!(cut.max_micros >= 50_000, "{cut:?}");
    assert_eq!(host.failures().len(), 1, "only panic failed");

    // A spent budget skips the whole chain: a budget cut for each, no call.
    host.apply_filter_until("render.head", input(), &req, Instant::now())
        .await;
    for plugin in ["hello", "panic", "slow-import"] {
        let s = one(plugin);
        assert_eq!(
            s.budget_cut,
            u64::from(plugin == "slow-import") + 1,
            "{s:?}"
        );
        assert_eq!(s.calls, 1, "{s:?}");
    }

    // The only slot is held by an action: the route is skipped as busy.
    let held = tokio::spawn({
        let host = host.clone();
        async move {
            host.dispatch_action("slow-import", "post.saved", saved("p1"))
                .await
        }
    });
    services.wait_entered(2).await;
    let route = RouteRequest {
        method: "GET".into(),
        path: "/hello".into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "ab".repeat(16),
        user: None,
    };
    let busy = host.handle_route("router", route.clone()).await;
    assert!(matches!(busy, Err(PluginError::Busy { .. })), "{busy:?}");
    services.open();
    held.await??;
    let r = one("router");
    assert_eq!(r.hook, "route GET /hello");
    assert_eq!((r.calls, r.failures, r.busy, r.budget_cut), (0, 0, 1, 0));
    assert_eq!(r.buckets, [0; 6]);
    // The action ran for the 100 ms the route waited, and more.
    let action = host
        .call_stats("slow-import")
        .into_iter()
        .find(|s| s.hook == "post.saved")
        .unwrap();
    assert_eq!((action.calls, action.failures), (1, 0));
    assert_eq!(action.buckets[..4], [0, 0, 0, 0], "{action:?}");
    // Now the route runs.
    assert_eq!(host.handle_route("router", route).await?.status, 200);
    assert_eq!(one("router").calls, 1);
    // hello, panic, slow-import (cut), the action and the route ran; panic failed.
    assert_eq!(host.totals(), (5, 1));
    // Uninstalling `router` (a commit without it) drops its statistics; the kept
    // plugins keep theirs, and the totals stay since the host was built.
    host.commit(Vec::new(), &["hello", "panic", "slow-import"]);
    assert!(host.call_stats("router").is_empty());
    assert_eq!(one("hello").calls, 1);
    assert_eq!(host.totals(), (5, 1));
    Ok(())
}
