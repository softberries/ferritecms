//! FerriteCMS core: business rules on top of the store. Pure modules (slugs, block
//! rendering) have no I/O; services compose store transactions.

mod actor;
pub mod blocks;
mod content;
mod error;
pub mod events;
mod menu;
mod options;
pub mod permalink;
mod redirects;
pub mod slug;
mod taxonomy;
mod tx;

pub use actor::{Actor, Capability, can};
pub use content::{ContentDraft, ContentService};
pub use error::CoreError;
pub use menu::MenuService;
pub use options::OptionService;
pub use permalink::{PermalinkInput, PermalinkRules, Permalinks};
pub use redirects::RedirectService;
pub use taxonomy::TaxonomyService;
