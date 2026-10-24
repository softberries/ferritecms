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
