mod common;

use std::collections::BTreeSet;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::engine::build_engine;
use ferrite_plugin_host::{HostConfig, Iface, InspectError, Manifest, inspect};

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
