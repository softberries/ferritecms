//! Rebuilds this crate when anything under `themes/` changes, a new theme folder
//! included. The starter themes are embedded with rust-embed (`package::Embedded`); its
//! derive tracks the files it embedded, but not a folder or file added later, so a new
//! theme was missing from the binary until something else rebuilt the crate (M4-4).
//! Cargo checks a watched directory recursively; `themes/` is part of the repository.

fn main() {
    println!("cargo:rerun-if-changed=../../themes");
}
