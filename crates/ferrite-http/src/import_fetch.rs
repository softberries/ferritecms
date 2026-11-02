//! Attachment files fetched from the old WordPress site (`ferrite import wordpress
//! --fetch-media`, M3-1 P13): [`HttpSource`], a [`MediaSource`] over the plugin `http`
//! client's SSRF rules.
//!
//! The client is `egress::media_client`: the plugin client's resolver looks a name up
//! once and refuses the whole answer when any address is not public
//! ([`egress::is_public_ip`]), reqwest connects only to the addresses it returned, and
//! the connected peer is checked again; no proxy, no `Referer`. Unlike plugin fetches,
//! `http` is allowed (uploads of older WordPress sites) and one file may take
//! [`egress::MEDIA_TIMEOUT`] (30 s), redirects and body included. Each URL (and each
//! redirect, at most [`egress::MAX_REDIRECTS`]) must be `http(s)`, without credentials;
//! a redirect may not go from `https` to `http`; an IP-literal host must pass the
//! address rule itself (reqwest does not resolve it). A `Content-Length` over the cap is
//! refused before the body is read (`MediaError::TooLarge`). Any URL may be
//! fetched within those rules: an export names its attachments by absolute URL, often
//! on a CDN.
//!
//! A file that cannot be fetched (refused, unreachable, not 2xx, too many redirects) is
//! missing (`Ok(None)`): the import reports it and keeps the old URL in the items. The
//! size cap is the import's (`max`), also enforced while staging.
//!
//! The MCP `media_upload` tool fetches a URL with the same client and rules but `https`
//! only ([`HttpSource::for_upload`]): `http` is refused as the first URL and a redirect
//! to it as any other refused hop.

use std::net::IpAddr;
use std::time::Instant;

use async_trait::async_trait;
use ferrite_import_wp::ImportError;
use ferrite_import_wp::run::{ByteStream, MediaSource};
use ferrite_media::MediaError;
use reqwest::header::{LOCATION, USER_AGENT};
use url::Url;

use crate::plugins::egress::{self, MAX_REDIRECTS, MEDIA_TIMEOUT, is_public_ip};

/// See the module docs.
pub struct HttpSource {
    client: reqwest::Client,
    allow_ip: fn(IpAddr) -> bool,
    /// Plain `http` is refused (uploads by URL); the import allows it.
    https_only: bool,
    /// `ferrite/<version> import`.
    agent: String,
    /// Log lines name the host only, never the URL (uploads by URL: a URL given by an
    /// assistant may carry a token in its path or query). The import logs full URLs.
    host_only_logs: bool,
}

impl HttpSource {
    /// The production source: only public addresses.
    pub fn new(version: &str) -> Result<Self, String> {
        Self::with_rule(version, is_public_ip, false)
    }

    /// The source of uploads by URL (MCP `media_upload`): only public addresses and
    /// only `https`.
    pub fn for_upload(version: &str) -> Result<Self, String> {
        Self::with_rule(version, is_public_ip, true)
    }

    /// Test seam: every address allowed, so tests can serve files from 127.0.0.1.
    #[cfg(feature = "test-seams")]
    pub fn allowing_private(version: &str) -> Result<Self, String> {
        Self::with_rule(version, |_| true, false)
    }

    fn with_rule(
        version: &str,
        allow_ip: fn(IpAddr) -> bool,
        https_only: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            client: egress::media_client(allow_ip)?,
            allow_ip,
            https_only,
            agent: format!("ferrite/{version} import"),
            host_only_logs: https_only,
        })
    }

    /// Log lines name the host only (see the field). `for_upload` sources already do.
    #[cfg(feature = "test-seams")]
    pub fn with_host_only_logs(mut self) -> Self {
        self.host_only_logs = true;
        self
    }

    /// What log lines say about `url`: the URL, or only its host.
    fn shown(&self, url: &Url) -> String {
        if self.host_only_logs {
            url.host_str().unwrap_or("?").to_owned()
        } else {
            url.to_string()
        }
    }

    /// `e` for a log line: reqwest errors carry the URL, which host-only logs drop.
    fn shown_error(&self, e: reqwest::Error) -> String {
        if self.host_only_logs {
            e.without_url().to_string()
        } else {
            e.to_string()
        }
    }

    /// Whether `url` may be requested: `http(s)`, no credentials, a host, and an IP
    /// literal only when the address rule allows it.
    fn allowed(&self, url: &Url) -> bool {
        let scheme_ok = match url.scheme() {
            "https" => true,
            "http" => !self.https_only,
            _ => false,
        };
        if !scheme_ok || !url.username().is_empty() || url.password().is_some() {
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

/// Whether a redirect from `from` to `to` may be followed: to `http(s)`, and never from
/// `https` down to `http`.
fn redirect_allowed(from: &Url, to: &Url) -> bool {
    matches!(
        (from.scheme(), to.scheme()),
        ("https", "https") | ("http", "http" | "https")
    )
}

#[async_trait]
impl MediaSource for HttpSource {
    async fn open(
        &self,
        _rel_path: &str,
        url: &str,
        max: u64,
    ) -> Result<Option<ByteStream>, ImportError> {
        let Ok(mut url) = Url::parse(url.trim()) else {
            return Ok(None);
        };
        // One deadline for the file: every hop, and the body, get what is left of it.
        let deadline = Instant::now() + MEDIA_TIMEOUT;
        for _ in 0..=MAX_REDIRECTS {
            if !self.allowed(&url) {
                tracing::warn!(url = %self.shown(&url), "attachment URL refused (address or scheme rule)");
                return Ok(None);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let response = match self
                .client
                .get(url.clone())
                .header(USER_AGENT, &self.agent)
                .timeout(left)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(url = %self.shown(&url), error = %self.shown_error(e), "attachment fetch failed");
                    return Ok(None);
                }
            };
            if let Some(peer) = response.remote_addr()
                && !(self.allow_ip)(peer.ip())
            {
                tracing::warn!(url = %self.shown(&url), "attachment fetch connected to a refused address");
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
                    Some(next) if redirect_allowed(&url, &next) => {
                        url = next;
                        continue;
                    }
                    _ => {
                        tracing::warn!(url = %self.shown(&url), "attachment redirect refused");
                        return Ok(None);
                    }
                }
            }
            if !status.is_success() {
                tracing::debug!(url = %self.shown(&url), %status, "attachment not fetched");
                return Ok(None);
            }
            if response.content_length().is_some_and(|n| n > max) {
                return Err(ImportError::Media(MediaError::TooLarge { limit: max }));
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
        tracing::debug!(url = %self.shown(&url), "attachment fetch: too many redirects");
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Upload sources log the host only; the import's source logs full URLs.
    #[test]
    fn upload_logs_name_the_host_only() {
        let u = Url::parse("https://cdn.example.org/a/b.png?token=secret").unwrap();
        let upload = HttpSource::for_upload("0.0.0").unwrap();
        assert_eq!(upload.shown(&u), "cdn.example.org");
        let import = HttpSource::new("0.0.0").unwrap();
        assert_eq!(import.shown(&u), u.as_str());
    }

    /// An upload source refuses plain `http` as well; the import's source does not.
    #[test]
    fn upload_urls_are_https_only() {
        let source = HttpSource::for_upload("0.0.0").unwrap();
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(source.allowed(&u("https://cdn.example.org/a.png")));
        assert!(!source.allowed(&u("http://cdn.example.org/a.png")));
        assert!(!source.allowed(&u("https://127.0.0.1/a.png")));
        assert!(!source.allowed(&u("https://user:pw@example.org/a.png")));
    }

    #[test]
    fn urls_follow_the_address_and_scheme_rules() {
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

    /// A redirect may not go from `https` to `http`, nor to another scheme.
    #[test]
    fn redirects_keep_https() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(redirect_allowed(
            &u("https://a.test/x"),
            &u("https://b.test/y")
        ));
        assert!(redirect_allowed(
            &u("http://a.test/x"),
            &u("https://a.test/x")
        ));
        assert!(redirect_allowed(
            &u("http://a.test/x"),
            &u("http://b.test/x")
        ));
        assert!(!redirect_allowed(
            &u("https://a.test/x"),
            &u("http://a.test/x")
        ));
        assert!(!redirect_allowed(
            &u("http://a.test/x"),
            &u("ftp://a.test/x")
        ));
    }

    /// A file whose `Content-Length` is over the cap is refused before its body is read
    /// (too large, not missing).
    #[tokio::test]
    async fn content_length_over_the_cap_is_refused_early() {
        let app =
            axum::Router::new().route("/big.png", axum::routing::get(|| async { vec![0u8; 4096] }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let source = HttpSource::allowing_private("0.0.0").unwrap();
        let url = format!("http://{addr}/big.png");
        let err = source.open("big.png", &url, 100).await.err().unwrap();
        assert!(
            matches!(
                err,
                ImportError::Media(ferrite_media::MediaError::TooLarge { limit: 100 })
            ),
            "{err:?}"
        );
        assert!(
            source
                .open("big.png", &url, 1 << 20)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// A name that resolves to an internal address is refused by the resolver: the
    /// file is missing, not fetched.
    #[tokio::test]
    async fn names_of_internal_addresses_are_missing() {
        let source = HttpSource::new("0.0.0").unwrap();
        let got = source
            .open("a.png", "http://localhost/a.png", 1 << 20)
            .await
            .unwrap();
        assert!(got.is_none());
    }
}
