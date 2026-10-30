//! `ferrite self-update` (M4-2 Task 4, P25/P26/P31): a fake GitHub-API-shaped release
//! server on localhost, a minisign key pair made in the test, and a "current binary" that is
//! a copy (a small shell script) in a temp folder, never the real test binary.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use axum::http::{StatusCode, Uri};
use sha2::{Digest, Sha256};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const NEW: &str = "9.9.9";

type Routes = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// A local HTTP server answering from a path -> body map (404 otherwise).
struct Server {
    addr: SocketAddr,
    routes: Routes,
}

impl Server {
    fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Routes = Arc::default();
        let shared = routes.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let app = axum::Router::new().fallback(move |uri: Uri| {
                    let routes = shared.clone();
                    async move {
                        // A server that never answers in time.
                        if uri.path().starts_with("/stall/") {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        }
                        match routes.lock().unwrap().get(uri.path()) {
                            Some(body) => (StatusCode::OK, body.clone()),
                            None => (StatusCode::NOT_FOUND, b"not found".to_vec()),
                        }
                    }
                });
                axum::serve(listener, app).await.unwrap();
            });
        });
        Server { addr, routes }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn set(&self, path: &str, body: Vec<u8>) {
        self.routes.lock().unwrap().insert(path.to_owned(), body);
    }

    /// Publishes release `version` with `files` as its assets, as both the latest release
    /// and `tags/v<version>`.
    fn publish(&self, version: &str, files: &[(String, Vec<u8>)]) {
        self.publish_at(version, files, "/dl/");
    }

    /// [`Server::publish`] with the download URLs under `prefix` (`/stall/` never answers).
    fn publish_at(&self, version: &str, files: &[(String, Vec<u8>)], prefix: &str) {
        let assets: Vec<serde_json::Value> = files
            .iter()
            .map(|(name, _)| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{}{prefix}{name}", self.base()),
                })
            })
            .collect();
        let json = serde_json::json!({
            "tag_name": format!("v{version}"),
            "draft": false,
            "prerelease": false,
            "assets": assets,
        });
        let body = serde_json::to_vec(&json).unwrap();
        self.set(
            "/repos/softberries/ferritecms/releases/latest",
            body.clone(),
        );
        self.set(
            &format!("/repos/softberries/ferritecms/releases/tags/v{version}"),
            body,
        );
        for (name, bytes) in files {
            self.set(&format!("/dl/{name}"), bytes.clone());
        }
    }
}

struct Keys {
    pk: minisign::PublicKey,
    sk: minisign::SecretKey,
}

impl Keys {
    fn new() -> Self {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        Keys {
            pk: kp.pk,
            sk: kp.sk,
        }
    }

    fn public_text(&self) -> String {
        self.pk.to_box().unwrap().into_string()
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        minisign::sign(Some(&self.pk), &self.sk, Cursor::new(data), None, None)
            .unwrap()
            .into_string()
            .into_bytes()
    }
}

/// A fake `ferrite` binary: prints `ferrite <version>` for `--version`, and a marker line
/// (exit 2, "pending") for `migrate status --data DIR`.
fn script(version: &str) -> Vec<u8> {
    script_with(version, 2)
}

/// [`script`] whose `migrate status` exits with `migrate_exit`.
fn script_with(version: &str, migrate_exit: u8) -> Vec<u8> {
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo \"ferrite {version}\"; exit 0; fi\n\
         if [ \"$1\" = \"migrate\" ] && [ \"$2\" = \"status\" ]; then\n\
           echo \"FAKE MIGRATE STATUS {version} $4\"; exit {migrate_exit}\n\
         fi\n\
         exit 1\n"
    )
    .into_bytes()
}

fn asset_name(version: &str) -> String {
    format!("ferrite-{version}-{}.tar.gz", ferrite::binary_target())
}

fn tarball(version: &str, binary: &[u8]) -> Vec<u8> {
    tarball_at("", version, binary)
}

/// [`tarball`] with every entry name starting with `prefix` (e.g. `./`, as `tar -C dir .`
/// writes). The names are written raw, so the prefix is kept as given.
fn tarball_at(prefix: &str, version: &str, binary: &[u8]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    let dir = format!("ferrite-{version}-{}", ferrite::binary_target());
    for (name, bytes, mode) in [
        ("README.md", b"# FerriteCMS\n".as_slice(), 0o644),
        ("ferrite", binary, 0o755),
    ] {
        let path = format!("{prefix}{dir}/{name}");
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        tar.append(&header, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sums(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = String::new();
    for (name, bytes) in files {
        out.push_str(&format!("{}  {name}\n", hex(&Sha256::digest(bytes))));
    }
    out.into_bytes()
}

/// `asset` with `SHA256SUMS` and its signature by `keys`.
fn signed(keys: &Keys, asset: (String, Vec<u8>)) -> Vec<(String, Vec<u8>)> {
    let list = sums(std::slice::from_ref(&asset));
    let sig = keys.sign(&list);
    vec![
        asset,
        ("SHA256SUMS".to_owned(), list),
        ("SHA256SUMS.minisig".to_owned(), sig),
    ]
}

/// A complete, valid release: the asset, `SHA256SUMS` and its signature.
fn release(keys: &Keys, version: &str) -> Vec<(String, Vec<u8>)> {
    signed(
        keys,
        (asset_name(version), tarball(version, &script(version))),
    )
}

/// The "installed" binary: a copy in its own folder.
struct Install {
    _tmp: tempfile::TempDir,
    exe: PathBuf,
}

impl Install {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        std::fs::create_dir(&dir).unwrap();
        let exe = dir.join("ferrite");
        std::fs::write(&exe, script(CURRENT)).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        Install { _tmp: tmp, exe }
    }

    fn dir(&self) -> &Path {
        self.exe.parent().unwrap()
    }

    fn prev(&self) -> PathBuf {
        self.dir().join("ferrite.prev")
    }

    /// The folder's entries, sorted.
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The binary is exactly the one installed by `new`, and nothing was left beside it.
    fn assert_untouched(&self, out: &Output) {
        assert_eq!(
            std::fs::read(&self.exe).unwrap(),
            script(CURRENT),
            "{}",
            text(out)
        );
        let mode = std::fs::metadata(&self.exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(self.entries(), vec!["ferrite".to_owned()], "{}", text(out));
    }
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn self_update(server: &Server, keys: &Keys, install: &Install) -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd.env("FERRITE_TEST_RELEASE_PUBKEY", keys.public_text());
    cmd.args(["self-update", "--base-url"])
        .arg(server.base())
        .arg("--test-current-exe")
        .arg(&install.exe);
    cmd
}

fn run(mut cmd: Command) -> Output {
    cmd.output().unwrap()
}

#[test]
fn self_update_installs_and_keeps_prev() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    server.publish(NEW, &release(&keys, NEW));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("ferrite {NEW}")), "{}", text(&out));
    assert!(stdout.contains("restart"), "{}", text(&out));

    assert_eq!(std::fs::read(&install.exe).unwrap(), script(NEW));
    assert_eq!(std::fs::read(install.prev()).unwrap(), script(CURRENT));
    for path in [&install.exe, &install.prev()] {
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "{}", path.display());
    }
    assert_eq!(install.entries(), vec!["ferrite", "ferrite.prev"]);
    let version = std::process::Command::new(&install.exe)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("ferrite {NEW}")
    );

    // A second update replaces the older `ferrite.prev`.
    let newer = "10.0.0";
    server.publish(newer, &release(&keys, newer));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(std::fs::read(&install.exe).unwrap(), script(newer));
    assert_eq!(std::fs::read(install.prev()).unwrap(), script(NEW));
    assert_eq!(install.entries(), vec!["ferrite", "ferrite.prev"]);
}

#[test]
fn self_update_refuses_bad_signature() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    // Signed by another key.
    let other = Keys::new();
    server.publish(NEW, &release(&other, NEW));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("signature"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);

    // Right key, but `SHA256SUMS` changed after signing.
    let mut files = release(&keys, NEW);
    files[1].1.extend_from_slice(b"0000  extra\n");
    server.publish(NEW, &files);
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("signature"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);
}

#[test]
fn self_update_refuses_sha_mismatch() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    let mut files = release(&keys, NEW);
    // The served asset differs from the one `SHA256SUMS` (validly signed) lists.
    files[0].1 = tarball(NEW, b"#!/bin/sh\necho evil\n");
    server.publish(NEW, &files);
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("sha256"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);
}

#[test]
fn self_update_refuses_missing_target_asset() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    // A release for another target only.
    let name = format!("ferrite-{NEW}-riscv64gc-unknown-none-elf.tar.gz");
    let asset = (name, tarball(NEW, &script(NEW)));
    server.publish(NEW, &signed(&keys, asset));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(ferrite::binary_target()),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);
}

#[test]
fn self_update_refuses_older_version() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    for version in ["0.0.1", CURRENT] {
        server.publish(version, &release(&keys, version));
        let mut cmd = self_update(&server, &keys, &install);
        cmd.args(["--yes", "--version", version]);
        let out = run(cmd);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("rollback"),
            "{}",
            text(&out)
        );
        install.assert_untouched(&out);
    }
}

#[test]
fn self_update_check_exit_2() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    server.publish(NEW, &release(&keys, NEW));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--check");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(NEW),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);

    // Up to date: exit 0, with and without --check.
    server.publish(CURRENT, &release(&keys, CURRENT));
    for check in [true, false] {
        let mut cmd = self_update(&server, &keys, &install);
        if check {
            cmd.arg("--check");
        }
        let out = run(cmd);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("up to date"),
            "{}",
            text(&out)
        );
        install.assert_untouched(&out);
    }
}

#[test]
fn self_update_keeps_binary_on_failure() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());

    // No release at all (the lookup is a 404).
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    install.assert_untouched(&out);

    // A signed release whose binary reports another version than the release's.
    let asset = (asset_name(NEW), tarball(NEW, &script("1.2.3")));
    server.publish(NEW, &signed(&keys, asset));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    install.assert_untouched(&out);

    // An asset that is not a gzip tarball.
    let asset = (asset_name(NEW), b"not a tarball".to_vec());
    server.publish(NEW, &signed(&keys, asset));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    install.assert_untouched(&out);

    // The new binary's `migrate status` says the schema is incompatible (exit 3).
    let asset = (asset_name(NEW), tarball(NEW, &script_with(NEW, 3)));
    server.publish(NEW, &signed(&keys, asset));
    let data = tempfile::tempdir().unwrap();
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes").arg("--data").arg(data.path());
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("exit 3"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);

    // A download that does not finish within the deadline.
    server.publish_at(NEW, &release(&keys, NEW), "/stall/");
    let mut cmd = self_update(&server, &keys, &install);
    cmd.args(["--yes", "--test-download-deadline-ms", "500"]);
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("took longer"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);

    // A valid release, but no --yes and no terminal.
    server.publish(NEW, &release(&keys, NEW));
    let out = run(self_update(&server, &keys, &install));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--yes"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);

    // An unwritable binary folder (skipped where permissions are not enforced, e.g. root).
    let dir = install.dir().to_owned();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.join("probe");
    let enforced = std::fs::File::create(&probe)
        .and_then(|mut f| f.write_all(b"x"))
        .is_err();
    let _ = std::fs::remove_file(&probe);
    if enforced {
        let mut cmd = self_update(&server, &keys, &install);
        cmd.arg("--yes");
        let out = run(cmd);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("owner"),
            "{}",
            text(&out)
        );
        install.assert_untouched(&out);
    } else {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn self_update_placeholder_key_refuses() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    server.publish(NEW, &release(&keys, NEW));
    // The same comment-only text as the repository's placeholder `release.pub`.
    let mut cmd = self_update(&server, &keys, &install);
    cmd.env(
        "FERRITE_TEST_RELEASE_PUBKEY",
        "# FerriteCMS release public key: placeholder, holds no key\n",
    )
    .arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("this build has no release key"),
        "{}",
        text(&out)
    );
    install.assert_untouched(&out);
}

#[test]
fn self_update_shows_migrate_status_with_data() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    server.publish(NEW, &release(&keys, NEW));
    let data = tempfile::tempdir().unwrap();
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes").arg("--data").arg(data.path());
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!(
            "FAKE MIGRATE STATUS {NEW} {}",
            data.path().display()
        )),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read(&install.exe).unwrap(), script(NEW));
}

#[test]
fn self_update_accepts_dot_prefixed_archive() {
    let (server, keys, install) = (Server::start(), Keys::new(), Install::new());
    let tgz = tarball_at("./", NEW, &script(NEW));
    // The names really start with `./` (written raw, not normalised by the tar crate).
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&tgz[..]));
    let names: Vec<Vec<u8>> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path_bytes().into_owned())
        .collect();
    assert!(names.iter().all(|n| n.starts_with(b"./")), "{names:?}");
    server.publish(NEW, &signed(&keys, (asset_name(NEW), tgz)));
    let mut cmd = self_update(&server, &keys, &install);
    cmd.arg("--yes");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(std::fs::read(&install.exe).unwrap(), script(NEW));
    assert_eq!(std::fs::read(install.prev()).unwrap(), script(CURRENT));
}
