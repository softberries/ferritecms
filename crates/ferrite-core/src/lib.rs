//! FerriteCMS core: business rules on top of the store. This module set is pure
//! (slugs, block rendering) and has no I/O.

pub mod blocks;
mod error;
pub mod slug;

pub use error::CoreError;
