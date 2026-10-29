//! Embeds the target triple (`ferrite::binary_target()`, M4-2 P27's `FERRITE_TARGET`),
//! which `self-update` uses to pick its release asset.
//!
//! It goes through a file in `OUT_DIR`, not `cargo:rustc-env`: cargo also exports
//! `rustc-env` values into the environment of `cargo run` and `cargo test` processes, and
//! a `FERRITE_*` variable there is read as configuration ("unknown key").

use std::path::PathBuf;

fn main() {
    // Cargo sets `TARGET` for build scripts: the triple being compiled for, not the build
    // host's, so a cross build embeds the right one.
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    if let Err(e) = std::fs::write(out.join("target-triple"), target) {
        panic!("cannot write the target triple to OUT_DIR: {e}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
