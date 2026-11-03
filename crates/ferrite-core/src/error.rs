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
    /// Invalid input attributable to one member whose name is data, not a fixed field: a
    /// theme or plugin setting key (M2a-2 P13), or the path of a field value such as
    /// `fields.chapters[1].related` (a relation's target type, M2b-3 P52). Mapped like
    /// [`InvalidField`](Self::InvalidField), with `key` as the field. The message never
    /// holds the value (it may be a secret).
    #[error("invalid {key}: {message}")]
    InvalidKey { key: String, message: String },
    /// Several rows of a batch (a redirect import) are invalid; nothing was written.
    /// Each entry names the row's index in the input and what is wrong with it.
    #[error("invalid input in {} rows", .0.len())]
    InvalidRows(Vec<RowError>),
    #[error("version conflict (current version {current_version})")]
    Conflict { current_version: i64 },
    /// What the caller checked changed before its write (no version to compare): for
    /// example a theme row replaced between loading its files and activating it. The
    /// message says what changed; retrying re-checks.
    #[error("changed concurrently: {0}")]
    Changed(&'static str),
    /// A node-local resource is out of capacity or stopping (the mail queue full or
    /// shutting down); retrying later may work. The text says which (`503 busy`).
    #[error("unavailable: {0}")]
    Unavailable(&'static str),
    /// A value core built itself was refused by its own checks: a bug, not the caller's
    /// input (`500`). The text says which.
    #[error("internal error: {0}")]
    Internal(&'static str),
    /// The operating system's random number generator failed (keys, nonces, tokens).
    #[error("system random number generator failed")]
    Random,
    /// Password hashing failed (the blocking worker panicked or argon2 refused its input).
    #[error("password hashing failed")]
    Hashing,
    #[error(transparent)]
    Store(StoreError),
    /// The write conflicts with what exists (`409`): a locale that still holds content,
    /// a translation group that already has a member in the locale. The text says what
    /// and names the existing row's id or the counts (M6-1).
    #[error("in use: {0}")]
    InUse(String),
    /// The type's publish gate refuses the item: these unsuppressed accessibility
    /// errors remain (at most 50; M6-4, design 16 §5). `422 content-quality`.
    #[error("accessibility errors block publishing")]
    QualityBlocked(Box<Vec<crate::quality::Finding>>),
}

/// One invalid row of a batch ([`CoreError::InvalidRows`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    /// The row's index in the input (0-based).
    pub row: usize,
    pub message: String,
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

impl CoreError {
    /// `InvalidKey { key, message }`.
    pub fn invalid_key(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidKey {
            key: key.into(),
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
