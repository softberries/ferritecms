//! `ferrite plugin` with signatures and the registry (M6-2, design 14 §9): signed
//! folders, `install NAME[@VERSION]`, `search`, `update`, `digest`, `keys` and `unbind`,
//! on a SQLite site with a `file://` registry and throwaway keys.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;

#[path = "../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

#[path = "../../ferrite-http/tests/support/registry_fixture.rs"]
mod registry_fixture;

use registry_fixture::{FileRegistryFixture, TestKey};

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn init(dir: &Path) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(dir)
        .assert()
        .success();
}

/// A site whose `[plugins]` uses the fixture's registry.
fn site_with_registry(tmp: &Path, fx: &FileRegistryFixture) -> PathBuf {
    let site = tmp.join("site");
    init(&site);
    let path = site.join("ferrite.toml");
    let config = std::fs::read_to_string(&path).unwrap().replacen(
        "[plugins]\n",
        &format!(
            "[plugins]\nregistry = \"{}\"\nregistry_key = \"{}\"\n",
            fx.url(),
            fx.registry_key.key.base64
        ),
        1,
    );
    std::fs::write(&path, config).unwrap();
    site
}

fn plugin(site: &Path, args: &[&str]) -> Command {
    let mut c = ferrite();
    c.arg("plugin").args(args).arg("--data").arg(site);
    c
}

fn out(mut c: Command, code: i32) -> (String, String) {
    let o = c.assert().code(code).get_output().clone();
    (
        String::from_utf8(o.stdout).unwrap(),
        String::from_utf8(o.stderr).unwrap(),
    )
}

fn hello() -> (String, Vec<u8>) {
    let m = std::fs::read_to_string(
        test_support::repo_root().join("plugins/examples/hello/ferrite-plugin.toml"),
    )
    .unwrap();
    (m, test_support::plugin_wasm("hello"))
}

/// A folder with hello's files, signed by `key` (with its public key file when `pub_file`).
fn signed_dir(parent: &Path, key: Option<&TestKey>, pub_file: bool) -> PathBuf {
    let dir = parent.join("hello-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let (m, w) = hello();
    std::fs::write(dir.join("ferrite-plugin.toml"), &m).unwrap();
    std::fs::write(dir.join("hello.wasm"), &w).unwrap();
    if let Some(key) = key {
        let payload = ferrite_plugin_host::trust::signing_payload(m.as_bytes(), &w);
        std::fs::write(
            dir.join("ferrite-plugin.minisig"),
            key.sign(payload.as_bytes()),
        )
        .unwrap();
        if pub_file {
            std::fs::write(dir.join("ferrite-plugin.pub"), key.public_file()).unwrap();
        }
    }
    dir
}

fn registry() -> FileRegistryFixture {
    let mut fx = FileRegistryFixture::new();
    let (m, w) = hello();
    fx.add_version("hello", "0.1.0", &m, &w, None);
    fx.add_version("hello", "0.2.0", &m, &w, None);
    fx.publish();
    fx
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn check_prints_verified_publisher() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = registry();
    let site = site_with_registry(tmp.path(), &fx);
    // A folder signed by the registry's publisher key: verified once the index is cached.
    let dir = signed_dir(tmp.path(), Some(&fx.publisher_key), false);
    out(plugin(&site, &["search"]), 0);
    let (stdout, _) = out(plugin(&site, &["check", "--dir", s(&dir)]), 0);
    assert!(
        stdout.contains("publisher: verified ferritecms.dev"),
        "{stdout}"
    );
    // Unsigned: the warning line.
    let tmp2 = tempfile::tempdir().unwrap();
    let plain = signed_dir(tmp2.path(), None, false);
    let (stdout, _) = out(plugin(&site, &["check", "--dir", s(&plain)]), 0);
    assert!(stdout.contains("! unsigned"), "{stdout}");
}

#[test]
fn install_dir_with_signature_records_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    init(&site);
    let key = TestKey::new();
    let dir = signed_dir(tmp.path(), Some(&key), true);
    let (stdout, _) = out(plugin(&site, &["install", "--dir", s(&dir), "--yes"]), 0);
    assert!(
        stdout.contains(&format!("! new publisher key {}", key.key.key_id)),
        "{stdout}"
    );
    assert!(stdout.contains("installed plugin hello 0.1.0"), "{stdout}");
    let (keys, _) = out(plugin(&site, &["keys", "list"]), 0);
    assert!(
        keys.contains(&format!("{}  approved", key.key.key_id)),
        "{keys}"
    );
}

#[test]
fn install_dir_stripped_signature_refused_exit_1() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    init(&site);
    let key = TestKey::new();
    let dir = signed_dir(tmp.path(), Some(&key), true);
    out(plugin(&site, &["install", "--dir", s(&dir), "--yes"]), 0);
    std::fs::remove_file(dir.join("ferrite-plugin.minisig")).unwrap();
    std::fs::remove_file(dir.join("ferrite-plugin.pub")).unwrap();
    let (_, stderr) = out(plugin(&site, &["install", "--dir", s(&dir), "--yes"]), 1);
    assert!(stderr.contains("refused (publisher)"), "{stderr}");
}

#[test]
fn install_name_from_file_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = registry();
    let site = site_with_registry(tmp.path(), &fx);
    let (stdout, _) = out(plugin(&site, &["install", "hello", "--yes", "--enable"]), 0);
    assert!(
        stdout.contains("installed plugin hello 0.2.0 (enabled)"),
        "{stdout}"
    );
    assert!(stdout.contains("publisher: verified"), "{stdout}");
}

#[test]
fn install_name_at_version() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = registry();
    let site = site_with_registry(tmp.path(), &fx);
    let (stdout, _) = out(plugin(&site, &["install", "hello@0.1.0", "--yes"]), 0);
    assert!(stdout.contains("installed plugin hello 0.1.0"), "{stdout}");
    let (_, stderr) = out(plugin(&site, &["install", "hello@9.9.9", "--yes"]), 1);
    assert!(stderr.contains("hello@9.9.9"), "{stderr}");
}

#[test]
fn install_name_and_dir_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    init(&site);
    out(
        plugin(&site, &["install", "hello", "--dir", "x", "--yes"]),
        2,
    );
    out(plugin(&site, &["install", "--yes"]), 2);
}

#[test]
fn search_lists_matches() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = registry();
    let site = site_with_registry(tmp.path(), &fx);
    let (stdout, _) = out(plugin(&site, &["search", "hel"]), 0);
    assert_eq!(stdout.trim(), "hello  0.2.0  FerriteCMS  The hello plugin");
    let (stdout, _) = out(plugin(&site, &["search", "zzz"]), 0);
    assert_eq!(stdout.trim(), "");
}

#[test]
fn update_check_exits_2_with_updates_0_without() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = registry();
    let site = site_with_registry(tmp.path(), &fx);
    out(plugin(&site, &["install", "hello@0.1.0", "--yes"]), 0);
    let (stdout, _) = out(plugin(&site, &["update", "--check"]), 2);
    assert_eq!(stdout.trim(), "hello  0.1.0 -> 0.2.0");
    out(plugin(&site, &["install", "hello", "--yes"]), 0);
    let (stdout, _) = out(plugin(&site, &["update", "--check"]), 0);
    assert_eq!(stdout.trim(), "no updates");
}

#[test]
fn update_installs_after_yes() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = registry();
    let site = site_with_registry(tmp.path(), &fx);
    out(plugin(&site, &["install", "hello@0.1.0", "--yes"]), 0);
    // Without a terminal and without --yes nothing is installed.
    out(plugin(&site, &["update"]), 1);
    let (stdout, _) = out(plugin(&site, &["update", "hello", "--yes"]), 0);
    assert!(stdout.contains("installed plugin hello 0.2.0"), "{stdout}");
    let (list, _) = out(plugin(&site, &["list"]), 0);
    assert!(list.contains("hello  0.2.0"), "{list}");
}

#[test]
fn digest_matches_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = signed_dir(tmp.path(), None, false);
    let (m, w) = hello();
    let want = ferrite_plugin_host::trust::signing_payload(m.as_bytes(), &w);
    let o = ferrite()
        .args(["plugin", "digest", "--dir"])
        .arg(&dir)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(o).unwrap(), want);
    let file = tmp.path().join("payload");
    ferrite()
        .args(["plugin", "digest", "--dir"])
        .arg(&dir)
        .arg("--out")
        .arg(&file)
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(file).unwrap(), want);
}

#[test]
fn keys_trust_list_revoke() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    init(&site);
    let key = TestKey::new();
    let file = tmp.path().join("ops.pub");
    std::fs::write(&file, key.public_file()).unwrap();
    out(
        plugin(&site, &["keys", "trust", s(&file), "--label", "ops team"]),
        0,
    );
    let id = key.key.key_id.to_string();
    let (list, _) = out(plugin(&site, &["keys", "list"]), 0);
    assert_eq!(list.trim(), format!("{id}  operator  ops team"));
    // Signed by the operator key: no `new key` line, installs.
    let dir = signed_dir(tmp.path(), Some(&key), false);
    let (stdout, _) = out(
        plugin(&site, &["install", "--dir", s(&dir), "--yes", "--enable"]),
        0,
    );
    assert!(stdout.contains(&format!("publisher: key {id}")), "{stdout}");
    let (stdout, _) = out(
        plugin(
            &site,
            &[
                "keys",
                "revoke",
                &id,
                "--reason",
                "laptop stolen",
                "--compromised",
            ],
        ),
        0,
    );
    assert!(stdout.contains("disabled plugin hello"), "{stdout}");
    let (list, _) = out(plugin(&site, &["keys", "list"]), 0);
    assert!(
        list.contains("revoked, compromised: laptop stolen"),
        "{list}"
    );
    let (list, _) = out(plugin(&site, &["list"]), 0);
    assert!(list.contains("publisher key revoked"), "{list}");
    out(
        plugin(
            &site,
            &["keys", "revoke", "0000000000000000", "--reason", "x"],
        ),
        1,
    );
}

#[test]
fn unbind_then_other_publisher_installs() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    init(&site);
    let a = TestKey::new();
    let dir = signed_dir(tmp.path(), Some(&a), true);
    out(plugin(&site, &["install", "--dir", s(&dir), "--yes"]), 0);
    let tmp_b = tempfile::tempdir().unwrap();
    let b = TestKey::new();
    let other = signed_dir(tmp_b.path(), Some(&b), true);
    out(plugin(&site, &["install", "--dir", s(&other), "--yes"]), 1);
    let (stdout, stderr) = out(plugin(&site, &["unbind", "hello"]), 0);
    assert!(stdout.contains("unbound plugin hello"), "{stdout}");
    assert!(stderr.contains("warning"), "{stderr}");
    out(plugin(&site, &["install", "--dir", s(&other), "--yes"]), 0);
}

#[test]
fn no_registry_search_exit_1() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    init(&site);
    let path = site.join("ferrite.toml");
    let config = std::fs::read_to_string(&path).unwrap().replacen(
        "[plugins]\n",
        "[plugins]\nregistry = \"\"\n",
        1,
    );
    std::fs::write(&path, config).unwrap();
    let (_, stderr) = out(plugin(&site, &["search"]), 1);
    assert!(stderr.contains("registry"), "{stderr}");
}
