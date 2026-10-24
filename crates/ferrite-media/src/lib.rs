//! Content-addressed media storage over `object_store` (local folder or S3).

mod config;
mod error;
mod image;
mod keys;
mod name;
mod sniff;
mod store;
pub mod svg;

pub use config::{Backend, MediaConfig, PublicUrlMode, S3Config};
pub use error::MediaError;
pub use image::{MAX_PIXELS, Processed, Variant, VariantSpec, default_variants, process_image};
pub use name::sanitize_filename;
pub use sniff::{inline_safe, is_raster_image, sniff_mime};
pub use store::{ArtifactRef, MediaStore, ReadStream};
