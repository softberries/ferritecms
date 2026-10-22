//! Filter-call latency (design 05 §6 / P8): fresh instance per call, with and without the
//! pooling allocator, plus bare instantiation.

#[path = "../src/test_support.rs"]
mod test_support;

use std::sync::Arc;

use anyhow::{Context, Result};
use criterion::Criterion;
use ferrite_plugin_host::engine::build_engine;
use ferrite_plugin_host::{
    FilterValue, Head, HostConfig, InMemoryServices, Manifest, PluginHost, RequestInfo,
};
use std::hint::black_box;
use test_support::{plugin_wasm, repo_root};
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

fn hello_host(pooling: bool) -> Result<PluginHost> {
    let cfg = HostConfig {
        pooling,
        ..HostConfig::default()
    };
    let host = PluginHost::new(cfg, Arc::new(InMemoryServices::new()))?;
    let manifest_path = repo_root().join("plugins/examples/hello/ferrite-plugin.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    host.install(Manifest::parse(&manifest)?, &plugin_wasm("hello"))?;
    Ok(host)
}

fn request() -> RequestInfo {
    RequestInfo {
        path: "/".into(),
        locale: "en".into(),
        is_logged_in: false,
        post: None,
    }
}

fn bench_hello(
    c: &mut Criterion,
    name: &str,
    pooling: bool,
    rt: &tokio::runtime::Runtime,
) -> Result<()> {
    let host = hello_host(pooling)?;
    let req = request();
    // A failing plugin returns its input unchanged and would make the numbers meaningless.
    let out =
        rt.block_on(host.apply_filter("render.head", FilterValue::Head(Head::default()), &req));
    match out {
        FilterValue::Head(head) if !head.metas.is_empty() => {}
        other => anyhow::bail!("hello did not add its meta tag: {other:?}"),
    }
    c.bench_function(name, |b| {
        b.to_async(rt).iter(|| async {
            black_box(
                host.apply_filter("render.head", FilterValue::Head(Head::default()), &req)
                    .await,
            )
        });
    });
    Ok(())
}

fn bench_instantiate(c: &mut Criterion, rt: &tokio::runtime::Runtime) -> Result<()> {
    let engine = build_engine(&HostConfig::default())?;
    let component = Component::new(&engine, plugin_wasm("hello"))?;
    let mut linker = Linker::<()>::new(&engine);
    linker.define_unknown_imports_as_traps(&component)?;
    let pre = linker.instantiate_pre(&component)?;
    // The epoch ticker is not running here; a far deadline keeps instantiation from trapping.
    c.bench_function("instantiate_only", |b| {
        b.to_async(rt).iter(|| async {
            let mut store = Store::new(&engine, ());
            store.set_fuel(50_000_000)?;
            store.set_epoch_deadline(1_000_000);
            let instance = pre.instantiate_async(&mut store).await?;
            Ok::<_, anyhow::Error>(black_box(instance))
        });
    });
    Ok(())
}

fn main() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut c = Criterion::default().configure_from_args();
    bench_hello(&mut c, "hello_render_head_fresh", false, &rt)?;
    bench_hello(&mut c, "hello_render_head_fresh_pooling", true, &rt)?;
    bench_instantiate(&mut c, &rt)?;
    c.final_summary();
    Ok(())
}
