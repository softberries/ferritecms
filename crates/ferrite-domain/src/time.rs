use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};

use crate::DomainError;

/// UTC instant with microsecond precision (lossless in SQLite and PostgreSQL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Current time, truncated to microseconds.
    pub fn now() -> Self {
        Self(Utc::now().trunc_subsecs(6))
    }

    pub fn from_micros(micros: i64) -> Result<Self, DomainError> {
        DateTime::from_timestamp_micros(micros)
            .map(Self)
            .ok_or(DomainError::TimestampOutOfRange(micros))
    }

    pub fn as_micros(&self) -> i64 {
        self.0.timestamp_micros()
    }

    pub fn to_rfc3339(&self) -> String {
        self.0.to_rfc3339()
    }

    pub fn as_datetime(&self) -> DateTime<Utc> {
        self.0
    }
}

impl From<DateTime<Utc>> for Timestamp {
    /// Truncates to microseconds.
    fn from(dt: DateTime<Utc>) -> Self {
        Self(dt.trunc_subsecs(6))
    }
}
