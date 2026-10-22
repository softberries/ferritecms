use ferrite_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("not found")]
    NotFound,
    /// Carries the name of the missing capability.
    #[error("forbidden: missing capability {0}")]
    Forbidden(&'static str),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("version conflict (current version {current_version})")]
    Conflict { current_version: i64 },
    #[error("store error: {0}")]
    Store(#[source] StoreError),
}

impl From<StoreError> for CoreError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Conflict { current_version } => Self::Conflict { current_version },
            StoreError::NotFound => Self::NotFound,
            StoreError::Invalid(msg) => Self::Invalid(msg),
            other => Self::Store(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_errors_map() {
        assert!(matches!(
            CoreError::from(StoreError::Conflict { current_version: 3 }),
            CoreError::Conflict { current_version: 3 }
        ));
        assert!(matches!(
            CoreError::from(StoreError::NotFound),
            CoreError::NotFound
        ));
        assert!(matches!(
            CoreError::from(StoreError::Invalid("x".into())),
            CoreError::Invalid(m) if m == "x"
        ));
        assert!(matches!(
            CoreError::from(StoreError::Aborted),
            CoreError::Store(_)
        ));
    }
}
