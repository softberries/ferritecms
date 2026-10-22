//! Install-time check of a component's imports and exports against its manifest.

use std::collections::BTreeSet;

use wasmtime::Engine;
use wasmtime::component::Component;

use crate::capability::{Iface, granted_interfaces};
use crate::error::InspectError;
use crate::manifest::Manifest;
use crate::output::sanitize_guest_text;

/// What a component imports and exports, after passing the capability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentReport {
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub ferrite_imports: BTreeSet<Iface>,
    pub world_version: String,
}

const GUEST_PREFIX: &str = "ferrite:plugin/guest@";
const SUPPORTED_WORLD: &str = "0.1.0";
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

fn wasi_allowed(name: &str) -> bool {
    let Some((bare, version)) = name.split_once('@') else {
        return false;
    };
    if !version.starts_with("0.2.") {
        return false;
    }
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
    inspect_component(engine, wasm, manifest).map(|(_, report)| report)
}

/// [`inspect`], also returning the compiled component so install compiles only once.
pub(crate) fn inspect_component(
    engine: &Engine,
    wasm: &[u8],
    manifest: &Manifest,
) -> Result<(Component, ComponentReport), InspectError> {
    if wasm.starts_with(b"\0asm\x01\0\0\0") {
        return Err(InspectError::NotAComponent);
    }
    let component = Component::new(engine, wasm)
        .map_err(|e| InspectError::Invalid(sanitize_guest_text(&format!("{e:#}"))))?;
    let ty = component.component_type();
    let imports: Vec<String> = ty.imports(engine).map(|(n, _)| n.to_string()).collect();
    let exports: Vec<String> = ty.exports(engine).map(|(n, _)| n.to_string()).collect();

    let granted = granted_interfaces(&manifest.capabilities);
    let mut ferrite_imports = BTreeSet::new();
    for import in &imports {
        if let Some(iface) = classify_import(import, &granted)? {
            ferrite_imports.insert(iface);
        }
    }
    let world_version = select_world(&exports)?;

    let report = ComponentReport {
        imports,
        exports,
        ferrite_imports,
        world_version,
    };
    Ok((component, report))
}

/// Decide one import: `Ok(Some(iface))` for a granted ferrite interface, `Ok(None)` for
/// allowed WASI or the shared types interface, `Err` otherwise.
fn classify_import(import: &str, granted: &BTreeSet<Iface>) -> Result<Option<Iface>, InspectError> {
    if import == TYPES_IMPORT {
        return Ok(None);
    }
    if import.starts_with(FERRITE_PREFIX) {
        let iface = all_ifaces()
            .into_iter()
            .find(|i| i.wit_name() == import)
            .ok_or_else(|| InspectError::ForbiddenImport(sanitize_guest_text(import)))?;
        if !granted.contains(&iface) {
            return Err(InspectError::UndeclaredImport {
                import: import.to_string(),
                capability: capability_name(iface),
            });
        }
        return Ok(Some(iface));
    }
    if wasi_allowed(import) {
        Ok(None)
    } else {
        Err(InspectError::ForbiddenImport(sanitize_guest_text(import)))
    }
}

/// Pick the guest world version from the export names, preferring the supported one.
fn select_world(exports: &[String]) -> Result<String, InspectError> {
    let mut other = None;
    for version in exports.iter().filter_map(|e| e.strip_prefix(GUEST_PREFIX)) {
        if version == SUPPORTED_WORLD {
            return Ok(version.to_string());
        }
        other.get_or_insert(version);
    }
    match other {
        Some(v) => Err(InspectError::UnsupportedWorld(sanitize_guest_text(v))),
        None => Err(InspectError::MissingGuestExport),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn granted() -> BTreeSet<Iface> {
        BTreeSet::from([Iface::Log, Iface::Settings])
    }

    #[test]
    fn rejects_adversarial_names() {
        for name in [
            "wasi:cli/stdout-evil@0.2.0",
            "wasi:clocksX/foo@0.2.0",
            "ferrite:plugin/kv@0.1.0x",
            "ferrite:plugin/log",
            "ferrite:plugin/log@0.2.0",
            "ferrite:pluginx/log@0.1.0",
            "wasi:sockets/tcp@0.2.0",
            "wasi:http/types@0.2.0",
            "wasi:cli/run@0.2.0",
            "wasi:cli/stdout",
            "wasi:cli/stdout@0.3.0",
            "wasi:io/streams",
            "wasi:io/streams@0.3.0",
        ] {
            assert_eq!(
                classify_import(name, &granted()),
                Err(InspectError::ForbiddenImport(name.to_string())),
                "{name}"
            );
        }
    }

    #[test]
    fn accepts_allowed_names() {
        for name in [
            "wasi:cli/stdout@0.2.9",
            "wasi:io/streams@0.2.0",
            "ferrite:plugin/types@0.1.0",
        ] {
            assert_eq!(classify_import(name, &granted()), Ok(None), "{name}");
        }
        assert_eq!(
            classify_import("ferrite:plugin/log@0.1.0", &granted()),
            Ok(Some(Iface::Log))
        );
        let with_kv = BTreeSet::from([Iface::Log, Iface::Kv]);
        assert_eq!(
            classify_import("ferrite:plugin/kv@0.1.0", &with_kv),
            Ok(Some(Iface::Kv))
        );
    }

    #[test]
    fn ungranted_kv_is_undeclared() {
        assert_eq!(
            classify_import("ferrite:plugin/kv@0.1.0", &granted()),
            Err(InspectError::UndeclaredImport {
                import: "ferrite:plugin/kv@0.1.0".into(),
                capability: "kv"
            })
        );
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn world_selection() {
        assert_eq!(
            select_world(&names(&["ferrite:plugin/guest@0.1.0"])),
            Ok("0.1.0".into())
        );
        assert_eq!(
            select_world(&names(&[
                "ferrite:plugin/guest@0.2.0",
                "ferrite:plugin/guest@0.1.0"
            ])),
            Ok("0.1.0".into())
        );
        assert_eq!(
            select_world(&names(&["ferrite:plugin/guest@0.2.0"])),
            Err(InspectError::UnsupportedWorld("0.2.0".into()))
        );
        assert_eq!(
            select_world(&names(&["other:thing/guest@0.1.0"])),
            Err(InspectError::MissingGuestExport)
        );
        assert_eq!(select_world(&[]), Err(InspectError::MissingGuestExport));
    }
}
