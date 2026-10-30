//! Markdown import (M4-3b P62): a folder of Markdown files to pages.
//!
//! [`scan()`] walks the folder into [`MdDoc`]s (the page tree, slugs, order, front matter
//! and the hashes a re-run compares) and the [`Problem`]s it met; [`split_front_matter`]
//! reads a file's TOML front matter; [`convert()`] turns a file's Markdown into the stored
//! block tree through the WordPress importer's HTML converter; [`map_id`] is a file's
//! key in `import_map`; [`locate`] finds the file a relative image or link names, never
//! outside the folder; [`run()`] imports the folder through an import run.

pub mod convert;
pub mod error;
pub mod front;
pub mod run;
pub mod scan;

pub use convert::{Converted, convert};
pub use error::MdError;
pub use front::{FrontMatter, split_front_matter};
pub use run::{MdOptions, MdReport, needs_hierarchy, run};
pub use scan::{
    FORMAT, MAX_DEPTH, MAX_FILE_BYTES, MAX_FILES, MdDoc, Miss, Problem, Scan, Target, lexical,
    locate, map_id, resolve, scan,
};
