//! Content-addressed media storage over `object_store` (local folder or S3).

mod config;
mod error;
mod image;
pub mod ingest;
mod keys;
mod name;
mod sniff;
mod store;
mod strip;
pub mod svg;

pub use config::{Backend, ConditionalPut, MediaConfig, PublicUrlMode, S3Config};
pub use error::MediaError;
pub use image::{MAX_PIXELS, Processed, Variant, VariantSpec, default_variants, process_image};
pub use name::sanitize_filename;
pub use sniff::{inline_safe, is_raster_image, sniff_mime};
pub use store::{
    ArtifactRef, MediaStore, OWNER_MARKER, OwnerMarker, ReadStream, StoredObject,
    validate_copy_if_not_exists,
};
pub use strip::strip_metadata;
