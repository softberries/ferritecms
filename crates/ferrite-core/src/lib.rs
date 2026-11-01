//! FerriteCMS core: business rules on top of the store. Pure modules (slugs, block
//! rendering) have no I/O; services compose store transactions.

pub mod actions;
mod actor;
pub mod auth;
pub mod blocks;
pub mod comments;
pub mod content;
pub mod diagnostics;
mod error;
pub mod events;
pub mod ext_settings;
mod extensions;
pub mod fields;
pub mod import;
pub mod jobs;
pub mod mail;
mod media;
mod menu;
pub mod oauth;
mod options;
pub mod permalink;
mod redirects;
pub mod seed;
pub mod settings;
pub mod site;
pub mod slug;
mod system;
mod taxonomy;
mod tx;
mod types;
mod users;

/// Longest parent chain (content items and terms) walked anywhere in core.
pub const MAX_PARENT_DEPTH: usize = 64;

pub use actor::{
    Access, Actor, Area, Capability, Scope, Scopes, can, can_edit, can_read, require, require_read,
    require_reader, role_can_edit, role_scopes,
};
pub use auth::{
    AuthService, Authenticated, IssuedSession, LoginAttempt, LoginOutcome, SessionInfo, Via,
};
pub use content::{AdminQuery, AutosaveDraft, ContentDraft, ContentService, MAX_REVISIONS_LISTED};
pub use error::{CoreError, RowError};
pub use extensions::{
    EXTENSIONS_LOCK_WAIT, ExtensionAdmin, ExtensionService, MAX_FAILURE_BUCKETS, RetainedPluginData,
};
pub use media::{
    MAX_MEDIA_ALT, MAX_MEDIA_TITLE, MAX_ORIGINAL_NAME, MediaItem, MediaPatch, MediaQuery,
    MediaService, NewUpload, TRASHED_FROM_META, check_file_name,
};
pub use menu::{
    MAX_MENU_ITEMS, MAX_MENU_LABEL, MAX_MENU_URL, MenuLocation, MenuService, is_allowed_menu_url,
    menu_locations,
};
pub use options::OptionService;
pub use permalink::{PermalinkInput, PermalinkRules, Permalinks, SwappablePermalinks};
pub use redirects::{
    ImportReport, MAX_IMPORT_ROWS, MAX_REDIRECT_QUERY, PermalinkRedirectReport, RedirectService,
    STRUCTURE_JOB_PAGES, STRUCTURE_PAGE,
};
pub use site::{
    DepKeys, FeedSource, FrontPage, ItemLink, Listing, MediaFile, MediaPublic, MediaVariant,
    PageItem, Previewed, PublicItem, SiteRead, SiteReader, SitemapEntry, SitemapFile, SitemapPage,
    TermPublic, UserPublic,
};
pub use system::{MetricsSeries, SystemService};
pub use taxonomy::TaxonomyService;
pub use types::{
    ContentTypeService, MAX_TYPE_LABEL, MAX_TYPE_NAME, NewType, TypePatch, is_type_name_shaped,
};
pub use users::{
    MAX_DISPLAY_NAME, MAX_EMAIL, MAX_LOGIN, MAX_USER_QUERY, NewAccount, OperatorUsers, UserPatch,
    UserService, is_bidi_control, normalize_display_name, normalize_email, normalize_login,
};
