//! The webhook transport (M6-6 task 4, design 18 §6.1): hostile targets are never
//! connected to (loopback, metadata, private, a name resolving to loopback, a mixed
//! answer, `http`, a redirect to an internal host), responses are bounded (an endless
//! body, 10 MiB, a slow server), and with `local_targets` a local receiver verifies the
//! Standard Webhooks signature with its own HMAC.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use ferrite_core::webhooks::{
    MAX_EXCERPT, OutgoingWebhook, SendOutcome, WebhookTransport, secret, sign,
};
use ferrite_http::plugins::egress::{EgressOverrides, is_local_target_ip, is_public_ip};
use ferrite_http::webhook_send::WebhookClient;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const MIB: usize = 1024 * 1024;

/// One request a receiver read.
#[derive(Debug, Clone, Default)]
struct Got {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Got {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A plain-HTTP receiver on 127.0.0.1 answering by path.
struct Receiver {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    got: Arc<Mutex<Vec<Got>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(io: &mut tokio::net::TcpStream) -> Option<Got> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = io.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = io.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Got {
        path,
        headers,
        body,
    })
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

async fn answer(io: &mut tokio::net::TcpStream, got: &Got, internal: SocketAddr) {
    let bytes = match got.path.as_str() {
        "/ok" => response(204, &[], b""),
        "/busy" => response(503, &[("Retry-After", "120")], b"try later"),
        "/to-internal" => response(302, &[("Location", &format!("http://{internal}/ok"))], b""),
        "/big" => response(200, &[], &vec![b'x'; 10 * MIB]),
        "/forever" => {
            if io
                .write_all(b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            let block = vec![b'y'; 1024];
            loop {
                if io.write_all(&block).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
        _ => response(404, &[], b"missing"),
    };
    let _ = io.write_all(&bytes).await;
}

impl Receiver {
    async fn start() -> Self {
        Self::start_with(None).await
    }

    async fn start_with(internal: Option<SocketAddr>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let internal = internal.unwrap_or(addr);
        let connections = Arc::new(AtomicUsize::new(0));
        let got = Arc::new(Mutex::new(Vec::new()));
        let (c, g) = (connections.clone(), got.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut tcp, _)) = listener.accept().await else {
                    return;
                };
                c.fetch_add(1, Ordering::SeqCst);
                let g = g.clone();
                tokio::spawn(async move {
                    if let Some(req) = read_request(&mut tcp).await {
                        g.lock().unwrap().push(req.clone());
                        answer(&mut tcp, &req, internal).await;
                    }
                });
            }
        });
        Self {
            addr,
            connections,
            got,
            task,
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

/// Answers fixed addresses per name.
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

fn client(
    names: &[(&str, Vec<SocketAddr>)],
    allow_ip: fn(IpAddr) -> bool,
    local_targets: bool,
    timeout: Duration,
) -> WebhookClient {
    let resolver = Arc::new(MapResolver(
        names
            .iter()
            .map(|(n, a)| ((*n).to_owned(), a.clone()))
            .collect(),
    ));
    WebhookClient::for_tests(
        EgressOverrides {
            resolver,
            roots: Vec::new(),
            allow_ip,
        },
        local_targets,
        timeout,
    )
    .unwrap()
}

fn strict(names: &[(&str, Vec<SocketAddr>)]) -> WebhookClient {
    client(names, is_public_ip, false, Duration::from_secs(5))
}

fn local(names: &[(&str, Vec<SocketAddr>)]) -> WebhookClient {
    client(names, is_local_target_ip, true, Duration::from_secs(5))
}

fn request(url: &str) -> OutgoingWebhook {
    OutgoingWebhook {
        url: Url::parse(url).unwrap(),
        headers: vec![
            ("webhook-id", "evt".into()),
            ("webhook-timestamp", "1".into()),
            ("webhook-signature", "v1,x".into()),
            ("content-type", "application/json".into()),
        ],
        body: b"{}".to_vec(),
    }
}

fn refused(out: &SendOutcome) -> bool {
    matches!(
        out,
        SendOutcome::Failed {
            class: "address not allowed",
            refused: true,
            ..
        }
    )
}

#[tokio::test]
async fn loopback_v4_v6_refused() {
    let r = Receiver::start().await;
    let port = r.addr.port();
    let c = strict(&[]);
    for url in [
        format!("https://127.0.0.1:{port}/ok"),
        format!("https://[::1]:{port}/ok"),
        format!("https://[::ffff:127.0.0.1]:{port}/ok"),
    ] {
        let out = c.send(request(&url)).await;
        assert!(refused(&out), "{url}: {out:?}");
    }
    assert_eq!(r.connections(), 0);
}

#[tokio::test]
async fn metadata_ip_refused_even_with_local_targets() {
    let c = local(&[("meta.test", vec!["169.254.169.254:80".parse().unwrap()])]);
    for url in [
        "http://169.254.169.254/latest/meta-data/",
        "http://meta.test/latest",
        "http://[fd00:ec2::254]/latest",
    ] {
        let out = c.send(request(url)).await;
        assert!(refused(&out), "{url}: {out:?}");
    }
}

#[tokio::test]
async fn private_literal_refused() {
    let c = strict(&[]);
    for url in [
        "https://10.0.0.1/",
        "https://192.168.1.1/",
        "https://172.16.0.1/",
    ] {
        assert!(refused(&c.send(request(url)).await), "{url}");
    }
}

#[tokio::test]
async fn name_resolving_to_loopback_refused() {
    let r = Receiver::start().await;
    let c = strict(&[("hooks.example.test", vec![r.addr])]);
    let out = c.send(request("https://hooks.example.test/ok")).await;
    assert!(refused(&out), "{out:?}");
    assert_eq!(r.connections(), 0);
}

#[tokio::test]
async fn mixed_answer_refused() {
    let r = Receiver::start().await;
    let c = strict(&[(
        "hooks.example.test",
        vec!["8.8.8.8:443".parse().unwrap(), r.addr],
    )]);
    let out = c.send(request("https://hooks.example.test/ok")).await;
    assert!(refused(&out), "{out:?}");
    assert_eq!(r.connections(), 0);
}

#[tokio::test]
async fn redirect_not_followed() {
    let internal = Receiver::start().await;
    let r = Receiver::start_with(Some(internal.addr)).await;
    let c = local(&[]);
    let out = c.send(request(&r.url("/to-internal"))).await;
    let SendOutcome::Answered { status, .. } = out else {
        panic!("{out:?}")
    };
    assert_eq!(status, 302);
    assert_eq!(internal.connections(), 0, "the redirect was followed");
}

#[tokio::test]
async fn http_refused_without_local_targets() {
    let r = Receiver::start().await;
    // Even with an address rule that would let loopback through.
    let c = client(&[], |_| true, false, Duration::from_secs(5));
    let out = c.send(request(&r.url("/ok"))).await;
    assert!(refused(&out), "{out:?}");
    assert_eq!(r.connections(), 0);
}

#[tokio::test]
async fn endless_body_bounded_to_4k() {
    let r = Receiver::start().await;
    let c = local(&[]);
    let started = std::time::Instant::now();
    let out = c.send(request(&r.url("/forever"))).await;
    let SendOutcome::Answered {
        status, excerpt, ..
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(status, 200);
    assert_eq!(excerpt.len(), MAX_EXCERPT);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn ten_mib_body_bounded() {
    let r = Receiver::start().await;
    let c = local(&[]);
    let out = c.send(request(&r.url("/big"))).await;
    let SendOutcome::Answered { excerpt, .. } = out else {
        panic!("{out:?}")
    };
    assert_eq!(excerpt, "x".repeat(MAX_EXCERPT));
}

#[tokio::test]
async fn slow_headers_time_out() {
    let r = Receiver::start().await;
    let c = client(&[], is_local_target_ip, true, Duration::from_millis(500));
    let started = std::time::Instant::now();
    let out = c.send(request(&r.url("/slow"))).await;
    assert!(
        matches!(
            out,
            SendOutcome::Failed {
                class: "timed out",
                ..
            }
        ),
        "{out:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn retry_after_is_read() {
    let r = Receiver::start().await;
    let out = local(&[]).send(request(&r.url("/busy"))).await;
    let SendOutcome::Answered {
        status,
        retry_after,
        excerpt,
        ..
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(status, 503);
    assert_eq!(retry_after, Some(Duration::from_secs(120)));
    assert_eq!(excerpt, "try later");
}

/// HMAC-SHA256 by hand (RFC 2104), independent of the `hmac` crate core signs with.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new()
        .chain_update(k.map(|b| b ^ 0x36))
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(k.map(|b| b ^ 0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

#[tokio::test]
async fn local_receiver_verifies_signature() {
    let r = Receiver::start().await;
    let secret = secret::generate().unwrap();
    let key = secret::parse(secret.expose()).unwrap();
    let body = br#"{"id":"evt_1","type":"content.published"}"#.to_vec();
    let ts = 1_700_000_000_i64;
    let sig = sign::header(&[&key], "evt_1", ts, &body);
    let out = local(&[])
        .send(OutgoingWebhook {
            url: Url::parse(&r.url("/ok")).unwrap(),
            headers: vec![
                ("webhook-id", "evt_1".into()),
                ("webhook-timestamp", ts.to_string()),
                ("webhook-signature", sig),
                ("content-type", "application/json".into()),
            ],
            body: body.clone(),
        })
        .await;
    assert!(
        matches!(out, SendOutcome::Answered { status: 204, .. }),
        "{out:?}"
    );
    let got = r.got.lock().unwrap()[0].clone();
    assert_eq!(got.body, body);
    assert!(got.header("user-agent").unwrap().ends_with(" webhooks"));
    assert_eq!(got.header("content-type"), Some("application/json"));
    let id = got.header("webhook-id").unwrap();
    let ts = got.header("webhook-timestamp").unwrap();
    let mut msg = format!("{id}.{ts}.").into_bytes();
    msg.extend_from_slice(&got.body);
    let want = format!("v1,{}", STANDARD.encode(hmac_sha256(&key, &msg)));
    assert_eq!(got.header("webhook-signature"), Some(want.as_str()));
}
