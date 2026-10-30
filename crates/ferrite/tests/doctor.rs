#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `ferrite doctor`: the runner, the output format, the exit codes and the file checks
//! (M4-3a Task 6; the database, media, cluster, clock and network checks are Task 7).

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

/// The checks Task 7 owns: `skip  <id>  not checked yet` until then.
const TASK_7: &[&str] = &[
    "db.connect",
    "db.schema",
    "db.identity",
    "media.store",
    "media.owner",
    "clock.db_skew",
    "clock.peers",
    "cluster.versions",
    "cluster.site_url",
    "smtp.connect",
    "tls.acme_directory",
    "net.listen",
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
        ("upgrade.record", "ok"),
        ("tls.dir", "info"),
        ("backup.target", "info"),
    ]
    .into_iter()
    .chain(TASK_7.iter().map(|id| (*id, "skip")))
    .collect();
    for (id, status) in &expected {
        assert_eq!(out.status(id), *status, "{id}\n{}", out.stdout);
    }
    assert_eq!(expected.len(), ORDER.len());
    for id in TASK_7 {
        assert_eq!(out.detail(id), "not checked yet", "{id}");
    }
    assert_eq!(
        out.summary,
        format!("doctor: 7 ok, 0 warn, 0 error, {} skip", TASK_7.len())
    );
    // The lock is not held.
    assert!(
        out.detail("db.lock_state").contains("no node"),
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
    assert_eq!(out.code, 0, "{}", out.stdout);

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

    let before = snapshot(tmp.path());
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
    assert_eq!(before, snapshot(tmp.path()), "the data folder changed");
    assert_eq!(repo_before, snapshot(&repo), "the backup target changed");
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
    assert!(!tmp.path().join("ferrite.lock").exists());
}
