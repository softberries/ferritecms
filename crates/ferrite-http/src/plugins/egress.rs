//! Outbound HTTPS for the plugin `http` import (M2a-2b Task 11, P16/P17, design 05 §6
//! "HTTP capability details").
//!
//! The plugin host has already checked the request (budget, method, `https` on 443, no
//! userinfo, a listed host, headers, body size). [`Egress::fetch`] checks the URL again
//! (it is public API) and then:
//!
//! * **Resolved addresses.** One reqwest [`Client`](reqwest::Client) per node with a
//!   resolver that looks the name up once and refuses the whole answer when **any**
//!   address is not public ([`is_public_ip`]); reqwest connects to the addresses it
//!   returned (no second lookup, so a rebinding name server cannot swap them), and the
//!   connected peer address is checked again. An IP-literal host is refused outright
//!   (reqwest would not ask the resolver for it).
//! * **Redirects** (pre-flight F27): reqwest follows none; [`Egress::fetch`] follows at
//!   most [`MAX_REDIRECTS`], each `Location` joined to the current URL and checked as
//!   the first URL was (listed host, `https`, port 443, no userinfo, no IP literal).
//!   301/302/303 continue as `GET` without a body (`HEAD` stays `HEAD`), 307/308 keep
//!   the method and body on the same host and are refused to another one; a hop to
//!   another (listed) host keeps only `Accept`/`Accept-Language` of the guest's headers.
//! * **Lookups.** At most [`LOOKUP_SLOTS`] at once per node, each slot held until
//!   `getaddrinfo` returns; one more fails at once ("resolver busy").
//! * **Bounds.** One [`TIMEOUT`] (5 s) for the whole fetch, connects, hops and body
//!   included; the body is read in chunks up to [`MAX_RESPONSE_BODY`]; at most
//!   [`MAX_RESPONSE_HEADERS`] response headers, without `Set-Cookie`.
//! * **Ambient state.** No proxy (environment variables are ignored), no cookie store,
//!   no `Referer`, no decompression (the features are not compiled in); the
//!   `User-Agent` is always `ferrite/<version> plugin/<name>`.
//! * **TLS.** rustls with the platform verifier (P17), falling back to the compiled-in
//!   `webpki-roots` when the OS has no CA roots (ruling R19); test roots only through
//!   `Egress::for_tests` (behind the dev-only `test-seams` feature).
//!
//! Errors reaching the guest name the class only ("address not allowed", "redirect not
//! allowed", "timed out", "TLS error", …), never a resolved address; the details go to
//! the debug log. Address and redirect refusals are logged at `warn` (plugin and class,
//! once a minute per plugin), so an operator sees a plugin probing.

use std::collections::HashMap;
use std::error::Error as StdError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ferrite_plugin_host::{HTTP_METHODS, HttpRequest, HttpResponse, http_header_refused};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, LOCATION, USER_AGENT};
use reqwest::{Method, StatusCode, redirect};
use tokio::sync::Semaphore;
use url::Url;

/// Most redirects one fetch follows.
pub const MAX_REDIRECTS: usize = 3;
/// The whole fetch, connects, redirects and body included (and each connect).
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Largest response body handed to the guest.
pub const MAX_RESPONSE_BODY: usize = 2 * 1024 * 1024;
/// Most response headers handed to the guest.
pub const MAX_RESPONSE_HEADERS: usize = 64;
/// Most DNS lookups running at once on a node (review round 1, Minor 2).
pub const LOOKUP_SLOTS: usize = 16;
/// Guest headers kept on a hop to another (listed) host: nothing that could carry a
/// credential meant for the first host.
const CROSS_HOST_HEADERS: [&str; 2] = ["accept", "accept-language"];
/// How often a plugin's refused fetches are logged at `warn`.
const REFUSAL_LOG_EVERY: Duration = Duration::from_secs(60);

const ADDRESS_NOT_ALLOWED: &str = "address not allowed";
const REDIRECT_NOT_ALLOWED: &str = "redirect not allowed";
const TIMED_OUT: &str = "timed out";
const TLS_ERROR: &str = "TLS error";
const BODY_TOO_LARGE: &str = "response body over 2 MiB";
const CONNECT_FAILED: &str = "connection failed";
const REQUEST_FAILED: &str = "request failed";
const INVALID_HEADER: &str = "invalid request header";
const RESOLVER_BUSY: &str = "resolver busy";

type BoxError = Box<dyn StdError + Send + Sync>;

/// Whether `ip` may be connected to: not loopback, private, link-local (cloud
/// metadata), CGNAT, multicast, broadcast, reserved or documentation space, in IPv4 or
/// in IPv6; IPv4-mapped (`::ffff:0:0/96`), IPv4-translated (SIIT, `::ffff:0:0:0/96`),
/// NAT64 (`64:ff9b::/96`) and 6to4 (`2002::/16`) addresses are judged by the IPv4
/// address they embed.
///
/// Refused IPv4: `0.0.0.0/8 10/8 100.64/10 127/8 169.254/16 172.16/12 192.0.0/24
/// 192.0.2/24 192.88.99/24 192.168/16 198.18/15 198.51.100/24 203.0.113/24 224/4 240/4`.
/// Refused IPv6: `::/96` (unspecified, loopback and the deprecated IPv4-compatible
/// form), `64:ff9b:1::/48`, `100::/64`, `2001::/32`, `2001:2::/48`, `2001:db8::/32`,
/// `3fff::/20`, `fc00::/7`, `fe80::/10`, `fec0::/10`, `ff00::/8`.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

/// (network, prefix length).
const V4_REFUSED: [(u32, u32); 15] = [
    (0x0000_0000, 8),  // 0.0.0.0/8 "this network"
    (0x0a00_0000, 8),  // 10/8 private
    (0x6440_0000, 10), // 100.64/10 CGNAT
    (0x7f00_0000, 8),  // 127/8 loopback
    (0xa9fe_0000, 16), // 169.254/16 link-local, cloud metadata
    (0xac10_0000, 12), // 172.16/12 private
    (0xc000_0000, 24), // 192.0.0/24 IETF protocol assignments
    (0xc000_0200, 24), // 192.0.2/24 TEST-NET-1
    (0xc058_6300, 24), // 192.88.99/24 6to4 relay anycast
    (0xc0a8_0000, 16), // 192.168/16 private
    (0xc612_0000, 15), // 198.18/15 benchmarking
    (0xc633_6400, 24), // 198.51.100/24 TEST-NET-2
    (0xcb00_7100, 24), // 203.0.113/24 TEST-NET-3
    (0xe000_0000, 4),  // 224/4 multicast
    (0xf000_0000, 4),  // 240/4 reserved, broadcast
];

const V6_REFUSED: [(u128, u32); 11] = [
    (0, 96),                           // ::/96
    (0x0064_ff9b_0001_u128 << 80, 48), // 64:ff9b:1::/48 local NAT64
    (0x0100_u128 << 112, 64),          // 100::/64 discard
    (0x2001_0000_u128 << 96, 32),      // 2001::/32 Teredo
    (0x2001_0002_0000_u128 << 80, 48), // 2001:2::/48 benchmarking
    (0x2001_0db8_u128 << 96, 32),      // 2001:db8::/32 documentation
    (0x3fff_u128 << 112, 20),          // 3fff::/20 documentation
    (0xfc00_u128 << 112, 7),           // fc00::/7 unique local
    (0xfe80_u128 << 112, 10),          // fe80::/10 link-local
    (0xfec0_u128 << 112, 10),          // fec0::/10 site-local
    (0xff00_u128 << 112, 8),           // ff00::/8 multicast
];

fn public_v4(ip: Ipv4Addr) -> bool {
    let n = u32::from(ip);
    !V4_REFUSED
        .iter()
        .any(|&(net, len)| n & (u32::MAX << (32 - len)) == net)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let n = u128::from(ip);
    let in_net = |net: u128, len: u32| n & (u128::MAX << (128 - len)) == net;
    // The low 32 bits as IPv4 (truncation intended).
    let low_v4 = Ipv4Addr::from(n as u32);
    if in_net(0xffff_u128 << 32, 96)
        || in_net(0xffff_u128 << 48, 96)
        || in_net(0x0064_ff9b_u128 << 96, 96)
    {
        return public_v4(low_v4);
    }
    if in_net(0x2002_u128 << 112, 16) {
        // 2002:AABB:CCDD::/48 embeds AA.BB.CC.DD (truncation intended).
        return public_v4(Ipv4Addr::from((n >> 80) as u32));
    }
    !V6_REFUSED.iter().any(|&(net, len)| in_net(net, len))
}

/// What the resolver answers when an address is refused (found again in the error
/// chain to name the class).
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(ADDRESS_NOT_ALLOWED)
    }
}

impl StdError for Refused {}

/// What the resolver answers when every lookup slot is taken ([`LOOKUP_SLOTS`]).
#[derive(Debug)]
struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(RESOLVER_BUSY)
    }
}

impl StdError for Busy {}

enum Lookup {
    /// `getaddrinfo` on the blocking pool with port 0, so the URL's port (443) applies
    /// (F30). The lookup slot moves into the blocking task, so it is held until
    /// `getaddrinfo` returns, even when the fetch was dropped.
    System,
    /// A test resolver (its port is kept: the URL names none).
    #[cfg(feature = "test-seams")]
    Custom(Arc<dyn Resolve>),
}

/// The client's resolver: one lookup, every address checked, only checked addresses
/// returned (P16).
/// At most [`LOOKUP_SLOTS`] lookups run at once on a node; one more fails at once
/// ("resolver busy") instead of queueing.
struct CheckedResolver {
    lookup: Lookup,
    allow_ip: fn(IpAddr) -> bool,
    slots: Arc<Semaphore>,
}

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_ip = self.allow_ip;
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            return Box::pin(async { Err(Box::new(Busy) as BoxError) });
        };
        match &self.lookup {
            Lookup::System => {
                let host = name.as_str().to_owned();
                Box::pin(async move {
                    let addrs = tokio::task::spawn_blocking(move || {
                        let _slot = slot;
                        (host.as_str(), 0)
                            .to_socket_addrs()
                            .map(Iterator::collect::<Vec<SocketAddr>>)
                    })
                    .await??;
                    checked(addrs, allow_ip)
                })
            }
            #[cfg(feature = "test-seams")]
            Lookup::Custom(inner) => {
                let answer = inner.resolve(name);
                Box::pin(async move {
                    let _slot = slot;
                    checked(answer.await?.collect(), allow_ip)
                })
            }
        }
    }
}

fn checked(addrs: Vec<SocketAddr>, allow_ip: fn(IpAddr) -> bool) -> Result<Addrs, BoxError> {
    if addrs.is_empty() {
        return Err("no addresses".into());
    }
    if addrs.iter().any(|a| !allow_ip(a.ip())) {
        return Err(Box::new(Refused));
    }
    Ok(Box::new(addrs.into_iter()))
}

/// Test seam: the resolver, the trusted roots (empty: the platform verifier with the
/// `webpki-roots` fallback, as in production) and the address policy.
#[cfg(feature = "test-seams")]
pub struct EgressOverrides {
    pub resolver: Arc<dyn Resolve>,
    pub roots: Vec<reqwest::Certificate>,
    pub allow_ip: fn(IpAddr) -> bool,
}

/// The node's outbound HTTPS client for plugins (see the module docs).
pub struct Egress {
    client: reqwest::Client,
    /// `ferrite/<version>`.
    agent: String,
    allow_ip: fn(IpAddr) -> bool,
    /// When each plugin's last refusal was logged at `warn`.
    refusals_logged: Mutex<HashMap<String, Instant>>,
}

/// Where a client's trusted roots come from.
enum Roots {
    /// The platform verifier (P17): the OS trust store.
    Platform,
    /// The Mozilla roots compiled in through `webpki-roots` (ruling R19 fallback), plus
    /// these (tests only; empty in production).
    WebPki(Vec<rustls::pki_types::CertificateDer<'static>>),
    /// Exactly these (tests).
    #[cfg(feature = "test-seams")]
    Only(Vec<reqwest::Certificate>),
}

impl Egress {
    /// The production client: the system resolver, [`is_public_ip`] and the platform
    /// roots, or, when the platform verifier cannot be built (a Linux host without a CA
    /// bundle), the compiled-in `webpki-roots` with a warning (ruling R19); `version`
    /// goes into the `User-Agent`. An error means neither could be built: `serve` then
    /// leaves `http` unconfigured instead of failing to boot.
    pub fn new(version: &str) -> Result<Self, String> {
        let client = with_fallback(
            || client(Lookup::System, is_public_ip, Roots::Platform),
            || client(Lookup::System, is_public_ip, Roots::WebPki(Vec::new())),
        )?;
        Ok(Self::from_client(version, client, is_public_ip))
    }

    /// Test seam: a client with `overrides` instead of the system resolver, the
    /// platform roots and [`is_public_ip`].
    #[cfg(feature = "test-seams")]
    pub fn for_tests(overrides: EgressOverrides) -> Result<Self, String> {
        let EgressOverrides {
            resolver,
            roots,
            allow_ip,
        } = overrides;
        let client = if roots.is_empty() {
            with_fallback(
                || client(Lookup::Custom(resolver.clone()), allow_ip, Roots::Platform),
                || {
                    client(
                        Lookup::Custom(resolver.clone()),
                        allow_ip,
                        Roots::WebPki(Vec::new()),
                    )
                },
            )?
        } else {
            client(Lookup::Custom(resolver), allow_ip, Roots::Only(roots))?
        };
        Ok(Self::from_client(
            env!("CARGO_PKG_VERSION"),
            client,
            allow_ip,
        ))
    }

    /// Test seam: the `webpki-roots` fallback client (what a host without a CA bundle
    /// gets), trusting `extra_pem` roots as well.
    #[cfg(feature = "test-seams")]
    pub fn for_tests_webpki(
        resolver: Arc<dyn Resolve>,
        allow_ip: fn(IpAddr) -> bool,
        extra_pem: &[u8],
    ) -> Result<Self, String> {
        use rustls::pki_types::{CertificateDer, pem::PemObject};
        let extra = CertificateDer::pem_slice_iter(extra_pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("bad test roots: {e}"))?;
        let client = client(Lookup::Custom(resolver), allow_ip, Roots::WebPki(extra))?;
        Ok(Self::from_client(
            env!("CARGO_PKG_VERSION"),
            client,
            allow_ip,
        ))
    }

    fn from_client(version: &str, client: reqwest::Client, allow_ip: fn(IpAddr) -> bool) -> Self {
        Self {
            client,
            agent: format!("ferrite/{version}"),
            allow_ip,
            refusals_logged: Mutex::new(HashMap::new()),
        }
    }

    /// Logs a refused fetch (an internal address or a redirect off the rules) at `warn`
    /// with the plugin and the class, never the address: at most once per plugin per
    /// [`REFUSAL_LOG_EVERY`]; the others go to `debug`.
    fn log_refusal(&self, plugin: &str, class: &'static str) {
        if self.refusal_due(plugin, Instant::now()) {
            tracing::warn!(
                plugin,
                refusal = class,
                "plugin http request refused (logged at most once a minute per plugin)"
            );
        } else {
            tracing::debug!(plugin, refusal = class, "plugin http request refused");
        }
    }

    /// Whether `plugin`'s refusal at `now` is logged at `warn` (and records it).
    fn refusal_due(&self, plugin: &str, now: Instant) -> bool {
        let mut logged = self
            .refusals_logged
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let due = logged
            .get(plugin)
            .is_none_or(|at| now.duration_since(*at) >= REFUSAL_LOG_EVERY);
        if due {
            // Bounded by the loaded plugins; forget stale entries now and then.
            if logged.len() >= 1024 {
                logged.retain(|_, at| now.duration_since(*at) < REFUSAL_LOG_EVERY);
            }
            logged.insert(plugin.to_owned(), now);
        }
        due
    }

    /// `req` from plugin `plugin`, whose manifest lists `hosts` (canonical names).
    pub async fn fetch(
        &self,
        plugin: &str,
        hosts: &[String],
        req: HttpRequest,
    ) -> Result<HttpResponse, String> {
        match tokio::time::timeout(TIMEOUT, self.fetch_within(plugin, hosts, req)).await {
            Ok(Ok(res)) => Ok(res),
            Ok(Err(class)) => {
                if class == ADDRESS_NOT_ALLOWED || class == REDIRECT_NOT_ALLOWED {
                    self.log_refusal(plugin, class);
                } else {
                    tracing::debug!(plugin, error = class, "plugin http request failed");
                }
                Err(class.to_owned())
            }
            Err(_) => {
                tracing::debug!(plugin, "plugin http request timed out");
                Err(TIMED_OUT.to_owned())
            }
        }
    }

    async fn fetch_within(
        &self,
        plugin: &str,
        hosts: &[String],
        req: HttpRequest,
    ) -> Result<HttpResponse, &'static str> {
        if !HTTP_METHODS.contains(&req.method.as_str()) {
            return Err("method not allowed");
        }
        let mut method =
            Method::from_bytes(req.method.as_bytes()).map_err(|_| "method not allowed")?;
        let mut url = check_url(req.url, hosts)?;
        let mut headers = self.request_headers(plugin, &req.headers)?;
        let mut body = req.body;
        let mut hops = 0;
        loop {
            let mut request = self
                .client
                .request(method.clone(), url.clone())
                .headers(headers.clone());
            if let Some(b) = &body {
                request = request.body(b.clone());
            }
            let res = request.send().await.map_err(|e| classify(plugin, &e))?;
            if let Some(peer) = res.remote_addr()
                && !(self.allow_ip)(peer.ip())
            {
                return Err(ADDRESS_NOT_ALLOWED);
            }
            let status = res.status();
            let location = res.headers().get(LOCATION).cloned();
            match location {
                Some(location) if is_redirect(status) => {
                    hops += 1;
                    if hops > MAX_REDIRECTS {
                        return Err(REDIRECT_NOT_ALLOWED);
                    }
                    let next = location
                        .to_str()
                        .ok()
                        .and_then(|l| url.join(l).ok())
                        .ok_or(REDIRECT_NOT_ALLOWED)?;
                    let next = check_url(next, hosts).map_err(|_| REDIRECT_NOT_ALLOWED)?;
                    let keeps_body = status == StatusCode::TEMPORARY_REDIRECT
                        || status == StatusCode::PERMANENT_REDIRECT;
                    if next.host_str() != url.host_str() {
                        // Review round 1, Minor 3: a 307/308 would re-send the body to
                        // another host, so it is refused; other hops to another listed
                        // host keep only the safe guest headers (and our User-Agent).
                        if keeps_body {
                            return Err(REDIRECT_NOT_ALLOWED);
                        }
                        headers.retain_safe();
                    }
                    if !keeps_body {
                        if method != Method::HEAD {
                            method = Method::GET;
                        }
                        body = None;
                        for name in ["content-type", "content-encoding", "content-language"] {
                            headers.remove(name);
                        }
                    }
                    url = next;
                }
                _ => return read_response(plugin, res, method == Method::HEAD).await,
            }
        }
    }

    /// The guest's headers (checked again: valid names and values, none refused) plus
    /// the `User-Agent`, which the guest cannot replace.
    fn request_headers(
        &self,
        plugin: &str,
        given: &[(String, String)],
    ) -> Result<HeaderMap, &'static str> {
        let mut headers = HeaderMap::with_capacity(given.len() + 1);
        for (name, value) in given {
            if http_header_refused(name) {
                return Err("request header not allowed");
            }
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| INVALID_HEADER)?;
            let value = HeaderValue::from_str(value).map_err(|_| INVALID_HEADER)?;
            if name != USER_AGENT {
                headers.append(name, value);
            }
        }
        let agent = HeaderValue::from_str(&format!("{} plugin/{plugin}", self.agent))
            .map_err(|_| INVALID_HEADER)?;
        headers.insert(USER_AGENT, agent);
        Ok(headers)
    }
}

/// Drops every header but [`CROSS_HOST_HEADERS`] and `User-Agent`.
trait RetainSafe {
    fn retain_safe(&mut self);
}

impl RetainSafe for HeaderMap {
    fn retain_safe(&mut self) {
        let drop: Vec<HeaderName> = self
            .keys()
            .filter(|k| *k != USER_AGENT && !CROSS_HOST_HEADERS.contains(&k.as_str()))
            .cloned()
            .collect();
        for name in drop {
            self.remove(name);
        }
    }
}

/// Builds with `primary`; when that fails, warns and builds with `fallback`.
fn with_fallback(
    primary: impl FnOnce() -> Result<reqwest::Client, String>,
    fallback: impl FnOnce() -> Result<reqwest::Client, String>,
) -> Result<reqwest::Client, String> {
    match primary() {
        Ok(client) => Ok(client),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "the platform certificate verifier is unavailable (no system CA roots?); \
                 plugin http trusts the compiled-in webpki roots instead"
            );
            fallback()
        }
    }
}

/// One plugin client: the checked resolver, no redirects, no proxy, no `Referer`,
/// `https` only, the 5 s bounds and `roots`.
fn client(
    lookup: Lookup,
    allow_ip: fn(IpAddr) -> bool,
    roots: Roots,
) -> Result<reqwest::Client, String> {
    build(lookup, allow_ip, roots, true, TIMEOUT)
}

/// The longest fetch of one WordPress attachment (`media_client`).
pub const MEDIA_TIMEOUT: Duration = Duration::from_secs(30);

/// The client the WordPress import fetches attachments with
/// ([`HttpSource`](crate::import_fetch::HttpSource)): the plugin client's checked
/// resolver (`allow_ip` on every resolved address), no redirects, no proxy, no
/// `Referer`, the platform roots with the `webpki-roots` fallback, but `http` allowed
/// (WordPress sites still serve uploads over it) and [`MEDIA_TIMEOUT`] per file.
pub(crate) fn media_client(allow_ip: fn(IpAddr) -> bool) -> Result<reqwest::Client, String> {
    with_fallback(
        || {
            build(
                Lookup::System,
                allow_ip,
                Roots::Platform,
                false,
                MEDIA_TIMEOUT,
            )
        },
        || {
            build(
                Lookup::System,
                allow_ip,
                Roots::WebPki(Vec::new()),
                false,
                MEDIA_TIMEOUT,
            )
        },
    )
}

/// A client with the checked resolver, no redirects, no proxy and no `Referer`.
fn build(
    lookup: Lookup,
    allow_ip: fn(IpAddr) -> bool,
    roots: Roots,
    https_only: bool,
    timeout: Duration,
) -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .dns_resolver(CheckedResolver {
            lookup,
            allow_ip,
            slots: Arc::new(Semaphore::new(LOOKUP_SLOTS)),
        })
        .redirect(redirect::Policy::none())
        .https_only(https_only)
        .no_proxy()
        .referer(false)
        .connect_timeout(TIMEOUT)
        .timeout(timeout);
    let builder = match roots {
        Roots::Platform => builder,
        #[cfg(feature = "test-seams")]
        Roots::Only(certs) => builder.tls_certs_only(certs),
        Roots::WebPki(extra) => builder.tls_backend_preconfigured(webpki_config(extra)?),
    };
    builder
        .build()
        .map_err(|e| format!("cannot build the outbound http client: {e}"))
}

/// A rustls config trusting `webpki-roots`, with the provider reqwest would pick (the
/// process default, else ring) and the ALPN reqwest sets for its own configs.
fn webpki_config(
    extra: Vec<rustls::pki_types::CertificateDer<'static>>,
) -> Result<rustls::ClientConfig, String> {
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let (_, refused) = roots.add_parsable_certificates(extra);
    if refused > 0 {
        return Err("a test root could not be parsed".into());
    }
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("cannot build the plugin http TLS config: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// The URL rules of the host's check, again, for the first URL and every redirect:
/// `https`, no userinfo, no port but 443, a domain (no IP literal) in `hosts` once made
/// canonical (lowercase, no trailing dot), which the returned URL carries.
fn check_url(mut url: Url, hosts: &[String]) -> Result<Url, &'static str> {
    if url.scheme() != "https" {
        return Err("only https URLs are allowed");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with credentials are not allowed");
    }
    // `url` drops the scheme's default port, so any port left is not 443.
    if url.port().is_some() {
        return Err("only port 443 is allowed");
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => return Err(ADDRESS_NOT_ALLOWED),
        None => return Err("invalid URL"),
    };
    let host = host.strip_suffix('.').unwrap_or(&host).to_owned();
    if !hosts.contains(&host) {
        return Err("host not allowed");
    }
    url.set_host(Some(&host)).map_err(|_| "invalid URL")?;
    Ok(url)
}

/// Headers (no `Set-Cookie`, at most [`MAX_RESPONSE_HEADERS`]) and the body, read in
/// chunks up to [`MAX_RESPONSE_BODY`].
async fn read_response(
    plugin: &str,
    mut res: reqwest::Response,
    head: bool,
) -> Result<HttpResponse, &'static str> {
    let status = res.status().as_u16();
    let headers = res
        .headers()
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "set-cookie" | "set-cookie2"))
        .take(MAX_RESPONSE_HEADERS)
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let mut body = Vec::new();
    if !head {
        if res
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BODY as u64)
        {
            return Err(BODY_TOO_LARGE);
        }
        while let Some(chunk) = res.chunk().await.map_err(|e| classify(plugin, &e))? {
            if body.len() + chunk.len() > MAX_RESPONSE_BODY {
                return Err(BODY_TOO_LARGE);
            }
            body.extend_from_slice(&chunk);
        }
    }
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// The class of a failed request, for the guest; the error itself goes to the debug
/// log.
fn classify(plugin: &str, e: &reqwest::Error) -> &'static str {
    tracing::debug!(plugin, error = ?e, "plugin http request failed");
    if in_chain::<Refused>(e) {
        ADDRESS_NOT_ALLOWED
    } else if in_chain::<Busy>(e) {
        RESOLVER_BUSY
    } else if e.is_timeout() {
        TIMED_OUT
    } else if in_chain::<rustls::Error>(e) {
        TLS_ERROR
    } else if e.is_connect() {
        CONNECT_FAILED
    } else {
        REQUEST_FAILED
    }
}

/// Whether an error of type `T` is in `e`'s source chain, looking inside `io::Error`s
/// too (their `source()` skips the error they wrap).
fn in_chain<T: StdError + 'static>(e: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(e);
    while let Some(err) = current {
        if err.is::<T>() {
            return true;
        }
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
            && in_chain::<T>(inner)
        {
            return true;
        }
        current = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(s: &str) -> bool {
        is_public_ip(s.parse().unwrap_or_else(|e| panic!("{s}: {e}")))
    }

    /// One case per refused range (its first, a middle and its last address where it
    /// matters) and the neighbours just outside, plus well-known public addresses.
    #[test]
    fn public_ip_table() {
        let refused = [
            // IPv4
            "0.0.0.0",
            "0.255.255.255",
            "10.0.0.1",
            "10.255.255.255",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.254",
            "169.254.169.254",
            "169.254.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            // IPv6
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::8.8.8.8",
            "100::1",
            "2001::1",
            "2001:0:ffff::1",
            "2001:2::1",
            "2001:db8::1",
            "2002:7f00:0001::1",
            "2002:c0a8:0101::1",
            "2002:a9fe:a9fe::1",
            "::ffff:0:10.0.0.1",
            "::ffff:0:127.0.0.1",
            "::ffff:0:169.254.169.254",
            "3fff::1",
            "3fff:fff::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "ff02::1",
        ];
        for ip in refused {
            assert!(!public(ip), "{ip} must be refused");
        }
        let allowed = [
            "8.8.8.8",
            "1.1.1.1",
            "9.255.255.255",
            "11.0.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "126.255.255.255",
            "128.0.0.1",
            "169.253.255.255",
            "169.255.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.0.3.1",
            "192.167.255.255",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
            "2606:4700::1111",
            "2a00:1450:4001::200e",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:0808:0808::1",
            "::ffff:0:8.8.8.8",
            "2001:4860:4860::8888",
            "4000::1",
        ];
        for ip in allowed {
            assert!(public(ip), "{ip} must be allowed");
        }
    }

    #[test]
    fn urls_are_checked_and_made_canonical() {
        let hosts = vec!["api.example.test".to_owned()];
        let check = |u: &str| check_url(Url::parse(u).unwrap(), &hosts).map(|u| u.to_string());
        assert_eq!(
            check("https://API.Example.TEST./a?b#c"),
            Ok("https://api.example.test/a?b#c".to_owned())
        );
        assert_eq!(
            check("https://api.example.test:443/"),
            Ok("https://api.example.test/".to_owned())
        );
        assert_eq!(
            check("https://api.example.test:444/"),
            Err("only port 443 is allowed")
        );
        assert_eq!(
            check("http://api.example.test/"),
            Err("only https URLs are allowed")
        );
        assert_eq!(
            check("https://:p@api.example.test/"),
            Err("URLs with credentials are not allowed")
        );
        assert_eq!(check("https://127.1/"), Err(ADDRESS_NOT_ALLOWED));
        assert_eq!(check("https://0x7f.0.0.1/"), Err(ADDRESS_NOT_ALLOWED));
        assert_eq!(check("https://[::ffff:7f00:1]/"), Err(ADDRESS_NOT_ALLOWED));
        assert_eq!(
            check("https://api.example.test.evil/"),
            Err("host not allowed")
        );
        assert_eq!(
            check("https://api.example.test../"),
            Err("host not allowed")
        );
    }

    /// Ruling R19: a platform verifier that cannot be built (no system CA roots) falls
    /// back to `webpki-roots`; when that fails too, the error reaches the caller (which
    /// leaves `http` unconfigured).
    #[test]
    fn roots_fall_back_to_webpki() {
        let used = std::cell::Cell::new(false);
        let fallback = || {
            used.set(true);
            client(Lookup::System, is_public_ip, Roots::WebPki(Vec::new()))
        };
        assert!(with_fallback(|| Err("no CA certificates".into()), fallback).is_ok());
        assert!(used.get(), "the fallback was used");
        let both = with_fallback(|| Err("no CA certificates".into()), || Err("broken".into()));
        assert_eq!(both.err().as_deref(), Some("broken"));
        // A working platform verifier is used as is.
        used.set(false);
        let fallback = || {
            used.set(true);
            client(Lookup::System, is_public_ip, Roots::WebPki(Vec::new()))
        };
        assert!(
            with_fallback(
                || client(Lookup::System, is_public_ip, Roots::WebPki(Vec::new())),
                fallback
            )
            .is_ok()
        );
        assert!(!used.get());
    }

    #[test]
    fn refusals_warn_once_a_minute_per_plugin() {
        let egress = Egress::from_client(
            "0",
            client(Lookup::System, is_public_ip, Roots::WebPki(Vec::new())).unwrap(),
            is_public_ip,
        );
        let t = Instant::now();
        assert!(egress.refusal_due("a", t));
        assert!(!egress.refusal_due("a", t + Duration::from_secs(59)));
        assert!(egress.refusal_due("b", t + Duration::from_secs(1)));
        assert!(egress.refusal_due("a", t + Duration::from_secs(60)));
    }

    #[test]
    fn resolver_refuses_any_non_public_answer() {
        let addr = |s: &str| SocketAddr::new(s.parse().unwrap(), 443);
        assert!(checked(vec![addr("8.8.8.8"), addr("1.1.1.1")], is_public_ip).is_ok());
        let mixed = checked(vec![addr("8.8.8.8"), addr("10.0.0.1")], is_public_ip);
        assert!(mixed.is_err_and(|e| e.is::<Refused>()));
        assert!(checked(Vec::new(), is_public_ip).is_err());
    }
}
