mod common;

use std::sync::Arc;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::HostServices as _;
use ferrite_plugin_host::{
    ActionEvent, FilterValue, Head, HostConfig, InMemoryServices, Manifest, MetaTag, PluginError,
    PluginHost, Post, RequestInfo, RouteRequest,
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

/// Task 11: meta-writer's `post.saved` action writes `seo.seen = "action"` on the
/// subject through the services (actions may write, unlike the render path).
#[tokio::test]
async fn dispatch_action_success_path() -> TestResult {
    let services = Arc::new(InMemoryServices::new());
    services.insert_post(Post {
        id: "p1".into(),
        kind: "post".into(),
        slug: "p1".into(),
        title: "P1".into(),
        excerpt: String::new(),
        status: "draft".into(),
        published: None,
        url: String::new(),
        author: String::new(),
        terms: Vec::new(),
    });
    let host = PluginHost::new(HostConfig::default(), services.clone())?;
    host.install(
        Manifest::parse(&plugin_manifest("meta-writer"))?,
        &plugin_wasm("meta-writer"),
    )?;
    let ev = ActionEvent {
        subject: Some("p1".into()),
        payload: serde_json::json!({"post": "p1"}),
    };
    host.dispatch_action("meta-writer", "post.saved", ev)
        .await?;
    assert_eq!(
        services.meta_get("p1", "seo.seen").await.as_deref(),
        Some("\"action\"")
    );
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    Ok(())
}

/// `subscribers` lists the enabled loaded plugins of an action hook, by priority then
/// name; a disabled plugin and a filter-only plugin are left out.
#[tokio::test]
async fn subscribers_are_enabled_plugins_in_priority_order() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    let slow = plugin_manifest("slow-import").replace("\"post.saved\"  = 10", "\"post.saved\" = 5");
    assert_ne!(slow, plugin_manifest("slow-import"), "priority replaced");
    host.install(Manifest::parse(&slow)?, &plugin_wasm("slow-import"))?;
    for name in ["meta-writer", "hello"] {
        host.install(Manifest::parse(&plugin_manifest(name))?, &plugin_wasm(name))?;
    }
    assert_eq!(
        host.subscribers("post.saved"),
        ["slow-import", "meta-writer"]
    );
    assert!(host.subscribers("user.created").is_empty());
    assert!(host.disable("slow-import", "operator"));
    assert_eq!(host.subscribers("post.saved"), ["meta-writer"]);
    Ok(())
}

fn route_req(method: &str, path: &str) -> RouteRequest {
    RouteRequest {
        method: method.into(),
        path: path.into(),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "ab".repeat(16),
        user: Some("u1".into()),
    }
}

#[tokio::test]
async fn handle_route_success_path() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    host.install(
        Manifest::parse(&plugin_manifest("router"))?,
        &plugin_wasm("router"),
    )?;
    let spec = host.route("router", "/hello")?;
    assert_eq!(spec.methods, ["GET"]);
    assert_eq!(spec.rate_limit, None);
    assert!(!spec.cross_origin);
    let res = host
        .handle_route("router", route_req("GET", "/hello"))
        .await?;
    assert_eq!(res.status, 200);
    assert_eq!(
        res.headers,
        [("Content-Type".to_owned(), "application/json".to_owned())]
    );
    let body: serde_json::Value = serde_json::from_slice(&res.body)?;
    assert_eq!(
        body,
        serde_json::json!({"user": "u1", "ip": "ab".repeat(16)})
    );
    let res = host.handle_route("router", route_req("GET", "/go")).await?;
    assert_eq!(res.status, 303);
    // Unknown path, wrong method, unknown plugin.
    assert!(matches!(
        host.route("router", "/nope"),
        Err(PluginError::RouteNotFound { .. })
    ));
    assert!(matches!(
        host.handle_route("router", route_req("POST", "/hello"))
            .await,
        Err(PluginError::RouteNotFound { .. })
    ));
    assert!(matches!(
        host.route("nope", "/hello"),
        Err(PluginError::UnknownPlugin(_))
    ));
    // A disabled plugin has no routes.
    host.disable("router", "test");
    assert!(matches!(
        host.route("router", "/hello"),
        Err(PluginError::Disabled { .. })
    ));
    assert!(host.failures().is_empty(), "{:?}", host.failures());
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

/// Install `name` with its `render.head` priority set to `priority`.
fn install_at(host: &PluginHost, name: &str, priority: i32) -> TestResult {
    let manifest = plugin_manifest(name).replace(
        "\"render.head\" = 10",
        &format!("\"render.head\" = {priority}"),
    );
    host.install(Manifest::parse(&manifest)?, &plugin_wasm(name))?;
    Ok(())
}

/// `content` of every meta tag, in output order (hello adds `ferrite-hello`; the order
/// fixtures add their own name).
async fn chain(host: &PluginHost) -> Vec<String> {
    match host
        .apply_filter("render.head", FilterValue::Head(Head::default()), &req())
        .await
    {
        FilterValue::Head(h) => h.metas.into_iter().map(|m| m.content).collect(),
        other => vec![format!("unexpected {other:?}")],
    }
}

#[tokio::test]
async fn filters_run_by_priority_then_name() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    // Installed in an order that matches neither priority nor name order.
    install_at(&host, "order-a", 10)?;
    install_at(&host, "hello", 10)?;
    install_at(&host, "order-b", 5)?;
    // order-b (5) first; hello and order-a tie at 10, broken by name.
    assert_eq!(chain(&host).await, ["order-b", "ferrite-hello", "order-a"]);

    // Reinstalling order-b at 20 moves it to the end of the chain.
    install_at(&host, "order-b", 20)?;
    assert_eq!(chain(&host).await, ["ferrite-hello", "order-a", "order-b"]);
    Ok(())
}
