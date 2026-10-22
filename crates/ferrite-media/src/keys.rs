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
