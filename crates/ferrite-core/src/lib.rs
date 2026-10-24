//! FerriteCMS core: business rules on top of the store. Pure modules (slugs, block
//! rendering) have no I/O; services compose store transactions.

mod actor;
pub mod auth;
pub mod blocks;
mod content;
mod error;
pub mod events;
pub mod ext_settings;
mod extensions;
pub mod fields;
pub mod jobs;
pub mod mail;
mod menu;
mod options;
pub mod permalink;
mod redirects;
pub mod settings;
pub mod site;
pub mod slug;
mod taxonomy;
mod tx;
mod types;
mod users;

/// Longest parent chain (content items and terms) walked anywhere in core.
pub(crate) const MAX_PARENT_DEPTH: usize = 64;

pub use actor::{
    Access, Actor, Area, Capability, Scope, Scopes, can, can_edit, can_read, require, require_read,
    require_reader, role_can_edit,
};
pub use auth::{AuthService, Authenticated, IssuedSession, LoginAttempt, LoginOutcome, Via};
pub use content::{AdminQuery, ContentDraft, ContentService, MAX_REVISIONS_LISTED};
pub use error::{CoreError, RowError};
pub use extensions::{EXTENSIONS_LOCK_WAIT, ExtensionAdmin, ExtensionService};
pub use menu::{MAX_MENU_ITEMS, MenuService};
pub use options::OptionService;
pub use permalink::{PermalinkInput, PermalinkRules, Permalinks, SwappablePermalinks};
pub use redirects::{ImportReport, MAX_IMPORT_ROWS, RedirectService};
pub use site::{
    DepKeys, FeedSource, FrontPage, ItemLink, Listing, MediaFile, MediaPublic, MediaVariant,
    PageItem, Previewed, PublicItem, SiteRead, SiteReader, SitemapEntry, SitemapPage, TermPublic,
    UserPublic,
};
pub use taxonomy::TaxonomyService;
pub use types::{ContentTypeService, MAX_TYPE_LABEL, MAX_TYPE_NAME, NewType, TypePatch};
pub use users::{
    MAX_DISPLAY_NAME, MAX_EMAIL, MAX_LOGIN, NewAccount, OperatorUsers, UserPatch, UserService,
    normalize_display_name, normalize_email, normalize_login,
};
