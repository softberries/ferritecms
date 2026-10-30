//! Errors of the front matter reader and the folder scan.

use std::path::PathBuf;

use thiserror::Error;

/// Why a file's front matter or the whole folder could not be read.
#[derive(Debug, Error)]
pub enum MdError {
    /// The front matter of `file` is not valid (P62): a `---` YAML block, an unterminated
    /// `+++` block, invalid TOML, an unknown key or a value of the wrong type. `line` is
    /// 1-based in the file.
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
}
