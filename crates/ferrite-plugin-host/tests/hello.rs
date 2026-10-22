mod common;

use std::sync::Arc;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    ActionEvent, FilterValue, Head, HostConfig, InMemoryServices, Manifest, MetaTag, PluginError,
    PluginHost, RequestInfo, RouteRequest,
};
use wasmtime::Engine;
use wasmtime::component::Component;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn hello_wasm_is_a_component() {
    let wasm = plugin_wasm("hello");
    Component::new(&Engine::default(), wasm).expect("hello.wasm is a valid component");
    Manifest::parse(&plugin_manifest("hello")).expect("hello manifest parses");
}

fn hello_host() -> TestResult<PluginHost> {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    host.install(
        Manifest::parse(&plugin_manifest("hello"))?,
        &plugin_wasm("hello"),
    )?;
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

fn generator() -> MetaTag {
    MetaTag {
        name: Some("generator".into()),
        property: None,
        content: "ferrite-hello".into(),
    }
}

#[tokio::test]
async fn hello_adds_generator_meta() -> TestResult {
    let host = hello_host()?;
    let out = host
        .apply_filter("render.head", FilterValue::Head(Head::default()), &req())
        .await;
    let FilterValue::Head(head) = out else {
        panic!("expected head, got {out:?}");
    };
    assert_eq!(head.metas, vec![generator()]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_calls() -> TestResult {
    let host = Arc::new(hello_host()?);
    let tasks: Vec<_> = (0..64)
        .map(|_| {
            let host = Arc::clone(&host);
            tokio::spawn(async move {
                host.apply_filter("render.head", FilterValue::Head(Head::default()), &req())
                    .await
            })
        })
        .collect();
    for task in tasks {
        let out = task.await?;
        assert_eq!(
            out,
            FilterValue::Head(Head {
                metas: vec![generator()],
                ..Head::default()
            })
        );
    }
    Ok(())
}

#[test]
fn install_rejects_limits_above_default() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    let wasm = plugin_wasm("hello");
    let src = plugin_manifest("hello");

    let too_big = Manifest::parse(&format!("{src}\n[limits]\nmemory = \"64MB\"\n"))?;
    let err = host
        .install(too_big, &wasm)
        .expect_err("64 MiB > 32 MiB default");
    assert!(
        matches!(
            err,
            PluginError::LimitAboveDefault {
                limit: "memory",
                ..
            }
        ),
        "{err}"
    );

    let too_much_fuel = Manifest::parse(&format!("{src}\n[limits]\nfuel = 50_000_001\n"))?;
    let err = host
        .install(too_much_fuel, &wasm)
        .expect_err("fuel above default");
    assert!(
        matches!(err, PluginError::LimitAboveDefault { limit: "fuel", .. }),
        "{err}"
    );
    assert!(!host.is_enabled("hello"));

    let lower = Manifest::parse(&format!(
        "{src}\n[limits]\nmemory = \"16MB\"\nfuel = 1_000_000\n"
    ))?;
    host.install(lower, &wasm)?;
    assert!(host.is_enabled("hello"));
    Ok(())
}

#[test]
fn install_rejects_undeclared_import() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    let err = host
        .install(
            Manifest::parse(&plugin_manifest("undeclared-import"))?,
            &plugin_wasm("undeclared-import"),
        )
        .expect_err("http not declared");
    assert!(matches!(err, PluginError::Inspect(_)), "{err}");
    assert!(!host.is_enabled("undeclared-import"));
    Ok(())
}

#[tokio::test]
async fn actions_and_routes_are_gated() -> TestResult {
    let host = hello_host()?;
    let ev = ActionEvent {
        subject: None,
        payload: serde_json::json!({}),
    };
    assert!(matches!(
        host.dispatch_action("nope", "post.saved", ev.clone()).await,
        Err(PluginError::UnknownPlugin(_))
    ));
    // hello does not subscribe to `post.saved`; `render.head` is a filter, not an action.
    for hook in ["post.saved", "render.head"] {
        assert!(matches!(
            host.dispatch_action("hello", hook, ev.clone()).await,
            Err(PluginError::NotSubscribed { .. })
        ));
    }
    let req = RouteRequest {
        method: "GET".into(),
        path: "/x".into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: String::new(),
        user: None,
    };
    assert!(matches!(
        host.handle_route("hello", req).await,
        Err(PluginError::RouteNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn mismatched_or_unknown_hooks_return_input() -> TestResult {
    let host = hello_host()?;
    let text = FilterValue::Text("t".into());
    assert_eq!(
        host.apply_filter("render.head", text.clone(), &req()).await,
        text
    );
    assert_eq!(
        host.apply_filter("no.such", text.clone(), &req()).await,
        text
    );
    assert_eq!(
        host.apply_filter("post.saved", text.clone(), &req()).await,
        text
    );
    Ok(())
}

#[tokio::test]
async fn hello_works_with_pooling_allocator() -> TestResult {
    let cfg = HostConfig {
        pooling: true,
        ..HostConfig::default()
    };
    let host = PluginHost::new(cfg, Arc::new(InMemoryServices::new()))?;
    host.install(
        Manifest::parse(&plugin_manifest("hello"))?,
        &plugin_wasm("hello"),
    )?;
    let out = host
        .apply_filter("render.head", FilterValue::Head(Head::default()), &req())
        .await;
    let FilterValue::Head(head) = out else {
        panic!("expected head, got {out:?}");
    };
    assert_eq!(head.metas, vec![generator()]);
    Ok(())
}
