//! Errors of the WXR reader and the import run.

use std::path::PathBuf;

use ferrite_core::CoreError;
use ferrite_media::MediaError;
use thiserror::Error;

/// Why a WXR file could not be read or imported. Offsets are bytes from the start of the
/// file.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("reading the file failed at byte {offset}: {msg}")]
    Io { offset: u64, msg: String },
    #[error("invalid XML at byte {offset}: {msg}")]
    Xml { offset: u64, msg: String },
    #[error("the file has a DOCTYPE, which WXR never has; refused")]
    Doctype,
    #[error("a node at byte {offset} is larger than 64 MiB")]
    TooLarge { offset: u64 },
    #[error("invalid UTF-8 at byte {offset}")]
    Utf8 { offset: u64 },
    #[error("missing {what} at byte {offset}")]
    Missing { what: &'static str, offset: u64 },
    /// A file or folder of the run (the WXR file, the work folder, an upload) failed.
    #[error("{}: {source}", path.display())]
    File {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The IR work file failed.
    #[error("the import's work file failed: {0}")]
    Ir(#[from] sqlx::Error),
    /// A record of the IR work file does not read back.
    #[error("a record of the import's work file is unreadable: {0}")]
    Record(#[from] serde_json::Error),
    /// Writing to the site failed; a run that was taken over is `Core(Changed)`, a busy
    /// one `Core(Unavailable)`.
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Media(#[from] MediaError),
    /// The run stopped before its end (the parse when the IR writer failed, or a test
    /// seam).
    #[error("the import was stopped")]
    Stopped,
}
