//! The `/api/v1` surface through `build_site` on both profiles (M2a-1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod harness;

mod auth;
mod autosave;
mod comments;
mod comments_admin;
mod content;
mod content_extras;
mod content_seo;
mod extensions_m2b3;
mod foundation;
mod idempotency;
mod import;
mod jobs;
mod mcp;
mod mcp_admin;
mod mcp_content;
mod mcp_reference;
mod mcp_structure;
mod media;
mod media_filters;
mod menu_locations;
mod metrics_series;
mod oauth;
mod plugins;
mod plugins_trust;
mod redirect_search;
mod reset;
mod sessions;
mod settings_ext;
mod site;
mod sweep;
mod system;
mod system_m2b3;
mod themes;
mod types;
mod users;
mod users_admin;
