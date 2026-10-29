use ferrite_domain::ArtifactHash;
use object_store::path::Path;
use uuid::Uuid;

use crate::MediaError;

/// A prefix is empty, or ends with `/` and does not start with `/`.
pub(crate) fn validate_prefix(prefix: &str) -> Result<(), MediaError> {
    if prefix.is_empty() {
        return Ok(());
    }
    if prefix.starts_with('/') {
        return Err(MediaError::Config(format!(
            "media prefix {prefix:?} must not start with '/'"
        )));
    }
    if !prefix.ends_with('/') {
        return Err(MediaError::Config(format!(
            "media prefix {prefix:?} must be empty or end with '/'"
        )));
    }
    // Every segment is a plain name: no empty segments (`//`), no `.`/`..`, no exotic bytes.
    for segment in prefix[..prefix.len() - 1].split('/') {
        let plain = !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !plain {
            return Err(MediaError::Config(format!(
                "media prefix {prefix:?}: segment {segment:?} must match [A-Za-z0-9._-]+ and not be '.' or '..'"
            )));
        }
    }
    Ok(())
}

pub(crate) fn probe_key(prefix: &str) -> Path {
    Path::from(format!("{prefix}tmp/probe-{}", Uuid::now_v7()))
}

/// The owner marker (M4-1 R16): `<prefix>ferrite-owner.json`, beside `artifacts/` and `tmp/`.
pub(crate) fn owner_key(prefix: &str) -> Path {
    Path::from(format!("{prefix}{}", crate::OWNER_MARKER))
}

/// Staging key for streaming uploads: `<prefix>tmp/<uuid v7>`.
pub(crate) fn tmp_key(prefix: &str) -> Path {
    Path::from(format!("{prefix}tmp/{}", Uuid::now_v7()))
}

/// `<prefix>artifacts/<first two hex chars>/<64-hex sha256>`
pub(crate) fn artifact_key(prefix: &str, hash: &ArtifactHash) -> Path {
    let h = hash.as_str();
    Path::from(format!("{prefix}artifacts/{}/{h}", &h[..2]))
}

/// Same layout as [`artifact_key`], relative to the public base (no escaping needed: hex only).
pub(crate) fn artifact_rel(prefix: &str, hash: &ArtifactHash) -> String {
    let h = hash.as_str();
    format!("{prefix}artifacts/{}/{h}", &h[..2])
}
