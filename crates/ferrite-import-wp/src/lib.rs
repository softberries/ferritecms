//! WordPress import: the WXR reader and the pieces the conversion builds on.
//!
//! [`wxr::parse`] streams a WXR file into [`model::Record`]s, one item at a time;
//! [`php::unserialize`] reads the PHP-serialised values WordPress stores in meta;
//! [`ir::Ir`] keeps a parsed file for the passes, and [`run::run`] drives an import
//! through [`pass`]; [`acf`] reads ACF field definitions and [`urls`] maps old media URLs;
//! [`zip_source`] reads attachment files from a zip of the uploads.

pub mod acf;
pub mod convert;
pub mod error;
pub mod ir;
pub mod meta;
pub mod model;
pub mod pass;
pub mod permalinks;
pub mod php;
pub mod report;
pub mod run;
pub mod seo;
pub mod urls;
pub mod wxr;
pub mod zip_source;

pub use error::ImportError;
