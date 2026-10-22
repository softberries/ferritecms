//! FerriteCMS core: business rules on top of the store. Pure modules (slugs, block
//! rendering) have no I/O; services compose store transactions.

mod actor;
pub mod blocks;
mod content;
mod error;
pub mod events;
pub mod slug;

pub use actor::{Actor, Capability, can};
pub use content::{ContentDraft, ContentService, PermalinkFn};
pub use error::CoreError;
