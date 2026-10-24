//! WASM plugins in page rendering (plan ruling P3, design 05 §7–§7a).
//!
//! * [`PluginReloader`]: brings the plugin host to the `plugins` rows, installing each
//!   enabled row from its artifact in the [`MediaStore`](ferrite_media::MediaStore)
//!   with one registry swap (hot reload, P18/P19); `serve`'s first reload installs the
//!   plugins at startup. A broken row is logged and skipped (an older loaded version keeps
//!   running); it never stops the site.
//! * [`SharedFailures`]: the node's [`FailureSink`](ferrite_plugin_host::FailureSink),
//!   sharing failure counts through the database for the cluster-wide auto-disable
//!   (P20).
//! * [`StoreHostServices`]: the store-backed [`HostServices`](ferrite_plugin_host::HostServices).
//!   `content.read` sees public items only, through [`SiteReader`](ferrite_core::SiteReader).
//! * [`run_filter`] / [`filter_item`]: apply render filters for one request and collect
//!   what they touched into a [`RenderTrace`] for the page cache.
//! * [`ServedBuffer`]: the node's bounded `request.served` buffer, flushed in batches
//!   into `plugin.action` jobs (P21).
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
mod failures;
mod load;
mod reload;
mod served;
mod services;

pub use apply::{PluginRequest, RenderTrace, filter_item, run_filter};
pub use failures::{FLUSH_EVERY, SharedFailures};
pub use load::LoadSummary;
pub use reload::{PluginReloader, PluginState, PluginStatus, ReloadReport};
pub use served::{
    CacheOutcome, MAX_SERVED_PATH, SERVED_BUFFER_CAP, SERVED_FLUSH_AT, SERVED_FLUSH_EVERY,
    SERVED_HOOK, ServedBuffer, ServedEvent, UaClass, ua_class,
};
pub use services::{KV_MAX_BYTES, KV_MAX_KEYS, StoreHostServices};
