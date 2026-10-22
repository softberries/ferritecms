mod common;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::Manifest;
use wasmtime::Engine;
use wasmtime::component::Component;

#[test]
fn hello_wasm_is_a_component() {
    let wasm = plugin_wasm("hello");
    Component::new(&Engine::default(), wasm).expect("hello.wasm is a valid component");
    Manifest::parse(&plugin_manifest("hello")).expect("hello manifest parses");
}
