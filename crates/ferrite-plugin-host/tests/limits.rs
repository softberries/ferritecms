//! Hostile plugins are stopped by fuel, epoch deadline, memory limit and trap handling,
//! and the host keeps serving afterwards.
mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    FilterValue, Head, HostConfig, InMemoryServices, Manifest, PluginHost, RequestInfo,
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
