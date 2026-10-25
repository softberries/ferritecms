//! The test seams (`NodeOptions::boot_barrier`, `Egress::for_tests`, …) sit behind the
//! dev-only `test-seams` feature (10-testing §2): each crate's dev-dependency on itself
//! enables it, so tests compile the seams and `cargo build -p ferrite --release` (the
//! Docker image) does not.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

/// `cargo tree -p ferrite -e features` plus `extra` edge kinds, as text.
fn feature_tree(edges: &str) -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["tree", "-p", "ferrite", "-e", edges, "--color", "never"])
        .output()
        .expect("cargo tree runs");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn release_build_has_no_seams() {
    let normal = feature_tree("features,normal");
    assert!(
        !normal.contains("test-seams"),
        "a normal build enables a test-seams feature:\n{normal}"
    );
    // Not vacuous: the dev edges (what tests build) do enable both crates' seams.
    let dev = feature_tree("features,normal,dev");
    for krate in ["ferrite", "ferrite-http"] {
        assert!(
            dev.contains(&format!("{krate} feature \"test-seams\"")),
            "tests do not enable {krate}/test-seams:\n{dev}"
        );
    }
}
