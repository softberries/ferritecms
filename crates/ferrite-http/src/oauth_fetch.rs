//! Client ID Metadata Documents fetched for the OAuth authorization server (design 12
//! §3.2, §7): [`OAuthFetcher`], the [`ClientMetadataFetcher`] core's `OAuthService`
//! calls when an authorization request names an `https` client id it has not cached.
//!
//! The rules are the plugin egress client's ([`egress`]):
//!
//! * **Addresses.** The checked resolver looks a name up once and refuses the whole
//!   answer when any address is not public ([`is_public_ip`]); reqwest connects only to
//!   the addresses it returned and the connected peer is checked again. An IP-literal
//!   host must pass the address rule itself (reqwest does not resolve it).
//! * **URLs.** `https` only, without user information, for the first URL and every
//!   redirect; at most [`MAX_REDIRECTS`] redirects (`Location` joined to the current
//!   URL).
//! * **Bounds.** One [`TIMEOUT`] (5 s) for the whole fetch, redirects and body included.
//!   A `Content-Length` over [`MAX_METADATA_DOCUMENT`] (16 KiB) is refused before the
//!   body is read; otherwise the body is read in chunks and the read stops as soon as
//!   it passes the cap ([`FetchError::TooLarge`]).
//! * **Ambient state.** No proxy, no cookie store, no `Referer`, no `Authorization`; the
//!   request sends `Accept: application/json` and `User-Agent: ferrite/<version> oauth`.
//!
//! A refused address, scheme, credential or redirect is [`FetchError::Refused`]; a
//! network error, timeout or a status other than 2xx is [`FetchError::Failed`]. Core
//! then keeps a cached copy for up to 7 days or shows the authorization error page.

use std::net::IpAddr;
use std::time::Instant;

use async_trait::async_trait;
use ferrite_core::oauth::{ClientMetadataFetcher, FetchError, MAX_METADATA_DOCUMENT};
use reqwest::header::{ACCEPT, LOCATION, USER_AGENT};
use url::Url;

use crate::plugins::egress::{self, MAX_REDIRECTS, TIMEOUT, is_public_ip};

/// See the module docs.
pub struct OAuthFetcher {
    client: reqwest::Client,
    allow_ip: fn(IpAddr) -> bool,
    /// `ferrite/<version> oauth`.
    agent: String,
}

impl OAuthFetcher {
    /// The production fetcher: the system resolver, [`is_public_ip`], the platform roots
    /// (with the `webpki-roots` fallback); `version` goes into the `User-Agent`.
    pub fn new(version: &str) -> Result<Self, String> {
        Ok(Self {
            client: egress::metadata_client()?,
            allow_ip: is_public_ip,
            agent: format!("ferrite/{version} oauth"),
        })
    }

    /// Test seam: `overrides`' resolver, trusted roots and address rule.
    #[cfg(feature = "test-seams")]
    pub fn for_tests(overrides: egress::EgressOverrides) -> Result<Self, String> {
        let allow_ip = overrides.allow_ip;
        Ok(Self {
            client: egress::metadata_client_for_tests(overrides)?,
            allow_ip,
            agent: format!("ferrite/{} oauth", env!("CARGO_PKG_VERSION")),
        })
    }

    /// Whether `url` may be requested: `https`, no user information, a host, and an IP
    /// literal only when the address rule allows it.
    fn check(&self, url: &Url) -> Result<(), FetchError> {
        if url.scheme() != "https" {
            return Err(FetchError::Refused("only https URLs are fetched".into()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(FetchError::Refused(
                "URLs with credentials are refused".into(),
            ));
        }
        let allowed = match url.host() {
            Some(url::Host::Domain(_)) => true,
            Some(url::Host::Ipv4(ip)) => (self.allow_ip)(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => (self.allow_ip)(IpAddr::V6(ip)),
            None => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(FetchError::Refused("address not allowed".into()))
        }
    }

    async fn fetch_within(&self, first: &Url, deadline: Instant) -> Result<Vec<u8>, FetchError> {
        let mut url = first.clone();
        let mut hops = 0;
        loop {
            self.check(&url)?;
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(FetchError::Failed("timed out".into()));
            }
            let mut response = self
                .client
                .get(url.clone())
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, &self.agent)
                .timeout(left)
                .send()
                .await
                .map_err(|e| request_error(&e))?;
            if let Some(peer) = response.remote_addr()
                && !(self.allow_ip)(peer.ip())
            {
                return Err(FetchError::Refused("address not allowed".into()));
            }
            let status = response.status();
            if status.is_redirection() {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(FetchError::Refused("too many redirects".into()));
                }
                url = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .and_then(|l| url.join(l).ok())
                    .ok_or_else(|| FetchError::Refused("redirect without a location".into()))?;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::Failed(format!("status {}", status.as_u16())));
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_METADATA_DOCUMENT as u64)
            {
                return Err(FetchError::TooLarge);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| request_error(&e))? {
                if body.len() + chunk.len() > MAX_METADATA_DOCUMENT {
                    // Dropping the response closes the connection: nothing more is read.
                    return Err(FetchError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(body);
        }
    }
}

#[async_trait]
impl ClientMetadataFetcher for OAuthFetcher {
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
        let deadline = Instant::now() + TIMEOUT;
        match tokio::time::timeout(TIMEOUT, self.fetch_within(url, deadline)).await {
            Ok(result) => result,
            Err(_) => Err(FetchError::Failed("timed out".into())),
        }
    }
}

/// A failed request as a [`FetchError`]: a refused address is `Refused`, the rest
/// `Failed` (the details go to the debug log).
fn request_error(e: &reqwest::Error) -> FetchError {
    tracing::debug!(error = ?e, "client metadata fetch failed");
    if egress::address_refused(e) {
        FetchError::Refused("address not allowed".into())
    } else if e.is_timeout() {
        FetchError::Failed("timed out".into())
    } else {
        FetchError::Failed("request failed".into())
    }
}

/// A fetcher that fetches nothing: what a node uses when no outbound TLS client could
/// be built. Every metadata-document client then needs a cached copy.
pub struct NoFetcher;

#[async_trait]
impl ClientMetadataFetcher for NoFetcher {
    async fn fetch(&self, _url: &Url) -> Result<Vec<u8>, FetchError> {
        Err(FetchError::Failed(
            "metadata documents cannot be fetched on this node".into(),
        ))
    }
}
