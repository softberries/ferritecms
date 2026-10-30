//! The media checks of `ferrite doctor`: `media.store` and `media.owner` (the GC's owner
//! marker against this database's install id).
//!
//! - **Local** (X4): no probe object is written. The root (and `tmp/`, if it exists) must
//!   be folders this process may write in, asked of the kernel with the `fs.*` checks'
//!   `access(2)` helpers; a missing root is reported, never created, and `tmp/` is never
//!   created either (so doctor run as root leaves no root-owned folder that would stop
//!   `serve`'s start probe).
//! - **S3**: the write, read and delete probe `serve` runs at start, under `tmp/` (P49's
//!   one write, nothing local). A network check: skipped with `--offline` (P51). Keys, the
//!   configured ones and `AWS_*` from the environment, are never printed.

use std::path::Path;

use ferrite_config::{Config, MediaSection};
use ferrite_media::MediaStore;
use ferrite_store::SCHEMA_META_INSTALL_ID;

use super::db::{Database, unavailable};
use super::fs::{NO_CONFIG, can_write_in};
use super::{Finding, NETWORK_TIMEOUT, scrub_s3_keys};
/// The detail of `media.owner` when the store was not opened.
const NO_STORE: &str = "the media store was not checked (see media.store)";

const ID: &str = "media.store";

/// `media.store`. Returns the store for `media.owner` when it passed.
pub(super) async fn store(config: Option<&Config>, offline: bool) -> (Finding, Option<MediaStore>) {
    let Some(config) = config else {
        return (Finding::skip(ID, NO_CONFIG), None);
    };
    match &config.media {
        MediaSection::Local { root, .. } => local(config, root),
        MediaSection::S3 { .. } => s3(config, offline).await,
    }
}

/// A local root: it and `tmp/` (if present) are folders this process may write in; nothing
/// is written or created (X4).
fn local(config: &Config, root: &Path) -> (Finding, Option<MediaStore>) {
    let tmp = root.join("tmp");
    for (dir, missing_is_fine) in [(root, false), (tmp.as_path(), true)] {
        let problem = match std::fs::metadata(dir) {
            Ok(meta) if meta.is_dir() && can_write_in(dir) => None,
            Ok(meta) if meta.is_dir() => Some(format!(
                "{} is not writable by this user (run doctor as the user that runs `serve`)",
                dir.display()
            )),
            Ok(_) => Some(format!("{} is not a folder", dir.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && missing_is_fine => None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(format!(
                "the local media root {} does not exist (a missing mount? `serve` would \
                 start with an empty one)",
                dir.display()
            )),
            Err(e) => Some(format!("cannot read {}: {}", dir.display(), e.kind())),
        };
        if let Some(problem) = problem {
            return (Finding::error(ID, problem), None);
        }
    }
    // The root exists, so opening it creates nothing.
    match crate::site::open_media(config) {
        Ok(media) => (
            Finding::ok(
                ID,
                format!(
                    "local {}: a writable folder (checked with access(2); no probe written)",
                    root.display()
                ),
            ),
            Some(media),
        ),
        Err(e) => (
            Finding::error(ID, format!("{}: {e:#}", root.display())),
            None,
        ),
    }
}

/// An S3 store: the write, read and delete probe (not with `--offline`).
async fn s3(config: &Config, offline: bool) -> (Finding, Option<MediaStore>) {
    let place: String = match &config.media {
        MediaSection::Local { root, .. } => return local(config, root),
        MediaSection::S3 {
            bucket,
            region,
            endpoint,
            access_key_id,
            prefix,
            ..
        } => {
            let at = match endpoint.as_deref() {
                Some(e) => format!("at {}", without_userinfo(e)),
                None => format!("in AWS region {region}"),
            };
            let prefix = if prefix.is_empty() {
                String::new()
            } else {
                format!(", prefix {prefix}")
            };
            let keys = if access_key_id.is_some() {
                ""
            } else {
                ", credentials from this shell's AWS_* environment"
            };
            let place = format!("s3 bucket {bucket} {at}{prefix}{keys}");
            if offline {
                return (
                    Finding::skip(ID, format!("{place}: not probed (--offline)")),
                    None,
                );
            }
            place
        }
    };
    // A server's error body can name the access key id.
    let clean = |e: String| scrub_s3_keys(e, Some(config));
    let media = match crate::site::open_media(config) {
        Ok(media) => media,
        Err(e) => return (Finding::error(ID, clean(format!("{place}: {e:#}"))), None),
    };
    match tokio::time::timeout(NETWORK_TIMEOUT, media.check()).await {
        Ok(Ok(())) => (
            Finding::ok(ID, format!("{place}: write, read and delete probe passed")),
            Some(media),
        ),
        Ok(Err(e)) => (
            Finding::error(ID, clean(format!("{place}: the probe failed: {e}"))),
            None,
        ),
        Err(_) => (
            Finding::error(
                ID,
                format!(
                    "{place}: the probe did not finish within {} s",
                    NETWORK_TIMEOUT.as_secs()
                ),
            ),
            None,
        ),
    }
}

/// An endpoint URL without any `user:password@`.
fn without_userinfo(endpoint: &str) -> String {
    match url::Url::parse(endpoint) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.as_str().trim_end_matches('/').to_owned()
        }
        Err(_) => endpoint.to_owned(),
    }
}

/// `media.owner`: the owner marker against `schema_meta.install_id`, as the GC judges it
/// (M4-1 R16): absent is `info` (the first GC pass writes it), another install's is a
/// warning (the GC deletes nothing), one that cannot be read a warning.
pub(super) async fn owner(
    media: Option<&MediaStore>,
    db: Option<&Database>,
    config: Option<&Config>,
) -> Finding {
    const ID: &str = "media.owner";
    let Some(media) = media else {
        let why = if config.is_some() {
            NO_STORE
        } else {
            NO_CONFIG
        };
        return Finding::skip(ID, why);
    };
    let Some(db) = db else {
        return Finding::skip(ID, unavailable(config));
    };
    let install = match db.meta(SCHEMA_META_INSTALL_ID).await {
        Ok(install) => install.filter(|s| !s.is_empty()),
        Err(e) => return Finding::warn(ID, format!("cannot read install_id: {e}")),
    };
    let marker = match tokio::time::timeout(NETWORK_TIMEOUT, media.read_owner()).await {
        Ok(Ok(marker)) => marker,
        Ok(Err(e)) => {
            return Finding::warn(
                ID,
                scrub_s3_keys(
                    format!(
                        "the owner marker cannot be read: {e} (the GC deletes nothing until \
                         it can)"
                    ),
                    config,
                ),
            );
        }
        Err(_) => {
            return Finding::warn(
                ID,
                format!(
                    "reading the owner marker did not finish within {} s",
                    NETWORK_TIMEOUT.as_secs()
                ),
            );
        }
    };
    match (marker, install) {
        (None, _) => Finding::info(
            ID,
            format!(
                "no {} yet: the first media GC pass writes one for this install",
                ferrite_media::OWNER_MARKER
            ),
        ),
        (Some(marker), Some(install)) if marker.install_id == install => {
            Finding::ok(ID, format!("owned by this install ({install})"))
        }
        (Some(marker), Some(_)) => Finding::warn(
            ID,
            format!(
                "the GC deletes nothing: {}",
                crate::gc::foreign(&crate::gc::describe(&marker))
            ),
        ),
        (Some(marker), None) => Finding::warn(
            ID,
            format!(
                "{} owns the media store and this database has no install_id (see \
                 db.identity): the GC deletes nothing",
                crate::gc::describe(&marker)
            ),
        ),
    }
}
