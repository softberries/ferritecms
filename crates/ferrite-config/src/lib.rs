//! `ferrite.toml` + `FERRITE_*` environment configuration (design 01 §4, 04 §3).
//!
//! Produces plain config structs; mapping them onto store/media crates is the binary's job.

mod cx;
mod error;
mod load;
mod model;
#[cfg(test)]
mod tests;

pub use error::{ConfigError, ConfigProblem};
pub use load::{InitMedia, load, render_default_toml};
pub use model::*;
