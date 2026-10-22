#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("invalid media configuration: {0}")]
    Config(String),
    #[error("storage backend error: {0}")]
    Backend(String),
    #[error("object not found")]
    NotFound,
    #[error("invalid range: {0}")]
    Range(String),
    #[error("image too large: {pixels} pixels")]
    ImageTooLarge { pixels: u64 },
    #[error("unsupported image: {0}")]
    UnsupportedImage(String),
    #[error("payload exceeds the {limit} byte limit")]
    TooLarge { limit: u64 },
    #[error("io error: {0}")]
    Io(String),
}

impl From<object_store::Error> for MediaError {
    fn from(err: object_store::Error) -> Self {
        match err {
            object_store::Error::NotFound { .. } => Self::NotFound,
            other => Self::Backend(other.to_string()),
        }
    }
}
