//! Attachment files fetched from the old WordPress site (`ferrite import wordpress
//! --fetch-media`, M3-1 P13): [`HttpSource`], a [`MediaSource`] over the plugin `http`
//! client's SSRF rules.
//!
//! The client is `egress::media_client`: the plugin client's resolver looks a name up
//! once and refuses the whole answer when any address is not public
//! ([`egress::is_public_ip`]), reqwest connects only to the addresses it returned, and
//! the connected peer is checked again; no proxy, no `Referer`. Unlike plugin fetches,
//! `http` is allowed (uploads of older WordPress sites) and one file may take
//! [`egress::MEDIA_TIMEOUT`] (30 s). Each URL (and each redirect, at most
//! [`egress::MAX_REDIRECTS`]) must be `http(s)`, without credentials; an IP-literal host
//! must pass the address rule itself (reqwest does not resolve it). Any URL may be
//! fetched within those rules: an export names its attachments by absolute URL, often
//! on a CDN.
//!
//! A file that cannot be fetched (refused, unreachable, not 2xx, too many redirects) is
//! missing (`Ok(None)`): the import reports it and keeps the old URL in the items. The
//! size cap is the import's (`max`), enforced while staging.

use std::net::IpAddr;

use async_trait::async_trait;
use ferrite_import_wp::ImportError;
use ferrite_import_wp::run::{ByteStream, MediaSource};
use reqwest::header::{LOCATION, USER_AGENT};
use url::Url;

use crate::plugins::egress::{self, MAX_REDIRECTS, is_public_ip};

/// See the module docs.
pub struct HttpSource {
    client: reqwest::Client,
    allow_ip: fn(IpAddr) -> bool,
    /// `ferrite/<version> import`.
    agent: String,
}

impl HttpSource {
    /// The production source: only public addresses.
    pub fn new(version: &str) -> Result<Self, String> {
        Self::with_rule(version, is_public_ip)
    }

    /// Test seam: every address allowed, so tests can serve files from 127.0.0.1.
    #[cfg(feature = "test-seams")]
    pub fn allowing_private(version: &str) -> Result<Self, String> {
        Self::with_rule(version, |_| true)
    }

    fn with_rule(version: &str, allow_ip: fn(IpAddr) -> bool) -> Result<Self, String> {
        Ok(Self {
            client: egress::media_client(allow_ip)?,
            allow_ip,
            agent: format!("ferrite/{version} import"),
        })
    }

    /// Whether `url` may be requested: `http(s)`, no credentials, a host, and an IP
    /// literal only when the address rule allows it.
    fn allowed(&self, url: &Url) -> bool {
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        match url.host() {
            Some(url::Host::Domain(_)) => true,
            Some(url::Host::Ipv4(ip)) => (self.allow_ip)(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => (self.allow_ip)(IpAddr::V6(ip)),
            None => false,
        }
    }
}

#[async_trait]
impl MediaSource for HttpSource {
    async fn open(
        &self,
        _rel_path: &str,
        url: &str,
        _max: u64,
    ) -> Result<Option<ByteStream>, ImportError> {
        let Ok(mut url) = Url::parse(url.trim()) else {
            return Ok(None);
        };
        for _ in 0..=MAX_REDIRECTS {
            if !self.allowed(&url) {
                tracing::warn!(url = %url, "attachment URL refused (address or scheme rule)");
                return Ok(None);
            }
            let response = match self
                .client
                .get(url.clone())
                .header(USER_AGENT, &self.agent)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(url = %url, error = %e, "attachment fetch failed");
                    return Ok(None);
                }
            };
            if let Some(peer) = response.remote_addr()
                && !(self.allow_ip)(peer.ip())
            {
                tracing::warn!(url = %url, "attachment fetch connected to a refused address");
                return Ok(None);
            }
            let status = response.status();
            if status.is_redirection() {
                let next = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .and_then(|l| url.join(l).ok());
                match next {
                    Some(next) => {
                        url = next;
                        continue;
                    }
                    None => return Ok(None),
                }
            }
            if !status.is_success() {
                tracing::debug!(url = %url, %status, "attachment not fetched");
                return Ok(None);
            }
            let body = futures::stream::unfold(Some(response), |state| async move {
                let mut response = state?;
                match response.chunk().await {
                    Ok(Some(bytes)) => Some((Ok(bytes), Some(response))),
                    Ok(None) => None,
                    Err(e) => Some((Err(std::io::Error::other(e)), None)),
                }
            });
            return Ok(Some(Box::pin(body)));
        }
        tracing::debug!(url = %url, "attachment fetch: too many redirects");
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn urls_follow_the_address_and_scheme_rules() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let source = HttpSource::new("0.0.0").unwrap();
        for ok in [
            "http://example.org/wp-content/uploads/a.png",
            "https://cdn.example.org/a.png",
            "https://93.184.216.34/a.png",
        ] {
            assert!(source.allowed(&Url::parse(ok).unwrap()), "{ok}");
        }
        for refused in [
            "http://127.0.0.1/a.png",
            "http://[::1]/a.png",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/a.png",
            "ftp://example.org/a.png",
            "http://user:pw@example.org/a.png",
            "file:///etc/passwd",
        ] {
            assert!(!source.allowed(&Url::parse(refused).unwrap()), "{refused}");
        }
    }

    /// A name that resolves to an internal address is refused by the resolver: the
    /// file is missing, not fetched.
    #[tokio::test]
    async fn names_of_internal_addresses_are_missing() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let source = HttpSource::new("0.0.0").unwrap();
        let got = source
            .open("a.png", "http://localhost/a.png", 1 << 20)
            .await
            .unwrap();
        assert!(got.is_none());
    }
}
