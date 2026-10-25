//! The real plugin `http` import (M2a-2b Task 11, P16/P17, Review Focus 4): resolved
//! addresses are checked (SSRF, DNS rebinding) and the connection goes to the checked
//! address only, redirects are re-checked per hop, bodies are capped, the call stays
//! volatile and a refused fetch never breaks a page.
//!
//! The listed host `api.example.test` (and `evil.example.test`) is pointed at an in-test
//! TLS server through the resolver seam; its certificate (`tests/fixtures/tls`) is the
//! only trusted root, except where a test checks the platform roots refuse it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferrite_domain::{ContentStatus, Role};
use ferrite_http::plugins::egress::{Egress, EgressOverrides, is_public_ip};
use ferrite_http::{
    HeadPage, HeadSite, PluginReloader, PluginRequest, RenderTrace, StoreHostServices, page_head,
};
use ferrite_plugin_host::{
    HostConfig, HostServices, HttpRequest, HttpResponse, PluginHost, RouteRequest,
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::{HOUR, SITE_URL, Site};
use crate::profile_tests;

profile_tests!(
    fetch_success_path,
    ssrf_targets_are_refused,
    rebinding_answer_is_refused,
    redirects_stay_on_the_allowlist,
    ssrf_render_head_never_breaks_the_page,
);

const LISTED: &str = "api.example.test";
const EVIL: &str = "evil.example.test";
const CERT: &[u8] = include_bytes!("../fixtures/tls/cert.pem");
const KEY: &[u8] = include_bytes!("../fixtures/tls/key.pem");
const MIB: usize = 1024 * 1024;

// ---- the in-test TLS server ----------------------------------------------------------

/// One request the test server read.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A TLS server on `127.0.0.1` answering scripted responses by path (see [`answer`]);
/// one request per connection.
struct TestServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    async fn start() -> Self {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT)
            .collect::<Result<_, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
        // F29: both rustls providers are compiled in and none is installed here.
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (c, s) = (connections.clone(), seen.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                c.fetch_add(1, Ordering::SeqCst);
                let (acceptor, seen) = (acceptor.clone(), s.clone());
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        serve_one(tls, seen).await;
                    }
                });
            }
        });
        Self {
            addr,
            connections,
            seen,
            task,
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn serve_one<S>(mut io: S, seen: Arc<Mutex<Vec<Seen>>>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match io.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        match io.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    let request = Seen {
        method,
        path,
        headers,
        body,
    };
    seen.lock().unwrap().push(request.clone());
    answer(&mut io, &request).await;
    let _ = io.shutdown().await;
}

fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

fn redirect(status: u16, to: &str) -> Vec<u8> {
    response(status, &[("Location", to)], b"")
}

/// Writes a 200 with no `Content-Length` and `body` bytes, then closes.
async fn unsized_body<S: tokio::io::AsyncWrite + Unpin>(io: &mut S, len: usize) {
    let head = b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\n";
    if io.write_all(head).await.is_err() {
        return;
    }
    let block = vec![b'x'; 64 * 1024];
    let mut left = len;
    while left > 0 {
        let n = left.min(block.len());
        if io.write_all(&block[..n]).await.is_err() {
            return;
        }
        left -= n;
    }
}

async fn answer<S: tokio::io::AsyncWrite + Unpin>(io: &mut S, req: &Seen) {
    let bytes = match req.path.as_str() {
        "/ok" => response(
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Set-Cookie", "session=secret"),
                ("X-Test", "yes"),
            ],
            b"hello",
        ),
        "/to-ok" => redirect(302, "/ok"),
        "/to-evil" => redirect(302, "https://evil.example.test/ok"),
        "/to-http" => redirect(302, "http://api.example.test/ok"),
        "/to-port" => redirect(302, "https://api.example.test:8443/ok"),
        "/to-userinfo" => redirect(302, "https://user@api.example.test/ok"),
        "/to-ip" => redirect(302, "https://127.0.0.1/ok"),
        "/post-307" => redirect(307, "/echo"),
        "/post-303" => redirect(303, "/echo"),
        "/post-307-evil" => redirect(307, "https://evil.example.test/echo"),
        "/post-308-evil" => redirect(308, "https://evil.example.test/echo"),
        "/echo" => {
            let mut body = format!("{} ", req.method).into_bytes();
            body.extend_from_slice(&req.body);
            response(200, &[], &body)
        }
        "/many-headers" => {
            let names: Vec<(String, String)> = (0..80)
                .map(|i| (format!("X-H-{i:03}"), i.to_string()))
                .collect();
            let refs: Vec<(&str, &str)> = names
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            response(200, &refs, b"")
        }
        "/big-cl" => response(200, &[], &vec![b'x'; 2 * MIB + 1]),
        "/big" => return unsized_body(io, 2 * MIB + 1).await,
        "/exact" => return unsized_body(io, 2 * MIB).await,
        "/forever" => {
            if io
                .write_all(b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            loop {
                if io.write_all(b"x").await.is_err() || io.flush().await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        p if p.starts_with("/hop/") => match p["/hop/".len()..].parse::<u32>() {
            Ok(0) => response(200, &[], b"landed"),
            Ok(n) => redirect(302, &format!("/hop/{}", n - 1)),
            Err(_) => response(404, &[], b""),
        },
        _ => response(404, &[], b"missing"),
    };
    let _ = io.write_all(&bytes).await;
}

// ---- resolvers and egress ------------------------------------------------------------

/// Answers fixed (replaceable) addresses per name, counting lookups.
struct MapResolver {
    answers: Mutex<HashMap<String, Vec<SocketAddr>>>,
    calls: AtomicUsize,
}

impl MapResolver {
    fn new(answers: &[(&str, Vec<SocketAddr>)]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(
                answers
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone()))
                    .collect(),
            ),
            calls: AtomicUsize::new(0),
        })
    }

    fn set(&self, name: &str, addrs: Vec<SocketAddr>) {
        self.answers.lock().unwrap().insert(name.to_owned(), addrs);
    }
}

impl Resolve for MapResolver {
    fn resolve(&self, name: Name) -> Resolving {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers.lock().unwrap().get(name.as_str()).cloned();
        Box::pin(async move {
            match answer {
                Some(addrs) => Ok(Box::new(addrs.into_iter()) as Addrs),
                None => Err("no such host".into()),
            }
        })
    }
}

/// Answers `first` on the first lookup and `later` on every other one: a rebinding
/// name server.
struct FlipResolver {
    first: SocketAddr,
    later: SocketAddr,
    calls: AtomicUsize,
}

impl Resolve for FlipResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let addr = if n == 0 { self.first } else { self.later };
        Box::pin(async move { Ok(Box::new(std::iter::once(addr)) as Addrs) })
    }
}

fn loopback_only(ip: IpAddr) -> bool {
    ip.is_loopback()
}

fn test_root() -> reqwest::Certificate {
    reqwest::Certificate::from_pem(CERT).unwrap()
}

/// Egress resolving both test names to `server`, trusting the test certificate and
/// letting loopback through (the test server's address).
fn egress_to(server: &TestServer) -> Egress {
    let resolver = MapResolver::new(&[(LISTED, vec![server.addr]), (EVIL, vec![server.addr])]);
    Egress::for_tests(EgressOverrides {
        resolver,
        roots: vec![test_root()],
        allow_ip: loopback_only,
    })
    .unwrap()
}

/// Egress with the real address policy, resolving the listed host to `addrs`.
fn egress_with_policy(resolver: Arc<dyn Resolve>) -> Egress {
    Egress::for_tests(EgressOverrides {
        resolver,
        roots: vec![test_root()],
        allow_ip: is_public_ip,
    })
    .unwrap()
}

fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: url::Url::parse(url).unwrap(),
        headers: Vec::new(),
        body: None,
    }
}

fn hosts(list: &[&str]) -> Vec<String> {
    list.iter().map(|h| (*h).to_owned()).collect()
}

// ---- plugin wiring -------------------------------------------------------------------

/// A plugin host whose services fetch through `egress`, with `names` installed.
async fn egress_host(site: &Site, egress: Egress, names: &[&str]) -> Arc<PluginHost> {
    let services = Arc::new(
        StoreHostServices::new(site.store.clone(), site.reader.clone(), SITE_URL)
            .with_egress(Arc::new(egress)),
    );
    let config = HostConfig {
        render_deadline: Duration::from_secs(5),
        ..HostConfig::default()
    };
    let host = Arc::new(PluginHost::new(config, services).unwrap());
    for name in names {
        site.install(name).await;
    }
    let report = PluginReloader::new(site.store.clone(), site.media.clone(), host.clone())
        .reload()
        .await
        .unwrap();
    let mut loaded = ferrite_http::LoadSummary::from(&report).loaded;
    loaded.sort();
    let mut want: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    want.sort();
    assert_eq!(loaded, want);
    host
}

fn route(query: &[(&str, &str)]) -> RouteRequest {
    RouteRequest {
        method: "GET".into(),
        path: "/get".into(),
        query: query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        headers: Vec::new(),
        body: Vec::new(),
        client_ip_hash: "h".into(),
        user: None,
    }
}

/// `plugin`'s `/get` answer for `query`.
async fn fetch_via(host: &PluginHost, plugin: &str, query: &[(&str, &str)]) -> String {
    let res = host.handle_route(plugin, route(query)).await.unwrap();
    assert_eq!(res.status, 200);
    String::from_utf8(res.body).unwrap()
}

fn url(path: &str) -> String {
    format!("https://{LISTED}{path}")
}

// ---- tests ---------------------------------------------------------------------------

/// Through the test server: status, body and headers (without `Set-Cookie`) reach the
/// guest; the server sees the `User-Agent` naming the plugin. Without an egress the
/// services answer "http is not configured".
async fn fetch_success_path(site: Site) -> Site {
    assert_eq!(
        site.services
            .http_fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
            .await,
        Err("http is not configured".to_owned())
    );
    let server = TestServer::start().await;
    let host = egress_host(&site, egress_to(&server), &["fetcher"]).await;
    assert_eq!(
        fetch_via(&host, "fetcher", &[("u", &url("/ok"))]).await,
        "ok 200 hello"
    );
    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, "/ok");
    assert_eq!(
        seen[0].header("user-agent"),
        Some(format!("ferrite/{} plugin/fetcher", env!("CARGO_PKG_VERSION")).as_str())
    );
    // The plugin cannot pick another User-Agent; its other headers are sent.
    let answer = fetch_via(
        &host,
        "fetcher",
        &[
            ("u", &url("/ok")),
            ("h", "User-Agent:spoofed"),
            ("h", "X-Api-Key:k1"),
        ],
    )
    .await;
    assert_eq!(answer, "ok 200 hello");
    let seen = server.seen();
    assert_eq!(
        seen[1].header("user-agent"),
        Some(format!("ferrite/{} plugin/fetcher", env!("CARGO_PKG_VERSION")).as_str())
    );
    assert_eq!(seen[1].header("x-api-key"), Some("k1"));
    assert!(seen[1].header("cookie").is_none());
    assert!(seen[1].header("referer").is_none());

    // The response headers, straight from the egress.
    let egress = egress_to(&server);
    let res: HttpResponse = egress
        .fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
        .await
        .unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"hello");
    let names: Vec<&str> = res.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"x-test"), "{names:?}");
    assert!(names.contains(&"content-type"), "{names:?}");
    assert!(!names.contains(&"set-cookie"), "{names:?}");
    // At most 64 headers are returned.
    let res = egress
        .fetch("fetcher", &hosts(&[LISTED]), get(&url("/many-headers")))
        .await
        .unwrap();
    assert_eq!(res.headers.len(), 64);
    // HEAD and a POST body.
    let mut post = get(&url("/echo"));
    post.method = "POST".into();
    post.body = Some(b"payload".to_vec());
    let res = egress
        .fetch("fetcher", &hosts(&[LISTED]), post)
        .await
        .unwrap();
    assert_eq!(res.body, b"POST payload");
    site
}

/// Review Focus 4: with the real address policy, the listed host resolving to an
/// internal address is refused before any connection, and the guest's error names the
/// class only.
async fn ssrf_targets_are_refused(site: Site) -> Site {
    let server = TestServer::start().await;
    let port = server.addr.port();
    let resolver = MapResolver::new(&[]);
    let host = egress_host(&site, egress_with_policy(resolver.clone()), &["ssrf"]).await;
    for ip in [
        "127.0.0.1",
        "10.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "64:ff9b::a9fe:a9fe",
        "fe80::1",
        "2002:c0a8:0101::1",
        "fc00::1",
        "0.0.0.0",
    ] {
        resolver.set(LISTED, vec![SocketAddr::new(ip.parse().unwrap(), port)]);
        assert_eq!(
            fetch_via(&host, "ssrf", &[("u", &url("/ok"))]).await,
            "err address not allowed",
            "{ip}"
        );
        assert!(host.failures().is_empty(), "{ip}: {:?}", host.failures());
    }
    assert_eq!(
        server.connections(),
        0,
        "no connection to any internal address"
    );
    site
}

/// Review Focus 4: an answer mixing a public and an internal address is refused as a
/// whole; and the connection uses the one lookup that was checked (a name server that
/// answers differently the second time is never asked again for that request).
async fn rebinding_answer_is_refused(site: Site) -> Site {
    let server = TestServer::start().await;
    let port = server.addr.port();
    let mixed = vec![
        SocketAddr::new("1.1.1.1".parse().unwrap(), port),
        SocketAddr::new("127.0.0.1".parse().unwrap(), port),
    ];
    let resolver = MapResolver::new(&[(LISTED, mixed)]);
    let host = egress_host(&site, egress_with_policy(resolver.clone()), &["ssrf"]).await;
    assert_eq!(
        fetch_via(&host, "ssrf", &[("u", &url("/ok"))]).await,
        "err address not allowed"
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.connections(), 0);

    // Checked once, connected to what was checked: loopback is allowed here, the
    // second answer (10.0.0.1, refused) is never looked up for this request.
    let flip = Arc::new(FlipResolver {
        first: server.addr,
        later: SocketAddr::new("10.0.0.1".parse().unwrap(), port),
        calls: AtomicUsize::new(0),
    });
    let egress = Egress::for_tests(EgressOverrides {
        resolver: flip.clone(),
        roots: vec![test_root()],
        allow_ip: loopback_only,
    })
    .unwrap();
    let res = egress
        .fetch("ssrf", &hosts(&[LISTED]), get(&url("/ok")))
        .await
        .unwrap();
    assert_eq!(res.body, b"hello");
    assert_eq!(flip.calls.load(Ordering::SeqCst), 1);
    // The next request resolves again and gets the refused answer.
    assert_eq!(
        egress
            .fetch("ssrf", &hosts(&[LISTED]), get(&url("/ok")))
            .await,
        Err("address not allowed".to_owned())
    );
    assert_eq!(server.connections(), 1);
    site
}

/// Review Focus 4: every hop is re-checked (listed host, `https`, port 443, no
/// userinfo, no IP literal), at most 3 are followed, and the method and body follow the
/// status rules.
async fn redirects_stay_on_the_allowlist(site: Site) -> Site {
    let server = TestServer::start().await;
    let host = egress_host(&site, egress_to(&server), &["fetcher"]).await;
    for path in [
        "/to-evil",
        "/to-http",
        "/to-port",
        "/to-userinfo",
        "/to-ip",
        "/hop/4",
    ] {
        let before = server.seen().len();
        assert_eq!(
            fetch_via(&host, "fetcher", &[("u", &url(path))]).await,
            "err redirect not allowed",
            "{path}"
        );
        let after = server.seen();
        let followed = &after[before..];
        assert!(
            followed
                .iter()
                .all(|s| s.path != "/ok" && s.path != "/hop/0"),
            "{path}: {followed:?}"
        );
    }
    assert_eq!(
        fetch_via(&host, "fetcher", &[("u", &url("/to-ok"))]).await,
        "ok 200 hello"
    );
    assert_eq!(
        fetch_via(&host, "fetcher", &[("u", &url("/hop/3"))]).await,
        "ok 200 landed"
    );
    // 307 keeps the method and body, 303 turns into a GET without one.
    assert_eq!(
        fetch_via(
            &host,
            "fetcher",
            &[("u", &url("/post-307")), ("m", "POST"), ("b", "3")]
        )
        .await,
        "ok 200 POST xxx"
    );
    assert_eq!(
        fetch_via(
            &host,
            "fetcher",
            &[("u", &url("/post-303")), ("m", "POST"), ("b", "3")]
        )
        .await,
        "ok 200 GET "
    );
    assert!(host.failures().is_empty(), "{:?}", host.failures());

    // A hop to another listed host is followed with only the safe guest headers
    // (review round 1, Minor 3): no `Authorization`, no API key.
    let egress = egress_to(&server);
    let mut req = get(&url("/to-evil"));
    req.headers = vec![
        ("Authorization".into(), "Bearer secret".into()),
        ("X-Api-Key".into(), "k1".into()),
        ("Accept".into(), "text/plain".into()),
    ];
    let res = egress
        .fetch("fetcher", &hosts(&[LISTED, EVIL]), req)
        .await
        .unwrap();
    assert_eq!(res.body, b"hello");
    let seen = server.seen();
    let last = seen.last().unwrap();
    assert_eq!(last.path, "/ok");
    assert_eq!(last.header("host"), Some(EVIL));
    assert!(last.header("authorization").is_none());
    assert!(last.header("x-api-key").is_none());
    assert_eq!(last.header("accept"), Some("text/plain"));
    assert_eq!(
        last.header("user-agent"),
        Some(format!("ferrite/{} plugin/fetcher", env!("CARGO_PKG_VERSION")).as_str())
    );
    // A 307/308 to another host would re-send the body there: refused.
    for path in ["/post-307-evil", "/post-308-evil"] {
        let mut post = get(&url(path));
        post.method = "POST".into();
        post.body = Some(b"SECRET".to_vec());
        let before = server.seen().len();
        assert_eq!(
            egress.fetch("fetcher", &hosts(&[LISTED, EVIL]), post).await,
            Err("redirect not allowed".to_owned()),
            "{path}"
        );
        assert_eq!(
            server.seen().len(),
            before + 1,
            "{path}: /echo never requested"
        );
    }
    site
}

/// The hostile filter's metadata fetch is refused, the page renders and the head is
/// volatile; nothing counts as a plugin failure.
async fn ssrf_render_head_never_breaks_the_page(site: Site) -> Site {
    let server = TestServer::start().await;
    let author = site.user("ada", Role::Author).await;
    let id = site
        .post(
            &author,
            "hello",
            "Hello",
            ContentStatus::Published,
            Some(-HOUR),
        )
        .await;
    let host = egress_host(&site, egress_to(&server), &["ssrf"]).await;
    let item = site.reader.by_id(id).await.unwrap().value.unwrap();
    let req = PluginRequest::public(&item.url, "en", Some(item.item.id));
    let mut trace = RenderTrace::default();
    let html = page_head(
        &host,
        site.store.as_ref(),
        HeadSite {
            title: "Site",
            url: SITE_URL,
            description: "",
        },
        HeadPage {
            item: Some(&item),
            path: &item.url,
        },
        &req,
        &mut trace,
    )
    .await;
    assert!(html.starts_with("<title>Hello – Site</title>"), "{html}");
    assert!(
        html.contains("<meta name=\"ssrf\" content=\"err host not allowed\">"),
        "{html}"
    );
    assert!(trace.volatile, "an http call makes the page volatile");
    assert!(host.failures().is_empty(), "{:?}", host.failures());
    assert!(host.is_enabled("ssrf"));
    assert_eq!(server.connections(), 0);
    site
}

/// Bodies are read in chunks up to 2 MiB: one byte more (with or without a
/// `Content-Length`) is refused, a never-ending body times out at the 5 s deadline.
#[tokio::test]
async fn response_body_is_capped() {
    let server = TestServer::start().await;
    let egress = egress_to(&server);
    let listed = hosts(&[LISTED]);
    let res = egress
        .fetch("fetcher", &listed, get(&url("/exact")))
        .await
        .unwrap();
    assert_eq!(res.body.len(), 2 * MIB);
    for path in ["/big", "/big-cl"] {
        assert_eq!(
            egress.fetch("fetcher", &listed, get(&url(path))).await,
            Err("response body over 2 MiB".to_owned()),
            "{path}"
        );
    }
    let start = Instant::now();
    assert_eq!(
        egress
            .fetch("fetcher", &listed, get(&url("/forever")))
            .await,
        Err("timed out".to_owned())
    );
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(4_500) && took < Duration::from_secs(8),
        "{took:?}"
    );
}

/// The production roots (the platform verifier, or `webpki-roots` on a host without a
/// CA bundle: ruling R19, so this never panics there) do not trust the self-signed test
/// certificate: a TLS error, named by class only.
#[tokio::test]
async fn untrusted_certificate_is_a_tls_error() {
    let server = TestServer::start().await;
    let egress = Egress::for_tests(EgressOverrides {
        resolver: MapResolver::new(&[(LISTED, vec![server.addr])]),
        roots: Vec::new(),
        allow_ip: loopback_only,
    })
    .unwrap();
    assert_eq!(
        egress
            .fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
            .await,
        Err("TLS error".to_owned())
    );
    assert!(server.seen().is_empty());
}

/// The egress re-checks what the host already checked (it is public API): a URL off the
/// list, `http`, another port, userinfo or an IP literal never reaches the resolver.
#[tokio::test]
async fn egress_rechecks_the_url() {
    let resolver = MapResolver::new(&[]);
    let egress = egress_with_policy(resolver.clone());
    let listed = hosts(&[LISTED]);
    for (u, want) in [
        ("https://other.example.test/", "host not allowed"),
        ("http://api.example.test/", "only https URLs are allowed"),
        ("https://api.example.test:8443/", "only port 443 is allowed"),
        (
            "https://u:p@api.example.test/",
            "URLs with credentials are not allowed",
        ),
        ("https://127.0.0.1/", "address not allowed"),
        ("https://[::1]/", "address not allowed"),
    ] {
        assert_eq!(
            egress.fetch("fetcher", &listed, get(u)).await,
            Err(want.to_owned()),
            "{u}"
        );
    }
    let mut bad = get(&url("/"));
    bad.headers = vec![("X-Bad".into(), "a\r\nInjected: 1".into())];
    assert_eq!(
        egress.fetch("fetcher", &listed, bad).await,
        Err("invalid request header".to_owned())
    );
    let mut bad = get(&url("/"));
    bad.method = "TRACE".into();
    assert_eq!(
        egress.fetch("fetcher", &listed, bad).await,
        Err("method not allowed".to_owned())
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
}

/// Ruling R19: the `webpki-roots` fallback client refuses the self-signed certificate,
/// and with it added as a root it completes a fetch (ALPN, HTTP/1.1, the checks).
#[tokio::test]
async fn webpki_fallback_client_works() {
    let server = TestServer::start().await;
    let resolver = || MapResolver::new(&[(LISTED, vec![server.addr])]);
    let plain = Egress::for_tests_webpki(resolver(), loopback_only, b"").unwrap();
    assert_eq!(
        plain
            .fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
            .await,
        Err("TLS error".to_owned())
    );
    let trusting = Egress::for_tests_webpki(resolver(), loopback_only, CERT).unwrap();
    let res = trusting
        .fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
        .await
        .unwrap();
    assert_eq!(
        (res.status, res.body.as_slice()),
        (200, b"hello".as_slice())
    );
}

/// Never answers.
struct HangingResolver {
    calls: AtomicUsize,
}

impl Resolve for HangingResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

/// Review round 1, Minor 2: at most `LOOKUP_SLOTS` lookups at once; one more fails at
/// once instead of queueing behind a hanging name server.
#[tokio::test]
async fn lookups_are_bounded_per_node() {
    use ferrite_http::plugins::egress::LOOKUP_SLOTS;
    let resolver = Arc::new(HangingResolver {
        calls: AtomicUsize::new(0),
    });
    let egress = Arc::new(egress_with_policy(resolver.clone()));
    let tasks: Vec<_> = (0..LOOKUP_SLOTS)
        .map(|_| {
            let egress = egress.clone();
            tokio::spawn(async move {
                egress
                    .fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
                    .await
            })
        })
        .collect();
    let start = Instant::now();
    while resolver.calls.load(Ordering::SeqCst) < LOOKUP_SLOTS {
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "lookups did not start"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let start = Instant::now();
    assert_eq!(
        egress
            .fetch("fetcher", &hosts(&[LISTED]), get(&url("/ok")))
            .await,
        Err("resolver busy".to_owned())
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), LOOKUP_SLOTS);
    for t in tasks {
        t.abort();
    }
}
