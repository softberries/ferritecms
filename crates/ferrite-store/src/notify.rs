//! Change notifications between nodes (design 04 §4, P1–P3): wake-up hints only, the
//! tables stay the source of truth.

use async_trait::async_trait;

use crate::StoreError;

/// What a [`Subscription`] listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// A committed `events` row (payload: its seq).
    Events,
    /// A committed new `jobs` row (payload: its kind).
    Jobs,
}

/// One wake-up from a [`Subscription`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Something was committed on the channel. A hint: poll the table; several commits
    /// may arrive as one notice (Postgres folds identical payloads within a transaction).
    Notified,
    /// Notifications may have been lost: the listening connection was lost and has been
    /// re-established, or an earlier `recv` failed and this call rebuilt the
    /// subscription. The receiver must catch up from the table (P3). Every possible gap
    /// is reported this way, so a receiver needs no other bookkeeping.
    Reconnected,
}

/// A live subscription to one [`Channel`] on its own connection. Dropping it stops
/// listening. It is independent of the store's pool: [`crate::Store::close`] does not
/// end it.
#[async_trait]
pub trait Subscription: Send {
    /// Waits for the next notice. An error means the connection could not be
    /// re-established; the next call retries (the caller backs off) and, once it
    /// succeeds, returns [`Notice::Reconnected`].
    ///
    /// Drain continuously or drop: undelivered notifications fill the server's queue,
    /// and on Postgres a full queue makes every committing NOTIFY fail.
    async fn recv(&mut self) -> Result<Notice, StoreError>;
}
