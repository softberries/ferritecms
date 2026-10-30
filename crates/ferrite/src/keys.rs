//! The site key (P4, R1): `auth.secret_key[_file]` when configured, otherwise (solo mode
//! only) `<data>/secret.key`, created on first use. The key is never logged or printed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use ferrite_config::{ClusterMode, Config};
use ferrite_core::auth::SiteKey;

/// File name of the solo site key in the data folder.
pub const SECRET_KEY_FILE: &str = "secret.key";

/// The site key for `config`. Configured → parsed; solo without one → read from, or
/// created as, `<data>/secret.key` (64 lowercase hex characters and a newline, mode 0600).
pub fn site_key(config: &Config) -> anyhow::Result<SiteKey> {
    if let Some(key) = &config.auth.secret_key {
        // `load` already checked the format; the error never contains the value.
        return SiteKey::from_hex(key.expose()).context("auth.secret_key");
    }
    if config.effective_cluster_mode() == ClusterMode::Cluster {
        // Config validation refuses this; kept so a hand-built `Config` cannot fall back
        // to a per-node key file.
        bail!("auth.secret_key: required in cluster mode (every node must use the same key)");
    }
    solo_key(&config.data_dir.join(SECRET_KEY_FILE))
}

fn solo_key(path: &Path) -> anyhow::Result<SiteKey> {
    match read_key(path)? {
        Some(key) => Ok(key),
        None => create_key(path),
    }
}

/// `None` when the file does not exist. On Unix, group or other permission bits on the
/// file are warned about (path and mode only) and the key is still used.
fn read_key(path: &Path) -> anyhow::Result<Option<SiteKey>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file
            .metadata()
            .with_context(|| format!("cannot read {}", path.display()))?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            tracing::warn!(
                path = %path.display(),
                mode = %format!("{mode:04o}"),
                "site key file is readable by group or others; chmod 600 it"
            );
        }
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("cannot read {}", path.display()))?;
    parse_key_text(path, &text).map(Some)
}

/// The key in a `secret.key` file's text (the value never appears in an error).
fn parse_key_text(path: &Path, text: &str) -> anyhow::Result<SiteKey> {
    let hex = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    match SiteKey::from_hex(hex) {
        Ok(key) => Ok(key),
        Err(_) => bail!(
            "{} is malformed: expected 64 hex characters (32 bytes)",
            path.display()
        ),
    }
}

/// `ferrite doctor`: whether `path` holds a well-formed key, `Ok(false)` when there is no
/// such file. Reads only: it never creates the key and does not log the mode warning.
pub(crate) fn validate_key_file(path: &Path) -> anyhow::Result<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        // Not UTF-8: not a key either (the content is not echoed).
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => bail!(
            "{} is malformed: expected 64 hex characters (32 bytes)",
            path.display()
        ),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    parse_key_text(path, &text).map(|_| true)
}

/// Creates the file atomically: the key is written to a uniquely named temp file (0600)
/// in the same folder and fsynced, then hard-linked to `path`, which fails rather than
/// overwrite. The temp file is removed either way, so a crash can leave only an unused
/// temp file, never a partial `secret.key`. If another process linked its key first,
/// that key is used instead.
fn create_key(path: &Path) -> anyhow::Result<SiteKey> {
    let key = SiteKey::generate().context("cannot generate a site key")?;
    let tmp = temp_path(path)?;
    let written = write_temp(&tmp, &key);
    let linked = written.and_then(|()| std::fs::hard_link(&tmp, path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_key(path)?
                .with_context(|| format!("{} disappeared while reading it", path.display()));
        }
        Err(e) => return Err(e).with_context(|| format!("cannot create {}", path.display())),
    }
    // Best effort: make the new directory entry durable.
    if let Some(dir) = path.parent()
        && let Ok(dir) = std::fs::File::open(dir)
    {
        let _ = dir.sync_all();
    }
    tracing::info!(path = %path.display(), "created a new site key");
    Ok(key)
}

/// `<dir>/.secret.key.<pid>.<16 random hex>.tmp`: unique per attempt, so a temp file a
/// crashed run left behind never collides with a new one.
fn temp_path(path: &Path) -> anyhow::Result<PathBuf> {
    let suffix: [u8; 8] =
        ferrite_core::auth::random_bytes().context("cannot generate a temp file name")?;
    let suffix = ferrite_domain::to_hex(&suffix);
    let name = path
        .file_name()
        .map_or_else(|| SECRET_KEY_FILE.into(), |n| n.to_string_lossy());
    Ok(path.with_file_name(format!(".{name}.{}.{suffix}.tmp", std::process::id())))
}

fn write_temp(tmp: &Path, key: &SiteKey) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(tmp)?;
    file.write_all(format!("{}\n", key.to_hex()).as_bytes())?;
    file.sync_all()
}
