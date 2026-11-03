//! [`LinkChecker`]: the external link checks of the content health run (M6-4, design 16
//! §6.3), over the egress module's checked client (`egress::link_client`).
//!
//! - The resolver refuses a name when any address is not public, reqwest connects only
//!   to the addresses it returned, and the connected peer is checked again; an IP-literal
//!   host must pass the address rule itself. No proxy, cookies or `Referer`.
//! - `http` and `https` only, default ports only (another port is `Unchecked("port")`,
//!   D11), no credentials in the URL.
//! - `HEAD`; `405` or `501` → `GET`, whose body is never read (the response is dropped
//!   after the headers). At most [`MAX_HOPS`] redirects, each hop checked as the first
//!   URL; [`LINK_TIMEOUT`] for the whole check.
//! - `robots.txt`: at most [`MAX_ROBOTS`] bytes read within [`ROBOTS_TIMEOUT`].
//! - Logs name the host only, never the URL.
//!
//! Never called from a request: only the health job holds a checker (D12).

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use ferrite_core::quality::{LinkOutcome, LinkProbe, RobotsFetch, robots::MAX_ROBOTS};
use reqwest::{
    Method, StatusCode,
    header::{LOCATION, USER_AGENT},
};
use url::Url;

use crate::plugins::egress::{self, LINK_TIMEOUT, is_public_ip};

/// Most redirects followed for one URL.
pub const MAX_HOPS: usize = 5;
/// The longest robots.txt fetch.
pub const ROBOTS_TIMEOUT: Duration = Duration::from_secs(5);

/// See the module docs.
pub struct LinkChecker {
    client: reqwest::Client,
    allow_ip: fn(IpAddr) -> bool,
    agent: String,
    timeout: Duration,
}

/// Why a URL is not requested.
enum Skip {
    Refused(&'static str),
    Unchecked(&'static str),
}

impl LinkChecker {
    /// The production checker: only public addresses. The User-Agent is
    /// `ferrite/<version> linkcheck (+<site_url>)`.
    pub fn new(version: &str, site_url: &str) -> Result<Self, String> {
        Ok(Self {
            client: egress::link_client()?,
            allow_ip: is_public_ip,
            agent: agent(version, site_url),
            timeout: LINK_TIMEOUT,
        })
    }

    /// Test seam: `overrides`' resolver, roots and address rule, and `timeout` for one
    /// check.
    #[cfg(feature = "test-seams")]
    pub fn for_tests(
        overrides: egress::EgressOverrides,
        site_url: &str,
        timeout: Duration,
    ) -> Result<Self, String> {
        let allow_ip = overrides.allow_ip;
        Ok(Self {
            client: egress::link_client_for_tests(overrides, timeout)?,
            allow_ip,
            agent: agent("test", site_url),
            timeout,
        })
    }

    /// Whether `url` may be requested at all.
    fn allowed(&self, url: &Url) -> Result<(), Skip> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Skip::Refused("scheme"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Skip::Refused("credentials"));
        }
        if url.port().is_some() {
            // `Url` drops a default port, so any port left is another one.
            return Err(Skip::Unchecked("port"));
        }
        match url.host() {
            Some(url::Host::Domain(_)) => Ok(()),
            Some(url::Host::Ipv4(ip)) if (self.allow_ip)(IpAddr::V4(ip)) => Ok(()),
            Some(url::Host::Ipv6(ip)) if (self.allow_ip)(IpAddr::V6(ip)) => Ok(()),
            Some(_) => Err(Skip::Refused("address")),
            None => Err(Skip::Refused("host")),
        }
    }

    /// One request; the response's headers only.
    async fn send(&self, method: Method, url: &Url) -> Result<reqwest::Response, LinkOutcome> {
        let res = self
            .client
            .request(method, url.clone())
            .header(USER_AGENT, &self.agent)
            .send()
            .await
            .map_err(|e| failure(url, &e))?;
        if let Some(peer) = res.remote_addr()
            && !(self.allow_ip)(peer.ip())
        {
            return Err(LinkOutcome::Refused("address"));
        }
        Ok(res)
    }

    /// Follows redirects from `url` with `method`; the final response (its body unread).
    async fn follow(
        &self,
        url: &Url,
        method: Method,
    ) -> Result<(reqwest::Response, Url), LinkOutcome> {
        let mut url = url.clone();
        for _ in 0..=MAX_HOPS {
            self.allowed(&url).map_err(skip_outcome)?;
            let res = self.send(method.clone(), &url).await?;
            let status = res.status();
            if status.is_redirection()
                && let Some(next) = res
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .and_then(|l| url.join(l).ok())
            {
                url = next;
                continue;
            }
            return Ok((res, url));
        }
        Err(LinkOutcome::Failed("too many redirects".into()))
    }

    async fn check_within(&self, url: &Url) -> LinkOutcome {
        let (res, _) = match self.follow(url, Method::HEAD).await {
            Ok(r) => r,
            Err(o) => return o,
        };
        let res = match res.status() {
            StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED => {
                drop(res);
                match self.follow(url, Method::GET).await {
                    Ok((r, _)) => r,
                    Err(o) => return o,
                }
            }
            _ => res,
        };
        let code = res.status().as_u16();
        // The body is never read.
        drop(res);
        classify(code)
    }
}

fn agent(version: &str, site_url: &str) -> String {
    format!("ferrite/{version} linkcheck (+{site_url})")
}

fn skip_outcome(s: Skip) -> LinkOutcome {
    match s {
        Skip::Refused(why) => LinkOutcome::Refused(why),
        Skip::Unchecked(why) => LinkOutcome::Unchecked(why),
    }
}

/// The outcome of a final status code.
fn classify(code: u16) -> LinkOutcome {
    match code {
        200..=399 => LinkOutcome::Ok(code),
        401 | 403 => LinkOutcome::Restricted(code),
        404 => LinkOutcome::NotFound(code),
        410 => LinkOutcome::Gone,
        429 => LinkOutcome::RateLimited,
        c => LinkOutcome::Failed(format!("http {c}")),
    }
}

/// A failed request: refused, no such host, or its class. Logged with the host only.
fn failure(url: &Url, e: &reqwest::Error) -> LinkOutcome {
    let host = url.host_str().unwrap_or("?");
    if egress::address_refused(e) {
        tracing::debug!(host, "link check: address not allowed");
        return LinkOutcome::Refused("address");
    }
    let text = format!("{:?}", e.without_url_ref());
    if e.is_connect()
        && (text.contains("lookup")
            || text.contains("dns")
            || text.contains("no addresses")
            || text.contains("no such host"))
    {
        tracing::debug!(host, "link check: no such host");
        return LinkOutcome::NoSuchHost;
    }
    let class = if e.is_timeout() {
        "timeout"
    } else {
        egress::error_class(e)
    };
    tracing::debug!(host, class, "link check failed");
    LinkOutcome::Failed(class.to_owned())
}

/// `reqwest::Error::without_url` takes the error by value; this formats a borrowed one
/// without its URL.
trait WithoutUrlRef {
    fn without_url_ref(&self) -> String;
}

impl WithoutUrlRef for reqwest::Error {
    fn without_url_ref(&self) -> String {
        let mut out = String::new();
        let mut cur: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(self);
        while let Some(e) = cur {
            out.push_str(&e.to_string().to_ascii_lowercase());
            out.push(' ');
            cur = e.source();
        }
        out
    }
}

#[async_trait]
impl LinkProbe for LinkChecker {
    async fn check(&self, url: &Url) -> LinkOutcome {
        let mut url = url.clone();
        url.set_fragment(None);
        if let Err(s) = self.allowed(&url) {
            return skip_outcome(s);
        }
        match tokio::time::timeout(self.timeout, self.check_within(&url)).await {
            Ok(o) => o,
            Err(_) => {
                tracing::debug!(host = url.host_str().unwrap_or("?"), "link check timed out");
                LinkOutcome::Failed("timeout".into())
            }
        }
    }

    async fn robots(&self, url: &Url) -> RobotsFetch {
        let mut robots = url.clone();
        robots.set_path("/robots.txt");
        robots.set_query(None);
        robots.set_fragment(None);
        let fetch = async {
            let (mut res, _) = match self.follow(&robots, Method::GET).await {
                Ok(r) => r,
                Err(_) => return RobotsFetch::SkipHost,
            };
            let status = res.status();
            if status.is_client_error() {
                return RobotsFetch::AllowAll;
            }
            if !status.is_success() {
                return RobotsFetch::SkipHost;
            }
            let mut body = Vec::new();
            loop {
                match res.chunk().await {
                    Ok(Some(chunk)) => {
                        let room = MAX_ROBOTS - body.len();
                        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
                        if body.len() >= MAX_ROBOTS {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => return RobotsFetch::SkipHost,
                }
            }
            RobotsFetch::Body(body)
        };
        let timeout = ROBOTS_TIMEOUT.min(self.timeout);
        tokio::time::timeout(timeout, fetch)
            .await
            .unwrap_or(RobotsFetch::SkipHost)
    }
}
