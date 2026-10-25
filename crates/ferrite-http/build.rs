//! Rebuilds this crate when the admin build (`admin/dist/`, embedded by
//! `admin::EmbeddedAdmin`) appears or changes. It never runs pnpm (07 §3).
//!
//! Cargo treats a missing `rerun-if-changed` path as changed on every build, so the
//! watched path is `admin/dist` when it exists, else `admin` when that exists (a new
//! `dist/` inside it changes it), else only this script (pre-flight F1).

use std::path::Path;

fn main() {
    let watched = ["../../admin/dist", "../../admin"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("build.rs");
    println!("cargo:rerun-if-changed={watched}");
}
