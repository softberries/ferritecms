//! FerriteCMS HTTP layer: the axum app serving the public site ([`app`]), the pure URL
//! router ([`route`]), plugin wiring ([`plugins`]) and the page `<head>` ([`head`]).

pub mod app;
pub mod head;
pub mod plugins;
pub mod route;

pub use app::{AppParts, AppState, HttpConfig, build_app, client::ClientIp, load_route_table};
pub use head::{HeadPage, HeadSite, core_head, page_head, render_head};
pub use plugins::{
    LoadSummary, PluginRequest, RenderTrace, StoreHostServices, filter_item, load_plugins,
    run_filter,
};
pub use route::{Candidate, NormalQuery, Route, RouteTable, SitemapPart, TypeInfo, resolve};
