//! The public app end to end on both profiles (Task 8a): every content route type,
//! canonical and redirect responses, error pages, HEAD, ETag/304, 405, health checks,
//! compression, Review Focus 1 and 2 over HTTP, plugin head tags and cache hits; previews
//! and logged-in requests (M2a-1 Task 12); the admin SPA under `/admin/` (M2b-1 Task 1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../plugins/common.rs"]
mod common;
mod harness;

mod admin;
mod assets;
mod budget;
mod comments;
mod content;
mod cost;
mod errors;
mod feeds;
mod freshness;
mod plugin_route;
mod preview;
mod protocol;
mod reload_race;
mod served;
