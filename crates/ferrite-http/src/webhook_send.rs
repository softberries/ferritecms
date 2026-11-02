//! The webhook transport (design 18 §6.1): [`WebhookClient`] sends the signed `POST`s
//! core builds, through the plugin egress client's checked resolver.
//!
//! * **Addresses.** The name is resolved once per request and the whole answer is
//!   refused when any address fails the rule ([`is_public_ip`], or with `[webhooks]
//!   local_targets` [`is_local_target_ip`]); reqwest connects only to the checked
//!   addresses and the connected peer is checked again. An IP-literal host must pass the
//!   rule itself.
//! * **No redirects** (a `3xx` is answered as is; core counts it a failure), no proxy, no
//!   cookie store, no `Referer`, no decompression. `https` only, unless `local_targets`.
//! * **Bounds.** [`WEBHOOK_TIMEOUT`] (15 s) for the whole request including the read;
//!   the response is read up to [`MAX_RESPONSE_READ`] (4 KiB) and then dropped; the
//!   first [`MAX_EXCERPT`] bytes, lossy UTF-8 with control characters replaced, are the
//!   excerpt. Only `Retry-After` is read from the headers.
//! * **Errors** name the class only; refusals are logged at `warn` with the URL host,
//!   once a minute per host.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ferrite_core::webhooks::{
    MAX_EXCERPT, MAX_RESPONSE_READ, OutgoingWebhook, SendOutcome, WebhookTransport,
};
use reqwest::header::{HeaderName, HeaderValue, RETRY_AFTER, USER_AGENT};
use url::Url;

use crate::plugins::egress::{
    self, WEBHOOK_TIMEOUT, error_class, is_local_target_ip, is_public_ip,
};

const ADDRESS_NOT_ALLOWED: &str = "address not allowed";
const TIMED_OUT: &str = "timed out";
const REQUEST_FAILED: &str = "request failed";
/// How often refusals for one host are logged at `warn`.
const REFUSAL_LOG_EVERY: Duration = Duration::from_secs(60);

/// See the module docs.
pub struct WebhookClient {
    client: reqwest::Client,
    allow_ip: fn(IpAddr) -> bool,
    local_targets: bool,
    timeout: Duration,
    /// `ferrite/<version> webhooks`.
    agent: String,
    refusals_logged: Mutex<HashMap<String, Instant>>,
}

/// The address rule for `local_targets`.
pub fn webhook_ip_rule(local_targets: bool) -> fn(IpAddr) -> bool {
    if local_targets {
        is_local_target_ip
    } else {
        is_public_ip
    }
}

impl WebhookClient {
    /// The production client: the system resolver, the rule of `local_targets`, the
    /// platform roots (with the `webpki-roots` fallback); `version` goes into the
    /// `User-Agent`. An error means no TLS client could be built: `serve` then uses
    /// [`NoTransport`](ferrite_core::webhooks::NoTransport).
    pub fn new(version: &str, local_targets: bool) -> Result<Self, String> {
        let allow_ip = webhook_ip_rule(local_targets);
        Ok(Self {
            client: egress::webhook_client(allow_ip, !local_targets)?,
            allow_ip,
            local_targets,
            timeout: WEBHOOK_TIMEOUT,
            agent: format!("ferrite/{version} webhooks"),
            refusals_logged: Mutex::default(),
        })
    }

    /// Test seam: `overrides`' resolver, roots and address rule, and a shorter
    /// `timeout`.
    #[cfg(feature = "test-seams")]
    pub fn for_tests(
        overrides: egress::EgressOverrides,
        local_targets: bool,
        timeout: Duration,
    ) -> Result<Self, String> {
        let allow_ip = overrides.allow_ip;
        Ok(Self {
            client: egress::webhook_client_for_tests(overrides, !local_targets, timeout)?,
            allow_ip,
            local_targets,
            timeout,
            agent: format!("ferrite/{} webhooks", env!("CARGO_PKG_VERSION")),
            refusals_logged: Mutex::default(),
        })
    }

    fn url_allowed(&self, url: &Url) -> bool {
        let scheme_ok = url.scheme() == "https" || (self.local_targets && url.scheme() == "http");
        let host_ok = match url.host() {
            Some(url::Host::Domain(d)) => !d.is_empty(),
            Some(url::Host::Ipv4(ip)) => (self.allow_ip)(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => (self.allow_ip)(IpAddr::V6(ip)),
            None => false,
        };
        scheme_ok && host_ok && url.username().is_empty() && url.password().is_none()
    }

    fn log_refusal(&self, url: &Url) {
        let host = url.host_str().unwrap_or_default().to_owned();
        let now = Instant::now();
        let mut logged = self
            .refusals_logged
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let due = logged
            .get(&host)
            .is_none_or(|at| now.duration_since(*at) >= REFUSAL_LOG_EVERY);
        if due {
            if logged.len() > 1000 {
                logged.clear();
            }
            logged.insert(host.clone(), now);
            tracing::warn!(host, "webhook refused: address not allowed");
        }
    }

    async fn send_within(&self, request: OutgoingWebhook) -> Result<SendOutcome, &'static str> {
        let mut builder = self
            .client
            .post(request.url.clone())
            .header(USER_AGENT, &self.agent)
            .body(request.body);
        for (name, value) in &request.headers {
            let name = HeaderName::from_static(name);
            let value = HeaderValue::from_str(value).map_err(|_| REQUEST_FAILED)?;
            builder = builder.header(name, value);
        }
        let started = Instant::now();
        let mut response = builder.send().await.map_err(|e| {
            tracing::debug!(error = ?e, "webhook request failed");
            error_class(&e)
        })?;
        if let Some(peer) = response.remote_addr()
            && !(self.allow_ip)(peer.ip())
        {
            return Err(ADDRESS_NOT_ALLOWED);
        }
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let mut body = Vec::with_capacity(1024);
        while body.len() < MAX_RESPONSE_READ {
            match response.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                // A body cut short still has its status: keep what was read.
                Ok(None) | Err(_) => break,
            }
        }
        // Dropping the response closes the connection: nothing more is read.
        drop(response);
        Ok(SendOutcome::Answered {
            status,
            retry_after,
            excerpt: excerpt(&body),
            elapsed: started.elapsed(),
        })
    }
}

/// The first [`MAX_EXCERPT`] bytes as lossy UTF-8, control characters (but newline and
/// tab) replaced by U+FFFD.
pub fn excerpt(body: &[u8]) -> String {
    let cut = &body[..body.len().min(MAX_EXCERPT)];
    String::from_utf8_lossy(cut)
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

/// `Retry-After` as a delay: delta-seconds, or an HTTP date (a past date is zero).
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let delta = at.with_timezone(&chrono::Utc) - chrono::Utc::now();
    Some(delta.to_std().unwrap_or(Duration::ZERO))
}

#[async_trait]
impl WebhookTransport for WebhookClient {
    async fn send(&self, request: OutgoingWebhook) -> SendOutcome {
        let started = Instant::now();
        let url = request.url.clone();
        if !self.url_allowed(&url) {
            self.log_refusal(&url);
            return SendOutcome::Failed {
                class: ADDRESS_NOT_ALLOWED,
                refused: true,
                elapsed: started.elapsed(),
            };
        }
        match tokio::time::timeout(self.timeout, self.send_within(request)).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(class)) => {
                let refused = class == ADDRESS_NOT_ALLOWED;
                if refused {
                    self.log_refusal(&url);
                }
                SendOutcome::Failed {
                    class,
                    refused,
                    elapsed: started.elapsed(),
                }
            }
            Err(_) => SendOutcome::Failed {
                class: TIMED_OUT,
                refused: false,
                elapsed: started.elapsed(),
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds_and_date() {
        assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after(" 7 "), Some(Duration::from_secs(7)));
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(Duration::ZERO)
        );
        let later = (chrono::Utc::now() + chrono::Duration::seconds(600))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let d = parse_retry_after(&later).unwrap();
        assert!(
            d > Duration::from_secs(590) && d <= Duration::from_secs(600),
            "{d:?}"
        );
        assert_eq!(parse_retry_after("soon"), None);
        assert_eq!(parse_retry_after("-5"), None);
    }

    #[test]
    fn excerpt_strips_control_chars() {
        assert_eq!(
            excerpt(b"ok\n\tfine\x1b[31m\x00"),
            "ok\n\tfine\u{FFFD}[31m\u{FFFD}"
        );
        assert_eq!(excerpt(&[b'a'; 5000]).len(), MAX_EXCERPT);
        assert_eq!(excerpt(&[0xff, b'x']), "\u{FFFD}x");
        assert_eq!(excerpt(b"<b>html</b>"), "<b>html</b>");
    }

    #[test]
    fn local_rule_keeps_metadata_refused() {
        let rule = webhook_ip_rule(true);
        for ok in [
            "127.0.0.1",
            "::1",
            "10.0.0.1",
            "192.168.1.2",
            "172.16.0.1",
            "fd12::1",
            "8.8.8.8",
        ] {
            assert!(rule(ok.parse().unwrap()), "{ok}");
        }
        for bad in [
            "169.254.169.254",
            "fe80::1",
            "fd00:ec2::254",
            "100.100.100.200",
            "::ffff:169.254.169.254",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!rule(bad.parse().unwrap()), "{bad}");
        }
        let strict = webhook_ip_rule(false);
        for bad in ["127.0.0.1", "10.0.0.1", "::1", "169.254.169.254"] {
            assert!(!strict(bad.parse().unwrap()), "{bad}");
        }
    }
}
