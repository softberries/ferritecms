//! Content-addressed media storage over `object_store` (local folder or S3).

mod config;
mod error;
mod keys;
mod store;

pub use config::{Backend, MediaConfig, PublicUrlMode, S3Config};
pub use error::MediaError;
pub use store::MediaStore;
