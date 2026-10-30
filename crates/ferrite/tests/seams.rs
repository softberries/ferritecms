//! The test seams (`NodeOptions::boot_barrier`, `Egress::for_tests`,
//! `AuthService::with_clock`, …) sit behind the
//! dev-only `test-seams` feature (10-testing §2): each crate's dev-dependency on itself
//! enables it, so tests compile the seams and `cargo build -p ferrite --release` (the
//! Docker image) does not.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

/// `cargo tree --locked --offline -p ferrite <args>`: whether it succeeded, its stdout and
/// its stderr (locked and offline: a test build has fetched every dependency already).
fn cargo_tree(args: &[&str]) -> (bool, String, String) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["tree", "--locked", "--offline", "-p", "ferrite"])
        .args(["--color", "never"])
        .args(args)
        .output()
        .expect("cargo tree runs");
    (
        out.status.success(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `cargo tree -p ferrite -e <edges>` as text.
fn feature_tree(edges: &str) -> String {
    let (ok, stdout, stderr) = cargo_tree(&["-e", edges]);
    assert!(ok, "cargo tree failed: {stderr}");
    stdout
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
    for krate in ["ferrite", "ferrite-http", "ferrite-core"] {
        assert!(
            dev.contains(&format!("{krate} feature \"test-seams\"")),
            "tests do not enable {krate}/test-seams:\n{dev}"
        );
    }
}

/// `cargo tree -p ferrite -i <krate> -e normal --target all` as text: empty when `krate`
/// is not in the release binary's dependency tree on any platform.
fn inverse_tree(krate: &str) -> String {
    let (ok, stdout, stderr) = cargo_tree(&["-i", krate, "-e", "normal", "--target", "all"]);
    // A package missing from `Cargo.lock` altogether is an error, not an empty tree; one
    // that is locked but outside the normal edges prints nothing.
    if !ok && stderr.contains("did not match any packages") {
        return String::new();
    }
    assert!(ok, "cargo tree failed: {stderr}");
    stdout
}

/// The release binary links only the `ring` crypto provider (M4-2 P28): aws-lc's C code
/// would need cmake and clang on every release target.
#[test]
fn no_aws_lc_in_tree() {
    for krate in ["aws-lc-sys", "aws-lc-rs"] {
        let tree = inverse_tree(krate);
        assert!(
            tree.trim().is_empty(),
            "{krate} is in the ferrite dependency tree:\n{tree}"
        );
    }
    // Not vacuous: the same query finds the provider that is linked.
    assert!(!inverse_tree("ring").trim().is_empty());
}

/// Tests run on the host they were built for, so the embedded triple is the host's
/// (M4-2 P27: `self-update` picks its asset by it). A `cargo test --target <non-host>` run
/// would fail here although the binary is right; CI tests only host targets.
#[test]
fn build_target_is_embedded() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .arg("-vV")
        .output()
        .expect("rustc -vV runs");
    let info = String::from_utf8(out.stdout).unwrap();
    let host = info
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("rustc -vV prints the host");
    assert_eq!(ferrite::binary_target(), host);
    // Embedded, not exported: cargo puts `rustc-env` values into the environment of
    // `cargo run`/`cargo test`, where a `FERRITE_*` name is read as configuration.
    assert!(std::env::var_os("FERRITE_TARGET").is_none());
}
