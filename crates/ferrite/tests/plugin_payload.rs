//! `.github/scripts/plugin-payload.sh` prints the same signing payload as
//! `ferrite_plugin_host::trust::signing_payload` (M6-2, design 14 §2.1).

use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/scripts/plugin-payload.sh")
}

#[test]
fn script_matches_signing_payload() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = b"[plugin]\nname = \"x\"\n";
    let wasm = b"\0asm\x01\0\0\0 some bytes";
    std::fs::write(dir.path().join("ferrite-plugin.toml"), manifest).unwrap();
    std::fs::write(dir.path().join("x.wasm"), wasm).unwrap();
    let out = Command::new("bash")
        .arg(script())
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        ferrite_plugin_host::trust::signing_payload(manifest, wasm)
    );
}

#[test]
fn script_refuses_a_folder_without_exactly_one_wasm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ferrite-plugin.toml"), b"x").unwrap();
    let run = || {
        Command::new("bash")
            .arg(script())
            .arg(dir.path())
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    std::fs::write(dir.path().join("a.wasm"), b"a").unwrap();
    std::fs::write(dir.path().join("b.wasm"), b"b").unwrap();
    let out = run();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("exactly one"));
}
