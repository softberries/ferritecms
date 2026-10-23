//! The site key (P4, R1): `auth.secret_key[_file]` when configured, otherwise (solo mode
//! only) `<data>/secret.key`, created on first use. The key is never logged or printed.

use std::io::Write;
use std::path::Path;

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

/// `None` when the file does not exist.
fn read_key(path: &Path) -> anyhow::Result<Option<SiteKey>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let hex = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(&text);
    match SiteKey::from_hex(hex) {
        Ok(key) => Ok(Some(key)),
        Err(_) => bail!(
            "{} is malformed: expected 64 hex characters (32 bytes)",
            path.display()
        ),
    }
}

/// Creates the file exclusively (`create_new`, 0600); if another process created it
/// first, its key is used instead.
fn create_key(path: &Path) -> anyhow::Result<SiteKey> {
    let key = SiteKey::generate().context("cannot generate a site key")?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = match opts.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_key(path)?
                .with_context(|| format!("{} disappeared while reading it", path.display()));
        }
        Err(e) => return Err(e).with_context(|| format!("cannot create {}", path.display())),
    };
    let written = file
        .write_all(format!("{}\n", key.to_hex()).as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(e) = written {
        // Do not leave a partial key behind for the next start to trip over.
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e).with_context(|| format!("cannot write {}", path.display()));
    }
    tracing::info!(path = %path.display(), "created a new site key");
    Ok(key)
}
