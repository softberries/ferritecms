//! Failing plugins never break a page: bad outputs are rejected, failures are classified
//! and recorded, and repeat offenders are disabled.
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    FailureKind, FilterValue, Head, HostConfig, InMemoryServices, Manifest, PluginHost, RequestInfo,
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
    let kind = last_kind(&host);
    assert!(
        matches!(kind, Some(FailureKind::InvalidOutput | FailureKind::Memory)),
        "{kind:?}"
    );
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
    let host = host_with(HostConfig::default(), &["oom"])?;
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
