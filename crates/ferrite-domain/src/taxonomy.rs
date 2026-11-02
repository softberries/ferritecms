use serde::{Deserialize, Serialize};

use crate::TermId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taxonomy {
    pub name: String,
    pub label: String,
    pub hierarchical: bool,
    /// Content type names this taxonomy applies to.
    pub content_types: Vec<String>,
    pub built_in: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub id: TermId,
    pub taxonomy: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_id: Option<TermId>,
    /// The term's locale (13 §2.5).
    pub locale: String,
    /// The id of the term translation group's original; the term's own id when alone.
    pub translation_group: TermId,
}
