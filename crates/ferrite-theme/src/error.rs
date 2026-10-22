use std::path::PathBuf;

/// Everything that can go wrong loading or rendering a theme.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// No embedded theme with that name.
    #[error("theme {0:?} not found")]
    NotFound(String),
    /// Reading a theme directory failed.
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The package is malformed: bad `theme.toml`, missing `templates/index.html`, a
    /// symlink, a bad path, too many or too large files. Every problem found is listed.
    #[error("invalid theme: {}", .0.join("; "))]
    Invalid(Vec<String>),
    /// Templates that failed to compile, each as `templates/<file>:<line>: <message>`.
    #[error("template errors: {}", .0.join("; "))]
    Templates(Vec<String>),
    /// The requested template is not part of the theme.
    #[error("template {0:?} not found in theme")]
    TemplateNotFound(String),
    /// A render hit a limit (fuel, output size, recursion depth, range size). The
    /// next render starts fresh.
    #[error("render limit: {0}")]
    Limit(String),
    /// A runtime template error, as `templates/<file>:<line>: <message>`.
    #[error("render error: {0}")]
    Render(String),
}
