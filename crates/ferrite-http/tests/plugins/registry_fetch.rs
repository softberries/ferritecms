//! The registry fetcher's rules (M6-2, design 14 §5.3): HTTPS only, bounded redirects,
//! no credentials, public addresses, size caps before and while reading; file
//! registries confined to their folder.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use ferrite_http::plugins::egress::{EgressOverrides, is_public_ip};
use ferrite_http::registry_fetch::{
    FileRegistry, HttpsRegistry, RegistryFetchError, RegistrySource, resolve,
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

const CERT: &[u8] = include_bytes!("../fixtures/tls/cert.pem");
const KEY: &[u8] = include_bytes!("../fixtures/tls/key.pem");
const HOST: &str = "api.example.test";
const T: Duration = Duration::from_secs(10);

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

async fn answer<S: tokio::io::AsyncWrite + Unpin>(io: &mut S, path: &str) {
    let bytes = match path {
        "/index.json" => response(200, &[], b"{\"schema\":1}"),
        "/to-index" => response(302, &[("Location", "/index.json")], b""),
        "/to-http" => response(
            302,
            &[("Location", "http://api.example.test/index.json")],
            b"",
        ),
        "/to-creds" => response(
            302,
            &[("Location", "https://u:p@api.example.test/index.json")],
            b"",
        ),
        "/big-cl" => response(200, &[], &vec![b'x'; 2048]),
        "/big" => {
            let _ = io
                .write_all(b"HTTP/1.1 200 X\r\nConnection: close\r\n\r\n")
                .await;
            let _ = io.write_all(&vec![b'x'; 64 * 1024]).await;
            return;
        }
        p if p.starts_with("/hop/") => match p["/hop/".len()..].parse::<u32>() {
            Ok(0) => response(200, &[], b"landed"),
            Ok(n) => response(302, &[("Location", &format!("/hop/{}", n - 1))], b""),
            Err(_) => response(404, &[], b""),
        },
        _ => response(404, &[], b""),
    };
    let _ = io.write_all(&bytes).await;
}

impl Server {
    async fn start() -> Self {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(CERT)
            .collect::<Result<_, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
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
        let task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tls.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).into_owned();
                    let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
                    answer(&mut tls, &path).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        Self { addr, task }
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("https://{HOST}:{}{path}", self.addr.port())).unwrap()
    }
}

struct Fixed(SocketAddr);

impl Resolve for Fixed {
    fn resolve(&self, _name: Name) -> Resolving {
        let addr = self.0;
        Box::pin(async move { Ok(Box::new(std::iter::once(addr)) as Addrs) })
    }
}

fn fetcher(server: &Server, allow_ip: fn(IpAddr) -> bool) -> HttpsRegistry {
    HttpsRegistry::for_tests(EgressOverrides {
        resolver: Arc::new(Fixed(server.addr)),
        roots: vec![reqwest::Certificate::from_pem(CERT).unwrap()],
        allow_ip,
    })
    .unwrap()
}

fn loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_tls_server_fetch_works() {
    let s = Server::start().await;
    let f = fetcher(&s, loopback);
    let body = f.get(&s.url("/index.json"), 1024, T).await.unwrap();
    assert_eq!(&body[..], b"{\"schema\":1}");
    let body = f.get(&s.url("/to-index"), 1024, T).await.unwrap();
    assert_eq!(&body[..], b"{\"schema\":1}");
    assert_eq!(
        f.get(&s.url("/missing"), 1024, T).await,
        Err(RegistryFetchError::NotFound)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_only() {
    let s = Server::start().await;
    let f = fetcher(&s, loopback);
    let http = Url::parse("http://api.example.test/index.json").unwrap();
    assert!(matches!(
        f.get(&http, 1024, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
    assert!(matches!(
        f.get(&s.url("/to-http"), 1024, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirects_capped_at_three() {
    let s = Server::start().await;
    let f = fetcher(&s, loopback);
    assert_eq!(
        &f.get(&s.url("/hop/3"), 1024, T).await.unwrap()[..],
        b"landed"
    );
    assert!(matches!(
        f.get(&s.url("/hop/4"), 1024, T).await,
        Err(RegistryFetchError::Refused(m)) if m.contains("redirects")
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credentials_refused() {
    let s = Server::start().await;
    let f = fetcher(&s, loopback);
    let creds = Url::parse(&format!("https://u:p@{HOST}:{}/index.json", s.addr.port())).unwrap();
    assert!(matches!(
        f.get(&creds, 1024, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
    assert!(matches!(
        f.get(&s.url("/to-creds"), 1024, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_address_refused() {
    let s = Server::start().await;
    // The production address rule, with a resolver answering 127.0.0.1.
    let f = fetcher(&s, is_public_ip);
    assert!(matches!(
        f.get(&s.url("/index.json"), 1024, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
    let literal = Url::parse("https://10.0.0.1/index.json").unwrap();
    assert!(matches!(
        f.get(&literal, 1024, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_length_over_max_refused_before_body() {
    let s = Server::start().await;
    let f = fetcher(&s, loopback);
    assert_eq!(
        f.get(&s.url("/big-cl"), 1024, T).await,
        Err(RegistryFetchError::TooLarge)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamed_body_over_max_stops() {
    let s = Server::start().await;
    let f = fetcher(&s, loopback);
    assert_eq!(
        f.get(&s.url("/big"), 1024, T).await,
        Err(RegistryFetchError::TooLarge)
    );
}

fn file_url(path: &std::path::Path) -> Url {
    Url::from_file_path(path).unwrap()
}

#[tokio::test]
async fn file_registry_reads_inside_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("seo")).unwrap();
    std::fs::write(dir.path().join("seo/a.wasm"), b"wasm").unwrap();
    let f = FileRegistry::new(dir.path().to_path_buf());
    let index = file_url(&dir.path().join("index.json"));
    let url = resolve(&index, "seo/a.wasm").unwrap();
    assert_eq!(&f.get(&url, 10, T).await.unwrap()[..], b"wasm");
    assert_eq!(f.get(&url, 3, T).await, Err(RegistryFetchError::TooLarge));
    let missing = resolve(&index, "seo/b.wasm").unwrap();
    assert_eq!(
        f.get(&missing, 10, T).await,
        Err(RegistryFetchError::NotFound)
    );
}

#[tokio::test]
async fn file_registry_refuses_dotdot_and_symlink_escape() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("registry");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(outer.path().join("secret"), b"secret").unwrap();
    let f = FileRegistry::new(root.clone());
    let index = file_url(&root.join("index.json"));
    // `..` is refused before any path is formed.
    assert!(resolve(&index, "../secret").is_err());
    // A URL built by hand that walks out is refused after canonicalisation.
    let escape = Url::parse(&format!("{}/../secret", file_url(&root))).unwrap();
    assert!(matches!(
        f.get(&escape, 100, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outer.path().join("secret"), root.join("link")).unwrap();
        let link = resolve(&index, "link").unwrap();
        assert!(matches!(
            f.get(&link, 100, T).await,
            Err(RegistryFetchError::Refused(_))
        ));
    }
}

#[tokio::test]
async fn file_registry_refuses_absolute_urls() {
    let dir = tempfile::tempdir().unwrap();
    let index = file_url(&dir.path().join("index.json"));
    assert!(resolve(&index, "https://example.dev/x.wasm").is_err());
    assert!(resolve(&index, "/etc/passwd").is_err());
    let f = FileRegistry::new(dir.path().to_path_buf());
    let https = Url::parse("https://example.dev/x").unwrap();
    assert!(matches!(
        f.get(&https, 10, T).await,
        Err(RegistryFetchError::Refused(_))
    ));
}
