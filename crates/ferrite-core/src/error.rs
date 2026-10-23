use ferrite_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("not found")]
    NotFound,
    /// Carries the name of the missing capability (`manage_users`), token scope
    /// (`content:write`) or proof (`current password`).
    #[error("forbidden: missing {0}")]
    Forbidden(&'static str),
    /// No valid credentials: unknown login, wrong password, expired session or token.
    #[error("not authenticated")]
    Unauthenticated,
    /// A rate limit was hit; retry after this many seconds.
    #[error("rate limited (retry after {retry_after_secs} s)")]
    RateLimited { retry_after_secs: u64 },
    #[error("invalid input: {0}")]
    Invalid(String),
    /// Invalid input attributable to one input member: `field` is its name as the API
    /// spells it (`slug`, `terms`, `parent_id`, …), so a form can show the message next
    /// to it. Otherwise like [`Invalid`](Self::Invalid).
    #[error("invalid {field}: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
    #[error("version conflict (current version {current_version})")]
    Conflict { current_version: i64 },
    /// The operating system's random number generator failed (keys, nonces, tokens).
    #[error("system random number generator failed")]
    Random,
    /// Password hashing failed (the blocking worker panicked or argon2 refused its input).
    #[error("password hashing failed")]
    Hashing,
    #[error(transparent)]
    Store(StoreError),
}

impl CoreError {
    /// `InvalidField { field, message }`.
    pub fn invalid_field(field: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidField {
            field,
            message: message.into(),
        }
    }
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
