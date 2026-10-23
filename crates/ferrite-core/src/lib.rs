//! FerriteCMS core: business rules on top of the store. Pure modules (slugs, block
//! rendering) have no I/O; services compose store transactions.

mod actor;
pub mod blocks;
mod content;
mod error;
pub mod events;
mod extensions;
pub mod jobs;
mod menu;
mod options;
pub mod permalink;
mod redirects;
pub mod site;
pub mod slug;
mod taxonomy;
mod tx;

/// Longest parent chain (content items and terms) walked anywhere in core.
pub(crate) const MAX_PARENT_DEPTH: usize = 64;

pub use actor::{Actor, Capability, can};
pub use content::{ContentDraft, ContentService};
pub use error::CoreError;
pub use extensions::{EXTENSIONS_LOCK_WAIT, ExtensionService};
pub use menu::MenuService;
pub use options::OptionService;
pub use permalink::{PermalinkInput, PermalinkRules, Permalinks, SwappablePermalinks};
pub use redirects::RedirectService;
pub use site::{
    DepKeys, FeedSource, FrontPage, ItemLink, Listing, MediaFile, MediaPublic, MediaVariant,
    PageItem, Previewed, PublicItem, SiteRead, SiteReader, SitemapEntry, SitemapPage, TermPublic,
    UserPublic,
};
pub use taxonomy::TaxonomyService;
