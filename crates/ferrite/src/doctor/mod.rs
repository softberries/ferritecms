//! `ferrite doctor`: checks a site without changing anything (M4-3a P48–P53).
//!
//! One line per check, `<status>  <check-id>  <detail>` with the status padded to five
//! characters, in the fixed order of [`run`]; then `doctor: N ok, N warn, N error, N skip`.
//! The exit code is 3 when the database schema is newer than this binary allows, else 1
//! when a check is an error, else 2 when one is a warning, else 0 ([`Report::exit_code`]).
//!
//! Doctor never writes (P49): it creates and changes no file, folder, key or schema. It
//! does not call `keys::site_key`, `gc::install_id` or `TlsCache::prepare`, and does not
//! open a local media root that does not exist. Permissions are asked of the kernel
//! (`access(2)`), not tried by writing a probe. Nothing prints a secret: database and SMTP
//! URLs, S3 keys and `secret.key` never appear.
//!
//! This module holds the runner, the status and the report; `fs.rs` and `backup.rs` the file
//! checks, `db.rs` the database checks (read-only: SQLite without its lock, Postgres only
//! once its schema is known to exist), `media.rs` the media probe and owner marker,
//! `cluster.rs` the clock and node-registry checks, and `net.rs` SMTP, the ACME directory,
//! the running node's endpoints and the release lookup.

mod backup;
mod cluster;
mod db;
mod fs;
mod media;
mod net;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use ferrite_config::Config;

use crate::config_cmd::{self, Checked};

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Fine.
    Ok,
    /// Something to know that needs no action (or does not apply to this site).
    Info,
    /// Works, but is likely to cause trouble; exit code 2.
    Warn,
    /// Broken; exit code 1.
    Error,
    /// Not run: offline, a node is running, the configuration did not load, or not wired
    /// in yet.
    Skip,
}

impl Status {
    /// The word printed for the status.
    pub fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Info => "info",
            Status::Warn => "warn",
            Status::Error => "error",
            Status::Skip => "skip",
        }
    }
}

/// The result of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The check's id from P53 (`fs.data_dir`, …); a check may report several findings.
    pub id: &'static str,
    pub status: Status,
    /// One line, never a secret.
    pub detail: String,
}

impl Finding {
    /// A finding; line breaks in `detail` become spaces so one finding is one line.
    pub fn new(id: &'static str, status: Status, detail: impl Into<String>) -> Self {
        let detail: String = detail.into();
        let detail = detail
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        Finding { id, status, detail }
    }

    pub fn ok(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Ok, detail)
    }

    pub fn info(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Info, detail)
    }

    pub fn warn(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Warn, detail)
    }

    pub fn error(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Error, detail)
    }

    pub fn skip(id: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Skip, detail)
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<5}  {}  {}",
            self.status.label(),
            self.id,
            self.detail
        )
    }
}

/// Every finding of a run, in output order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// `db.schema` found the database newer than this binary allows (as `migrate status`
    /// does): exit code 3.
    pub incompatible: bool,
}

impl Report {
    /// P48: 3 when the schema is incompatible, else 1 when any check is an error, else 2
    /// when any is a warning, else 0.
    pub fn exit_code(&self) -> u8 {
        if self.incompatible {
            3
        } else if self.count(Status::Error) > 0 {
            1
        } else if self.count(Status::Warn) > 0 {
            2
        } else {
            0
        }
    }

    /// The number of findings with `status`.
    pub fn count(&self, status: Status) -> usize {
        self.findings.iter().filter(|f| f.status == status).count()
    }

    /// `doctor: N ok, N warn, N error, N skip` (`info` lines are not counted).
    pub fn summary(&self) -> String {
        format!(
            "doctor: {} ok, {} warn, {} error, {} skip",
            self.count(Status::Ok),
            self.count(Status::Warn),
            self.count(Status::Error),
            self.count(Status::Skip)
        )
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for finding in &self.findings {
            writeln!(f, "{finding}")?;
        }
        writeln!(f, "{}", self.summary())
    }
}

/// `ferrite doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// The data folder to check.
    #[arg(long)]
    pub data: PathBuf,
    /// Make no network connections: checks that need one (an S3 target, SMTP, the ACME
    /// directory, the release lookup) are skipped.
    #[arg(long)]
    pub offline: bool,
    /// Test seam: the GitHub API root for `release.latest` instead of
    /// https://api.github.com (plain HTTP allowed), as `self-update --base-url`.
    #[cfg(feature = "test-seams")]
    #[arg(long, hide = true)]
    pub base_url: Option<String>,
}

/// The `ferrite doctor` command: prints the report to stdout and exits per
/// [`Report::exit_code`]. Never fails: a problem is a line in the report.
pub async fn execute(args: DoctorArgs) -> anyhow::Result<ExitCode> {
    #[cfg(feature = "test-seams")]
    let release_api = args.base_url.as_deref();
    #[cfg(not(feature = "test-seams"))]
    let release_api = None;
    let report = run_with(&args.data, args.offline, release_api).await;
    print!("{report}");
    Ok(ExitCode::from(report.exit_code()))
}

/// Runs every check against the site in `data`, in the order of P53. With `offline`, no
/// network connection is made.
pub async fn run(data: &Path, offline: bool) -> Report {
    run_with(data, offline, None).await
}

/// [`run`], with `release_api` (the test seam) replacing the GitHub API root of the
/// release lookup.
async fn run_with(data: &Path, offline: bool, release_api: Option<&str>) -> Report {
    let mut findings = vec![version_binary()];
    let (config, loaded) = config_load(data);
    findings.extend(loaded);
    let config = config.as_ref();

    findings.push(fs::data_dir(data));
    findings.push(fs::config_file(data));
    findings.push(fs::secret_key(data, config));
    findings.push(fs::database_files(data, config));
    findings.push(fs::backups_dir(data, config));
    findings.push(fs::themes_dir(data, config));
    let (lock_state, lock_held) = fs::lock_state(data, config);
    findings.push(lock_state);
    let (connect, database) = db::connect(config, lock_held).await;
    findings.push(connect);
    let database = database.as_ref();
    let (schema, incompatible) = db::schema(database, config).await;
    findings.push(schema);
    findings.push(db::identity(database, config).await);
    findings.push(fs::upgrade_record(data, config));
    let (media_store, media) = media::store(config, offline).await;
    findings.push(media_store);
    findings.push(media::owner(media.as_ref(), database, config).await);
    findings.push(cluster::db_skew(database, config).await);
    let registry = cluster::registry(database, config).await;
    findings.push(cluster::peers(&registry));
    findings.push(cluster::versions(&registry));
    findings.push(cluster::site_url(&registry, config));
    findings.push(net::smtp(config, offline).await);
    findings.push(fs::tls_dir(data, config));
    findings.push(net::acme_directory(config, offline).await);
    findings.push(net::listen(config, running_node(config, lock_held, &registry)).await);
    findings.push(backup::target(data, config, offline).await);
    findings.push(net::release(offline, release_api).await);

    Report {
        findings,
        incompatible,
    }
}

/// Whether a node runs for this data folder (P50): SQLite, the lock is held; Postgres, a
/// live node row carries this configuration's `cluster.node_name` (without one, doctor
/// cannot tell which row would be this folder's).
fn running_node(
    config: Option<&Config>,
    lock_held: bool,
    registry: &cluster::Registry,
) -> net::Node {
    let Some(config) = config else {
        return net::Node::NotRunning(fs::NO_CONFIG);
    };
    match config.database_kind() {
        ferrite_config::DbKind::Sqlite { .. } if lock_held => net::Node::Running,
        ferrite_config::DbKind::Sqlite { .. } => {
            net::Node::NotRunning("no node is running on this data folder")
        }
        ferrite_config::DbKind::Postgres { .. } => {
            match config.cluster.node_name.as_deref().map(str::trim) {
                Some(name) if !name.is_empty() && registry.has_node(name) => net::Node::Running,
                Some(name) if !name.is_empty() && registry.was_read() => {
                    net::Node::NotRunning("no live node has this cluster.node_name")
                }
                Some(name) if !name.is_empty() => {
                    net::Node::NotRunning("the node registry was not read (see db.connect)")
                }
                _ => net::Node::NotRunning(
                    "cluster.node_name is not set, so doctor cannot tell whether a node runs \
                     for this folder",
                ),
            }
        }
    }
}

fn version_binary() -> Finding {
    Finding::info(
        "version.binary",
        format!(
            "ferrite {} ({})",
            env!("CARGO_PKG_VERSION"),
            crate::binary_target().trim()
        ),
    )
}

/// `config.load`: the configuration loads as `serve` loads it. Every problem is an error,
/// every warning a warning. A missing `ferrite.toml` is an error here (the loader itself
/// would use defaults). The configuration is returned when it loaded, even if a check that
/// `serve` runs afterwards refused it, so the other checks can still use it.
fn config_load(data: &Path) -> (Option<Config>, Vec<Finding>) {
    const ID: &str = "config.load";
    let file = data.join(crate::backup::CONFIG_FILE);
    match std::fs::metadata(&file) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (
                None,
                vec![Finding::error(
                    ID,
                    format!(
                        "{} not found (create the site with `ferrite init`)",
                        file.display()
                    ),
                )],
            );
        }
        Err(e) => {
            return (
                None,
                vec![Finding::error(
                    ID,
                    format!("cannot read {}: {}", file.display(), e.kind()),
                )],
            );
        }
    }
    let env = match crate::utf8_env() {
        Ok(env) => env,
        Err(e) => return (None, vec![Finding::error(ID, format!("{e:#}"))]),
    };
    match config_cmd::check(data, env) {
        Checked::Ok { config, warnings } => {
            let mut out = vec![Finding::ok(ID, config_cmd::summary_fields(&config))];
            out.extend(warnings.into_iter().map(|w| Finding::warn(ID, w)));
            (Some(config), out)
        }
        Checked::Refused { config, problem } => {
            let finding = Finding::error(ID, format!("{}: {}", problem.path, problem.message));
            (Some(config), vec![finding])
        }
        Checked::Invalid(e) => {
            let out = e
                .problems
                .iter()
                .map(|p| Finding::error(ID, format!("{}: {}", p.path, p.message)))
                .collect();
            (None, out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(statuses: &[Status], incompatible: bool) -> Report {
        Report {
            findings: statuses
                .iter()
                .map(|s| Finding::new("x", *s, "d"))
                .collect(),
            incompatible,
        }
    }

    #[test]
    fn exit_code_follows_p48() {
        use Status::{Error, Info, Ok, Skip, Warn};
        assert_eq!(report(&[], false).exit_code(), 0);
        assert_eq!(report(&[Ok, Info, Skip], false).exit_code(), 0);
        assert_eq!(report(&[Ok, Warn], false).exit_code(), 2);
        assert_eq!(report(&[Warn, Error], false).exit_code(), 1);
        // Incompatible wins over everything.
        assert_eq!(report(&[Ok], true).exit_code(), 3);
        assert_eq!(report(&[Error, Warn], true).exit_code(), 3);
    }

    #[test]
    fn lines_pad_the_status_to_five_and_stay_on_one_line() {
        let f = Finding::new("fs.data_dir", Status::Ok, "a\nb\r\nc");
        assert_eq!(f.to_string(), "ok     fs.data_dir  a b  c");
        assert_eq!(Finding::skip("x", "y").to_string(), "skip   x  y");
        assert_eq!(Finding::error("x", "y").to_string(), "error  x  y");
        assert_eq!(Finding::warn("x", "y").to_string(), "warn   x  y");
        assert_eq!(Finding::info("x", "y").to_string(), "info   x  y");
    }

    #[test]
    fn summary_counts_everything_but_info() {
        use Status::{Error, Info, Ok, Skip, Warn};
        let r = report(&[Ok, Ok, Info, Warn, Error, Skip, Skip, Skip], false);
        assert_eq!(r.summary(), "doctor: 2 ok, 1 warn, 1 error, 3 skip");
        assert!(
            r.to_string()
                .ends_with("doctor: 2 ok, 1 warn, 1 error, 3 skip\n")
        );
    }
}
