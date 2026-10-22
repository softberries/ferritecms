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
    Ok(())
}

pub(crate) fn probe_key(prefix: &str) -> Path {
    Path::from(format!("{prefix}tmp/probe-{}", Uuid::now_v7()))
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
