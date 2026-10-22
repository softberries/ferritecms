use serde::{Deserialize, Serialize};

use crate::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub kind: String,
    pub keys: Vec<String>,
    pub origin_node: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEvent {
    pub kind: String,
    pub keys: Vec<String>,
    pub origin_node: Option<String>,
}
