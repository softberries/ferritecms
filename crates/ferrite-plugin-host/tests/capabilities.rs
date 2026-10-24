mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::engine::build_engine;
use ferrite_plugin_host::{
    ActionEvent, HostConfig, HostServices, Iface, InMemoryServices, InspectError, Manifest,
    PluginError, PluginHost, Post, inspect,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn manifest(name: &str) -> TestResult<Manifest> {
    Ok(Manifest::parse(&plugin_manifest(name))?)
}

fn engine() -> TestResult<wasmtime::Engine> {
    Ok(build_engine(&HostConfig::default())?)
}

#[test]
fn hello_passes_inspection() -> TestResult {
    let report = inspect(&engine()?, &plugin_wasm("hello"), &manifest("hello")?)?;
    assert_eq!(report.ferrite_imports, BTreeSet::from([Iface::Log]));
    assert!(
        report
            .exports
            .iter()
            .any(|e| e == "ferrite:plugin/guest@0.1.0")
    );
    assert_eq!(report.world_version, "0.1.0");
    Ok(())
}

#[test]
fn undeclared_http_import_rejected() -> TestResult {
    let err = inspect(
        &engine()?,
        &plugin_wasm("undeclared-import"),
        &manifest("undeclared-import")?,
    )
    .expect_err("must be rejected");
    assert_eq!(
        err,
        InspectError::UndeclaredImport {
            import: "ferrite:plugin/http@0.1.0".into(),
            capability: "http"
        }
    );
    Ok(())
}

#[test]
fn sockets_import_rejected() -> TestResult {
    let err = inspect(&engine()?, &plugin_wasm("sockets"), &manifest("sockets")?)
        .expect_err("must be rejected");
    match err {
        InspectError::ForbiddenImport(s) => assert!(s.starts_with("wasi:sockets/"), "{s}"),
        other => panic!("unexpected error: {other:?}"),
    }
    Ok(())
}

#[test]
fn garbage_bytes_rejected() -> TestResult {
    let engine = engine()?;
    let m = manifest("hello")?;
    let inputs: [Vec<u8>; 3] = [Vec::new(), b"\0asm\x01\0\0\0".to_vec(), vec![0xAB; 1024]];
    for bytes in inputs {
        let err = inspect(&engine, &bytes, &m).expect_err("must be rejected");
        assert!(
            matches!(err, InspectError::NotAComponent | InspectError::Invalid(_)),
            "{err:?}"
        );
    }
    Ok(())
}

fn item(id: &str, kind: &str, status: &str) -> Post {
    Post {
        id: id.into(),
        kind: kind.into(),
        slug: id.into(),
        title: id.into(),
        excerpt: String::new(),
        status: status.into(),
        published: None,
        url: String::new(),
        author: String::new(),
        terms: Vec::new(),
    }
}

/// P23: `meta.set` only reaches items of the types `content.read` grants (meta-writer:
/// `["post"]`); the item's type is looked up whatever its status.
#[tokio::test]
async fn meta_set_scoped_to_read_types() -> TestResult {
    let services = Arc::new(InMemoryServices::new());
    services.insert_post(item("p1", "post", "publish"));
    services.insert_post(item("g1", "page", "publish"));
    let host = PluginHost::new(HostConfig::default(), services.clone())?;
    host.install(manifest("meta-writer")?, &plugin_wasm("meta-writer"))?;
    let saved = |id: &str| ActionEvent {
        subject: Some(id.into()),
        payload: serde_json::json!({}),
    };
    let refused = host
        .dispatch_action("meta-writer", "post.saved", saved("g1"))
        .await;
    assert!(
        matches!(&refused, Err(PluginError::CallFailed { reason, .. }) if reason.contains("no such post")),
        "{refused:?}"
    );
    assert_eq!(services.meta_get("g1", "seo.seen").await, None);
    // A missing item answers the same.
    let missing = host
        .dispatch_action("meta-writer", "post.saved", saved("nope"))
        .await;
    assert!(missing.is_err(), "{missing:?}");
    host.dispatch_action("meta-writer", "post.saved", saved("p1"))
        .await?;
    assert_eq!(
        services.meta_get("p1", "seo.seen").await.as_deref(),
        Some("\"action\"")
    );
    Ok(())
}

fn fetch_route(query: &[(&str, &str)]) -> ferrite_plugin_host::RouteRequest {
    ferrite_plugin_host::RouteRequest {
        method: "GET".into(),
        path: "/get".into(),
        query: query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "h".into(),
        user: None,
    }
}

/// M2a-2b Task 11: the host's `http.fetch` checks (budget, method, URL, scheme,
/// userinfo, port, listed host, headers, body) answer the guest with an error before any
/// service call, and never count as a plugin failure. `fetcher` lists `api.example.test`.
#[tokio::test]
async fn fetch_preflight_refusals() -> TestResult {
    let services = Arc::new(InMemoryServices::new());
    let host = PluginHost::new(HostConfig::default(), services.clone())?;
    host.install(manifest("fetcher")?, &plugin_wasm("fetcher"))?;
    let big = (1024 * 1024 + 1).to_string();
    let long_value = format!("x:{}", "v".repeat(4096));
    let cases: Vec<(Vec<(&str, &str)>, &str)> = vec![
        (
            vec![("u", "http://api.example.test/")],
            "err only https URLs are allowed",
        ),
        (
            vec![("u", "https://api.example.test:8443/")],
            "err only port 443 is allowed",
        ),
        (
            vec![("u", "https://user:pw@api.example.test/")],
            "err URLs with credentials are not allowed",
        ),
        (
            vec![("u", "https://user@api.example.test/")],
            "err URLs with credentials are not allowed",
        ),
        (
            vec![("u", "https://other.example.test/")],
            "err host not allowed",
        ),
        (
            vec![("u", "https://api.example.test.evil.test/")],
            "err host not allowed",
        ),
        (
            vec![("u", "https://169.254.169.254/")],
            "err host not allowed",
        ),
        (vec![("u", "https://[::1]/")], "err host not allowed"),
        (vec![("u", "not a url")], "err invalid URL"),
        (
            vec![("u", "ftp://api.example.test/")],
            "err only https URLs are allowed",
        ),
        (
            vec![("u", "https://api.example.test/"), ("m", "TRACE")],
            "err method not allowed",
        ),
        (
            vec![("u", "https://api.example.test/"), ("m", "get")],
            "err method not allowed",
        ),
        (
            vec![("u", "https://api.example.test/"), ("h", "Host:evil.test")],
            "err request header not allowed",
        ),
        (
            vec![("u", "https://api.example.test/"), ("h", "cookie:a=b")],
            "err request header not allowed",
        ),
        (
            vec![
                ("u", "https://api.example.test/"),
                ("h", "Proxy-Authorization:x"),
            ],
            "err request header not allowed",
        ),
        (
            vec![
                ("u", "https://api.example.test/"),
                ("h", "Transfer-Encoding:chunked"),
            ],
            "err request header not allowed",
        ),
        (
            vec![
                ("u", "https://api.example.test/"),
                ("h", long_value.as_str()),
            ],
            "err request header over 4 KiB",
        ),
        (
            vec![
                ("u", "https://api.example.test/"),
                ("m", "POST"),
                ("b", big.as_str()),
            ],
            "err request body over 1 MiB",
        ),
    ];
    for (query, expected) in &cases {
        let res = host.handle_route("fetcher", fetch_route(query)).await?;
        assert_eq!(String::from_utf8_lossy(&res.body), *expected, "{query:?}");
    }
    let mut many: Vec<(&str, &str)> = vec![("u", "https://api.example.test/")];
    many.extend(std::iter::repeat_n(("h", "x-a:1"), 33));
    let res = host.handle_route("fetcher", fetch_route(&many)).await?;
    assert_eq!(
        String::from_utf8_lossy(&res.body),
        "err too many request headers"
    );
    assert!(
        services.http_fetches().is_empty(),
        "no refusal reached the services"
    );

    // The listed host in another spelling is the listed host; the service gets the
    // canonical URL, the plugin's name and its list.
    let res = host
        .handle_route(
            "fetcher",
            fetch_route(&[("u", "https://API.EXAMPLE.TEST./x?q=1"), ("h", "x-a:1")]),
        )
        .await?;
    assert_eq!(String::from_utf8_lossy(&res.body), "ok 200 in-memory");
    let fetches = services.http_fetches();
    assert_eq!(fetches.len(), 1);
    let (plugin, hosts, req) = &fetches[0];
    assert_eq!(plugin, "fetcher");
    assert_eq!(hosts, &["api.example.test"]);
    assert_eq!(req.method, "GET");
    assert_eq!(req.url.as_str(), "https://api.example.test/x?q=1");
    assert_eq!(req.headers, [("x-a".to_owned(), "1".to_owned())]);
    assert_eq!(req.body, None);

    // At most 3 fetches per call: the 4th is refused.
    let res = host
        .handle_route(
            "fetcher",
            fetch_route(&[("u", "https://api.example.test/"), ("n", "4")]),
        )
        .await?;
    assert_eq!(
        String::from_utf8_lossy(&res.body),
        "ok 200 in-memory\nok 200 in-memory\nok 200 in-memory\nerr budget exceeded"
    );
    assert_eq!(services.http_fetches().len(), 4);
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    assert!(host.is_enabled("fetcher"));
    Ok(())
}
