//! Linker construction: only the interfaces a plugin was granted are defined, so an
//! ungranted import fails at `instantiate_pre` even if inspection were bypassed.

use std::collections::BTreeSet;

use wasmtime::Engine;
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::v0_1::ferrite::plugin::{
    content, http, kv, log, mail, meta, options, settings, types,
};
use crate::capability::Iface;
use crate::error::PluginError;
use crate::state::HostState;

fn engine_err(e: wasmtime::Error) -> PluginError {
    PluginError::Engine(format!("{e:#}"))
}

/// The allowed WASI p2 subset (design 05 §5): io, clocks, random, cli (incl. terminal-*)
/// and filesystem (no preopens are configured). `wasi:sockets/*` and `wasi:http/*` are
/// deliberately not linked, unlike `wasmtime_wasi::p2::add_to_linker_async`, which also
/// adds sockets.
fn add_wasi_subset(l: &mut Linker<HostState>) -> wasmtime::Result<()> {
    use wasmtime_wasi::cli::{WasiCli, WasiCliView};
    use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
    use wasmtime_wasi::filesystem::{WasiFilesystem, WasiFilesystemView};
    use wasmtime_wasi::p2::bindings::{cli, clocks, filesystem, random};
    use wasmtime_wasi::random::WasiRandom;

    type S = HostState;
    wasmtime_wasi_io::add_to_linker_async(l)?;
    clocks::wall_clock::add_to_linker::<S, WasiClocks>(l, S::clocks)?;
    clocks::monotonic_clock::add_to_linker::<S, WasiClocks>(l, S::clocks)?;
    random::random::add_to_linker::<S, WasiRandom>(l, |s| s.wasi.random())?;
    random::insecure::add_to_linker::<S, WasiRandom>(l, |s| s.wasi.random())?;
    random::insecure_seed::add_to_linker::<S, WasiRandom>(l, |s| s.wasi.random())?;
    cli::exit::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::environment::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::stdin::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::stdout::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::stderr::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::terminal_input::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::terminal_output::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::terminal_stdin::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::terminal_stdout::add_to_linker::<S, WasiCli>(l, S::cli)?;
    cli::terminal_stderr::add_to_linker::<S, WasiCli>(l, S::cli)?;
    filesystem::types::add_to_linker::<S, WasiFilesystem>(l, S::filesystem)?;
    filesystem::preopens::add_to_linker::<S, WasiFilesystem>(l, S::filesystem)?;
    Ok(())
}

/// Linker with the allowed WASI subset, the shared `types` interface, and exactly the
/// granted `ferrite:plugin` interfaces.
pub(crate) fn build_linker(
    engine: &Engine,
    granted: &BTreeSet<Iface>,
) -> Result<Linker<HostState>, PluginError> {
    let mut linker = Linker::new(engine);
    add_wasi_subset(&mut linker).map_err(engine_err)?;
    types::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |s| s).map_err(engine_err)?;
    for iface in granted {
        let l = &mut linker;
        match iface {
            Iface::Log => log::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Content => content::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Meta => meta::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Options => options::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Kv => kv::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Http => http::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Settings => settings::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
            Iface::Mail => mail::add_to_linker::<_, HasSelf<HostState>>(l, |s| s),
        }
        .map_err(engine_err)?;
    }
    Ok(linker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HostConfig;
    use crate::engine::build_engine;
    use crate::test_support::plugin_wasm;
    use wasmtime::component::Component;

    #[test]
    fn instantiate_pre_fails_without_granted_http() {
        let engine = build_engine(&HostConfig::default()).expect("engine");
        let linker =
            build_linker(&engine, &BTreeSet::from([Iface::Log, Iface::Settings])).expect("linker");
        let component =
            Component::new(&engine, plugin_wasm("undeclared-import")).expect("component");
        let err = linker
            .instantiate_pre(&component)
            .err()
            .expect("http import must not resolve");
        let msg = format!("{err:#}");
        assert!(msg.contains("ferrite:plugin/http"), "{msg}");
    }

    #[test]
    fn sockets_are_not_linked() {
        let engine = build_engine(&HostConfig::default()).expect("engine");
        let all = BTreeSet::from([
            Iface::Log,
            Iface::Content,
            Iface::Meta,
            Iface::Options,
            Iface::Kv,
            Iface::Http,
            Iface::Settings,
            Iface::Mail,
        ]);
        let linker = build_linker(&engine, &all).expect("linker");
        let component = Component::new(&engine, plugin_wasm("sockets")).expect("component");
        let err = linker
            .instantiate_pre(&component)
            .err()
            .expect("sockets import must not resolve");
        let msg = format!("{err:#}");
        assert!(msg.contains("wasi:sockets"), "{msg}");
    }

    #[test]
    fn granted_http_links() {
        let engine = build_engine(&HostConfig::default()).expect("engine");
        let linker = build_linker(
            &engine,
            &BTreeSet::from([Iface::Log, Iface::Settings, Iface::Http]),
        )
        .expect("linker");
        let component =
            Component::new(&engine, plugin_wasm("undeclared-import")).expect("component");
        linker
            .instantiate_pre(&component)
            .expect("all imports resolve");
    }
}
