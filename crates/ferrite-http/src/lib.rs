//! FerriteCMS HTTP layer. So far: the pure URL router ([`route`]), plugin wiring
//! ([`plugins`]) and the page `<head>` ([`head`]); the axum app that serves the routes
//! comes next.

pub mod head;
pub mod plugins;
pub mod route;

pub use head::{HeadPage, HeadSite, core_head, page_head, render_head};
pub use plugins::{
    LoadSummary, PluginRequest, Preview, RenderTrace, StoreHostServices, filter_item, load_plugins,
    run_filter,
};
pub use route::{Candidate, NormalQuery, Route, RouteTable, SitemapPart, TypeInfo, resolve};
