use ferrite_domain::DomainError;

/// Errors returned by every storage backend.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    #[error("version conflict (current version {current_version})")]
    Conflict { current_version: i64 },
    #[error("unique constraint violated: {constraint}")]
    UniqueViolation { constraint: String },
    #[error("foreign key violated: {constraint}")]
    ForeignKey { constraint: String },
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("unsupported on this backend: {0}")]
    Unsupported(&'static str),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("incompatible schema: {0}")]
    Incompatible(String),
    /// An earlier database error inside this transaction (or nested transaction) aborted
    /// it: every later call fails with this, and `commit` rolls back and returns it.
    #[error("transaction aborted by an earlier error; roll it back")]
    Aborted,
    /// No connection became free in time (on SQLite: the single writer is held, e.g. by
    /// an open `Tx` in the same task).
    #[error("database busy: timed out waiting for a connection")]
    Busy,
    #[error("database error: {0}")]
    Database(String),
}

impl From<DomainError> for StoreError {
    fn from(e: DomainError) -> Self {
        StoreError::Invalid(e.to_string())
    }
}
