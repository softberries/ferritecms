//! FerriteCMS HTTP layer: the axum app serving the public site ([`app`]) and the REST
//! API under `/api/v1` ([`api`]; merged by [`build_site`]), the pure URL
//! router ([`route`]), plugin wiring ([`plugins`]), the install pipeline
//! ([`extensions`]) and the page `<head>` ([`head`]).

pub mod api;
pub mod app;
pub mod extensions;
pub mod head;
pub mod mail;
pub mod plugins;
pub mod route;

pub use app::{
    AppParts, AppState, HttpConfig, LoggedIn, build_app, build_site, client::ClientIp,
    load_route_table,
};
pub use head::{HeadPage, HeadSite, core_head, page_head, render_head};
pub use plugins::{
    FLUSH_EVERY, LoadSummary, PluginReloader, PluginRequest, PluginState, PluginStatus,
    ReloadReport, RenderTrace, SharedFailures, StoreHostServices, filter_item, run_filter,
};
pub use route::{Candidate, NormalQuery, Route, RouteTable, SitemapPart, TypeInfo, resolve};
