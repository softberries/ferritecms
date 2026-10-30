//! `backup.target`: the `[backup] to` repository is reachable and writable, and its newest
//! snapshot is recent enough for the schedule. Nothing is created or written (P49): a
//! local repository is read from the folder, an S3 one is listed (skipped with `--offline`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{NaiveDateTime, Utc};
use ferrite_config::{BackupSchedule, Config, DbKind};

use super::Finding;
use super::fs::can_write_in;
use crate::backup::repo::{Repo, SNAPSHOT_FORMAT};
use crate::backup::target::{RepoLocation, Target};

const ID: &str = "backup.target";
/// How long listing an S3 prefix may take (P51).
const S3_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn target(data: &Path, config: Option<&Config>, offline: bool) -> Finding {
    let Some(config) = config else {
        return Finding::skip(ID, "the configuration did not load (see config.load)");
    };
    let Some(to) = config.backup.to.as_deref() else {
        return Finding::info(ID, "no [backup] target configured");
    };
    let Ok(Target::Repo(location)) = Target::parse(to, data) else {
        // `config.load` reports why.
        return Finding::skip(
            ID,
            "backup.to is not a usable repository location (see config.load)",
        );
    };
    let schedule = config.backup.schedule;
    // A Postgres site's scheduled backup is a series of export archives.
    let exports = matches!(config.database_kind(), DbKind::Postgres { .. });
    match &location {
        RepoLocation::Dir(dir) => local(dir, schedule, exports).await,
        RepoLocation::S3(url) => {
            if offline {
                return Finding::skip(ID, format!("{url} is not probed (--offline)"));
            }
            match tokio::time::timeout(S3_TIMEOUT, Repo::survey(&location, exports)).await {
                Err(_) => Finding::warn(
                    ID,
                    format!("{url}: no answer within {} s", S3_TIMEOUT.as_secs()),
                ),
                // The backup uses `AWS_*` from the service's environment, which may differ
                // from this shell's.
                Ok(Err(e)) => {
                    Finding::warn(ID, format!("{url}: {e:#} (is AWS_* set for this shell?)"))
                }
                Ok(Ok(found)) => judged(url, found, schedule, exports),
            }
        }
    }
}

/// A local repository folder.
async fn local(dir: &Path, schedule: BackupSchedule, exports: bool) -> Finding {
    // A scheduled backup that cannot write is broken; with the schedule off it is only a
    // folder the next manual `ferrite backup` may not reach.
    let unusable = |detail: String| {
        if schedule == BackupSchedule::Off {
            Finding::warn(ID, detail)
        } else {
            Finding::error(ID, detail)
        }
    };
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Finding::error(ID, format!("{} exists and is not a folder", dir.display()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The first backup creates it, so the nearest folder that exists must be writable.
            let parent = nearest_existing(dir);
            return if can_write_in(&parent) {
                Finding::info(
                    ID,
                    format!(
                        "{} does not exist yet; the first backup creates it",
                        dir.display()
                    ),
                )
            } else {
                unusable(format!(
                    "{} does not exist and {} is not writable by this user",
                    dir.display(),
                    parent.display()
                ))
            };
        }
        Err(e) => {
            return unusable(format!("cannot read {}: {}", dir.display(), e.kind()));
        }
    }
    if !can_write_in(dir) {
        return unusable(format!(
            "{} is not writable by this user (under systemd the unit needs ReadWritePaths=)",
            dir.display()
        ));
    }
    match Repo::survey(&RepoLocation::Dir(dir.to_path_buf()), exports).await {
        Err(e) => Finding::error(ID, format!("{e:#}")),
        Ok(found) => judged(&dir.display().to_string(), found, schedule, exports),
    }
}

/// The first existing ancestor of `path` (or `/`).
fn nearest_existing(path: &Path) -> PathBuf {
    path.ancestors()
        .skip(1)
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Judges what a survey found: `None` is an empty location, `Some` the names, oldest first.
fn judged(
    place: &str,
    found: Option<Vec<String>>,
    schedule: BackupSchedule,
    exports: bool,
) -> Finding {
    let what = if exports {
        "export archive"
    } else {
        "snapshot"
    };
    let names = match found {
        None => {
            return Finding::info(
                ID,
                format!("{place} is empty; the first backup initialises it"),
            );
        }
        Some(names) => names,
    };
    let Some(newest) = names.last() else {
        return Finding::info(ID, format!("{place} is a repository with no {what} yet"));
    };
    let Some(made) = name_time(newest) else {
        return Finding::warn(ID, format!("{place}: {newest} is not a {what} name"));
    };
    let age = (Utc::now().naive_utc() - made).to_std().unwrap_or_default();
    let count = names.len();
    let detail = format!("{place}: newest {what} {} ago ({count} in all)", human(age));
    match schedule.every_secs() {
        Some(every) if age.as_secs() > every.saturating_mul(2) => Finding::warn(
            ID,
            format!(
                "{detail}; the {} schedule expects one at least every {}, so backups may be failing (check the log)",
                schedule.as_str(),
                human(Duration::from_secs(every))
            ),
        ),
        _ => Finding::ok(ID, detail),
    }
}

/// The time in a snapshot or export name (`<yyyymmddThhmmssZ>[.ferrite.tar.zst]`).
fn name_time(name: &str) -> Option<NaiveDateTime> {
    let stamp = name.get(..16)?;
    NaiveDateTime::parse_from_str(stamp, SNAPSHOT_FORMAT).ok()
}

/// `45 s`, `12 min`, `3 h`, `2 d 4 h`.
fn human(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=89 => format!("{s} s"),
        90..=5399 => format!("{} min", s / 60),
        5400..=172_799 => format!("{} h", s / 3600),
        _ => format!("{} d {} h", s / 86_400, (s % 86_400) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human(Duration::from_secs(5)), "5 s");
        assert_eq!(human(Duration::from_secs(600)), "10 min");
        assert_eq!(human(Duration::from_secs(3 * 3600)), "3 h");
        assert_eq!(
            human(Duration::from_secs(2 * 86_400 + 4 * 3600 + 5)),
            "2 d 4 h"
        );
    }

    #[test]
    fn names_carry_their_time() {
        assert!(name_time("20261029T101500Z").is_some());
        assert!(name_time("20261029T101500Z.ferrite.tar.zst").is_some());
        assert!(name_time("latest").is_none());
    }
}
