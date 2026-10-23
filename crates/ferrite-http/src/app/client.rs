//! The client's address (`server.trusted_proxies`), for logs **and for the per-IP rate
//! limits** of login and bearer tokens (`login:ip:<hash>`, `token:ip:<hash>`; 07 §2).
//!
//! What is honoured, and only when the TCP peer is inside
//! [`HttpConfig::trusted_proxies`](super::HttpConfig::trusted_proxies):
//!
//! * `X-Forwarded-For`: the client IP is the right-most entry that is not itself a
//!   trusted proxy (entries are read right to left, so a client cannot spoof its
//!   address by sending its own header through the proxy). An entry that does not
//!   parse, or a header line that is not visible ASCII, stops the walk and the last
//!   trusted hop is used. If every entry is trusted the left-most one is the client
//!   (ruling R31: only this header is honoured).
//! * `X-Forwarded-Proto` and `X-Forwarded-Host` are **not used** by anything in M1c-2,
//!   so they are ignored from every peer: absolute URLs are built from `site.url`, and
//!   the cache key's host is pinned to it (ruling R25).
//!
//! From any other peer, every forwarding header is ignored and the peer address is the
//! client. Behind a reverse proxy that is not listed, every request therefore has the
//! proxy's address, and the per-IP limits become one bucket for the whole site (10 bad
//! logins a minute from anyone lock everyone out of logging in). An `X-Forwarded-For`
//! from an untrusted peer is the sign of that misconfiguration, so it is logged as a
//! warning once per peer address (for the first [`MAX_WARNED_PEERS`] peers of the
//! process; M2a-1 R26). The result is stored as a [`ClientIp`] request extension and
//! logged as `client_ip` on the request span. Requests without a peer address
//! (in-process tests) get no extension.

use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use ipnet::IpNet;

/// Request extension: the client's IP address (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

/// The client address of a request from `peer` with `headers`.
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &[IpNet]) -> IpAddr {
    let peer = peer.to_canonical();
    let is_trusted = |ip: &IpAddr| trusted.iter().any(|net| net.contains(ip));
    if !is_trusted(&peer) {
        return peer;
    }
    // Every X-Forwarded-For line, in order, as one list. A line that is not visible
    // ASCII is one unparsable entry (`None`): the walk stops there, it is not skipped.
    let entries: Vec<Option<&str>> = headers
        .get_all("x-forwarded-for")
        .iter()
        .flat_map(|v| match v.to_str() {
            Ok(line) => line.split(',').map(|e| Some(e.trim())).collect::<Vec<_>>(),
            Err(_) => vec![None],
        })
        .collect();
    let mut client = peer;
    for entry in entries.iter().rev() {
        let Some(ip) = entry.and_then(parse_entry) else {
            return client;
        };
        client = ip;
        if !is_trusted(&ip) {
            return ip;
        }
    }
    client
}

/// An `X-Forwarded-For` entry: an IP address, optionally with a port (`1.2.3.4:5`,
/// `[::1]:5`).
fn parse_entry(entry: &str) -> Option<IpAddr> {
    entry
        .parse::<IpAddr>()
        .or_else(|_| entry.parse::<SocketAddr>().map(|s| s.ip()))
        .ok()
        .map(|ip| ip.to_canonical())
}

/// Most distinct untrusted peers whose `X-Forwarded-For` is warned about per process.
pub const MAX_WARNED_PEERS: usize = 64;

/// The state of [`record_client`]: the trusted proxies, and the untrusted peers already
/// warned about.
pub(crate) struct ClientResolver {
    trusted: Arc<[IpNet]>,
    warned: Mutex<HashSet<IpAddr>>,
}

impl ClientResolver {
    pub(crate) fn new(trusted: Arc<[IpNet]>) -> Self {
        Self {
            trusted,
            warned: Mutex::new(HashSet::new()),
        }
    }

    /// Whether a request from `peer` with `headers` should log the misconfigured-proxy
    /// warning: the peer is not trusted, the request has `X-Forwarded-For`, and this
    /// peer was not reported yet (at most [`MAX_WARNED_PEERS`] peers are).
    fn should_warn(&self, peer: IpAddr, headers: &HeaderMap) -> bool {
        let peer = peer.to_canonical();
        if !headers.contains_key("x-forwarded-for")
            || self.trusted.iter().any(|net| net.contains(&peer))
        {
            return false;
        }
        // A poisoned lock only means another thread panicked mid-insert: the set is
        // still a valid set.
        let mut warned = self.warned.lock().unwrap_or_else(|e| e.into_inner());
        warned.len() < MAX_WARNED_PEERS && warned.insert(peer)
    }
}

/// Middleware: inserts [`ClientIp`] when the connection's peer address is known.
pub(crate) async fn record_client(
    State(resolver): State<Arc<ClientResolver>>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() {
        if resolver.should_warn(peer.ip(), req.headers()) {
            tracing::warn!(
                peer = %peer.ip().to_canonical(),
                "X-Forwarded-For from a peer outside server.trusted_proxies is ignored; \
                 if this is your reverse proxy, add it to server.trusted_proxies, or the \
                 per-IP login and token rate limits treat every client as this one address"
            );
        }
        let ip = client_ip(peer.ip(), req.headers(), &resolver.trusted);
        req.extensions_mut().insert(ClientIp(ip));
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn nets(list: &[&str]) -> Vec<IpNet> {
        list.iter().map(|n| n.parse().unwrap()).collect()
    }

    fn xff(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn untrusted_forwarding_is_warned_once_per_peer() {
        let resolver = ClientResolver::new(nets(&["10.0.0.0/8"]).into());
        let h = xff(&["1.1.1.1"]);
        // A trusted proxy, or no header: never.
        assert!(!resolver.should_warn(ip("10.0.0.5"), &h));
        assert!(!resolver.should_warn(ip("9.9.9.9"), &HeaderMap::new()));
        // Once per untrusted peer (IPv4-mapped counts as IPv4).
        assert!(resolver.should_warn(ip("9.9.9.9"), &h));
        assert!(!resolver.should_warn(ip("9.9.9.9"), &h));
        assert!(!resolver.should_warn(ip("::ffff:9.9.9.9"), &h));
        assert!(resolver.should_warn(ip("8.8.8.8"), &h));
        // At most MAX_WARNED_PEERS peers in all.
        for i in 2..MAX_WARNED_PEERS {
            let peer = IpAddr::from([192, 0, 2, u8::try_from(i).unwrap()]);
            assert!(resolver.should_warn(peer, &h), "{peer}");
        }
        assert!(!resolver.should_warn(ip("203.0.113.1"), &h));
    }

    #[test]
    fn untrusted_peer_headers_are_ignored() {
        let h = xff(&["1.1.1.1"]);
        assert_eq!(client_ip(ip("9.9.9.9"), &h, &[]), ip("9.9.9.9"));
        let trusted = nets(&["10.0.0.0/8"]);
        assert_eq!(client_ip(ip("9.9.9.9"), &h, &trusted), ip("9.9.9.9"));
    }

    #[test]
    fn trusted_peer_gives_rightmost_untrusted_entry() {
        let trusted = nets(&["10.0.0.0/8", "192.168.1.1/32"]);
        // The client prepended a fake address; the proxy appended the real one.
        let h = xff(&["6.6.6.6, 2.2.2.2, 192.168.1.1"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("2.2.2.2"));
        // Several header lines are one list.
        let h = xff(&["6.6.6.6", "2.2.2.2"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("2.2.2.2"));
        // No header: the proxy itself.
        assert_eq!(
            client_ip(ip("10.0.0.5"), &HeaderMap::new(), &trusted),
            ip("10.0.0.5")
        );
        // All trusted: the left-most.
        let h = xff(&["10.1.1.1, 10.2.2.2"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("10.1.1.1"));
    }

    #[test]
    fn ports_v6_and_garbage() {
        let trusted = nets(&["10.0.0.0/8"]);
        let h = xff(&["[2001:db8::1]:443"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("2001:db8::1"));
        let h = xff(&["3.3.3.3:8080"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("3.3.3.3"));
        // Garbage stops the walk at the last trusted hop.
        let h = xff(&["4.4.4.4, not-an-ip, 10.9.9.9"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("10.9.9.9"));
        // A non-visible-ASCII line also stops the walk (it is not skipped).
        let mut h = xff(&["4.4.4.4"]);
        h.append(
            "x-forwarded-for",
            HeaderValue::from_bytes(b"\xff 7.7.7.7").unwrap(),
        );
        h.append("x-forwarded-for", HeaderValue::from_static("10.9.9.9"));
        assert_eq!(client_ip(ip("10.0.0.5"), &h, &trusted), ip("10.9.9.9"));
        // An IPv4-mapped peer is matched as IPv4.
        let h = xff(&["5.5.5.5"]);
        assert_eq!(
            client_ip(ip("::ffff:10.0.0.5"), &h, &trusted),
            ip("5.5.5.5")
        );
    }
}
