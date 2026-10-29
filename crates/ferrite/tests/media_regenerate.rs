//! `ferrite media regenerate` (M4-1 Task 11, P18): variants made in process after a theme
//! adds sizes. The site is a SQLite folder with the demo content, whose one media item is a
//! 1280 x 720 PNG.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use sqlx::{AssertSqlSafe, Connection, SqliteConnection};

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// A seeded site: `init`, then `seed demo` (blog theme: `card` and `hero` next to the core
/// sizes; `large` is wider than the original and is never made).
fn site() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(&data)
        .assert()
        .success();
    ferrite()
        .args(["seed", "demo", "--data"])
        .arg(&data)
        .assert()
        .success();
    (tmp, data)
}

/// A copy of the blog theme named `plus` with two more sizes: `small` (made) and `huge`
/// (wider than the original, so never made).
fn plus_theme(tmp: &Path) -> PathBuf {
    let dst = tmp.join("plus");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/blog"),
        &dst,
    );
    let manifest = std::fs::read_to_string(dst.join("theme.toml")).unwrap();
    let manifest = manifest
        .replace("name        = \"blog\"", "name        = \"plus\"")
        .replace(
            "[images]\n",
            "[images]\nsmall = { width = 300 }\nhuge  = { width = 5000 }\n",
        );
    assert!(manifest.contains("name        = \"plus\""));
    assert!(manifest.contains("small ="));
    std::fs::write(dst.join("theme.toml"), manifest).unwrap();
    dst
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn install_plus(tmp: &Path, data: &Path) {
    let dir = plus_theme(tmp);
    ferrite()
        .args(["theme", "install", "--activate", "--dir"])
        .arg(&dir)
        .arg("--data")
        .arg(data)
        .assert()
        .success();
}

async fn connect(data: &Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", data.join("ferrite.db").display()))
        .await
        .unwrap()
}

/// The variant names recorded on the site's one media row.
fn variant_names(data: &Path) -> BTreeSet<String> {
    block_on(async {
        let mut conn = connect(data).await;
        let json: String = sqlx::query_scalar("SELECT variants FROM media")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        v.as_object().unwrap().keys().cloned().collect()
    })
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

fn sql(data: &Path, statement: &'static str) {
    block_on(async {
        let mut conn = connect(data).await;
        sqlx::query(AssertSqlSafe(statement))
            .execute(&mut conn)
            .await
            .unwrap();
    });
}

fn item_id(data: &Path) -> String {
    block_on(async {
        let mut conn = connect(data).await;
        let id: Vec<u8> = sqlx::query_scalar("SELECT item_id FROM media")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        uuid::Uuid::from_slice(&id).unwrap().to_string()
    })
}

/// The stored original of the media row.
fn original_blob(data: &Path) -> PathBuf {
    let hash: String = block_on(async {
        let mut conn = connect(data).await;
        sqlx::query_scalar("SELECT artifact_hash FROM media")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    });
    data.join("media/artifacts").join(&hash[..2]).join(hash)
}

fn regenerate(data: &Path, extra: &[&str], code: i32) -> (String, String) {
    let mut cmd = ferrite();
    cmd.args(["media", "regenerate", "--data"])
        .arg(data)
        .args(extra);
    let out = cmd.assert().code(code).get_output().clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn regenerate_adds_new_theme_size() {
    let (tmp, data) = site();
    assert_eq!(
        variant_names(&data),
        set(&["card", "hero", "medium", "thumbnail"])
    );
    install_plus(tmp.path(), &data);
    let (out, _) = regenerate(&data, &[], 0);
    assert_eq!(out.trim(), "regenerated 1, skipped 0, failed 0");
    // `small` is new; `huge` and `large` are wider than the original and never made.
    assert_eq!(
        variant_names(&data),
        set(&["card", "hero", "medium", "small", "thumbnail"])
    );
}

#[test]
fn regenerate_skips_complete_items() {
    let (tmp, data) = site();
    // Nothing is missing: `large` is wider than the original and does not count.
    let (out, _) = regenerate(&data, &[], 0);
    assert_eq!(out.trim(), "regenerated 0, skipped 1, failed 0");
    install_plus(tmp.path(), &data);
    regenerate(&data, &[], 0);
    // A second run is idempotent although `huge` is never made.
    let (out, _) = regenerate(&data, &[], 0);
    assert_eq!(out.trim(), "regenerated 0, skipped 1, failed 0");
}

#[test]
fn regenerate_all_redoes() {
    let (_tmp, data) = site();
    let (out, _) = regenerate(&data, &["--all"], 0);
    assert_eq!(out.trim(), "regenerated 1, skipped 0, failed 0");
    assert_eq!(
        variant_names(&data),
        set(&["card", "hero", "medium", "thumbnail"])
    );
}

#[test]
fn regenerate_item_flag() {
    let (tmp, data) = site();
    install_plus(tmp.path(), &data);
    let id = item_id(&data);
    let (out, _) = regenerate(&data, &["--item", &id], 0);
    assert_eq!(out.trim(), "regenerated 1, skipped 0, failed 0");
    assert!(variant_names(&data).contains("small"));
    // Complete now: skipped, unless `--all`.
    let (out, _) = regenerate(&data, &["--item", &id], 0);
    assert_eq!(out.trim(), "regenerated 0, skipped 1, failed 0");
    let (out, _) = regenerate(&data, &["--item", &id, "--all"], 0);
    assert_eq!(out.trim(), "regenerated 1, skipped 0, failed 0");
    // An id that is no media item is an error.
    ferrite()
        .args(["media", "regenerate", "--data"])
        .arg(&data)
        .args(["--item", "00000000-0000-4000-8000-000000000000"])
        .assert()
        .code(1);
}

#[test]
fn regenerate_skips_svg_and_oversize() {
    let (tmp, data) = site();
    install_plus(tmp.path(), &data);
    let blob = original_blob(&data);

    // Not a raster image.
    sql(&data, "UPDATE media SET mime = 'image/svg+xml'");
    let (out, _) = regenerate(&data, &["--all"], 0);
    assert_eq!(out.trim(), "regenerated 0, skipped 1, failed 0");
    sql(&data, "UPDATE media SET mime = 'image/png'");

    // Above 64 MiB.
    let original = std::fs::read(&blob).unwrap();
    std::fs::write(&blob, vec![0u8; 64 * 1024 * 1024 + 1]).unwrap();
    let (out, _) = regenerate(&data, &["--all"], 0);
    assert_eq!(out.trim(), "regenerated 0, skipped 1, failed 0");
    assert_eq!(
        variant_names(&data),
        set(&["card", "hero", "medium", "thumbnail"])
    );

    // The same with `--item`.
    let id = item_id(&data);
    let (out, _) = regenerate(&data, &["--item", &id], 0);
    assert_eq!(out.trim(), "regenerated 0, skipped 1, failed 0");

    // Back to a good original: it is processed again.
    std::fs::write(&blob, original).unwrap();
    let (out, _) = regenerate(&data, &[], 0);
    assert_eq!(out.trim(), "regenerated 1, skipped 0, failed 0");
}

#[test]
fn regenerate_failed_item_exits_2() {
    let (tmp, data) = site();
    install_plus(tmp.path(), &data);
    std::fs::write(original_blob(&data), b"this is not an image").unwrap();
    let (out, err) = regenerate(&data, &[], 2);
    assert_eq!(out.trim(), "regenerated 0, skipped 0, failed 1");
    assert!(err.contains("failed "), "{err}");
    // The row keeps its variants.
    assert_eq!(
        variant_names(&data),
        set(&["card", "hero", "medium", "thumbnail"])
    );
}
