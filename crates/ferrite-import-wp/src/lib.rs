//! WordPress import: the WXR reader and the pieces the conversion builds on.
//!
//! [`wxr::parse`] streams a WXR file into [`model::Record`]s, one item at a time;
//! [`php::unserialize`] reads the PHP-serialised values WordPress stores in meta.

pub mod convert;
pub mod error;
pub mod model;
pub mod php;
pub mod wxr;

pub use error::ImportError;
