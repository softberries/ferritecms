//! The file checks of `ferrite doctor`: `fs.*`, `db.lock_state`, `upgrade.record` and
//! `tls.dir`. They read metadata and ask the kernel whether this process may read or write
//! (`access(2)`); they create, change and delete nothing (P49).

use std::fs::{File, Metadata, TryLockError};
use std::path::{Path, PathBuf};

use ferrite_config::{ClusterMode, Config, DbKind};

use super::Finding;
use crate::keys::{SECRET_KEY_FILE, validate_key_file};

/// The detail of a check that needs the configuration when it did not load.
const NO_CONFIG: &str = "the configuration did not load (see config.load)";
/// SQLite's lock file in the data folder (held by a running node).
const LOCK_FILE: &str = "ferrite.lock";

// ---- what this process may do, and who owns what ----

/// Whether this process may do `access` to `path`, by the kernel's own check with the
/// effective uid and gid (as `open` would): ACLs, groups and read-only mounts count, and
/// nothing is written. Some container seccomp profiles answer `faccessat2` (which
/// `EACCESS` needs) with `EPERM`; then the plain real-id check is asked instead, which is
/// the same answer for a process that is not setuid.
#[cfg(unix)]
fn may(path: &Path, access: rustix::fs::Access) -> bool {
    use rustix::fs::{AtFlags, CWD};
    match rustix::fs::accessat(CWD, path, access, AtFlags::EACCESS) {
        Ok(()) => true,
        Err(rustix::io::Errno::PERM) => rustix::fs::access(path, access).is_ok(),
        Err(_) => false,
    }
}

/// Whether this process may list `dir` (read and enter).
#[cfg(unix)]
fn can_list(dir: &Path) -> bool {
    use rustix::fs::Access;
    may(dir, Access::READ_OK | Access::EXEC_OK)
}

/// Whether this process may create and delete files in `dir`.
#[cfg(unix)]
pub(super) fn can_write_in(dir: &Path) -> bool {
    use rustix::fs::Access;
    may(dir, Access::WRITE_OK | Access::EXEC_OK)
}

/// Whether this process may read and write the file `path`.
#[cfg(unix)]
fn can_read_write(path: &Path) -> bool {
    use rustix::fs::Access;
    may(path, Access::READ_OK | Access::WRITE_OK)
}

/// Without Unix permissions the best answer is the read-only attribute.
#[cfg(not(unix))]
fn can_list(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok()
}

#[cfg(not(unix))]
pub(super) fn can_write_in(dir: &Path) -> bool {
    std::fs::metadata(dir).is_ok_and(|m| !m.permissions().readonly())
}

#[cfg(not(unix))]
fn can_read_write(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.permissions().readonly())
}

/// The permission bits (`0o777` mask) of `meta`; `None` where there are none.
#[cfg(unix)]
fn mode(meta: &Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode(_: &Metadata) -> Option<u32> {
    None
}

/// The owner's uid; `None` where there is none.
#[cfg(unix)]
fn owner(meta: &Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.uid())
}

#[cfg(not(unix))]
fn owner(_: &Metadata) -> Option<u32> {
    None
}

/// `0644`.
fn show_mode(mode: u32) -> String {
    format!("{mode:04o}")
}

/// Whether any of `bits` is set in the mode; `false` where modes do not exist.
fn has_bits(meta: &Metadata, bits: u32) -> Option<u32> {
    mode(meta).filter(|m| m & bits != 0)
}

// ---- fs.* ----

/// `fs.data_dir`: exists, is a folder, is readable and writable by this user; a mode with
/// world bits is a warning (`init` creates a new folder 0750).
pub(super) fn data_dir(data: &Path) -> Finding {
    const ID: &str = "fs.data_dir";
    let meta = match std::fs::metadata(data) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Finding::error(ID, format!("{} does not exist", data.display()));
        }
        Err(e) => {
            return Finding::error(ID, format!("cannot read {}: {}", data.display(), e.kind()));
        }
    };
    if !meta.is_dir() {
        return Finding::error(ID, format!("{} is not a folder", data.display()));
    }
    if !can_list(data) {
        return Finding::error(
            ID,
            format!("{} is not readable by this user", data.display()),
        );
    }
    if !can_write_in(data) {
        return Finding::error(
            ID,
            format!("{} is not writable by this user", data.display()),
        );
    }
    if let Some(m) = has_bits(&meta, 0o007) {
        return Finding::warn(
            ID,
            format!(
                "{} has mode {}: everyone can access it; chmod o-rwx (0750 is recommended)",
                data.display(),
                show_mode(m)
            ),
        );
    }
    let owner = owner(&meta).map_or_else(String::new, |uid| format!(", owner uid {uid}"));
    let mode = mode(&meta).map_or_else(String::new, |m| format!("mode {}, ", show_mode(m)));
    Finding::ok(
        ID,
        format!("{} ({mode}readable and writable{owner})", data.display()),
    )
}

/// `fs.config_file`: `ferrite.toml` may hold the database URL, S3 secret and SMTP
/// credentials, so group and other bits are a warning. A missing file is `config.load`'s
/// error, not a second one here.
pub(super) fn config_file(data: &Path) -> Finding {
    const ID: &str = "fs.config_file";
    let path = data.join(crate::backup::CONFIG_FILE);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Finding::skip(ID, format!("{} not found", path.display()));
        }
        Err(e) => {
            return Finding::warn(ID, format!("cannot read {}: {}", path.display(), e.kind()));
        }
    };
    match has_bits(&meta, 0o077) {
        Some(m) => Finding::warn(
            ID,
            format!(
                "{} has mode {}: it can hold the database URL and credentials; chmod 600 it",
                path.display(),
                show_mode(m)
            ),
        ),
        None => Finding::ok(
            ID,
            match mode(&meta) {
                Some(m) => format!("{} (mode {})", path.display(), show_mode(m)),
                None => path.display().to_string(),
            },
        ),
    }
}

/// `fs.secret_key`: with `auth.secret_key` configured there is no key file to check; solo
/// without it, `secret.key` must exist and be well formed (error) and must have no group
/// or other bits (warning). Reads the file; never creates the key `serve` would create.
pub(super) fn secret_key(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "fs.secret_key";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    if config.auth.secret_key.is_some() {
        return Finding::ok(
            ID,
            "the key comes from the configuration (auth.secret_key[_file]); secret.key is not used",
        );
    }
    if config.effective_cluster_mode() == ClusterMode::Cluster {
        return Finding::error(
            ID,
            "cluster mode needs auth.secret_key (every node must use the same key)",
        );
    }
    let path = data.join(SECRET_KEY_FILE);
    match validate_key_file(&path) {
        Ok(false) => Finding::error(
            ID,
            format!(
                "{} does not exist: `serve` would create a new key, and data sealed with the \
                 old one (sessions, TOTP secrets, plugin secrets) could not be read",
                path.display()
            ),
        ),
        Err(e) => Finding::error(ID, format!("{e:#}")),
        Ok(true) => match std::fs::metadata(&path)
            .ok()
            .as_ref()
            .and_then(|m| has_bits(m, 0o077))
        {
            Some(m) => Finding::warn(
                ID,
                format!(
                    "{} has mode {}: readable by group or others; chmod 600 it",
                    path.display(),
                    show_mode(m)
                ),
            ),
            None => Finding::ok(ID, format!("{} is a valid key", path.display())),
        },
    }
}

/// `fs.database_files`: SQLite's database, `-wal`, `-shm` and lock files that exist must be
/// readable and writable by this user (error) and owned by the data folder's owner
/// (warning: a root-run command leaves root-owned files the service then cannot write).
pub(super) fn database_files(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "fs.database_files";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    let DbKind::Sqlite { path } = config.database_kind() else {
        return Finding::info(ID, "Postgres: the database is not in the data folder");
    };
    let dir_owner = std::fs::metadata(data).ok().as_ref().and_then(owner);
    let candidates = [
        path.clone(),
        suffixed(&path, "-wal"),
        suffixed(&path, "-shm"),
        data.join(LOCK_FILE),
    ];
    let mut present = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for file in &candidates {
        let name = file.file_name().map_or_else(
            || file.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let meta = match std::fs::metadata(file) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if *file == path {
                    errors.push(format!("{} does not exist", file.display()));
                }
                continue;
            }
            Err(e) => {
                errors.push(format!("cannot read {name}: {}", e.kind()));
                continue;
            }
        };
        if !can_read_write(file) {
            errors.push(format!("{name} is not readable and writable by this user"));
        }
        if let (Some(file_owner), Some(dir_owner)) = (owner(&meta), dir_owner)
            && file_owner != dir_owner
        {
            warnings.push(format!(
                "{name} is owned by uid {file_owner}, the data folder by uid {dir_owner} \
                 (a command run as another user left it; chown it to the service user)"
            ));
        }
        present.push(name);
    }
    if !errors.is_empty() {
        errors.extend(warnings);
        return Finding::error(ID, errors.join("; "));
    }
    if !warnings.is_empty() {
        return Finding::warn(ID, warnings.join("; "));
    }
    Finding::ok(
        ID,
        format!("{} are readable and writable", present.join(", ")),
    )
}

/// `<path>-wal` for `<path>`.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// `fs.backups_dir`: SQLite's pre-migration snapshots go to `<data>/backups/`; if it
/// exists it must be writable (warning), and its `pre-*.db` files are counted.
pub(super) fn backups_dir(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "fs.backups_dir";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    if !matches!(config.database_kind(), DbKind::Sqlite { .. }) {
        return Finding::info(ID, "Postgres: no pre-migration snapshots are taken");
    }
    let dir = data.join("backups");
    match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Finding::warn(ID, format!("{} is not a folder", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Finding::info(
                ID,
                format!("{} does not exist (no snapshot taken yet)", dir.display()),
            );
        }
        Err(e) => {
            return Finding::warn(ID, format!("cannot read {}: {}", dir.display(), e.kind()));
        }
    }
    if !can_write_in(&dir) {
        return Finding::warn(
            ID,
            format!(
                "{} is not writable by this user: a migration could not take its snapshot",
                dir.display()
            ),
        );
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            return Finding::warn(ID, format!("cannot list {}: {}", dir.display(), e.kind()));
        }
    };
    let count = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("pre-") && name.ends_with(".db")
        })
        .count();
    Finding::ok(
        ID,
        format!(
            "{} is writable, {count} pre-migration snapshot(s)",
            dir.display()
        ),
    )
}

/// `fs.themes_dir`: solo reads `<data>/themes/` at start, so it must be readable when it
/// exists; cluster mode ignores theme folders (themes are artifacts).
pub(super) fn themes_dir(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "fs.themes_dir";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    if config.effective_cluster_mode() == ClusterMode::Cluster {
        return Finding::info(
            ID,
            "cluster mode ignores theme folders (themes are installed as artifacts)",
        );
    }
    let dir = data.join("themes");
    match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Finding::warn(ID, format!("{} is not a folder", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Finding::info(ID, format!("no {} folder (optional)", dir.display()));
        }
        Err(e) => {
            return Finding::warn(ID, format!("cannot read {}: {}", dir.display(), e.kind()));
        }
    }
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            let count = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .count();
            Finding::ok(
                ID,
                format!("{} is readable, {count} theme folder(s)", dir.display()),
            )
        }
        Err(e) => Finding::warn(
            ID,
            format!("{} cannot be listed: {}", dir.display(), e.kind()),
        ),
    }
}

// ---- db.lock_state ----

/// Whether another process holds `<data>/ferrite.lock`.
enum Lock {
    /// Nobody holds it (or there is no lock file yet).
    Free,
    /// A running node holds it.
    Held,
    /// The file could not be opened or locked.
    Unknown(std::io::ErrorKind),
}

/// Tries the lock without waiting and releases it at once. Never creates the file.
fn lock(data: &Path) -> Lock {
    let file = match File::open(data.join(LOCK_FILE)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Lock::Free,
        Err(e) => return Lock::Unknown(e.kind()),
    };
    match file.try_lock() {
        Ok(()) => {
            // Dropping the file releases it; unlocking first only makes that explicit.
            let _ = file.unlock();
            Lock::Free
        }
        Err(TryLockError::WouldBlock) => Lock::Held,
        Err(TryLockError::Error(e)) => Lock::Unknown(e.kind()),
    }
}

/// `db.lock_state` (P50): whether a node is running on this data folder. Only SQLite has a
/// data-folder lock.
pub(super) fn lock_state(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "db.lock_state";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    if !matches!(config.database_kind(), DbKind::Sqlite { .. }) {
        return Finding::info(ID, "Postgres: there is no data-folder lock");
    }
    match lock(data) {
        Lock::Free => Finding::info(ID, "no node is running on this data folder"),
        Lock::Held => Finding::info(
            ID,
            format!("a node is running (it holds {LOCK_FILE} in the data folder)"),
        ),
        Lock::Unknown(kind) => {
            Finding::info(ID, format!("cannot tell whether a node is running: {kind}"))
        }
    }
}

// ---- upgrade.record ----

/// `upgrade.record` (file part): `upgrade.json` parses (warning if not), is not a provisional
/// record (an upgrade that did not finish) and its named snapshot exists. Only SQLite takes
/// a snapshot: a finished Postgres record has `snapshot: null` (09 §1), so a null snapshot
/// with a schema move is provisional on SQLite alone.
pub(super) fn upgrade_record(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "upgrade.record";
    let rec = match crate::upgrade::load(data) {
        Ok(Some(rec)) => rec,
        Ok(None) => return Finding::ok(ID, "no upgrade.json (nothing to roll back to)"),
        Err(e) => return Finding::warn(ID, format!("{e:#}")),
    };
    let from = rec.from.as_deref().unwrap_or("unknown");
    let moved = format!(
        "{from} -> {} (schema {} -> {}), {}",
        rec.to,
        rec.schema_from,
        rec.schema_to,
        rec.at.to_rfc3339()
    );
    let sqlite = config.is_some_and(|c| matches!(c.database_kind(), DbKind::Sqlite { .. }));
    match &rec.snapshot {
        None if sqlite && rec.schema_from < rec.schema_to => Finding::warn(
            ID,
            format!(
                "provisional record: the move {moved} did not finish; check `ferrite migrate status`"
            ),
        ),
        None => Finding::ok(ID, format!("{moved}; no snapshot")),
        Some(snapshot) => {
            // Relative to the data folder; absolute when the backups folder is elsewhere.
            if data.join(snapshot).exists() {
                Finding::ok(ID, format!("{moved}; snapshot {}", snapshot.display()))
            } else {
                Finding::warn(
                    ID,
                    format!(
                        "the snapshot {} that upgrade.json names is missing ({moved}); `ferrite rollback --restore-snapshot` cannot use it",
                        snapshot.display()
                    ),
                )
            }
        }
    }
}

// ---- tls.dir ----

/// `tls.dir` (ACME mode): `<data>/tls/` should be 0700 and its files 0600. `serve` tightens
/// them at start, so a violation is a warning.
pub(super) fn tls_dir(data: &Path, config: Option<&Config>) -> Finding {
    const ID: &str = "tls.dir";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    if config.server.tls.mode() != "acme" {
        return Finding::info(ID, "TLS is off (server.tls.mode)");
    }
    let dir = data.join("tls");
    let meta = match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => meta,
        Ok(_) => return Finding::warn(ID, format!("{} is not a folder", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Finding::info(
                ID,
                format!("{} does not exist yet; `serve` creates it", dir.display()),
            );
        }
        Err(e) => {
            return Finding::warn(ID, format!("cannot read {}: {}", dir.display(), e.kind()));
        }
    };
    let mut loose = Vec::new();
    if let Some(m) = has_bits(&meta, 0o077) {
        loose.push(format!("tls/ has mode {} (want 0700)", show_mode(m)));
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            return Finding::warn(ID, format!("cannot list {}: {}", dir.display(), e.kind()));
        }
    };
    let mut files = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        files += 1;
        if let Some(m) = has_bits(&meta, 0o077) {
            loose.push(format!(
                "{} has mode {} (want 0600)",
                entry.file_name().to_string_lossy(),
                show_mode(m)
            ));
        }
    }
    if loose.is_empty() {
        Finding::ok(
            ID,
            format!("{} (0700), {files} file(s) (0600)", dir.display()),
        )
    } else {
        Finding::warn(
            ID,
            format!("{}; `serve` tightens them at start", loose.join("; ")),
        )
    }
}
