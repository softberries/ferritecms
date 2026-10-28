//! Errors of the WXR reader.

use thiserror::Error;

/// Why a WXR file could not be read. Offsets are bytes from the start of the file.
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
}
