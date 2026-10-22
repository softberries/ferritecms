//! Wasmtime-based host for FerriteCMS plugins.

mod bindings;
mod types;

pub use types::{
    ActionEvent, FilterValue, Head, LinkTag, MetaTag, Post, RequestInfo, RouteRequest,
    RouteResponse, ScriptTag, TermRef,
};
