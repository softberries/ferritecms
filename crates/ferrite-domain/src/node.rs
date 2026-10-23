//! The node registry (design 04 §3, 02 §4 "Cluster", ruling P14): one row per running
//! `ferrite serve` process, refreshed by its heartbeat.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{NodeId, Timestamp};

/// Input for registering a node; `started_at` and `heartbeat_at` come from the database
/// clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewNode {
    pub id: NodeId,
    pub name: String,
    /// The binary's version.
    pub version: String,
    /// The highest migration this binary knows.
    pub schema_version: i64,
    /// A JSON object (P14 lists its keys).
    pub meta: Value,
}

/// A stored node row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub version: String,
    pub schema_version: i64,
    pub started_at: Timestamp,
    pub heartbeat_at: Timestamp,
    pub meta: Value,
}
