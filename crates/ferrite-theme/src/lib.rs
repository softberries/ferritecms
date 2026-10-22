//! FerriteCMS themes (06 §1, §2, §4, §7): theme packages ([`Theme`], [`ThemeSource`]),
//! template-hierarchy resolution ([`resolve_template`]) and the sandboxed MiniJinja
//! renderer ([`Renderer`]).
//!
//! Themes execute no code: templates get preloaded data plus a fixed set of safe
//! helpers; the only data-backed helpers go through a [`HelperBridge`]. See
//! [`render`] for the sandbox, the globals and the dependency keys recorded.

mod bridge;
mod error;
mod filters;
pub mod hierarchy;
mod manifest;
mod package;
pub mod render;
mod view;

pub use bridge::{
    Bridged, HelperBridge, ImageRef, MAX_QUERY_LIMIT, MAX_RELATED, MenuNode, MenuTree, NoBridge,
};
pub use error::ThemeError;
pub use hierarchy::{TemplateKey, error_template, resolve_template};
pub use manifest::{ImageSize, SettingKind, SettingSpec, ThemeManifest};
pub use package::{MAX_FILES, MAX_TOTAL_BYTES, Theme, ThemeSource};
pub use render::{
    FUEL, MAX_BRIDGE_CALLS, MAX_OUTPUT, RenderContext, RenderOutput, Renderer, RequestInfo,
    SiteInfo,
};
