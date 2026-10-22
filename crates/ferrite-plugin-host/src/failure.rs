//! Failure records of plugin calls (design 05 §8). Classification and the tracker that
//! records them land in Task 7; this task only defines the public shape.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Trap,
    OutOfFuel,
    Deadline,
    Memory,
    Instantiate,
    GuestError,
    InvalidOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRecord {
    pub plugin: String,
    pub hook: String,
    pub kind: FailureKind,
    pub at: Instant,
    pub duration: Duration,
}
