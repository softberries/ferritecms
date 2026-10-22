//! Hostile plugins are stopped by fuel, epoch deadline, memory limit and trap handling,
//! and the host keeps serving afterwards.
mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    FilterValue, Head, HostConfig, HostServices, InMemoryServices, Manifest, PluginHost, Post,
    RequestInfo,
};

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
    assert!(elapsed < Duration::from_millis(200), "took {elapsed:?}");
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
    assert!(elapsed >= Duration::from_millis(40), "took {elapsed:?}");
    assert!(elapsed < Duration::from_millis(150), "took {elapsed:?}");
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
    async fn get_post(&self, id: &str) -> Option<Post> {
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
    async fn setting_get(&self, _: &str, _: &str) -> Option<String> {
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
    // (50 ms × 1.5 = 75 ms) can end the call.
    let (out, elapsed) = slow_import_call(Duration::from_secs(2)).await?;
    assert_eq!(out, input());
    assert!(elapsed >= Duration::from_millis(70), "took {elapsed:?}");
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
    Ok(())
}
