//! Per-node page cache with a dependency index, and the outbox-polling invalidation
//! loop that keeps it consistent with writes (06 §6, 04 §4).
//!
//! * [`PageCache`]: moka-backed, bounded by bytes (`cache.max_bytes`), keyed by
//!   [`CacheKey`]. Each [`CachedPage`] carries the dependency keys recorded while it
//!   was rendered (grammar and emitter/renderer contract: `ferrite_core::events`);
//!   [`PageCache::invalidate`] drops every page indexed under any of the given keys,
//!   and `all` drops everything. Not cached: status >= 400, `Set-Cookie`.
//! * [`InvalidationLoop`]: polls the `events` outbox from `max_seq()` at boot and
//!   applies each event to the cache, following the sequence-gap rule of 04 §4.
//! * [`etag_for`] / [`if_none_match_matches`]: ETag and `304` helpers.
//!
//! Correctness over hit rate: a stale page after an edit is the bug this crate exists
//! to prevent, so every ambiguous case (unknown event kind, a render overlapping an
//! invalidation, a failed boot read) errs towards dropping pages.

mod cache;
mod http;
mod index;
mod invalidation;
mod page;

pub use cache::PageCache;
pub use http::{etag_for, if_none_match_matches};
pub use index::ALL_KEY;
pub use invalidation::{EventSource, GAP_TIMEOUT, InvalidationLoop, POLL_BATCH};
pub use page::{CacheKey, CachedPage, Variant};
