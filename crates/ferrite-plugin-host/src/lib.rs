//! Wasmtime-based host for FerriteCMS plugins.

mod bindings;
mod capability;
mod config;
pub mod engine;
mod error;
mod inspect;
mod manifest;
mod types;

pub use capability::{
    DeadlineClass, FilterType, HookKind, HookSpec, Iface, granted_interfaces, hook_catalogue,
};
pub use config::HostConfig;
pub use error::{InspectError, ManifestError, ManifestProblem, PluginError};
pub use inspect::{ComponentReport, inspect};
pub use manifest::{
    Capabilities, HtmlMode, LimitRequest, MailScope, Manifest, PluginMeta, RouteSpec, SettingKind,
    SettingSpec,
};

pub use types::{
    ActionEvent, FilterValue, Head, LinkTag, MetaTag, Post, RequestInfo, RouteRequest,
    RouteResponse, ScriptTag, TermRef,
};
