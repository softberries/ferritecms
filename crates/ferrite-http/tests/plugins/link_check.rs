//! M6-4 task 7: the external link checker's rules (design 16 §6.3): HEAD then GET,
//! bounded and checked redirects, public addresses only (every hop), default ports,
//! no credentials, the classification, robots.txt, and bodies never read.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrite_core::quality::{LinkOutcome, LinkProbe, RobotsFetch};
use ferrite_http::link_check::LinkChecker;
use ferrite_http::plugins::egress::{EgressOverrides, is_public_ip};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

struct Server {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
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

async fn answer(io: &mut tokio::net::TcpStream, method: &str, path: &str) {
    let bytes = match (method, path) {
        (_, "/ok") => response(200, &[], b""),
        ("HEAD", "/get-only") => response(405, &[], b""),
        ("GET", "/get-only") => response(200, &[], b"fine"),
        (_, "/missing") => response(404, &[], b""),
        (_, "/gone") => response(410, &[], b""),
        (_, "/private") => response(403, &[], b""),
        (_, "/login") => response(401, &[], b""),
        (_, "/slow-down") => response(429, &[], b""),
        (_, "/broken") => response(503, &[], b""),
        (_, "/to-private") => response(302, &[("Location", "http://inside.test/ok")], b""),
        (_, "/to-port") => response(302, &[("Location", "http://site.test:8081/ok")], b""),
        ("HEAD", "/endless") => response(405, &[], b""),
        ("GET", "/endless") => {
            let _ = io
                .write_all(b"HTTP/1.1 200 X\r\nContent-Length: 1073741824\r\n\r\n")
                .await;
            for _ in 0..1000 {
                if io.write_all(&[b'x'; 64 * 1024]).await.is_err() {
                    return;
                }
            }
            return;
        }
        (_, "/never") => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
        (_, p) if p.starts_with("/hop/") => match p["/hop/".len()..].parse::<u32>() {
            Ok(0) => response(200, &[], b""),
            Ok(n) => response(301, &[("Location", &format!("/hop/{}", n - 1))], b""),
            Err(_) => response(404, &[], b""),
        },
        (_, "/robots.txt") => response(200, &[], b"User-agent: *\nDisallow: /secret/\n"),
        _ => response(404, &[], b""),
    };
    let _ = io.write_all(&bytes).await;
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut tcp, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tcp.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).into_owned();
                    let mut words = head.split(' ');
                    let method = words.next().unwrap_or("GET").to_owned();
                    let path = words.next().unwrap_or("/").to_owned();
                    answer(&mut tcp, &method, &path).await;
                    let _ = tcp.shutdown().await;
                });
            }
        });
        Self { addr, task }
    }
}

struct MapResolver(Vec<(String, Vec<SocketAddr>)>);

impl Resolve for MapResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let found = self
            .0
            .iter()
            .find(|(n, _)| n == name.as_str())
            .map(|(_, a)| a.clone());
        Box::pin(async move {
            match found {
                Some(a) => Ok(Box::new(a.into_iter()) as Addrs),
                None => Err("no such host".into()),
            }
        })
    }
}

/// `site.test` is the server (loopback allowed for it alone by the address rule:
/// `127.0.0.1` passes, other private addresses do not); `inside.test` resolves to
/// `10.0.0.1`, `v6.test` to `::1`, `meta.test` to `169.254.169.254`.
fn checker(server: &Server, timeout: Duration) -> LinkChecker {
    fn rule(ip: IpAddr) -> bool {
        ip == IpAddr::from([127, 0, 0, 1]) || is_public_ip(ip)
    }
    let map = vec![
        ("site.test".to_owned(), vec![server.addr]),
        (
            "inside.test".to_owned(),
            vec!["10.0.0.1:80".parse().unwrap()],
        ),
        ("v6.test".to_owned(), vec!["[::1]:80".parse().unwrap()]),
        (
            "meta.test".to_owned(),
            vec!["169.254.169.254:80".parse().unwrap()],
        ),
    ];
    LinkChecker::for_tests(
        EgressOverrides {
            resolver: Arc::new(MapResolver(map)),
            roots: Vec::new(),
            allow_ip: rule,
        },
        "https://example.com",
        timeout,
    )
    .unwrap()
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[tokio::test]
async fn classification() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    for (path, want) in [
        ("/ok", LinkOutcome::Ok(200)),
        ("/missing", LinkOutcome::NotFound(404)),
        ("/gone", LinkOutcome::Gone),
        ("/private", LinkOutcome::Restricted(403)),
        ("/login", LinkOutcome::Restricted(401)),
        ("/slow-down", LinkOutcome::RateLimited),
        ("/broken", LinkOutcome::Failed("http 503".into())),
    ] {
        assert_eq!(
            c.check(&url(&format!("http://site.test{path}"))).await,
            want,
            "{path}"
        );
    }
    assert_eq!(
        c.check(&url("http://nowhere.test/")).await,
        LinkOutcome::NoSuchHost
    );
}

#[tokio::test]
async fn head_then_get_on_405() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    assert_eq!(
        c.check(&url("http://site.test/get-only")).await,
        LinkOutcome::Ok(200)
    );
}

#[tokio::test]
async fn redirect_hops_checked_and_capped_at_5() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    assert_eq!(
        c.check(&url("http://site.test/hop/5")).await,
        LinkOutcome::Ok(200)
    );
    assert_eq!(
        c.check(&url("http://site.test/hop/6")).await,
        LinkOutcome::Failed("too many redirects".into())
    );
    assert_eq!(
        c.check(&url("http://site.test/to-private")).await,
        LinkOutcome::Refused("address")
    );
    assert_eq!(
        c.check(&url("http://site.test/to-port")).await,
        LinkOutcome::Unchecked("port")
    );
}

#[tokio::test]
async fn redirect_to_private_address_refused() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    for host in ["inside.test", "v6.test", "meta.test"] {
        assert_eq!(
            c.check(&url(&format!("http://{host}/x"))).await,
            LinkOutcome::Refused("address"),
            "{host}"
        );
    }
}

#[tokio::test]
async fn ip_literal_private_refused() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    for u in [
        "http://10.1.2.3/",
        "http://[::1]/",
        "http://169.254.169.254/latest",
        "http://192.168.0.1/",
    ] {
        assert_eq!(
            c.check(&url(u)).await,
            LinkOutcome::Refused("address"),
            "{u}"
        );
    }
}

#[tokio::test]
async fn non_default_port_unchecked_and_credentials_refused() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    assert_eq!(
        c.check(&url("http://site.test:8080/ok")).await,
        LinkOutcome::Unchecked("port")
    );
    assert_eq!(
        c.check(&url("https://site.test:8443/ok")).await,
        LinkOutcome::Unchecked("port")
    );
    // A default port spelled out is the default port.
    assert_eq!(
        c.check(&url("http://site.test:80/ok")).await,
        LinkOutcome::Ok(200)
    );
    assert_eq!(
        c.check(&url("http://u:p@site.test/ok")).await,
        LinkOutcome::Refused("credentials")
    );
    assert_eq!(
        c.check(&url("ftp://site.test/ok")).await,
        LinkOutcome::Refused("scheme")
    );
}

#[tokio::test]
async fn timeout_is_failed() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_millis(300));
    let start = Instant::now();
    assert_eq!(
        c.check(&url("http://site.test/never")).await,
        LinkOutcome::Failed("timeout".into())
    );
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn body_never_read() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    let start = Instant::now();
    assert_eq!(
        c.check(&url("http://site.test/endless")).await,
        LinkOutcome::Ok(200)
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn robots_fetched_and_capped() {
    let s = Server::start().await;
    let c = checker(&s, Duration::from_secs(10));
    match c.robots(&url("http://site.test/some/page")).await {
        RobotsFetch::Body(b) => {
            let rules = ferrite_core::quality::robots::parse(&b);
            assert!(!rules.allows("/secret/x"));
            assert!(rules.allows("/ok"));
        }
        other => panic!("{other:?}"),
    }
    // Unknown hosts and refused addresses skip the host.
    assert_eq!(
        c.robots(&url("http://inside.test/")).await,
        RobotsFetch::SkipHost
    );
}
