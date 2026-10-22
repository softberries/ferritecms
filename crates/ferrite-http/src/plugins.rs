//! WASM plugins in page rendering (plan ruling P3, design 05 §7–§7a).
//!
//! * [`load_plugins`]: at startup, every enabled `plugins` row is installed from its
//!   artifact in the [`MediaStore`](ferrite_media::MediaStore). A broken row is logged
//!   and skipped; it never stops the site.
//! * [`StoreHostServices`]: the store-backed [`HostServices`](ferrite_plugin_host::HostServices).
//!   `content.read` sees public items only, through [`SiteReader`](ferrite_core::SiteReader).
//! * [`run_filter`] / [`filter_item`]: apply render filters for one request and collect
//!   what they touched into a [`RenderTrace`] for the page cache.
//!
//! **Request scope.** `HostServices` methods get no request argument (the trait mirrors
//! the WIT imports), so per-request state travels in a tokio task-local set by
//! [`run_filter`] around the plugin calls: the preview item (05 §7a: during an
//! authenticated preview, `get-post` on that one item returns it) and the dependency
//! keys [`SiteReader`](ferrite_core::SiteReader) reports beyond what the plugin host
//! records itself (`user:<id>`, `term:<id>`, ancestors). Plugin calls run inside the
//! caller's task (no `spawn`), so the task-local is visible to every import. Outside
//! a scope (action hooks, routes) there is no preview and the extra keys are dropped.

mod apply;
mod load;
mod services;

pub use apply::{PluginRequest, Preview, RenderTrace, filter_item, run_filter};
pub use load::{LoadSummary, load_plugins};
pub use services::{KV_MAX_BYTES, KV_MAX_KEYS, StoreHostServices};
