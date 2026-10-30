//! Embeds the target triple (`ferrite::binary_target()`, M4-2 P27's `FERRITE_TARGET`),
//! which `self-update` uses to pick its release asset.
//!
//! It goes through a file in `OUT_DIR`, not `cargo:rustc-env`: cargo also exports
//! `rustc-env` values into the environment of `cargo run` and `cargo test` processes, and
//! a `FERRITE_*` variable there is read as configuration ("unknown key").

use std::path::PathBuf;

fn main() {
    // Cargo sets both for every build script. `TARGET` is the triple being compiled for,
    // not the build host's, so a cross build embeds the right one. A missing value fails
    // the build: a binary with a made-up triple would find no release asset.
    let Ok(target) = std::env::var("TARGET") else {
        panic!("TARGET is not set: build.rs must run under cargo");
    };
    let Some(out) = std::env::var_os("OUT_DIR") else {
        panic!("OUT_DIR is not set: build.rs must run under cargo");
    };
    let path = PathBuf::from(out).join("target-triple");
    if let Err(e) = std::fs::write(&path, target) {
        panic!("cannot write the target triple to {}: {e}", path.display());
    }
    println!("cargo:rerun-if-changed=build.rs");
}
