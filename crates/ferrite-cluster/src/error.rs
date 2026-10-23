use ferrite_store::StoreError;

/// Errors from setting up cluster components.
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Two job handlers registered the same kind.
    #[error("two job handlers for kind {0:?}")]
    DuplicateHandler(String),
}
