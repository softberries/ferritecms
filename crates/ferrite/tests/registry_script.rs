//! `.github/scripts/registry-index.py` (M6-2, design 14 §11) writes an index the binary
//! reads: two runs (serial 1, then 2 with the first as previous) over a staged folder
//! signed with a throwaway key, signed with a throwaway registry key, checked with
//! `trust::verify_index`, `RegistryIndex::parse` and `verify_bundle`. One format, two
//! writers, one reader. Without `python3` it prints `SKIP` (and fails when `CI=true`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use ferrite_domain::plugin_trust::PublisherKey;
use ferrite_plugin_host::trust::{signing_payload, verify_bundle, verify_index};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python() -> Option<&'static str> {
    let ok = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        return Some("python3");
    }
    assert!(
        std::env::var("CI").is_err(),
        "python3 is required in CI for registry_script"
    );
    println!("SKIP: python3 not found");
    None
}

fn key() -> (minisign::KeyPair, PublisherKey) {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let pk = PublisherKey::parse(&kp.pk.to_box().unwrap().into_string())
        .unwrap()
        .unwrap();
    (kp, pk)
}

fn sign(kp: &minisign::KeyPair, data: &[u8]) -> String {
    minisign::sign(Some(&kp.pk), &kp.sk, data, None, None)
        .unwrap()
        .into_string()
}

fn stage(dir: &Path, name: &str, version: &str, kp: &minisign::KeyPair) {
    let folder = dir.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    let manifest = format!(
        "[plugin]\nname = \"{name}\"\nversion = \"{version}\"\npublisher = \"ferritecms.dev\"\n\
         world = \"ferrite:plugin/plugin@0.1.0\"\ndescription = \"The {name} plugin\"\n\
         license = \"AGPL-3.0-only\"\n"
    );
    let wasm = format!("\0asm {name} {version}").into_bytes();
    std::fs::write(folder.join("ferrite-plugin.toml"), &manifest).unwrap();
    std::fs::write(folder.join(format!("{name}.wasm")), &wasm).unwrap();
    std::fs::write(
        folder.join("ferrite-plugin.minisig"),
        sign(kp, signing_payload(manifest.as_bytes(), &wasm).as_bytes()),
    )
    .unwrap();
}

fn run(
    py: &str,
    tmp: &Path,
    previous: &str,
    version: &str,
    staged: &Path,
    out: &Path,
    extra: &Path,
) {
    let out = Command::new(py)
        .arg(root().join(".github/scripts/registry-index.py"))
        .args(["--previous", previous])
        .arg("--publishers")
        .arg(root().join("registry/publishers.json"))
        .arg("--yanked")
        .arg(tmp.join("yanked.json"))
        .args(["--version", version])
        .args([
            "--base-url",
            &format!("https://github.com/softberries/ferritecms/releases/download/v{version}"),
        ])
        .arg("--plugins")
        .arg(staged)
        .arg("--out")
        .arg(out)
        .arg("--extra-key")
        .arg(extra)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn script_output_parses_and_verifies() {
    let Some(py) = python() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (publisher, publisher_pk) = key();
    let (registry, registry_pk) = key();
    let pub_file = tmp.path().join("plugins.pub");
    std::fs::write(&pub_file, publisher_pk.to_file()).unwrap();
    std::fs::write(
        tmp.path().join("yanked.json"),
        r#"[{"name": "seo", "version": "0.1.0", "reason": "broken"}]"#,
    )
    .unwrap();

    let first = tmp.path().join("first");
    stage(&first, "seo", "0.1.0", &publisher);
    stage(&first, "analytics", "0.1.0", &publisher);
    let one = tmp.path().join("one.json");
    run(py, tmp.path(), "-", "0.1.0", &first, &one, &pub_file);
    let second = tmp.path().join("second");
    stage(&second, "seo", "0.2.0", &publisher);
    stage(&second, "analytics", "0.2.0", &publisher);
    let two = tmp.path().join("two.json");
    run(
        py,
        tmp.path(),
        one.to_str().unwrap(),
        "0.2.0",
        &second,
        &two,
        &pub_file,
    );

    for (path, serial) in [(&one, 1), (&two, 2)] {
        let bytes = std::fs::read(path).unwrap();
        let sig = sign(&registry, &bytes);
        let index = verify_index(&registry_pk, &bytes, &sig).unwrap();
        assert_eq!(index.serial, serial);
        assert_eq!(
            index.publisher_of_key(&publisher_pk.key_id).unwrap().id,
            "ferritecms.dev"
        );
    }
    let bytes = std::fs::read(&two).unwrap();
    let index = verify_index(&registry_pk, &bytes, &sign(&registry, &bytes)).unwrap();
    let seo = index.plugin("seo").unwrap();
    assert_eq!(seo.versions.len(), 2);
    assert_eq!(seo.versions[0].version, "0.2.0", "newest first");
    // registry/yanked.json is applied to every listed version.
    assert_eq!(
        index.version("seo", "0.1.0").unwrap().yanked.as_deref(),
        Some("broken")
    );
    assert!(
        seo.versions[0]
            .manifest
            .url
            .ends_with("/ferrite-plugin-seo-0.2.0.toml")
    );
    // Every version's signature verifies over the staged files.
    for (dir, version) in [(&first, "0.1.0"), (&second, "0.2.0")] {
        for name in ["seo", "analytics"] {
            let m = std::fs::read(dir.join(name).join("ferrite-plugin.toml")).unwrap();
            let w = std::fs::read(dir.join(name).join(format!("{name}.wasm"))).unwrap();
            let v = index.version(name, version).unwrap();
            assert_eq!(v.manifest.sha256, ferrite_domain::to_hex(&sha(&m)));
            verify_bundle(&m, &w, &v.signature, std::slice::from_ref(&publisher_pk)).unwrap();
        }
    }
}

#[test]
fn script_refuses_a_key_the_publisher_does_not_list() {
    let Some(py) = python() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (publisher, _) = key();
    let (_, other_pk) = key();
    let other = tmp.path().join("other.pub");
    std::fs::write(&other, other_pk.to_file()).unwrap();
    std::fs::write(tmp.path().join("yanked.json"), "[]").unwrap();
    let staged = tmp.path().join("staged");
    stage(&staged, "seo", "0.1.0", &publisher);
    let out = Command::new(py)
        .arg(root().join(".github/scripts/registry-index.py"))
        .args([
            "--previous",
            "-",
            "--version",
            "0.1.0",
            "--base-url",
            "https://x.example",
        ])
        .arg("--publishers")
        .arg(root().join("registry/publishers.json"))
        .arg("--yanked")
        .arg(tmp.path().join("yanked.json"))
        .arg("--plugins")
        .arg(&staged)
        .arg("--out")
        .arg(tmp.path().join("out.json"))
        .arg("--extra-key")
        .arg(&other)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a current key"));
}

fn sha(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}
