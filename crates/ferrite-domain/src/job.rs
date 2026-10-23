//! Background jobs (design 04 §6): the row type, the enqueue input and the lease token.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{JobId, NodeId, Timestamp, string_enum};

/// Attempts a job gets when [`NewJob::max_attempts`] is not changed.
pub const DEFAULT_MAX_ATTEMPTS: i32 = 5;

string_enum!(
    /// Lifecycle state of a job. A retry is `Queued` with `last_error` set; the terminal
    /// failure state is `Dead` (there is no `failed`, ruling P8).
    #[derive(PartialOrd, Ord)]
    JobStatus {
        Queued => "queued",
        Running => "running",
        Done => "done",
        Dead => "dead",
    }
);

/// A stored job row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub kind: String,
    pub payload: Value,
    pub unique_key: Option<String>,
    pub run_at: Timestamp,
    pub priority: i32,
    pub status: JobStatus,
    /// Claims so far, including the current one while `Running`.
    pub attempts: i32,
    pub max_attempts: i32,
    pub locked_by: Option<NodeId>,
    pub locked_until: Option<Timestamp>,
    pub last_error: Option<String>,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

impl Job {
    /// The lease token of a running job (ruling P6): every lease-holder write is
    /// conditional on it. `None` unless the job is `Running` with a holder.
    pub fn lease(&self) -> Option<JobLease> {
        match (self.status, self.locked_by) {
            (JobStatus::Running, Some(node)) => Some(JobLease {
                id: self.id,
                node,
                attempt: self.attempts,
            }),
            _ => None,
        }
    }
}

/// Input for enqueueing a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewJob {
    pub kind: String,
    /// A JSON object.
    pub payload: Value,
    /// At most one job with this key exists, in any status.
    pub unique_key: Option<String>,
    /// `None`: now (database clock).
    pub run_at: Option<Timestamp>,
    /// Higher runs first.
    pub priority: i32,
    pub max_attempts: i32,
}

impl NewJob {
    /// A job of `kind` due now: no unique key, priority 0, [`DEFAULT_MAX_ATTEMPTS`].
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            unique_key: None,
            run_at: None,
            priority: 0,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

/// Lease token of one claim: job id, holding node and attempt number (ruling P6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobLease {
    pub id: JobId,
    pub node: NodeId,
    pub attempt: i32,
}
