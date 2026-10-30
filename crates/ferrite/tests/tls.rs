#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The built-in TLS (M4-2 P33) on in-process solo nodes: HSTS only on the TLS listener,
//! the redirect listener, the `<data>/tls/` file modes, and a real ACME run against
//! Pebble in Docker (`acme_against_pebble_serves_https`, only with
//! `FERRITE_TEST_PEBBLE=1`; the image is pinned by digest).

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrite::tls::{TlsCache, TlsSeams};
use ferrite::{NodeOptions, OpenMode, RunningNode, open_store, start};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::sign::CertifiedKey;
use rustls_acme::{AccountCache, CertCache};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SITE_HOST: &str = "site.test";
const PEBBLE_IMAGE: &str = "ghcr.io/letsencrypt/pebble@sha256:ddf230642b1a584f519f32e347de1b05a6e4c1f6c35c1863b33effeab5f78199";

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A solo data folder with `[server.tls]` on; `extra` is appended to the TLS section.
fn site(dir: &Path, site_url: &str, extra: &str) -> ferrite_config::Config {
    let toml = format!(
        "[site]\nurl = \"{site_url}\"\n[server]\nlisten = \"127.0.0.1:0\"\n\
         [server.tls]\nmode = \"acme\"\ncontact = \"mailto:ops@{SITE_HOST}\"\n{extra}"
    );
    std::fs::write(dir.join("ferrite.toml"), toml).unwrap();
    let env = vec![("FERRITE_AUTH__SECRET_KEY".to_owned(), "ab".repeat(32))];
    ferrite_config::load(dir, env).unwrap()
}

/// A self-signed certificate for `host`: what the node serves and what the client trusts.
fn self_signed(host: &str) -> (Arc<CertifiedKey>, CertificateDer<'static>) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec![host.to_owned()]).unwrap();
    let der = cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let signer = provider().key_provider.load_private_key(key).unwrap();
    (Arc::new(CertifiedKey::new(vec![der.clone()], signer)), der)
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

fn parse(raw: &[u8]) -> Reply {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Reply {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

fn request(method: &str, host: &str, path: &str) -> String {
    format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
}

/// One HTTP/1.1 request over plain TCP.
async fn http(addr: SocketAddr, method: &str, host: &str, path: &str) -> Reply {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(request(method, host, path).as_bytes())
        .await
        .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    parse(&raw)
}

/// One HTTP/1.1 request over TLS (SNI `sni`, trusting only `roots`).
async fn https(
    addr: SocketAddr,
    sni: &str,
    roots: &[CertificateDer<'static>],
    path: &str,
) -> std::io::Result<Reply> {
    let mut store = RootCertStore::empty();
    for r in roots {
        store.add(r.clone()).unwrap();
    }
    let mut config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(sni.to_owned()).unwrap();
    let mut tls = connector.connect(name, tcp).await?;
    tls.write_all(request("GET", sni, path).as_bytes()).await?;
    let mut raw = Vec::new();
    // A peer that closes without close_notify still gave us the whole response.
    match tls.read_to_end(&mut raw).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(e) => return Err(e),
    }
    Ok(parse(&raw))
}

async fn boot(config: &ferrite_config::Config, tls: TlsSeams) -> RunningNode {
    let store = open_store(config, OpenMode::CreateIfMissing).await.unwrap();
    start(
        config,
        store,
        NodeOptions {
            tls,
            ..NodeOptions::default()
        },
    )
    .await
    .unwrap()
}

/// HSTS on every TLS response (pages, API, errors), never on the plain listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hsts_only_on_tls() {
    let dir = tempfile::tempdir().unwrap();
    let config = site(
        dir.path(),
        &format!("https://{SITE_HOST}"),
        "listen = \"127.0.0.1:0\"\n",
    );
    let (cert, root) = self_signed(SITE_HOST);
    let node = boot(
        &config,
        TlsSeams {
            cert: Some(cert),
            ..TlsSeams::default()
        },
    )
    .await;
    let tls_addr = node.tls_addr.expect("TLS listener");
    assert!(node.redirect_addr.is_none());
    for path in ["/healthz", "/", "/no-such-page", "/api/v1/auth/session"] {
        let r = https(tls_addr, SITE_HOST, std::slice::from_ref(&root), path)
            .await
            .unwrap();
        assert_ne!(r.status, 0, "{path}: {}", r.head);
        assert_eq!(
            r.header("strict-transport-security"),
            Some("max-age=31536000"),
            "{path}: {}",
            r.head
        );
        let plain = http(node.addr, "GET", SITE_HOST, path).await;
        assert_eq!(plain.status, r.status, "{path}");
        assert_eq!(
            plain.header("strict-transport-security"),
            None,
            "{path}: {}",
            plain.head
        );
    }
    // The plain listener still answers the healthcheck.
    assert_eq!(
        http(node.addr, "GET", "x", "/healthz").await.body.trim(),
        "ok"
    );
    node.shutdown().await.unwrap();
}

/// `redirect_http` answers every request with 301 to the https site and serves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_listener_only_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let config = site(
        dir.path(),
        &format!("https://{SITE_HOST}"),
        "listen = \"127.0.0.1:0\"\nredirect_http = \"127.0.0.1:0\"\n",
    );
    let (cert, _) = self_signed(SITE_HOST);
    let node = boot(
        &config,
        TlsSeams {
            cert: Some(cert),
            ..TlsSeams::default()
        },
    )
    .await;
    let at = node.redirect_addr.expect("redirect listener");
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/healthz"),
        ("GET", "/hello-world/?a=1&b=2"),
        ("HEAD", "/admin/"),
        ("POST", "/api/v1/auth/login"),
        ("GET", "/_media/x/y.png"),
    ] {
        // The Host header does not choose the target.
        let r = http(at, method, "evil.test", path).await;
        assert_eq!(r.status, 301, "{method} {path}: {}", r.head);
        assert_eq!(
            r.header("location"),
            Some(format!("https://{SITE_HOST}{path}").as_str()),
            "{method} {path}"
        );
        assert_eq!(r.header("strict-transport-security"), None);
        assert!(!r.body.contains("<html"), "{method} {path}: {}", r.body);
    }
    node.shutdown().await.unwrap();
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// The account key and certificates are written 0600 in a 0700 `<data>/tls/`, and a
/// looser file left there is tightened when the cache is prepared.
#[tokio::test]
async fn tls_files_are_0600() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().join("tls");
    let cache = TlsCache::new(dir.clone());
    cache.prepare().unwrap();
    assert_eq!(mode(&dir), 0o700);
    let domains = vec![SITE_HOST.to_owned()];
    let contact = vec![format!("mailto:ops@{SITE_HOST}")];
    let url = "https://acme.test/dir";
    assert_eq!(cache.load_cert(&domains, url).await.unwrap(), None);
    cache.store_cert(&domains, url, b"cert pem").await.unwrap();
    cache
        .store_account(&contact, url, b"account key")
        .await
        .unwrap();
    assert_eq!(
        cache.load_cert(&domains, url).await.unwrap().as_deref(),
        Some(&b"cert pem"[..])
    );
    assert_eq!(
        cache.load_account(&contact, url).await.unwrap().as_deref(),
        Some(&b"account key"[..])
    );
    // Rewritten in place (renewal) keeps the mode.
    cache.store_cert(&domains, url, b"renewed").await.unwrap();
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2, "{files:?}");
    for f in &files {
        assert_eq!(mode(f), 0o600, "{}", f.display());
    }
    // A loose file and directory are tightened.
    std::fs::set_permissions(&files[0], std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    TlsCache::new(dir.clone()).prepare().unwrap();
    assert_eq!(mode(&files[0]), 0o600);
    assert_eq!(mode(&dir), 0o700);
}

/// The Pebble container this test started, removed (by its name) on drop.
struct Pebble {
    name: String,
}

impl Drop for Pebble {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn pem_certs(pem: &str) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .map(Result::unwrap)
        .collect()
}

/// A real ACME run: Pebble (in Docker) issues the certificate after validating
/// TLS-ALPN-01 against the node's TLS listener, reached from the container as
/// `host.docker.internal` (Docker Desktop), so the domain is that name. The served
/// certificate chains to Pebble's root, responses carry HSTS, and `<data>/tls/` holds
/// the account key and certificate as 0600 files.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acme_against_pebble_serves_https() {
    if std::env::var("FERRITE_TEST_PEBBLE").as_deref() != Ok("1") {
        eprintln!("SKIP: Pebble ACME test skipped (FERRITE_TEST_PEBBLE is not 1)");
        return;
    }
    const DOMAIN: &str = "host.docker.internal";
    let tls_port = free_port();
    let work = tempfile::tempdir().unwrap();
    // The stock config, with the TLS-ALPN-01 port set to the node's TLS listener.
    let pebble_config = format!(
        r#"{{"pebble": {{"listenAddress": "0.0.0.0:14000", "managementListenAddress": "0.0.0.0:15000",
        "certificate": "test/certs/localhost/cert.pem", "privateKey": "test/certs/localhost/key.pem",
        "httpPort": 5002, "tlsPort": {tls_port}, "ocspResponderURL": "", "externalAccountBindingRequired": false}}}}"#
    );
    std::fs::write(work.path().join("pebble.json"), pebble_config).unwrap();
    let pebble = Pebble {
        name: format!("ferrite-test-pebble-{}", uuid::Uuid::now_v7().simple()),
    };
    let mount = format!("{}:/cfg:ro", work.path().display());
    docker(&[
        "run",
        "-d",
        "--name",
        &pebble.name,
        "-p",
        "127.0.0.1:0:14000",
        "-p",
        "127.0.0.1:0:15000",
        "-e",
        "PEBBLE_VA_NOSLEEP=1",
        "-e",
        "PEBBLE_WFE_NONCEREJECT=0",
        "-v",
        &mount,
        PEBBLE_IMAGE,
        "-config",
        "/cfg/pebble.json",
    ]);
    let port = |p: &str| -> u16 {
        let out = docker(&["port", &pebble.name, p]);
        let first = out.lines().next().unwrap();
        first.rsplit(':').next().unwrap().parse().unwrap()
    };
    let (acme_port, mgmt_port) = (port("14000/tcp"), port("15000/tcp"));
    let minica_path = work.path().join("minica.pem");
    docker(&[
        "cp",
        &format!("{}:/test/certs/pebble.minica.pem", pebble.name),
        minica_path.to_str().unwrap(),
    ]);
    let minica = pem_certs(&std::fs::read_to_string(&minica_path).unwrap());
    let mgmt: SocketAddr = format!("127.0.0.1:{mgmt_port}").parse().unwrap();
    // Pebble is up once its management interface answers.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if https(mgmt, "localhost", &minica, "/roots/0").await.is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "Pebble did not start");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let dir = tempfile::tempdir().unwrap();
    let config = site(
        dir.path(),
        &format!("https://{DOMAIN}"),
        &format!(
            "listen = \"0.0.0.0:{tls_port}\"\ndirectory = \"https://localhost:{acme_port}/dir\"\n"
        ),
    );
    let node = boot(
        &config,
        TlsSeams {
            acme_roots: minica.clone(),
            ..TlsSeams::default()
        },
    )
    .await;
    let tls_addr: SocketAddr = format!("127.0.0.1:{tls_port}").parse().unwrap();
    assert_eq!(node.tls_addr.map(|a| a.port()), Some(tls_port));

    // Pebble's issuing root (generated at its start).
    let root = https(mgmt, "localhost", &minica, "/roots/0").await.unwrap();
    assert_eq!(root.status, 200, "{}", root.head);
    let roots = pem_certs(&root.body);
    assert!(!roots.is_empty(), "{}", root.body);
    let deadline = Instant::now() + Duration::from_secs(90);
    let reply = loop {
        match https(tls_addr, DOMAIN, &roots, "/healthz").await {
            Ok(r) => break r,
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "no certificate from Pebble: {e}; pebble logs:\n{}",
                    docker(&["logs", &pebble.name])
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    };
    assert_eq!(reply.status, 200, "{}", reply.head);
    assert_eq!(reply.body.trim(), "ok");
    assert_eq!(
        reply.header("strict-transport-security"),
        Some("max-age=31536000")
    );
    let tls_dir = dir.path().join("tls");
    assert_eq!(mode(&tls_dir), 0o700);
    let files: Vec<_> = std::fs::read_dir(&tls_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2, "account key and certificate: {files:?}");
    for f in &files {
        assert_eq!(mode(f), 0o600, "{}", f.display());
    }
    // The validation was real (PEBBLE_VA_ALWAYS_VALID is not set): Pebble's VA connected
    // back to the TLS listener (nothing listens on its HTTP-01 port).
    let out = Command::new("docker")
        .args(["logs", &pebble.name])
        .output()
        .unwrap();
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("pebble logs:\n{logs}");
    assert!(
        logs.contains("Starting 3 validations")
            && logs.contains("set VALID by completed challenge"),
        "{logs}"
    );
    node.shutdown().await.unwrap();
    drop(pebble);
}
