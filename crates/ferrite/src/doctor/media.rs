//! The media checks of `ferrite doctor`: `media.store` (the write, read and delete probe
//! `serve` runs at start) and `media.owner` (the GC's owner marker against this database's
//! install id). The probe object under `tmp/` is the one write doctor makes (P49); a local
//! root that does not exist is reported, never created. S3 is a network check: skipped with
//! `--offline` (P51). Keys are never printed.

use std::time::Duration;

use ferrite_config::{Config, MediaSection};
use ferrite_media::MediaStore;
use ferrite_store::SCHEMA_META_INSTALL_ID;

use super::Finding;
use super::db::{Database, unavailable};
use super::fs::NO_CONFIG;

/// How long the probe, or reading the owner marker, may take (P51).
const MEDIA_TIMEOUT: Duration = Duration::from_secs(5);
/// The detail of `media.owner` when the store was not opened.
const NO_STORE: &str = "the media store was not checked (see media.store)";

/// `media.store`: a local root must exist; then the probe runs (S3: not with `--offline`).
/// Returns the store for `media.owner` when the probe passed.
pub(super) async fn store(config: Option<&Config>, offline: bool) -> (Finding, Option<MediaStore>) {
    const ID: &str = "media.store";
    let Some(config) = config else {
        return (Finding::skip(ID, NO_CONFIG), None);
    };
    // The configured S3 keys, scrubbed from any error text (a server's error body can
    // name the access key id).
    let (place, keys): (String, Vec<&str>) = match &config.media {
        MediaSection::Local { root, .. } => {
            match std::fs::metadata(root) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    let detail = format!("{} is not a folder", root.display());
                    return (Finding::error(ID, detail), None);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let detail = format!(
                        "the local media root {} does not exist (a missing mount? `serve` \
                         would start with an empty one)",
                        root.display()
                    );
                    return (Finding::error(ID, detail), None);
                }
                Err(e) => {
                    let detail = format!("cannot read {}: {}", root.display(), e.kind());
                    return (Finding::error(ID, detail), None);
                }
            }
            (format!("local {}", root.display()), Vec::new())
        }
        MediaSection::S3 {
            bucket,
            region,
            endpoint,
            access_key_id,
            secret_access_key,
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
            let keys = [
                access_key_id.as_deref(),
                secret_access_key.as_ref().map(|s| s.expose()),
            ];
            (place, keys.into_iter().flatten().collect())
        }
    };
    let clean = |e: String| {
        keys.iter()
            .filter(|k| !k.is_empty())
            .fold(e, |e, k| e.replace(k, "***"))
    };
    let media = match crate::site::open_media(config) {
        Ok(media) => media,
        Err(e) => return (Finding::error(ID, clean(format!("{place}: {e:#}"))), None),
    };
    match tokio::time::timeout(MEDIA_TIMEOUT, media.check()).await {
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
                    MEDIA_TIMEOUT.as_secs()
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
        Err(e) => return Finding::warn(ID, format!("cannot read install_id: {e:#}")),
    };
    let marker = match tokio::time::timeout(MEDIA_TIMEOUT, media.read_owner()).await {
        Ok(Ok(marker)) => marker,
        Ok(Err(e)) => {
            return Finding::warn(
                ID,
                format!(
                    "the owner marker cannot be read: {e} (the GC deletes nothing until it can)"
                ),
            );
        }
        Err(_) => {
            return Finding::warn(
                ID,
                format!(
                    "reading the owner marker did not finish within {} s",
                    MEDIA_TIMEOUT.as_secs()
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
