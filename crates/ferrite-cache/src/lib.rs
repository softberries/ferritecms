//! Per-node page cache with a dependency index, and the outbox-polling invalidation
//! loop that keeps it consistent with writes (06 §6, 04 §4).
//!
//! * [`PageCache`]: moka-backed, bounded by bytes (`cache.max_bytes`), keyed by
//!   [`CacheKey`]. Each [`CachedPage`] carries the dependency keys recorded while it
//!   was rendered (grammar and emitter/renderer contract: `ferrite_core::events`);
//!   [`PageCache::invalidate`] drops every page indexed under any of the given keys,
//!   and `all` drops everything. Not cached: status >= 400, `Set-Cookie`. Pages enter
//!   the cache only through [`PageCache::get_or_render`] (the guarded path). The one
//!   exception to the status rule: [`PageCache::get_or_render_error`] stores a 404/410
//!   page under an [internal key](CacheKey::internal) (the shared error-page body).
//! * [`InvalidationLoop`]: polls the `events` outbox and applies each event to the
//!   cache, following the sequence-gap rule of 04 §4 (skipping a gap flushes and
//!   watches the skipped seqs for late commits). The boot cursor is `max_seq()`, or the
//!   first hole among the last 1000 events if there is one; create the loop (which
//!   reads it, and fails on a store error) before serving traffic. With `listen` it
//!   also polls on every `LISTEN/NOTIFY` wake-up and checks for pruned history after a
//!   reconnect (P3); an [`EventHook`] sees each applied event, for node state beyond the
//!   cache.
//! * [`etag_for`] / [`if_none_match_matches`]: ETag and `304` helpers.
//!
//! Correctness over hit rate: a stale page after an edit is the bug this crate exists
//! to prevent, so every ambiguous case (unknown event kind, a render overlapping an
//! invalidation, a skipped sequence gap) errs towards dropping pages.

mod cache;
mod http;
mod index;
mod invalidation;
mod page;

pub use cache::{CacheStats, PageCache};
pub use http::{etag_for, if_none_match_matches};
pub use index::ALL_KEY;
pub use invalidation::{
    EventHook, EventSource, GAP_TIMEOUT, InvalidationLoop, LATE_HORIZON, MAX_LATE, POLL_BATCH,
};
pub use page::{CacheKey, CachedPage, Variant};
