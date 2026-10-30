//! Built-in TLS for solo sites (M4-2 P33; design 09 §5): certificates from an ACME CA
//! through `rustls-acme`, on the `ring` provider, validated with TLS-ALPN-01 on the TLS
//! listener itself (no port 80 needed for the challenge).
//!
//! - `bind` binds `server.tls.listen` (and `redirect_http` when set) and builds the
//!   ACME state; `spawn_acme` drives it (load the cached account and certificate,
//!   order, renew) until the background tasks stop.
//! - [`TlsListener`] is an axum [`Listener`]: TCP accept plus the TLS handshake, the
//!   handshakes run concurrently inside `accept` (bounded, 10 s each), so a slow client
//!   holds up nobody, and the peer's address stays the connection's address
//!   (`ConnectInfo<SocketAddr>`, client IP and trusted proxies work as on plain HTTP).
//!   A TLS-ALPN-01 validation connection (ALPN `acme-tls/1` only) gets the challenge
//!   certificate and is closed after the handshake.
//! - [`TlsCache`] keeps the account key and the certificates in `<data>/tls/`: the folder
//!   0700, each file 0600, written to a temporary file and renamed into place.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use axum::serve::Listener;
use ferrite_config::AcmeTls;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls::server::ResolvesServerCert;
use rustls_acme::{AccountCache, AcmeConfig, AcmeState, CertCache, is_tls_alpn_challenge};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::sync::CancellationToken;

/// How long a client has for the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Handshakes in progress at once per listener; past it, accepting waits.
const MAX_HANDSHAKES: usize = 1024;
/// The protocols offered by ALPN, most preferred first.
const ALPN: [&[u8]; 2] = [b"h2", b"http/1.1"];

/// Test seams of the TLS listener (`NodeOptions::tls`); `Default` is production.
#[derive(Default)]
pub struct TlsSeams {
    /// Extra roots the ACME client trusts (a test CA such as Pebble's).
    #[cfg(feature = "test-seams")]
    pub acme_roots: Vec<rustls::pki_types::CertificateDer<'static>>,
    /// Serve this certificate and run no ACME client.
    #[cfg(feature = "test-seams")]
    pub cert: Option<Arc<rustls::sign::CertifiedKey>>,
}

/// The ACME state this node drives.
pub(crate) type Acme = AcmeState<io::Error, io::Error>;

/// What [`bind`] sets up.
pub(crate) struct Bound {
    pub listener: TlsListener,
    pub addr: SocketAddr,
    pub redirect: Option<(TcpListener, SocketAddr)>,
    /// `None` only with a test certificate.
    pub acme: Option<Acme>,
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Binds the TLS listener (and the redirect listener) of `tls` and prepares
/// `<data_dir>/tls/`; nothing runs until the listener is served and [`spawn_acme`].
pub(crate) async fn bind(tls: &AcmeTls, data_dir: &Path, seams: TlsSeams) -> anyhow::Result<Bound> {
    let tcp = TcpListener::bind(tls.listen)
        .await
        .with_context(|| format!("cannot listen on {} (server.tls.listen)", tls.listen))?;
    let addr = tcp.local_addr()?;
    let redirect = match tls.redirect_http {
        Some(at) => {
            let l = TcpListener::bind(at)
                .await
                .with_context(|| format!("cannot listen on {at} (server.tls.redirect_http)"))?;
            let a = l.local_addr()?;
            Some((l, a))
        }
        None => None,
    };
    #[cfg(feature = "test-seams")]
    if let Some(cert) = seams.cert {
        let resolver: Arc<dyn ResolvesServerCert> = Arc::new(Fixed(cert));
        return Ok(Bound {
            listener: TlsListener::new(tcp, server_config(resolver)?, None),
            addr,
            redirect,
            acme: None,
        });
    }
    let dir = data_dir.join("tls");
    let cache = TlsCache::new(dir.clone());
    cache
        .prepare()
        .with_context(|| format!("cannot prepare {}", dir.display()))?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    #[cfg(feature = "test-seams")]
    for r in seams.acme_roots {
        roots.add(r).context("test ACME root")?;
    }
    #[cfg(not(feature = "test-seams"))]
    let TlsSeams {} = seams;
    let client = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .context("TLS client protocol versions")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let state = AcmeConfig::new_with_client_config(&tls.domains, Arc::new(client))
        .contact_push(&tls.contact)
        .directory(tls.directory.url())
        .cache(cache)
        .state();
    let challenge = state.challenge_rustls_config_with_provider(provider());
    let main = server_config(state.resolver())?;
    Ok(Bound {
        listener: TlsListener::new(tcp, main, Some(challenge)),
        addr,
        redirect,
        acme: Some(state),
    })
}

/// The server config of ordinary connections: the given certificates, ALPN h2 and
/// HTTP/1.1.
fn server_config(resolver: Arc<dyn ResolvesServerCert>) -> anyhow::Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .context("TLS server protocol versions")?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = ALPN.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(config))
}

/// A test certificate, whatever the SNI.
#[cfg(feature = "test-seams")]
#[derive(Debug)]
struct Fixed(Arc<rustls::sign::CertifiedKey>);

#[cfg(feature = "test-seams")]
impl ResolvesServerCert for Fixed {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// Drives the ACME state until `stop`: the cached certificate is deployed at once, a
/// missing or expiring one ordered (rustls-acme renews when a third of the validity is left and backs
/// off after failures). Until a certificate is deployed, TLS handshakes fail.
pub(crate) fn spawn_acme(mut state: Acme, stop: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                () = stop.cancelled() => break,
                event = state.next() => event,
            };
            match event {
                Some(Ok(ok)) => tracing::info!(event = ?ok, "acme"),
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "acme: certificate not obtained yet; retrying with backoff")
                }
                None => break,
            }
        }
    })
}

type Handshake = Pin<Box<dyn Future<Output = Option<(TlsStream<TcpStream>, SocketAddr)>> + Send>>;

/// The TLS listener served by axum (see the module docs).
pub struct TlsListener {
    tcp: TcpListener,
    config: Arc<ServerConfig>,
    /// The TLS-ALPN-01 challenge config; `None` with a test certificate.
    challenge: Option<Arc<ServerConfig>>,
    pending: FuturesUnordered<Handshake>,
}

impl fmt::Debug for TlsListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsListener")
            .field("tcp", &self.tcp)
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl TlsListener {
    fn new(
        tcp: TcpListener,
        config: Arc<ServerConfig>,
        challenge: Option<Arc<ServerConfig>>,
    ) -> Self {
        Self {
            tcp,
            config,
            challenge,
            pending: FuturesUnordered::new(),
        }
    }

    fn handshake(&self, tcp: TcpStream, peer: SocketAddr) -> Handshake {
        let config = self.config.clone();
        let challenge = self.challenge.clone();
        Box::pin(async move {
            let done = tokio::time::timeout(HANDSHAKE_TIMEOUT, async move {
                let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), tcp)
                    .await
                    .ok()?;
                if let Some(challenge) = challenge
                    && is_tls_alpn_challenge(&start.client_hello())
                {
                    // The validation only needs the handshake.
                    let mut tls = start.into_stream(challenge).await.ok()?;
                    let _ = tokio::io::AsyncWriteExt::shutdown(&mut tls).await;
                    tracing::info!(peer = %peer, "acme: answered a TLS-ALPN-01 validation");
                    return None;
                }
                start.into_stream(config).await.ok()
            })
            .await;
            match done {
                Ok(Some(tls)) => Some((tls, peer)),
                Ok(None) => None,
                Err(_) => {
                    tracing::debug!(peer = %peer, "TLS handshake timed out");
                    None
                }
            }
        })
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let room = self.pending.len() < MAX_HANDSHAKES;
            tokio::select! {
                accepted = self.tcp.accept(), if room => match accepted {
                    Ok((tcp, peer)) => {
                        let _ = tcp.set_nodelay(true);
                        let h = self.handshake(tcp, peer);
                        self.pending.push(h);
                    }
                    Err(e) => accept_error(e).await,
                },
                Some(done) = self.pending.next(), if !self.pending.is_empty() => {
                    if let Some(conn) = done {
                        return conn;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// As axum's own TCP listener: a connection error is the client's; anything else (out
/// of file descriptors) is logged and retried after a second.
async fn accept_error(e: io::Error) {
    if matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!(error = %e, "TLS listener: accept failed");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// The ACME account key and certificates in `<data>/tls/` (see the module docs). File
/// names hash the contact or domains with the directory URL, so a changed config
/// starts a new account or order instead of reusing the old one.
#[derive(Debug, Clone)]
pub struct TlsCache {
    dir: PathBuf,
}

impl TlsCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Creates the folder if missing and tightens it (0700) and every file in it (0600).
    pub fn prepare(&self) -> io::Result<()> {
        create_private_dir(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in std::fs::read_dir(&self.dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o600))?;
                }
            }
        }
        Ok(())
    }

    fn name(kind: &str, parts: &[String], directory_url: &str) -> String {
        let mut h = Sha256::new();
        for p in parts {
            h.update(p.as_bytes());
            h.update([0]);
        }
        h.update(directory_url.as_bytes());
        let hex: String = h
            .finalize()
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{kind}-{hex}.pem")
    }

    async fn read(&self, name: String) -> io::Result<Option<Vec<u8>>> {
        let path = self.dir.join(name);
        match tokio::task::spawn_blocking(move || std::fs::read(path)).await {
            Ok(Ok(bytes)) => Ok(Some(bytes)),
            Ok(Err(e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    async fn write(&self, name: String, bytes: Vec<u8>) -> io::Result<()> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || write_private(&dir, &name, &bytes))
            .await
            .map_err(io::Error::other)?
    }
}

/// Writes `dir/name` as a 0600 file: a temporary file first (created 0600), synced,
/// then renamed over the old one, then the folder synced. A folder removed while the
/// node runs is made again 0700 (not with the umask's mode).
fn write_private(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if !dir.is_dir() {
        create_private_dir(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::now_v7().simple()));
    let written = (|| {
        let mut f = options.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    match written.and_then(|()| std::fs::rename(&tmp, dir.join(name))) {
        // The rename is durable only once the folder is synced: a power loss would
        // otherwise lose a fresh certificate and cost a new order.
        Ok(()) => sync_dir(dir),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// `dir` (and missing parents) created, then set to 0700.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// fsync of a folder's entries; a no-op where a folder cannot be opened as a file.
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[async_trait]
impl CertCache for TlsCache {
    type EC = io::Error;

    async fn load_cert(
        &self,
        domains: &[String],
        directory_url: &str,
    ) -> io::Result<Option<Vec<u8>>> {
        self.read(Self::name("cert", domains, directory_url)).await
    }

    async fn store_cert(
        &self,
        domains: &[String],
        directory_url: &str,
        cert: &[u8],
    ) -> io::Result<()> {
        self.write(Self::name("cert", domains, directory_url), cert.to_vec())
            .await
    }
}

#[async_trait]
impl AccountCache for TlsCache {
    type EA = io::Error;

    async fn load_account(
        &self,
        contact: &[String],
        directory_url: &str,
    ) -> io::Result<Option<Vec<u8>>> {
        self.read(Self::name("account", contact, directory_url))
            .await
    }

    async fn store_account(
        &self,
        contact: &[String],
        directory_url: &str,
        account: &[u8],
    ) -> io::Result<()> {
        self.write(
            Self::name("account", contact, directory_url),
            account.to_vec(),
        )
        .await
    }
}
