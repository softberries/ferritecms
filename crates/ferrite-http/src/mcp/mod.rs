//! The MCP endpoint `POST /mcp` (design 12 §2, §3.1, §5, §6): AI assistants manage the
//! site through the Model Context Protocol, as the user whose token they hold.
//!
//! * **Transport.** Streamable HTTP from the official SDK (`rmcp`), **stateless**: no
//!   sessions (`NeverSessionManager`, `legacy_session_mode = false`), JSON answers
//!   (`json_response`), one [`FerriteMcp`] per request. Clients of
//!   the 2026-07-28 revision use `server/discover` and per-request metadata; older ones
//!   `initialize` and are served statelessly too. Bodies of at most [`MAX_MCP_BODY`]
//!   (`413` beyond, enforced by the SDK while it reads).
//! * **In front of it** ([`auth::guard`], in this order): the off switch (`mcp.enabled`
//!   off: `404` for any method, the OAuth router's gate), a method other than `POST`
//!   (`405`, `Allow: POST`: no server-initiated stream, no session to delete), a foreign
//!   `Origin` (`403`), then exactly one `Authorization: Bearer` token resolved by
//!   `AuthService::resolve_token` (a personal API token or an OAuth access token; never
//!   the session cookie), else `401` with the `resource_metadata` challenge. The
//!   resolved [`Caller`] goes into the request's extensions, where every
//!   tool reads it.
//! * **The SDK's own `Host` check is off**: it only knows loopback names, and the
//!   endpoint is reached under `site.url`'s host (or a proxy's). DNS rebinding gains
//!   nothing here: every request needs a bearer token, which a page cannot attach by
//!   itself, and a foreign `Origin` is refused anyway.
//! * **Tools** (`tools/`, one file per area) are thin adapters to the core services
//!   with the caller's `Actor`; core checks every permission. Each tool call counts on
//!   the user's limit (120 a minute, `AuthService::hit_mcp_limit`) before it runs; over
//!   it the call is an `isError` result. Results and errors are built by [`results`];
//!   successful writes are audited with [`FerriteMcp::audit`].
//! * Every answer is `Cache-Control: no-store`.
//! * **Shutdown.** The SDK's cancellation token is not wired to the node's shutdown:
//!   answers are plain JSON (no streams to end), and axum's graceful shutdown already
//!   lets the open requests finish.

pub mod auth;
pub mod prompts;
pub mod resources;
pub mod results;
pub mod server;
pub mod tools;

use std::sync::Arc;

use axum::{
    Router,
    http::{HeaderValue, header},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};

use tower_http::set_header::SetResponseHeaderLayer;

use crate::api::ApiState;

pub use server::{Caller, FerriteMcp};

/// Where the endpoint is mounted.
pub const PATH: &str = "/mcp";
/// The largest request body: a 6 MiB media upload as base64 (8 MiB) plus the JSON
/// envelope (design 12 §2, §5.2).
pub const MAX_MCP_BODY: usize = 9 << 20;

/// `POST /mcp` with the guards of the module docs.
pub fn router(api: Arc<ApiState>) -> Router {
    let routers = Arc::new(FerriteMcp::routers());
    let factory = {
        let api = api.clone();
        move || Ok(FerriteMcp::new(api.clone(), routers.clone()))
    };
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_sse_retry(None)
        .with_max_request_body_bytes(MAX_MCP_BODY)
        .disable_allowed_hosts()
        .disable_allowed_origins();
    let service =
        StreamableHttpService::new(factory, Arc::new(NeverSessionManager::default()), config);
    Router::new()
        .route_service(PATH, service)
        .layer(axum::middleware::from_fn_with_state(
            api.clone(),
            auth::guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            api,
            crate::oauth::gate,
        ))
        // Answers are per user (and tokens are in the requests): never stored by a cache.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
}
