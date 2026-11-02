//! Outgoing webhooks (M6-6, design 18): the event catalogue, Standard Webhooks
//! signing, sealed secrets, the URL rules, the retry schedule, [`WebhookService`] and
//! [`emit`], which the write paths call inside their own `Tx` (the job queue is the
//! outbox). The transport that sends lives in `ferrite-http` ([`WebhookTransport`]).

use std::time::Duration;

pub mod catalogue;
mod deliver;
pub mod schedule;
pub mod secret;
mod service;
pub mod sign;
mod transport;
pub mod url;

pub use catalogue::{EVENT_TYPES, EventTypeInfo, PAYLOAD_VERSION, PING, check_patterns, matches};
pub use deliver::{Delivered, render_body};
pub use service::{
    DeliveryDetail, PAUSED, WebhookInput, WebhookPatch, WebhookService, WebhookState, WebhookView,
};
pub use transport::{NoTransport, OutgoingWebhook, SendOutcome, WebhookTransport};
pub use url::{MAX_URL, UrlPolicy, check_url, host_of};

/// Endpoints per site.
pub const MAX_ENDPOINTS: u64 = 20;
/// Longest endpoint name, in characters.
pub const MAX_NAME: usize = 100;
/// The whole request timeout (connect, TLS, send, headers and the read).
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// Largest request body.
pub const MAX_WEBHOOK_BODY: usize = 64 * 1024;
/// The response is read up to this many bytes, then dropped.
pub const MAX_RESPONSE_READ: usize = 4 * 1024;
/// The stored response excerpt, at most.
pub const MAX_EXCERPT: usize = 1024;
/// Deliveries one "redeliver since" call queues, at most.
pub const MAX_REDELIVER: u32 = 1000;
/// How long events (and by cascade their deliveries and attempts) are kept.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// An endpoint failing for longer than this ...
pub const AUTO_DISABLE_AFTER: Duration = Duration::from_secs(5 * 24 * 3600);
/// ... with at least this many failed attempts in a row is disabled (`failing`).
pub const AUTO_DISABLE_FAILURES: i32 = 10;
/// How long the previous secret keeps signing after a rotation.
pub const ROTATION_WINDOW: Duration = Duration::from_secs(24 * 3600);
/// Titles in payloads are cut at this many characters.
pub const MAX_TITLE: usize = 300;
/// Comment bodies in payloads are cut at this many characters.
pub const MAX_COMMENT_BODY: usize = 2000;
