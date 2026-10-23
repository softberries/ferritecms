//! The client's address, for logs (`server.trusted_proxies`).
//!
//! What is honoured, and only when the TCP peer is inside
//! [`HttpConfig::trusted_proxies`](super::HttpConfig::trusted_proxies):
//!
//! * `X-Forwarded-For`: the client IP is the right-most entry that is not itself a
//!   trusted proxy (entries are read right to left, so a client cannot spoof its
//!   address by sending its own header through the proxy). An entry that does not
//!   parse stops the walk and the last trusted hop is used. If every entry is trusted
//!   the left-most one is the client.
//! * `X-Forwarded-Proto` and `X-Forwarded-Host` are **not used** by anything in M1c-2,
//!   so they are ignored from every peer: absolute URLs are built from `site.url`, and
//!   the cache key's host is pinned to it (ruling R25).
//!
//! From any other peer, every forwarding header is ignored and the peer address is the
//! client. The result is stored as a [`ClientIp`] request extension and logged as
//! `client_ip` on the request span. Requests without a peer address (in-process tests)
//! get no extension.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
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
    // Every X-Forwarded-For line, in order, as one list.
    let entries: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    let mut client = peer;
    for entry in entries.iter().rev() {
        let Some(ip) = parse_entry(entry) else {
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

/// Middleware: inserts [`ClientIp`] when the connection's peer address is known.
pub(crate) async fn record_client(
    State(trusted): State<Arc<[IpNet]>>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() {
        let ip = client_ip(peer.ip(), req.headers(), &trusted);
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
        // An IPv4-mapped peer is matched as IPv4.
        let h = xff(&["5.5.5.5"]);
        assert_eq!(
            client_ip(ip("::ffff:10.0.0.5"), &h, &trusted),
            ip("5.5.5.5")
        );
    }
}
