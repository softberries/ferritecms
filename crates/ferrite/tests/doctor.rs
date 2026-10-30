#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `ferrite doctor`: the runner, the output format, the exit codes, the file checks (M4-3a
//! Task 6) and the database, media, cluster, clock and network checks (Task 7). A test that
//! runs doctor without `--offline` points the release lookup at a local address with the
//! hidden `--base-url` seam, so no test reaches GitHub.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

/// A fresh site in the existing temp folder `dir`: `init` leaves an existing folder as it
/// is (the temp folder's mode depends on the platform), so it is set to 0750 here, the mode
/// `init` gives a folder it creates (`fresh_init_into_a_new_folder_is_clean`).
fn init_sqlite(dir: &Path) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(dir)
        .assert()
        .success();
    chmod(dir, 0o750);
}

/// One output line of `doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    status: String,
    id: String,
    detail: String,
}

struct Output {
    code: i32,
    lines: Vec<Line>,
    summary: String,
    stdout: String,
    stderr: String,
}

impl Output {
    fn status(&self, id: &str) -> &str {
        let found: Vec<&Line> = self.lines.iter().filter(|l| l.id == id).collect();
        assert!(
            !found.is_empty(),
            "no line for {id} in:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
        &found[0].status
    }

    fn detail(&self, id: &str) -> &str {
        &self
            .lines
            .iter()
            .find(|l| l.id == id)
            .unwrap_or_else(|| panic!("no line for {id} in:\n{}", self.stdout))
            .detail
    }

    fn ids(&self) -> Vec<&str> {
        let mut seen = Vec::new();
        for l in &self.lines {
            if !seen.contains(&l.id.as_str()) {
                seen.push(l.id.as_str());
            }
        }
        seen
    }

    fn count(&self, status: &str) -> usize {
        self.lines.iter().filter(|l| l.status == status).count()
    }
}

/// Parses `<status padded to 5>  <id>  <detail>` lines and the summary line.
fn parse(out: &std::process::Output, code: i32) -> Output {
    let stdout = String::from_utf8(out.stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(out.stderr.clone()).expect("utf-8 stderr");
    let mut lines = Vec::new();
    let mut summary = String::new();
    for raw in stdout.lines() {
        if raw.starts_with("doctor:") {
            summary = raw.to_owned();
            continue;
        }
        assert!(raw.len() > 7, "short line {raw:?}");
        let (status, rest) = raw.split_at(5);
        assert!(
            rest.starts_with("  "),
            "status is padded to 5 + 2 spaces: {raw:?}"
        );
        let status = status.trim_end();
        assert!(
            ["ok", "info", "warn", "error", "skip"].contains(&status),
            "unknown status in {raw:?}"
        );
        let rest = &rest[2..];
        let (id, detail) = rest
            .split_once("  ")
            .unwrap_or_else(|| panic!("no detail in {raw:?}"));
        lines.push(Line {
            status: status.to_owned(),
            id: id.to_owned(),
            detail: detail.to_owned(),
        });
    }
    Output {
        code,
        lines,
        summary,
        stdout,
        stderr,
    }
}

fn doctor_args(data: &Path, extra: &[&str]) -> Output {
    let mut cmd = ferrite();
    cmd.arg("doctor").arg("--data").arg(data).args(extra);
    run(cmd)
}

fn run(mut cmd: Command) -> Output {
    let out = cmd.output().expect("run ferrite");
    let code = out.status.code().expect("exit code (not a signal)");
    parse(&out, code)
}

fn offline(data: &Path) -> Output {
    doctor_args(data, &["--offline"])
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

/// The fixed check order of P53; Task 7's checks are `skip` lines until it wires them in.
const ORDER: &[&str] = &[
    "version.binary",
    "config.load",
    "fs.data_dir",
    "fs.config_file",
    "fs.secret_key",
    "fs.database_files",
    "fs.backups_dir",
    "fs.themes_dir",
    "db.lock_state",
    "db.connect",
    "db.schema",
    "db.identity",
    "upgrade.record",
    "media.store",
    "media.owner",
    "clock.db_skew",
    "clock.peers",
    "cluster.versions",
    "cluster.site_url",
    "smtp.connect",
    "tls.dir",
    "tls.acme_directory",
    "net.listen",
    "backup.target",
    "release.latest",
];

#[test]
fn fresh_site_offline_has_no_errors_and_the_pinned_statuses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = offline(tmp.path());
    assert_eq!(out.code, 0, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.count("error"), 0, "{}", out.stdout);
    assert_eq!(out.count("warn"), 0, "{}", out.stdout);
    // The order is P53's, one line per check on a fresh site.
    assert_eq!(out.ids(), ORDER, "{}", out.stdout);
    let expected: BTreeMap<&str, &str> = [
        ("version.binary", "info"),
        ("config.load", "ok"),
        ("fs.data_dir", "ok"),
        ("fs.config_file", "ok"),
        ("fs.secret_key", "ok"),
        ("fs.database_files", "ok"),
        ("fs.backups_dir", "ok"),
        ("fs.themes_dir", "info"),
        ("db.lock_state", "info"),
        ("db.connect", "ok"),
        ("db.schema", "ok"),
        ("db.identity", "ok"),
        ("upgrade.record", "ok"),
        // The local probe is not a network check: it runs offline too.
        ("media.store", "ok"),
        // No GC pass has written the owner marker yet.
        ("media.owner", "info"),
        ("clock.db_skew", "skip"),
        ("clock.peers", "skip"),
        ("cluster.versions", "skip"),
        ("cluster.site_url", "skip"),
        // Mail and TLS are off: nothing to connect to, offline or not.
        ("smtp.connect", "info"),
        ("tls.dir", "info"),
        ("tls.acme_directory", "info"),
        ("net.listen", "skip"),
        ("backup.target", "info"),
        ("release.latest", "skip"),
    ]
    .into_iter()
    .collect();
    for (id, status) in &expected {
        assert_eq!(out.status(id), *status, "{id}\n{}", out.stdout);
    }
    assert_eq!(expected.len(), ORDER.len());
    assert!(
        !out.stdout.contains("not checked yet"),
        "every check is wired: {}",
        out.stdout
    );
    assert_eq!(out.summary, "doctor: 11 ok, 0 warn, 0 error, 6 skip");
    // The lock is not held, and SQLite has no clock or cluster to compare.
    assert!(
        out.detail("db.lock_state").contains("no node"),
        "{}",
        out.stdout
    );
    assert!(
        out.detail("clock.db_skew").contains("SQLite"),
        "{}",
        out.stdout
    );
    for id in ["clock.peers", "cluster.versions", "cluster.site_url"] {
        assert!(
            out.detail(id).contains("solo SQLite site"),
            "{id}: {}",
            out.stdout
        );
    }
    assert!(
        out.detail("release.latest").contains("--offline"),
        "{}",
        out.stdout
    );
    assert!(
        out.detail("net.listen").contains("no node"),
        "{}",
        out.stdout
    );
}

#[test]
fn output_lines_are_padded_and_summary_is_last() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = offline(tmp.path());
    let first = out.stdout.lines().next().unwrap();
    assert!(
        first.starts_with("info   version.binary  ferrite "),
        "{first}"
    );
    let second = out.stdout.lines().nth(1).unwrap();
    assert!(second.starts_with("ok     config.load  "), "{second}");
    assert!(out.stdout.lines().last().unwrap().starts_with("doctor: "));
    // Everything goes to stdout; nothing is logged to stderr on a healthy site.
    assert_eq!(out.stderr, "", "{}", out.stderr);
}

#[test]
fn version_binary_reports_version_and_target() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = offline(tmp.path());
    let detail = out.detail("version.binary");
    assert!(detail.contains(env!("CARGO_PKG_VERSION")), "{detail}");
    assert!(detail.contains('-'), "a target triple: {detail}");
}

#[test]
fn loose_secret_key_mode_is_a_warning_with_exit_2() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    chmod(&tmp.path().join("secret.key"), 0o644);
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.secret_key"), "warn", "{}", out.stdout);
    assert!(
        out.detail("fs.secret_key").contains("0644"),
        "{}",
        out.stdout
    );
    assert_eq!(out.count("error"), 0);
    assert_eq!(out.code, 2);
    assert!(out.summary.contains("1 warn"), "{}", out.summary);
}

#[test]
fn loose_config_file_mode_is_a_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    chmod(&tmp.path().join("ferrite.toml"), 0o664);
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.config_file"), "warn", "{}", out.stdout);
    assert_eq!(out.code, 2);
}

#[test]
fn world_bits_on_the_data_folder_are_a_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    chmod(tmp.path(), 0o777);
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.data_dir"), "warn", "{}", out.stdout);
    assert_eq!(out.code, 2);
    // Read access for everyone is already too loose; it is the only warning on an
    // otherwise fresh site.
    chmod(tmp.path(), 0o755);
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.data_dir"), "warn", "{}", out.stdout);
    assert_eq!(out.count("warn"), 1, "{}", out.stdout);
    assert_eq!(out.code, 2);
    // Group bits alone are fine (0750 is the documented mode).
    chmod(tmp.path(), 0o750);
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.data_dir"), "ok", "{}", out.stdout);
    assert_eq!(out.code, 0);
}

#[test]
fn missing_secret_key_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::remove_file(tmp.path().join("secret.key")).unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.secret_key"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
    // Doctor never creates the key `serve` would create.
    assert!(!tmp.path().join("secret.key").exists());
}

#[test]
fn malformed_secret_key_is_an_error_and_the_key_is_not_printed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(tmp.path().join("secret.key"), "not-a-key-hunter2\n").unwrap();
    chmod(&tmp.path().join("secret.key"), 0o600);
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.secret_key"), "error", "{}", out.stdout);
    assert!(!out.stdout.contains("hunter2"), "{}", out.stdout);
    assert_eq!(out.code, 1);
}

#[test]
fn configured_key_needs_no_key_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::remove_file(tmp.path().join("secret.key")).unwrap();
    let mut cmd = ferrite();
    cmd.arg("doctor")
        .arg("--data")
        .arg(tmp.path())
        .arg("--offline")
        .env("FERRITE_AUTH__SECRET_KEY", "ab".repeat(32));
    let out = run(cmd);
    assert_eq!(out.status("fs.secret_key"), "ok", "{}", out.stdout);
    assert!(
        out.detail("fs.secret_key").contains("auth.secret_key"),
        "{}",
        out.stdout
    );
    assert!(!out.stdout.contains(&"ab".repeat(32)), "{}", out.stdout);
}

#[test]
fn missing_config_file_is_an_error_and_exit_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::remove_file(tmp.path().join("ferrite.toml")).unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("config.load"), "error", "{}", out.stdout);
    assert!(
        out.detail("config.load").contains("ferrite.toml"),
        "{}",
        out.stdout
    );
    assert_eq!(out.code, 1);
    // Still the whole list, in order.
    assert_eq!(out.ids(), ORDER, "{}", out.stdout);
    assert!(!tmp.path().join("ferrite.toml").exists(), "nothing created");
}

#[test]
fn broken_toml_is_an_error_not_a_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = https://hunter2.example.com\n",
    )
    .unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("config.load"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
    assert!(!out.stderr.contains("panicked"), "{}", out.stderr);
    assert!(!out.stdout.contains("hunter2"), "{}", out.stdout);
    assert!(out.summary.contains("error"), "{}", out.summary);
}

#[test]
fn invalid_config_lists_each_problem_as_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"ftp://x\"\n[cluster]\nworkers = 0\n",
    )
    .unwrap();
    let out = offline(tmp.path());
    let errors: Vec<&Line> = out
        .lines
        .iter()
        .filter(|l| l.id == "config.load" && l.status == "error")
        .collect();
    assert!(errors.len() >= 2, "{}", out.stdout);
    assert!(errors.iter().any(|l| l.detail.starts_with("site.url:")));
    assert!(
        errors
            .iter()
            .any(|l| l.detail.starts_with("cluster.workers:"))
    );
    assert_eq!(out.code, 1);
}

#[test]
fn missing_data_folder_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("nope");
    let out = offline(&missing);
    assert_eq!(out.status("fs.data_dir"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
    assert!(!missing.exists(), "doctor creates nothing");
    assert!(!out.stderr.contains("panicked"), "{}", out.stderr);
}

#[test]
fn data_path_that_is_a_file_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("file");
    std::fs::write(&file, "x").unwrap();
    let out = offline(&file);
    assert_eq!(out.status("fs.data_dir"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
}

#[test]
fn unwritable_data_folder_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    chmod(tmp.path(), 0o500);
    let out = offline(tmp.path());
    chmod(tmp.path(), 0o700);
    if is_root() {
        // root passes every permission check.
        return;
    }
    assert_eq!(out.status("fs.data_dir"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
}

fn is_root() -> bool {
    // A file mode 0 can still be opened by root.
    let tmp = tempfile::tempdir().expect("tempdir");
    let f = tmp.path().join("f");
    std::fs::write(&f, "x").unwrap();
    chmod(&f, 0);
    std::fs::File::open(&f).is_ok()
}

#[test]
fn database_files_must_exist_and_be_writable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    chmod(&tmp.path().join("ferrite.db"), 0o400);
    let out = offline(tmp.path());
    chmod(&tmp.path().join("ferrite.db"), 0o600);
    if !is_root() {
        assert_eq!(out.status("fs.database_files"), "error", "{}", out.stdout);
        assert_eq!(out.code, 1);
    }
    std::fs::remove_file(tmp.path().join("ferrite.db")).unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.database_files"), "error", "{}", out.stdout);
    assert!(
        !tmp.path().join("ferrite.db").exists(),
        "doctor must not create the database"
    );
}

#[test]
fn backups_dir_counts_pre_migration_snapshots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let backups = tmp.path().join("backups");
    // `init` took a snapshot of its own; start from none.
    for entry in std::fs::read_dir(&backups).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for name in [
        "pre-0010-to-0011-20261029T101500Z.db",
        "pre-0011-to-0012-x.db",
    ] {
        std::fs::write(backups.join(name), b"x").unwrap();
    }
    std::fs::write(backups.join("backup-1.db"), b"x").unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.backups_dir"), "ok", "{}", out.stdout);
    assert!(
        out.detail("fs.backups_dir").contains("2 pre-migration"),
        "{}",
        out.detail("fs.backups_dir")
    );
    // A missing folder is not an error (`serve` does not need it to exist to start).
    std::fs::remove_dir_all(&backups).unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.backups_dir"), "info", "{}", out.stdout);
    assert!(!backups.exists());
}

#[test]
fn themes_dir_is_read_in_solo_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let themes = tmp.path().join("themes");
    std::fs::create_dir(&themes).unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("fs.themes_dir"), "ok", "{}", out.stdout);
    if !is_root() {
        chmod(&themes, 0);
        let out = offline(tmp.path());
        chmod(&themes, 0o700);
        assert_eq!(out.status("fs.themes_dir"), "warn", "{}", out.stdout);
    }
}

#[test]
fn cluster_mode_ignores_database_files_and_theme_folders() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n",
    )
    .unwrap();
    chmod(&tmp.path().join("ferrite.toml"), 0o600);
    let mut cmd = ferrite();
    cmd.arg("doctor")
        .arg("--data")
        .arg(tmp.path())
        .arg("--offline")
        .env("FERRITE_DATABASE__URL", "postgres://u:hunter2@db.invalid/x")
        .env("FERRITE_MEDIA__BACKEND", "s3")
        .env("FERRITE_MEDIA__S3__BUCKET", "b")
        .env("FERRITE_MEDIA__S3__REGION", "us-east-1")
        .env("FERRITE_AUTH__SECRET_KEY", "ab".repeat(32));
    let out = run(cmd);
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert_eq!(out.status("fs.database_files"), "info", "{}", out.stdout);
    assert_eq!(out.status("fs.themes_dir"), "info", "{}", out.stdout);
    assert!(
        out.detail("fs.themes_dir").contains("cluster"),
        "{}",
        out.stdout
    );
    assert_eq!(out.status("db.lock_state"), "info", "{}", out.stdout);
    // The database URL's password is never printed.
    assert!(!out.stdout.contains("hunter2"), "{}", out.stdout);
    assert!(!out.stderr.contains("hunter2"), "{}", out.stderr);
}

fn write_upgrade(dir: &Path, json: &str) {
    std::fs::write(dir.join("upgrade.json"), json).unwrap();
}

#[test]
fn upgrade_record_cases() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    // A complete record whose snapshot exists.
    std::fs::write(tmp.path().join("backups/pre-0001-to-0002.db"), b"x").unwrap();
    write_upgrade(
        tmp.path(),
        r#"{"v":1,"from":"0.9.0","to":"0.10.0","schema_from":1,"schema_to":2,
            "snapshot":"backups/pre-0001-to-0002.db","at":"2026-10-29T10:00:00Z"}"#,
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("upgrade.record"), "ok", "{}", out.stdout);
    assert!(
        out.detail("upgrade.record").contains("0.9.0"),
        "{}",
        out.stdout
    );
    // A named snapshot that is gone.
    write_upgrade(
        tmp.path(),
        r#"{"v":1,"from":"0.9.0","to":"0.10.0","schema_from":1,"schema_to":2,
            "snapshot":"backups/gone.db","at":"2026-10-29T10:00:00Z"}"#,
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("upgrade.record"), "warn", "{}", out.stdout);
    assert!(
        out.detail("upgrade.record").contains("gone.db"),
        "{}",
        out.stdout
    );
    assert_eq!(out.code, 2);
    // A provisional record: an upgrade that did not finish.
    write_upgrade(
        tmp.path(),
        r#"{"v":1,"from":"0.9.0","to":"0.10.0","schema_from":1,"schema_to":2,
            "snapshot":null,"at":"2026-10-29T10:00:00Z"}"#,
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("upgrade.record"), "warn", "{}", out.stdout);
    assert!(
        out.detail("upgrade.record").contains("provisional"),
        "{}",
        out.stdout
    );
    // A record that does not parse.
    write_upgrade(tmp.path(), "{ not json");
    let out = offline(tmp.path());
    assert_eq!(out.status("upgrade.record"), "warn", "{}", out.stdout);
    // Another format version.
    write_upgrade(
        tmp.path(),
        r#"{"v":7,"from":null,"to":"9.0.0","schema_from":1,"schema_to":2,
            "snapshot":null,"at":"2026-10-29T10:00:00Z"}"#,
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("upgrade.record"), "warn", "{}", out.stdout);
}

#[test]
fn a_finished_postgres_record_without_snapshot_is_ok() {
    // On Postgres no snapshot is ever taken, so a finished record that moved the schema has
    // `snapshot: null` (09 §1); only SQLite's null snapshot means an unfinished move.
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n",
    )
    .unwrap();
    chmod(&tmp.path().join("ferrite.toml"), 0o600);
    write_upgrade(
        tmp.path(),
        r#"{"v":1,"from":"0.9.0","to":"0.10.0","schema_from":1,"schema_to":2,
            "snapshot":null,"at":"2026-10-29T10:00:00Z"}"#,
    );
    let mut cmd = ferrite();
    cmd.arg("doctor")
        .arg("--data")
        .arg(tmp.path())
        .arg("--offline")
        .env("FERRITE_DATABASE__URL", "postgres://u@db.invalid/x")
        .env("FERRITE_MEDIA__BACKEND", "s3")
        .env("FERRITE_MEDIA__S3__BUCKET", "b")
        .env("FERRITE_MEDIA__S3__REGION", "us-east-1")
        .env("FERRITE_AUTH__SECRET_KEY", "ab".repeat(32));
    let out = run(cmd);
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert_eq!(out.status("upgrade.record"), "ok", "{}", out.stdout);
    assert_eq!(out.count("warn"), 0, "{}", out.stdout);
    // The only error: the database host does not exist (the database is always checked,
    // even offline).
    let errors: Vec<&str> = out
        .lines
        .iter()
        .filter(|l| l.status == "error")
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(errors, ["db.connect"], "{}", out.stdout);
    assert_eq!(out.code, 1, "{}", out.stdout);

    // The same record on a SQLite site is an upgrade that did not finish.
    let mut cmd = ferrite();
    cmd.arg("doctor")
        .arg("--data")
        .arg(tmp.path())
        .arg("--offline");
    let out = run(cmd);
    assert_eq!(out.status("upgrade.record"), "warn", "{}", out.stdout);
}

fn write_acme_config(dir: &Path) {
    std::fs::write(
        dir.join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n\
         [server.tls]\nmode = \"acme\"\ndomains = [\"example.com\"]\ncontact = \"mailto:ops@example.com\"\n",
    )
    .unwrap();
    chmod(&dir.join("ferrite.toml"), 0o600);
}

#[test]
fn tls_dir_modes_in_acme_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    write_acme_config(tmp.path());
    // Not created yet: info.
    let out = offline(tmp.path());
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert_eq!(out.status("tls.dir"), "info", "{}", out.stdout);
    assert!(
        !tmp.path().join("tls").exists(),
        "doctor must not create tls/"
    );
    // Right modes.
    let tls = tmp.path().join("tls");
    std::fs::create_dir(&tls).unwrap();
    chmod(&tls, 0o700);
    std::fs::write(tls.join("cert-abc.pem"), "x").unwrap();
    chmod(&tls.join("cert-abc.pem"), 0o600);
    let out = offline(tmp.path());
    assert_eq!(out.status("tls.dir"), "ok", "{}", out.stdout);
    // A loose file, then a loose folder.
    chmod(&tls.join("cert-abc.pem"), 0o644);
    let out = offline(tmp.path());
    assert_eq!(out.status("tls.dir"), "warn", "{}", out.stdout);
    chmod(&tls.join("cert-abc.pem"), 0o600);
    chmod(&tls, 0o755);
    let out = offline(tmp.path());
    assert_eq!(out.status("tls.dir"), "warn", "{}", out.stdout);
    assert_eq!(out.code, 2);
    // Doctor changed none of it.
    assert_eq!(mode_of(&tls), 0o755);
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn write_backup_config(dir: &Path, to: &str, schedule: &str) {
    std::fs::write(
        dir.join("ferrite.toml"),
        format!(
            "[site]\nurl = \"https://example.com\"\n[backup]\nschedule = \"{schedule}\"\nto = \"{to}\"\n"
        ),
    )
    .unwrap();
    chmod(&dir.join("ferrite.toml"), 0o600);
}

/// A repository folder with one complete snapshot named `name`.
fn fake_repo(repo: &Path, snapshots: &[&str]) {
    std::fs::create_dir_all(repo).unwrap();
    std::fs::write(
        repo.join("ferrite-backup-repo.json"),
        r#"{"format":1,"created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    for name in snapshots {
        let dir = repo.join("snapshots").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), "{}").unwrap();
    }
}

fn utc_name(age: chrono::Duration) -> String {
    (chrono::Utc::now() - age)
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

#[test]
fn backup_target_snapshot_age_against_the_schedule() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let repo = outside.path().join("repo");
    write_backup_config(tmp.path(), repo.to_str().unwrap(), "daily");

    // The folder does not exist yet: the first backup creates it.
    let out = offline(tmp.path());
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert_eq!(out.status("backup.target"), "info", "{}", out.stdout);
    assert!(!repo.exists(), "doctor must not create the repository");

    // A fresh snapshot.
    fake_repo(&repo, &[&utc_name(chrono::Duration::hours(3))]);
    let out = offline(tmp.path());
    assert_eq!(out.status("backup.target"), "ok", "{}", out.stdout);

    // Older than twice the daily interval.
    std::fs::remove_dir_all(&repo).unwrap();
    fake_repo(
        &repo,
        &[
            &utc_name(chrono::Duration::hours(80)),
            &utc_name(chrono::Duration::hours(60)),
        ],
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("backup.target"), "warn", "{}", out.stdout);
    assert_eq!(out.code, 2);

    // A weekly schedule tolerates the same age.
    write_backup_config(tmp.path(), repo.to_str().unwrap(), "weekly");
    let out = offline(tmp.path());
    assert_eq!(out.status("backup.target"), "ok", "{}", out.stdout);

    // A repository with no snapshot yet.
    std::fs::remove_dir_all(&repo).unwrap();
    fake_repo(&repo, &[]);
    let out = offline(tmp.path());
    assert_eq!(out.status("backup.target"), "info", "{}", out.stdout);
}

#[test]
fn backup_target_s3_is_skipped_offline() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    write_backup_config(tmp.path(), "s3://bucket/site", "daily");
    let out = offline(tmp.path());
    assert_eq!(out.status("backup.target"), "skip", "{}", out.stdout);
    assert!(
        out.detail("backup.target").contains("offline"),
        "{}",
        out.stdout
    );
}

#[test]
fn backup_target_without_write_access_is_flagged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let repo = outside.path().join("repo");
    fake_repo(&repo, &[&utc_name(chrono::Duration::hours(1))]);
    write_backup_config(tmp.path(), repo.to_str().unwrap(), "daily");
    chmod(&repo, 0o500);
    let out = offline(tmp.path());
    chmod(&repo, 0o700);
    if is_root() {
        return;
    }
    assert_eq!(out.status("backup.target"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
}

#[test]
fn secrets_in_the_config_never_appear() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n\
         [mail]\nsmtp_url = \"smtps://user:hunter2@mail.example.com\"\nfrom = \"ops@example.com\"\n",
    )
    .unwrap();
    chmod(&tmp.path().join("ferrite.toml"), 0o600);
    let out = offline(tmp.path());
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert!(!out.stdout.contains("hunter2"), "{}", out.stdout);
    assert!(!out.stderr.contains("hunter2"), "{}", out.stderr);
}

/// `(mode, size, mtime)` of every file and folder under `root`.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        out.insert(
            dir.clone(),
            (
                meta.permissions().mode(),
                meta.len(),
                meta.modified().unwrap(),
            ),
        );
        if meta.is_dir() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                stack.push(entry.unwrap().path());
            }
        }
    }
    out
}

#[test]
fn doctor_changes_nothing_under_the_data_folder() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    // A site with the optional things doctor looks at.
    std::fs::create_dir(tmp.path().join("themes")).unwrap();
    std::fs::write(tmp.path().join("backups/pre-1-to-2.db"), b"x").unwrap();
    write_upgrade(
        tmp.path(),
        r#"{"v":1,"from":"0.9.0","to":"0.10.0","schema_from":1,"schema_to":2,
            "snapshot":null,"at":"2026-10-29T10:00:00Z"}"#,
    );
    let repo = outside.path().join("repo");
    fake_repo(&repo, &[&utc_name(chrono::Duration::hours(1))]);
    let toml = std::fs::read_to_string(tmp.path().join("ferrite.toml")).unwrap();
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        format!(
            "{toml}\n[backup]\nschedule = \"daily\"\nto = \"{}\"\n",
            repo.display()
        ),
    )
    .unwrap();
    chmod(&tmp.path().join("ferrite.toml"), 0o600);
    // A loose key file, a provisional upgrade record and the rest: every branch that
    // reports must stay read-only.
    chmod(&tmp.path().join("secret.key"), 0o644);

    let before = unchanged_view(tmp.path());
    let repo_before = snapshot(&repo);
    // Let a changed mtime show up even on coarse clocks.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let out = offline(tmp.path());
    // It really ran (a usage error would be exit 2 with no lines).
    assert_eq!(out.ids(), ORDER, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.code, 2, "{}", out.stdout);
    assert_eq!(out.status("fs.secret_key"), "warn");
    assert_eq!(out.status("upgrade.record"), "warn");
    assert_eq!(out.status("backup.target"), "ok");
    // The database checks ran (read-only) and the media probe ran.
    assert_eq!(out.status("db.connect"), "ok", "{}", out.stdout);
    assert_eq!(out.status("db.schema"), "ok", "{}", out.stdout);
    assert_eq!(out.status("db.identity"), "ok", "{}", out.stdout);
    assert_eq!(out.status("media.store"), "ok", "{}", out.stdout);
    assert_eq!(
        before,
        unchanged_view(tmp.path()),
        "the data folder changed"
    );
    assert_eq!(repo_before, snapshot(&repo), "the backup target changed");

    // Online too (the release lookup goes to a closed local port).
    std::thread::sleep(std::time::Duration::from_millis(50));
    let out = doctor_args(tmp.path(), &["--base-url", &closed_url()]);
    assert_eq!(out.ids(), ORDER, "{}\n{}", out.stdout, out.stderr);
    assert_eq!(out.status("release.latest"), "info", "{}", out.stdout);
    assert_eq!(
        before,
        unchanged_view(tmp.path()),
        "the data folder changed"
    );

    // A database without its WAL files (a clean shutdown checkpointed them away): the
    // read-only open must not create them.
    let db = tmp.path().join("ferrite.db");
    checkpoint(&db);
    assert!(!tmp.path().join("ferrite.db-wal").exists());
    let before = unchanged_view(tmp.path());
    std::thread::sleep(std::time::Duration::from_millis(50));
    let out = offline(tmp.path());
    assert_eq!(out.status("db.schema"), "ok", "{}", out.stdout);
    assert_eq!(
        before,
        unchanged_view(tmp.path()),
        "the data folder changed"
    );
    assert!(!tmp.path().join("ferrite.db-wal").exists());
    assert!(!tmp.path().join("ferrite.db-shm").exists());
}

/// [`snapshot`] of a data folder minus what doctor may touch: the media probe's `tmp/`
/// folder (and the media root's own mtime and size, which change when `tmp/` is first
/// made), the only write P49 allows; and the mtime of SQLite's `-shm` file, the shared
/// memory index every reader of a WAL database updates (no data is written).
fn unchanged_view(root: &Path) -> BTreeMap<PathBuf, (u32, u64, SystemTime)> {
    let media = root.join("media");
    snapshot(root)
        .into_iter()
        .filter(|(path, _)| !path.starts_with(media.join("tmp")))
        .map(|(path, (mode, len, mtime))| {
            if path == media {
                (path, (mode, 0, SystemTime::UNIX_EPOCH))
            } else if path.to_string_lossy().ends_with("-shm") {
                (path, (mode, len, SystemTime::UNIX_EPOCH))
            } else {
                (path, (mode, len, mtime))
            }
        })
        .collect()
}

/// Checkpoints `db` and closes it as its last connection, so SQLite removes `-wal` and
/// `-shm` (what a clean `serve` shutdown leaves).
fn checkpoint(db: &Path) {
    use sqlx::Connection;
    let db = db.to_owned();
    std::thread::spawn(move || {
        rt().block_on(async move {
            let mut conn = sqlx::SqliteConnection::connect(&format!("sqlite://{}", db.display()))
                .await
                .expect("connect");
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&mut conn)
                .await
                .expect("checkpoint");
            conn.close().await.expect("close");
        });
    })
    .join()
    .unwrap();
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// An `http://` URL on a local port nothing listens on.
fn closed_url() -> String {
    format!("http://127.0.0.1:{}", closed_port())
}

fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn help_documents_exit_codes() {
    let out = ferrite().args(["doctor", "--help"]).output().unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("--offline"), "{help}");
    assert!(help.contains("--data"), "{help}");
    assert!(help.contains("Exit code"), "{help}");
}

fn init_into(dir: &Path) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(dir)
        .assert()
        .success();
}

#[test]
fn init_gives_a_new_data_folder_mode_0750_and_leaves_an_existing_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // A new leaf under missing parents: the parents are made as usual, the leaf is 0750
    // (less if the umask takes more away, never more).
    let site = tmp.path().join("a").join("b").join("site");
    init_into(&site);
    let mode = mode_of(&site);
    assert_eq!(mode & 0o027, 0, "no group write, no world bits: {mode:o}");
    assert_eq!(mode & 0o700, 0o700, "{mode:o}");
    // An existing folder keeps whatever mode it has.
    let existing = tmp.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    chmod(&existing, 0o755);
    init_into(&existing);
    assert_eq!(mode_of(&existing), 0o755);
    chmod(&existing, 0o775);
    std::fs::remove_file(existing.join("ferrite.toml")).unwrap();
    std::fs::remove_file(existing.join("ferrite.db")).unwrap();
    init_into(&existing);
    assert_eq!(mode_of(&existing), 0o775);
}

#[test]
fn fresh_init_into_a_new_folder_is_clean() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_into(&site);
    let out = offline(&site);
    assert_eq!(out.count("warn"), 0, "{}", out.stdout);
    assert_eq!(out.count("error"), 0, "{}", out.stdout);
    assert_eq!(out.code, 0, "{}", out.stdout);
    assert_eq!(out.status("fs.data_dir"), "ok", "{}", out.stdout);
}

#[test]
fn lock_state_sees_a_held_lock_and_releases_its_own() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let lock_path = tmp.path().join("ferrite.lock");
    let held = std::fs::File::options()
        .write(true)
        .open(&lock_path)
        .expect("init leaves ferrite.lock");
    held.try_lock().expect("take the lock like a node");
    let out = offline(tmp.path());
    assert_eq!(out.status("db.lock_state"), "info", "{}", out.stdout);
    assert!(
        out.detail("db.lock_state").contains("a node is running"),
        "{}",
        out.stdout
    );
    // The file checks work while a node runs.
    assert_eq!(out.status("fs.database_files"), "ok", "{}", out.stdout);
    drop(held);
    // Doctor released the lock it tried: a node can start again.
    let out = offline(tmp.path());
    assert!(
        out.detail("db.lock_state").contains("no node"),
        "{}",
        out.stdout
    );
    let again = std::fs::File::open(&lock_path).unwrap();
    again.try_lock().expect("doctor left no lock behind");
}

#[test]
fn doctor_does_not_create_a_missing_lock_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::remove_file(tmp.path().join("ferrite.lock")).unwrap();
    let out = offline(tmp.path());
    assert!(
        out.detail("db.lock_state").contains("no node"),
        "{}",
        out.stdout
    );
    // The database checks still run (read-only, no lock needed).
    assert_eq!(out.status("db.schema"), "ok", "{}", out.stdout);
    assert!(!tmp.path().join("ferrite.lock").exists());
}

// ---- Task 7: the database, media, cluster, clock and network checks ----

/// `doctor --data DIR` with `env`, the release lookup pointed at a closed local port.
fn online(data: &Path, env: &[(&str, String)]) -> Output {
    let mut cmd = ferrite();
    cmd.arg("doctor")
        .arg("--data")
        .arg(data)
        .args(["--base-url", &closed_url()]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    run(cmd)
}

/// `doctor --data DIR --offline` with `env`.
fn offline_env(data: &Path, env: &[(&str, String)]) -> Output {
    let mut cmd = ferrite();
    cmd.arg("doctor").arg("--data").arg(data).arg("--offline");
    for (k, v) in env {
        cmd.env(k, v);
    }
    run(cmd)
}

/// Runs `statements` against the SQLite database `db`, then closes it.
fn sqlite_exec(db: &Path, statements: &[&str]) {
    use sqlx::Connection;
    let db = db.to_owned();
    let statements: Vec<String> = statements.iter().map(|s| (*s).to_owned()).collect();
    std::thread::spawn(move || {
        rt().block_on(async move {
            let mut conn = sqlx::SqliteConnection::connect(&format!("sqlite://{}", db.display()))
                .await
                .expect("connect");
            for sql in statements {
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut conn)
                    .await
                    .expect("statement");
            }
            conn.close().await.expect("close");
        });
    })
    .join()
    .unwrap();
}

fn sqlite_meta(db: &Path, key: &str) -> String {
    use sqlx::Connection;
    let (db, key) = (db.to_owned(), key.to_owned());
    std::thread::spawn(move || {
        rt().block_on(async move {
            let mut conn = sqlx::SqliteConnection::connect(&format!("sqlite://{}", db.display()))
                .await
                .expect("connect");
            let value: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = ?")
                .bind(&key)
                .fetch_one(&mut conn)
                .await
                .expect("schema_meta");
            conn.close().await.expect("close");
            value
        })
    })
    .join()
    .unwrap()
}

/// `ferrite serve`, killed on drop.
struct Serve(std::process::Child);

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `serve` for `data` with `env` and waits for its "Listening on" line.
fn serve(data: &Path, env: &[(&str, String)]) -> Serve {
    use std::io::BufRead;
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_ferrite"));
    cmd.env_clear()
        .args(["serve", "--data"])
        .arg(data)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn serve");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if line.starts_with("Listening on http://") {
                let _ = tx.send(());
            }
        }
    });
    let serve = Serve(child);
    rx.recv_timeout(std::time::Duration::from_secs(60))
        .expect("serve did not report its address");
    serve
}

#[test]
fn a_running_sqlite_site_is_read_without_its_lock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let env = [(
        "FERRITE_SERVER__LISTEN",
        format!("127.0.0.1:{}", closed_port()),
    )];
    let _serve = serve(tmp.path(), &env);
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("db.lock_state"), "info", "{}", out.stdout);
    assert!(
        out.detail("db.lock_state").contains("a node is running"),
        "{}",
        out.stdout
    );
    for id in [
        "db.connect",
        "db.schema",
        "db.identity",
        "media.store",
        "net.listen",
    ] {
        assert_eq!(out.status(id), "ok", "{id}\n{}\n{}", out.stdout, out.stderr);
    }
    assert!(
        out.detail("db.connect").contains("read-only"),
        "{}",
        out.stdout
    );
    let listen = out.detail("net.listen");
    assert!(
        listen.contains("/healthz") && listen.contains("/readyz"),
        "{listen}"
    );
    assert_eq!(out.count("error"), 0, "{}", out.stdout);
    // Doctor took nothing from the node: it still answers.
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("net.listen"), "ok", "{}", out.stdout);
}

#[test]
fn net_listen_is_skipped_without_a_node_and_flags_a_silent_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = online(tmp.path(), &[]);
    assert_eq!(out.status("net.listen"), "skip", "{}", out.stdout);
    // A process holds the lock, but nothing answers on server.listen.
    let held = std::fs::File::options()
        .write(true)
        .open(tmp.path().join("ferrite.lock"))
        .unwrap();
    held.try_lock().unwrap();
    let env = [(
        "FERRITE_SERVER__LISTEN",
        format!("127.0.0.1:{}", closed_port()),
    )];
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("net.listen"), "warn", "{}", out.stdout);
    // The database checks still run, read-only.
    assert_eq!(out.status("db.schema"), "ok", "{}", out.stdout);
}

#[test]
fn a_schema_newer_than_the_binary_allows_is_exit_3() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    // Pending migrations: a warning.
    ferrite()
        .args([
            "migrate",
            "down",
            "--to",
            "1",
            "--allow-lossy",
            "--yes",
            "--data",
        ])
        .arg(tmp.path())
        .assert()
        .success();
    let out = offline(tmp.path());
    assert_eq!(out.status("db.schema"), "warn", "{}", out.stdout);
    assert!(
        out.detail("db.schema").contains("migrate up"),
        "{}",
        out.stdout
    );
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(tmp.path())
        .assert()
        .success();
    // A newer schema that requires a newer binary (as the `migrate status` test fakes it).
    sqlite_exec(
        &tmp.path().join("ferrite.db"),
        &[
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (99, 'future', 1, x'00', 0)",
            "INSERT INTO schema_meta (key, value) VALUES ('min_binary_version', '99.0.0')",
        ],
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("db.schema"), "error", "{}", out.stdout);
    assert!(out.detail("db.schema").contains("99.0.0"), "{}", out.stdout);
    assert_eq!(out.code, 3, "{}", out.stdout);
}

#[test]
fn identity_checks_site_id_and_the_last_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let db = tmp.path().join("ferrite.db");
    sqlite_exec(
        &db,
        &["UPDATE schema_meta SET value = '99.0.0' WHERE key = 'last_binary_version'"],
    );
    let out = offline(tmp.path());
    assert_eq!(out.status("db.identity"), "warn", "{}", out.stdout);
    assert!(
        out.detail("db.identity").contains("99.0.0"),
        "{}",
        out.stdout
    );
    sqlite_exec(&db, &["DELETE FROM schema_meta WHERE key = 'site_id'"]);
    let out = offline(tmp.path());
    assert_eq!(out.status("db.identity"), "error", "{}", out.stdout);
    assert_eq!(out.code, 1);
}

#[test]
fn an_owner_marker_of_another_install_is_a_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let marker = tmp.path().join("media").join("ferrite-owner.json");
    std::fs::write(
        &marker,
        r#"{"install_id":"another-install","site_id":"s","claimed_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("media.owner"), "warn", "{}", out.stdout);
    let detail = out.detail("media.owner");
    assert!(
        detail.contains("another-install") && detail.contains("--claim"),
        "{detail}"
    );
    assert_eq!(out.code, 2);
    // A marker naming this install.
    let install = sqlite_meta(&tmp.path().join("ferrite.db"), "install_id");
    std::fs::write(
        &marker,
        format!(
            r#"{{"install_id":"{install}","site_id":"s","claimed_at":"2026-01-01T00:00:00Z"}}"#
        ),
    )
    .unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("media.owner"), "ok", "{}", out.stdout);
    // A marker that is not one.
    std::fs::write(&marker, "not json").unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("media.owner"), "warn", "{}", out.stdout);
}

#[test]
fn a_missing_local_media_root_is_an_error_and_is_not_created() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::remove_dir_all(tmp.path().join("media")).unwrap();
    let out = offline(tmp.path());
    assert_eq!(out.status("media.store"), "error", "{}", out.stdout);
    assert_eq!(out.status("media.owner"), "skip", "{}", out.stdout);
    assert!(!tmp.path().join("media").exists(), "doctor created media/");
}

/// An in-process S3 server (s3s-fs) with one bucket, `doctor-test`.
struct S3 {
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    endpoint: String,
}

const S3_SECRET: &str = "s3-secret-hunter2";

fn s3_server() -> S3 {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder as ConnBuilder;
    use s3s::auth::SimpleAuth;
    use s3s::service::S3ServiceBuilder;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("doctor-test")).unwrap();
    let rt = rt();
    let root = dir.path().to_owned();
    let addr = rt.block_on(async move {
        let fs = s3s_fs::FileSystem::new(&root).expect("s3s-fs");
        let mut builder = S3ServiceBuilder::new(fs);
        builder.set_auth(SimpleAuth::from_single("test-access", S3_SECRET));
        let service = builder.build();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let service = service.clone();
                tokio::spawn(async move {
                    let http = ConnBuilder::new(TokioExecutor::new());
                    let _ = http.serve_connection(TokioIo::new(socket), service).await;
                });
            }
        });
        addr
    });
    S3 {
        _rt: rt,
        _dir: dir,
        endpoint: format!("http://{addr}"),
    }
}

fn s3_env(s3: &S3, bucket: &str) -> Vec<(&'static str, String)> {
    vec![
        ("FERRITE_MEDIA__BACKEND", "s3".into()),
        ("FERRITE_MEDIA__S3__BUCKET", bucket.into()),
        ("FERRITE_MEDIA__S3__REGION", "us-east-1".into()),
        ("FERRITE_MEDIA__S3__ENDPOINT", s3.endpoint.clone()),
        ("FERRITE_MEDIA__S3__ACCESS_KEY_ID", "test-access".into()),
        ("FERRITE_MEDIA__S3__SECRET_ACCESS_KEY", S3_SECRET.into()),
        ("FERRITE_MEDIA__S3__ALLOW_HTTP", "true".into()),
    ]
}

#[test]
fn media_store_probes_s3_and_never_prints_its_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    // `init` wrote a local media root; the S3 settings come from the environment.
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n",
    )
    .unwrap();
    chmod(&tmp.path().join("ferrite.toml"), 0o600);
    let s3 = s3_server();
    let out = online(tmp.path(), &s3_env(&s3, "doctor-test"));
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert_eq!(out.status("media.store"), "ok", "{}", out.stdout);
    let detail = out.detail("media.store");
    assert!(
        detail.contains("doctor-test") && detail.contains(&s3.endpoint),
        "{detail}"
    );
    assert_eq!(out.status("media.owner"), "info", "{}", out.stdout);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains(S3_SECRET), "{text}");
    }
    // A store doctor cannot use. (s3s-fs creates a missing bucket on the first put, so a
    // wrong bucket name passes here; a wrong secret key is refused like one.)
    let mut wrong = s3_env(&s3, "doctor-test");
    wrong.retain(|(k, _)| *k != "FERRITE_MEDIA__S3__SECRET_ACCESS_KEY");
    wrong.push((
        "FERRITE_MEDIA__S3__SECRET_ACCESS_KEY",
        "wrong-hunter2-key".into(),
    ));
    let out = online(tmp.path(), &wrong);
    assert_eq!(out.status("media.store"), "error", "{}", out.stdout);
    assert_eq!(out.status("media.owner"), "skip", "{}", out.stdout);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains("hunter2"), "{text}");
    }
    // An endpoint nothing answers on.
    let mut gone = s3_env(&s3, "doctor-test");
    gone.retain(|(k, _)| *k != "FERRITE_MEDIA__S3__ENDPOINT");
    gone.push(("FERRITE_MEDIA__S3__ENDPOINT", closed_url()));
    let out = online(tmp.path(), &gone);
    assert_eq!(out.status("media.store"), "error", "{}", out.stdout);
    // Offline: not probed.
    let out = offline_env(tmp.path(), &s3_env(&s3, "doctor-test"));
    assert_eq!(out.status("media.store"), "skip", "{}", out.stdout);
    assert_eq!(out.status("media.owner"), "skip", "{}", out.stdout);
}

#[test]
fn smtp_connect_failure_is_a_warning_without_credentials() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let port = closed_port();
    let env = [
        (
            "FERRITE_MAIL__SMTP_URL",
            format!("smtp://doctor-user:hunter2@127.0.0.1:{port}"),
        ),
        ("FERRITE_MAIL__FROM", "ops@example.com".to_owned()),
    ];
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("smtp.connect"), "warn", "{}", out.stdout);
    let detail = out.detail("smtp.connect");
    assert!(
        detail.contains(&format!("127.0.0.1:{port}")) && detail.contains("plain"),
        "{detail}"
    );
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("doctor-user"), "{text}");
    }
    let out = offline_env(tmp.path(), &env);
    assert_eq!(out.status("smtp.connect"), "skip", "{}", out.stdout);
    // Mail off.
    let out = online(tmp.path(), &[]);
    assert_eq!(out.status("smtp.connect"), "info", "{}", out.stdout);
}

#[test]
fn acme_directory_that_does_not_answer_is_a_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        format!(
            "[site]\nurl = \"https://example.com\"\n\
             [server.tls]\nmode = \"acme\"\ncontact = \"mailto:ops@example.com\"\n\
             directory = \"https://127.0.0.1:{}/directory\"\n",
            closed_port()
        ),
    )
    .unwrap();
    chmod(&tmp.path().join("ferrite.toml"), 0o600);
    let out = online(tmp.path(), &[]);
    assert_eq!(out.status("config.load"), "ok", "{}", out.stdout);
    assert_eq!(out.status("tls.acme_directory"), "warn", "{}", out.stdout);
    let out = offline(tmp.path());
    assert_eq!(out.status("tls.acme_directory"), "skip", "{}", out.stdout);
}

/// A local server answering `GET /repos/softberries/ferritecms/releases/latest` with a
/// release tagged `tag`.
fn release_server(tag: &str) -> (tokio::runtime::Runtime, String) {
    let body =
        serde_json::json!({"tag_name": tag, "draft": false, "prerelease": false, "assets": []})
            .to_string();
    let rt = rt();
    let addr = rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/repos/softberries/ferritecms/releases/latest",
            axum::routing::get(move || {
                let body = body.clone();
                async move { body }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    });
    (rt, format!("http://{addr}"))
}

#[test]
fn release_latest_warns_on_a_newer_release_and_informs_when_unreachable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let (_rt, base) = release_server("v9.9.9");
    let out = doctor_args(tmp.path(), &["--base-url", &base]);
    assert_eq!(out.status("release.latest"), "warn", "{}", out.stdout);
    assert!(
        out.detail("release.latest").contains("9.9.9"),
        "{}",
        out.stdout
    );
    assert_eq!(out.code, 2, "{}", out.stdout);
    let (_rt2, base) = release_server(&format!("v{}", env!("CARGO_PKG_VERSION")));
    let out = doctor_args(tmp.path(), &["--base-url", &base]);
    assert_eq!(out.status("release.latest"), "ok", "{}", out.stdout);
    let out = doctor_args(tmp.path(), &["--base-url", &closed_url()]);
    assert_eq!(out.status("release.latest"), "info", "{}", out.stdout);
    assert_eq!(out.code, 0, "{}", out.stdout);
}

// ---- Postgres ----

const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";

struct PgSchema {
    url: String,
    schema: String,
}

impl PgSchema {
    /// `None` (after a SKIP line) when `FERRITE_TEST_PG_URL` is unset; panics in CI.
    fn new() -> Option<Self> {
        let Ok(url) = std::env::var(PG_URL_ENV) else {
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("{PG_URL_ENV} must be set in CI");
            }
            eprintln!("SKIP: Postgres doctor test skipped ({PG_URL_ENV} is not set)");
            return None;
        };
        let schema = format!("t_{}", uuid::Uuid::now_v7().simple());
        Some(Self { url, schema })
    }

    fn schema_exists(&self, schema: &str) -> bool {
        use sqlx::Connection;
        let (url, schema) = (self.url.clone(), schema.to_owned());
        std::thread::spawn(move || {
            rt().block_on(async move {
                let mut conn = sqlx::PgConnection::connect(&url).await.expect("connect");
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)",
                )
                .bind(&schema)
                .fetch_one(&mut conn)
                .await
                .expect("query");
                conn.close().await.expect("close");
                exists
            })
        })
        .join()
        .unwrap()
    }

    /// Registers a node row, as a running `serve` would, with `version` and `meta`.
    fn register(&self, name: &str, version: &str, meta: serde_json::Value) {
        use ferrite_store::Store;
        let (url, schema) = (self.url.clone(), self.schema.clone());
        let (name, version) = (name.to_owned(), version.to_owned());
        std::thread::spawn(move || {
            rt().block_on(async move {
                let store =
                    ferrite_store_postgres::PgStore::open(ferrite_store_postgres::PgOptions {
                        max_connections: 1,
                        schema: Some(schema),
                        ..ferrite_store_postgres::PgOptions::new(url)
                    })
                    .await
                    .expect("open pg");
                store
                    .read()
                    .nodes()
                    .register(&ferrite_domain::NewNode {
                        id: ferrite_domain::NodeId::new(),
                        name,
                        version,
                        schema_version: 12,
                        meta,
                    })
                    .await
                    .expect("register");
            });
        })
        .join()
        .unwrap();
    }
}

impl Drop for PgSchema {
    fn drop(&mut self) {
        use sqlx::Connection;
        let url = self.url.clone();
        let sql = format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema);
        let done = std::thread::spawn(move || {
            rt().block_on(async move {
                let mut conn = sqlx::PgConnection::connect(&url).await?;
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut conn)
                    .await?;
                conn.close().await
            })
        })
        .join();
        if let Ok(Err(e)) = done {
            eprintln!("warning: could not drop test schema {}: {e}", self.schema);
        }
    }
}

fn init_pg(dir: &Path, pg: &PgSchema) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--db", &pg.url])
        .args(["--schema", &pg.schema, "--data"])
        .arg(dir)
        .assert()
        .success();
    chmod(dir, 0o750);
}

#[test]
fn postgres_site_checks_and_the_cluster_view() {
    let Some(pg) = PgSchema::new() else { return };
    let tmp = tempfile::tempdir().expect("tempdir");
    init_pg(tmp.path(), &pg);
    let out = online(tmp.path(), &[]);
    assert_eq!(out.status("db.connect"), "ok", "{}", out.stdout);
    assert!(
        out.detail("db.connect").contains("Postgres"),
        "{}",
        out.stdout
    );
    assert_eq!(out.status("db.schema"), "ok", "{}", out.stdout);
    assert_eq!(out.status("db.identity"), "ok", "{}", out.stdout);
    // The local clock and the server's agree here; either way it is a measured line.
    assert!(
        ["ok", "warn"].contains(&out.status("clock.db_skew")),
        "{}",
        out.stdout
    );
    assert!(out.detail("clock.db_skew").contains("ms"), "{}", out.stdout);
    for id in ["clock.peers", "cluster.versions", "cluster.site_url"] {
        assert_eq!(out.status(id), "info", "{id}\n{}", out.stdout);
    }
    assert_eq!(out.status("net.listen"), "skip", "{}", out.stdout);
    assert_eq!(out.count("error"), 0, "{}", out.stdout);

    // Two live nodes on different versions, one of them for another site URL and with a
    // skewed clock.
    pg.register(
        "node-a",
        "0.1.0",
        serde_json::json!({"site_url": "https://example.com", "mode": "solo", "clock_skew_ms": 5}),
    );
    pg.register(
        "node-b",
        "0.2.0",
        serde_json::json!({"site_url": "https://other.example", "mode": "solo", "clock_skew_ms": 4000}),
    );
    let out = online(tmp.path(), &[]);
    assert_eq!(out.status("cluster.versions"), "warn", "{}", out.stdout);
    let versions = out.detail("cluster.versions");
    assert!(
        versions.contains("0.1.0") && versions.contains("0.2.0"),
        "{versions}"
    );
    assert_eq!(out.status("cluster.site_url"), "warn", "{}", out.stdout);
    assert!(
        out.detail("cluster.site_url").contains("node-b"),
        "{}",
        out.stdout
    );
    assert!(
        !out.detail("cluster.site_url").contains("node-a"),
        "{}",
        out.stdout
    );
    assert_eq!(out.status("clock.peers"), "warn", "{}", out.stdout);
    assert!(
        out.detail("clock.peers").contains("node-b"),
        "{}",
        out.stdout
    );
    assert_eq!(out.code, 2, "{}", out.stdout);

    // A live row with this configuration's node name is this folder's node: net.listen
    // probes it (nothing answers on that port here).
    let env = [
        ("FERRITE_CLUSTER__NODE_NAME", "node-a".to_owned()),
        (
            "FERRITE_SERVER__LISTEN",
            format!("127.0.0.1:{}", closed_port()),
        ),
    ];
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("net.listen"), "warn", "{}", out.stdout);
    let env = [("FERRITE_CLUSTER__NODE_NAME", "node-z".to_owned())];
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("net.listen"), "skip", "{}", out.stdout);
}

#[test]
fn postgres_missing_schema_is_an_error_and_is_not_created() {
    let Some(pg) = PgSchema::new() else { return };
    let tmp = tempfile::tempdir().expect("tempdir");
    init_pg(tmp.path(), &pg);
    let missing = format!("{}_missing", pg.schema);
    let env = [("FERRITE_DATABASE__SCHEMA", missing.clone())];
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("db.connect"), "error", "{}", out.stdout);
    assert!(
        out.detail("db.connect").contains(&missing),
        "{}",
        out.stdout
    );
    for id in [
        "db.schema",
        "db.identity",
        "clock.db_skew",
        "cluster.versions",
    ] {
        assert_eq!(out.status(id), "skip", "{id}\n{}", out.stdout);
    }
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(!pg.schema_exists(&missing), "doctor created the schema");
    assert!(pg.schema_exists(&pg.schema));
}

#[test]
fn postgres_password_is_never_printed() {
    let Some(pg) = PgSchema::new() else { return };
    let tmp = tempfile::tempdir().expect("tempdir");
    init_pg(tmp.path(), &pg);
    let mut url = url::Url::parse(&pg.url).unwrap();
    url.set_password(Some("pg-hunter2")).unwrap();
    let env = [("FERRITE_DATABASE__URL", url.to_string())];
    // Whether the server checks the password or not, it never shows.
    let out = online(tmp.path(), &env);
    assert!(
        ["ok", "error"].contains(&out.status("db.connect")),
        "{}",
        out.stdout
    );
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains("pg-hunter2"), "{text}");
    }
    // An unreachable server: an error, still without the password.
    let env = [(
        "FERRITE_DATABASE__URL",
        format!(
            "postgres://ferrite:pg-hunter2@127.0.0.1:{}/x",
            closed_port()
        ),
    )];
    let out = online(tmp.path(), &env);
    assert_eq!(out.status("db.connect"), "error", "{}", out.stdout);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains("pg-hunter2"), "{text}");
    }
}
