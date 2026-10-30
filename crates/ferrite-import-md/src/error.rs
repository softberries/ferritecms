//! Errors of the front matter reader, the folder scan and the import run.

use std::path::PathBuf;

use ferrite_core::CoreError;
use ferrite_media::MediaError;
use thiserror::Error;

/// Why a file's front matter, the whole folder or the import run failed.
#[derive(Debug, Error)]
pub enum MdError {
    /// The front matter of `file` is not valid (P62): a `---` YAML block, an unterminated
    /// `+++` block, invalid TOML, an unknown key, an empty `title` or `slug`, or a value
    /// of the wrong type. `line` is 1-based in the file.
    #[error("{}:{line}: {message}", file.display())]
    FrontMatter {
        file: PathBuf,
        line: usize,
        message: String,
    },
    /// The folder to import is not a folder.
    #[error("{} is not a folder", path.display())]
    NotADirectory { path: PathBuf },
    /// The folder holds more Markdown files or folders than an import takes (P62).
    #[error("the folder holds more than {limit} Markdown files or folders")]
    TooManyFiles { limit: usize },
    /// Reading the folder or one of its entries failed.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A write or read of the import run failed (another run took it over: `Changed`).
    #[error(transparent)]
    Core(#[from] CoreError),
    /// The media store failed (not a refused file: that is a reported problem).
    #[error(transparent)]
    Media(#[from] MediaError),
    /// A worker thread of the run stopped.
    #[error("the import stopped: {0}")]
    Stopped(String),
}
