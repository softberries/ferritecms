//! Install-time check of a component's imports and exports against its manifest.

use std::collections::BTreeSet;

use wasmtime::Engine;
use wasmtime::component::Component;

use crate::capability::{Iface, granted_interfaces};
use crate::error::InspectError;
use crate::manifest::Manifest;

/// What a component imports and exports, after passing the capability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentReport {
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub ferrite_imports: BTreeSet<Iface>,
    pub world_version: String,
}

const GUEST_PREFIX: &str = "ferrite:plugin/guest@";
const FERRITE_PREFIX: &str = "ferrite:plugin/";
const TYPES_IMPORT: &str = "ferrite:plugin/types@0.1.0";

/// Allowed WASI interfaces (design 05 §5), compared without the `@version` suffix.
const ALLOWED_WASI: &[&str] = &[
    "wasi:cli/stdout",
    "wasi:cli/stderr",
    "wasi:cli/stdin",
    "wasi:cli/environment",
    "wasi:cli/exit",
    "wasi:cli/terminal-input",
    "wasi:cli/terminal-output",
    "wasi:cli/terminal-stdin",
    "wasi:cli/terminal-stdout",
    "wasi:cli/terminal-stderr",
];
const ALLOWED_WASI_PACKAGES: &[&str] = &[
    "wasi:clocks/",
    "wasi:random/",
    "wasi:io/",
    "wasi:filesystem/",
];

fn capability_name(iface: Iface) -> &'static str {
    match iface {
        Iface::Log => "log",
        Iface::Content => "content.read",
        Iface::Meta => "content.meta",
        Iface::Options => "options.read",
        Iface::Kv => "kv",
        Iface::Http => "http",
        Iface::Settings => "settings",
        Iface::Mail => "mail",
    }
}

fn strip_version(name: &str) -> &str {
    name.split_once('@').map_or(name, |(n, _)| n)
}

fn wasi_allowed(name: &str) -> bool {
    let bare = strip_version(name);
    ALLOWED_WASI.contains(&bare) || ALLOWED_WASI_PACKAGES.iter().any(|p| bare.starts_with(p))
}

fn all_ifaces() -> [Iface; 8] {
    [
        Iface::Log,
        Iface::Content,
        Iface::Meta,
        Iface::Options,
        Iface::Kv,
        Iface::Http,
        Iface::Settings,
        Iface::Mail,
    ]
}

/// Compile `wasm` and verify every import is allowed for `manifest`.
pub fn inspect(
    engine: &Engine,
    wasm: &[u8],
    manifest: &Manifest,
) -> Result<ComponentReport, InspectError> {
    if wasm.starts_with(b"\0asm\x01\0\0\0") {
        return Err(InspectError::NotAComponent);
    }
    let component =
        Component::new(engine, wasm).map_err(|e| InspectError::Invalid(format!("{e:#}")))?;
    let ty = component.component_type();
    let imports: Vec<String> = ty.imports(engine).map(|(n, _)| n.to_string()).collect();
    let exports: Vec<String> = ty.exports(engine).map(|(n, _)| n.to_string()).collect();

    let granted = granted_interfaces(&manifest.capabilities);
    let mut ferrite_imports = BTreeSet::new();
    for import in &imports {
        if import.starts_with(FERRITE_PREFIX) {
            if import == TYPES_IMPORT {
                continue;
            }
            let iface = all_ifaces()
                .into_iter()
                .find(|i| i.wit_name() == import)
                .ok_or_else(|| InspectError::ForbiddenImport(import.clone()))?;
            if !granted.contains(&iface) {
                return Err(InspectError::UndeclaredImport {
                    import: import.clone(),
                    capability: capability_name(iface),
                });
            }
            ferrite_imports.insert(iface);
        } else if !wasi_allowed(import) {
            return Err(InspectError::ForbiddenImport(import.clone()));
        }
    }

    let guest = exports
        .iter()
        .find_map(|e| e.strip_prefix(GUEST_PREFIX))
        .ok_or(InspectError::MissingGuestExport)?;
    if guest != "0.1.0" {
        return Err(InspectError::UnsupportedWorld(guest.to_string()));
    }
    let world_version = guest.to_string();

    Ok(ComponentReport {
        imports,
        exports,
        ferrite_imports,
        world_version,
    })
}
