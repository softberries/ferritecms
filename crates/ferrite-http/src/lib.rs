//! FerriteCMS HTTP layer. So far: the pure URL router ([`route`]); the axum app that
//! serves its routes comes next.

pub mod route;

pub use route::{Candidate, NormalQuery, Route, RouteTable, SitemapPart, TypeInfo, resolve};
