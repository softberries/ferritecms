//! `ferrite theme pack --dir DIR --out FILE` (M2b-3 P42): the canonical bundle of a
//! theme folder, written without a data folder, config or database. Its output for the
//! admin's `blog-min/` fixture is the committed `blog-min.tar` that the admin's
//! TypeScript packer must reproduce byte for byte (and that `POST /themes` installs:
//! `ferrite-http` `blog_min_fixture_installs`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use bytes::Bytes;
use ferrite_theme::{Theme, ThemeSource, pack};
use sha2::{Digest, Sha256};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../admin/src/appearance/__fixtures__")
}

/// Runs `ferrite theme pack` with an empty environment; returns stdout.
fn run_pack(dir: &Path, out: &Path) -> String {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear()
        .args(["theme", "pack", "--dir"])
        .arg(dir)
        .arg("--out")
        .arg(out);
    let output = cmd.assert().success().get_output().stdout.clone();
    String::from_utf8(output).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn theme_pack_output_installs() {
    let dir = fixtures().join("blog-min");
    let tmp = tempfile::tempdir().unwrap();
    let (first, second) = (tmp.path().join("a.tar"), tmp.path().join("b.tar"));

    let printed = run_pack(&dir, &first);
    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(
        printed,
        format!(
            "packed 4 files, {} bytes, sha256 {}\n",
            bytes.len(),
            hex(&bytes)
        )
    );
    // Packing twice gives the same bytes, which are the library's and the committed
    // fixture's.
    run_pack(&dir, &second);
    assert_eq!(std::fs::read(&second).unwrap(), bytes);
    assert_eq!(pack(&ThemeSource::Dir(dir.clone())).unwrap(), bytes);
    assert_eq!(
        std::fs::read(fixtures().join("blog-min.tar")).unwrap(),
        bytes,
        "blog-min.tar is stale: rerun `ferrite theme pack --dir {} --out …`",
        dir.display()
    );

    // What the server's upload does with it: unpacked and compiled as `blog-min`.
    let theme = Theme::load(&ThemeSource::Bundle(Bytes::from(bytes.clone()))).unwrap();
    assert_eq!(theme.name(), "blog-min");
    assert!(theme.asset("css/site.css").is_some());
    // The hidden file is left out; the long path is split into ustar prefix and name.
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains(".hidden-note"), "hidden file packed");
    let long = walk(&dir)
        .into_iter()
        .find(|p| p.len() > 100)
        .expect("a path over 100 bytes");
    let (prefix, name) = long.rsplit_once('/').unwrap();
    let header = bytes
        .chunks(512)
        .find(|h| h.starts_with(name.as_bytes()))
        .expect("the long path's header");
    assert!(header[345..].starts_with(prefix.as_bytes()), "ustar prefix");
    assert!(theme.asset(long.strip_prefix("assets/").unwrap()).is_some());
}

/// Every non-hidden file under `root`, relative, with `/`.
fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                out.push(rel.to_str().unwrap().replace('\\', "/"));
            }
        }
    }
    out
}

/// A folder that is not a theme is refused and nothing is written.
#[test]
fn theme_pack_refuses_a_broken_theme() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("broken");
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(
        dir.join("theme.toml"),
        "[theme]\nname = \"broken\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("templates/index.html"), "{% if %}").unwrap();
    let out = tmp.path().join("out.tar");
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear()
        .args(["theme", "pack", "--dir"])
        .arg(&dir)
        .arg("--out")
        .arg(&out);
    let stderr = cmd.assert().code(1).get_output().stderr.clone();
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(stderr.contains("templates/index.html"), "{stderr}");
    assert!(!out.exists());
}
