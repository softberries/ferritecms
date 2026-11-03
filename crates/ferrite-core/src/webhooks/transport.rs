//! How a delivery leaves the node (design 18 §6.1): core builds the request, the
//! transport (`ferrite_http::webhook_send::WebhookClient`) sends it through the checked
//! egress resolver.

use std::time::Duration;

use async_trait::async_trait;
use url::Url;

/// One signed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingWebhook {
    pub url: Url,
    /// `webhook-id`, `webhook-timestamp`, `webhook-signature`, `content-type`.
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// What came of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// The receiver answered (any status; `3xx` is not followed).
    Answered {
        status: u16,
        /// `Retry-After` of a `429` or `503`, as a delay.
        retry_after: Option<Duration>,
        /// At most 1 KiB of the body, lossy UTF-8, control characters replaced.
        excerpt: String,
        elapsed: Duration,
    },
    /// No answer: the error class only (`address not allowed`, `timed out`, `TLS error`,
    /// `connection failed`, `request failed`), never a resolved address.
    Failed {
        class: &'static str,
        /// The address rules refused the target.
        refused: bool,
        elapsed: Duration,
    },
}

/// Sends webhook requests. Implementations never follow redirects, never use a proxy and
/// connect only to addresses their rule allows.
#[async_trait]
pub trait WebhookTransport: Send + Sync {
    async fn send(&self, request: OutgoingWebhook) -> SendOutcome;
}

/// The transport of a node that cannot send (its TLS client could not be built): every
/// send fails, so deliveries are retried on the schedule and may land on another node.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTransport;

#[async_trait]
impl WebhookTransport for NoTransport {
    async fn send(&self, _request: OutgoingWebhook) -> SendOutcome {
        SendOutcome::Failed {
            class: "webhooks cannot be sent from this node",
            refused: false,
            elapsed: Duration::ZERO,
        }
    }
}
