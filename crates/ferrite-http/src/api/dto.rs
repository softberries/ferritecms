//! Request and response bodies of the API, one module per resource (P12). They derive
//! `Serialize`/`Deserialize` and `utoipa::ToSchema`; the domain types stay without
//! utoipa (M1a R3). Bodies that carry passwords, codes or tokens derive no `Debug`.

pub mod auth;
pub mod content;
pub mod extensions;
pub mod site;
pub mod types;
pub mod users;
