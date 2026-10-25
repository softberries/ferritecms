//! FerriteCMS HTTP layer: the axum app serving the public site ([`app`]) and the REST
//! API under `/api/v1` ([`api`]; merged by [`build_site`]), the pure URL
//! router ([`route`]), plugin wiring ([`plugins`]), the install pipeline
//! ([`extensions`]), media upload staging ([`media`]) and the page `<head>` ([`head`]).

pub mod api;
pub mod app;
pub mod extensions;
pub mod head;
pub mod mail;
pub mod media;
pub mod plugins;
pub mod route;

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
