//! The `/api/v1` surface through `build_site` on both profiles (M2a-1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod harness;

mod auth;
mod comments;
mod content;
mod foundation;
mod idempotency;
mod media;
mod plugins;
mod reset;
mod settings_ext;
mod site;
mod sweep;
mod themes;
mod types;
mod users;
