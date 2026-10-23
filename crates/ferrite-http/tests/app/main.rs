//! The public app end to end on both profiles (Task 8a): every content route type,
//! canonical and redirect responses, error pages, HEAD, ETag/304, 405, health checks,
//! compression, Review Focus 1 and 2 over HTTP, plugin head tags and cache hits.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../plugins/common.rs"]
mod common;
mod harness;

mod assets;
mod content;
mod cost;
mod errors;
mod feeds;
mod freshness;
mod protocol;
