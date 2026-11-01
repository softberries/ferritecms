//! FerriteCMS HTTP layer: the axum app serving the public site ([`app`]), the embedded
//! admin SPA under `/admin/` ([`admin`]) and the REST API under `/api/v1` ([`api`];
//! merged by [`build_site`]), the pure URL
//! router ([`route`]), plugin wiring ([`plugins`]), the install pipeline
//! ([`extensions`]), media upload staging ([`media`]), the page `<head>` ([`head`]) and
//! the metrics registry with its `/metrics` app ([`metrics`]), the OAuth authorization
//! server's endpoints ([`oauth`]) and its metadata-document fetcher ([`oauth_fetch`]),
//! and HSTS plus the https redirect app for the built-in TLS ([`tls`]).

pub mod admin;
pub mod api;
pub mod app;
pub mod extensions;
pub mod head;
pub mod import_blobs;
pub mod import_fetch;
pub mod mail;
pub mod media;
pub mod metrics;
pub mod oauth;
pub mod oauth_fetch;
pub mod plugins;
pub mod route;
pub mod tls;

pub use app::{
    AppParts, AppState, HttpConfig, LoggedIn, build_app, build_site, client::ClientIp,
    load_route_table,
};
pub use head::{HeadPage, HeadSite, core_head, page_head, render_head};
pub use plugins::{
    CacheOutcome, FLUSH_EVERY, LoadSummary, MAX_SERVED_PATH, PluginReloader, PluginRequest,
    PluginState, PluginStatus, ReloadReport, RenderTrace, SERVED_BUFFER_CAP, SERVED_FLUSH_AT,
    SERVED_FLUSH_EVERY, SERVED_HOOK, ServedBuffer, ServedEvent, SharedFailures, StoreHostServices,
    UaClass, filter_footer, filter_item, run_filter, ua_class,
};
pub use route::{Candidate, NormalQuery, Route, RouteTable, SitemapPart, TypeInfo, resolve};
