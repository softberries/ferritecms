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

/// Linker with allowed WASI (p2), the shared `types` interface, and exactly the
/// granted `ferrite:plugin` interfaces.
pub(crate) fn build_linker(
    engine: &Engine,
    granted: &BTreeSet<Iface>,
) -> Result<Linker<HostState>, PluginError> {
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(engine_err)?;
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
