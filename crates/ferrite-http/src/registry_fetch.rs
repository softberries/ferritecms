//! Fetching a plugin registry's files (M6-2, design 14 §5.3): the index, its signature
//! and the plugins' manifests and components, over HTTPS ([`HttpsRegistry`]) or from a
//! folder ([`FileRegistry`], `file:///…` for air-gapped sites and tests), behind one
//! trait, [`RegistrySource`].
//!
//! **HTTPS.** The rules of the OAuth metadata fetcher ([`crate::oauth_fetch`]): the
//! checked resolver refuses the whole answer when any address is not public, the
//! connected peer is checked again, `https` only and no user information for the first
//! URL and every redirect, at most [`MAX_REDIRECTS`] redirects (GitHub release downloads
//! redirect once to its asset host), no proxy, cookies, `Referer` or `Authorization`.
//! One deadline per file (the caller's: 30 s for the index, 60 s for a component). A
//! `Content-Length` above the file's cap is refused before the body; the body read stops
//! as soon as it passes it. `User-Agent: ferrite/<version> registry`.
//!
//! **Files.** The index's folder is the root: a URL must name a file inside it after
//! canonicalisation (no `..`, no symlink out), and is size-checked before it is read.
//!
//! File URLs in an index resolve against the index URL ([`resolve`]); in a file registry
//! absolute URLs are refused, in an HTTPS one they must be `https`.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::header::{LOCATION, USER_AGENT};
use url::Url;

use crate::plugins::egress::{self, MAX_REDIRECTS, is_public_ip};

/// How long an index or its signature may take.
pub const INDEX_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a manifest or component may take.
pub const COMPONENT_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a registry file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryFetchError {
    /// A URL, address, redirect or path the rules refuse (nothing was read).
    #[error("refused: {0}")]
    Refused(String),
    /// A network error, a timeout, or a status other than 2xx.
    #[error("{0}")]
    Failed(String),
    /// The file is larger than its cap.
    #[error("the file is larger than allowed")]
    TooLarge,
    /// No such file (`404`, or a missing file).
    #[error("not found")]
    NotFound,
}

/// Where a registry's files come from.
#[async_trait]
pub trait RegistrySource: Send + Sync {
    /// The bytes at `url`, at most `max`, within `timeout`.
    async fn get(
        &self,
        url: &Url,
        max: u64,
        timeout: Duration,
    ) -> Result<Bytes, RegistryFetchError>;
}

/// Resolves an index entry's file `url` against `index` (design 14 §5.2): relative paths
/// join the index URL; absolute ones must be `https` in an `https` registry and are
/// refused in a file registry.
pub fn resolve(index: &Url, url: &str) -> Result<Url, RegistryFetchError> {
    ferrite_domain::plugin_registry::check_file_url(url)
        .map_err(|e| RegistryFetchError::Refused(e.0))?;
    let absolute = url.starts_with("https://");
    match (index.scheme(), absolute) {
        ("file", true) => Err(RegistryFetchError::Refused(
            "a file registry may name only files in its own folder".into(),
        )),
        (_, true) => Url::parse(url).map_err(|e| RegistryFetchError::Refused(e.to_string())),
        (_, false) => index
            .join(url)
            .map_err(|e| RegistryFetchError::Refused(e.to_string())),
    }
}

/// The source for registry `url` (`https://…` or `file:///…`).
pub fn source_for(url: &Url, version: &str) -> Result<Arc<dyn RegistrySource>, String> {
    match url.scheme() {
        "https" => Ok(Arc::new(HttpsRegistry::new(version)?)),
        "file" => {
            let index = url
                .to_file_path()
                .map_err(|()| "a file registry needs an absolute path".to_owned())?;
            let root = index
                .parent()
                .ok_or_else(|| "a file registry needs a folder".to_owned())?
                .to_path_buf();
            Ok(Arc::new(FileRegistry::new(root)))
        }
        other => Err(format!(
            "registry URLs are https:// or file:///, not {other}:"
        )),
    }
}

/// See the module docs.
pub struct HttpsRegistry {
    client: reqwest::Client,
    allow_ip: fn(IpAddr) -> bool,
    agent: String,
}

impl HttpsRegistry {
    /// The production fetcher: the system resolver, [`is_public_ip`] and the platform
    /// roots (with the `webpki-roots` fallback).
    pub fn new(version: &str) -> Result<Self, String> {
        Ok(Self {
            client: egress::registry_client()?,
            allow_ip: is_public_ip,
            agent: format!("ferrite/{version} registry"),
        })
    }

    /// Test seam: `overrides`' resolver, trusted roots and address rule.
    #[cfg(feature = "test-seams")]
    pub fn for_tests(overrides: egress::EgressOverrides) -> Result<Self, String> {
        let allow_ip = overrides.allow_ip;
        Ok(Self {
            client: egress::registry_client_for_tests(overrides)?,
            allow_ip,
            agent: format!("ferrite/{} registry", env!("CARGO_PKG_VERSION")),
        })
    }

    fn check(&self, url: &Url) -> Result<(), RegistryFetchError> {
        if url.scheme() != "https" {
            return Err(RegistryFetchError::Refused(
                "only https URLs are fetched".into(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(RegistryFetchError::Refused(
                "URLs with credentials are refused".into(),
            ));
        }
        let allowed = match url.host() {
            Some(url::Host::Domain(_)) => true,
            Some(url::Host::Ipv4(ip)) => (self.allow_ip)(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => (self.allow_ip)(IpAddr::V6(ip)),
            None => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(RegistryFetchError::Refused("address not allowed".into()))
        }
    }

    async fn fetch_within(
        &self,
        first: &Url,
        max: u64,
        deadline: Instant,
    ) -> Result<Bytes, RegistryFetchError> {
        let mut url = first.clone();
        let mut hops = 0;
        loop {
            self.check(&url)?;
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(RegistryFetchError::Failed("timed out".into()));
            }
            let mut response = self
                .client
                .get(url.clone())
                .header(USER_AGENT, &self.agent)
                .timeout(left)
                .send()
                .await
                .map_err(|e| request_error(&e))?;
            if let Some(peer) = response.remote_addr()
                && !(self.allow_ip)(peer.ip())
            {
                return Err(RegistryFetchError::Refused("address not allowed".into()));
            }
            let status = response.status();
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(RegistryFetchError::Refused("too many redirects".into()));
                }
                url = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .and_then(|l| url.join(l).ok())
                    .ok_or_else(|| {
                        RegistryFetchError::Refused("redirect without a location".into())
                    })?;
                continue;
            }
            if status.as_u16() == 404 {
                return Err(RegistryFetchError::NotFound);
            }
            if !status.is_success() {
                return Err(RegistryFetchError::Failed(format!(
                    "status {}",
                    status.as_u16()
                )));
            }
            if response.content_length().is_some_and(|n| n > max) {
                return Err(RegistryFetchError::TooLarge);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| request_error(&e))? {
                if (body.len() + chunk.len()) as u64 > max {
                    return Err(RegistryFetchError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(Bytes::from(body));
        }
    }
}

#[async_trait]
impl RegistrySource for HttpsRegistry {
    async fn get(
        &self,
        url: &Url,
        max: u64,
        timeout: Duration,
    ) -> Result<Bytes, RegistryFetchError> {
        let deadline = Instant::now() + timeout;
        match tokio::time::timeout(timeout, self.fetch_within(url, max, deadline)).await {
            Ok(result) => result,
            Err(_) => Err(RegistryFetchError::Failed("timed out".into())),
        }
    }
}

fn request_error(e: &reqwest::Error) -> RegistryFetchError {
    tracing::debug!(error = ?e, "registry fetch failed");
    if egress::address_refused(e) {
        RegistryFetchError::Refused("address not allowed".into())
    } else if e.is_timeout() {
        RegistryFetchError::Failed("timed out".into())
    } else {
        RegistryFetchError::Failed("request failed".into())
    }
}

/// A registry in a local folder (see the module docs).
pub struct FileRegistry {
    root: PathBuf,
}

impl FileRegistry {
    /// `root` is the folder of the index file.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl RegistrySource for FileRegistry {
    async fn get(
        &self,
        url: &Url,
        max: u64,
        _timeout: Duration,
    ) -> Result<Bytes, RegistryFetchError> {
        if url.scheme() != "file" {
            return Err(RegistryFetchError::Refused(
                "a file registry reads only file URLs".into(),
            ));
        }
        let path = url
            .to_file_path()
            .map_err(|()| RegistryFetchError::Refused("not a local path".into()))?;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let root = std::fs::canonicalize(&root)
                .map_err(|_| RegistryFetchError::Failed("the registry folder is missing".into()))?;
            let real = match std::fs::canonicalize(&path) {
                Ok(p) => p,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(RegistryFetchError::NotFound);
                }
                Err(_) => return Err(RegistryFetchError::Failed("cannot read the file".into())),
            };
            if !real.starts_with(&root) {
                return Err(RegistryFetchError::Refused(
                    "the file is outside the registry folder".into(),
                ));
            }
            let meta = std::fs::metadata(&real)
                .map_err(|_| RegistryFetchError::Failed("cannot read the file".into()))?;
            if !meta.is_file() {
                return Err(RegistryFetchError::NotFound);
            }
            if meta.len() > max {
                return Err(RegistryFetchError::TooLarge);
            }
            let bytes = std::fs::read(&real)
                .map_err(|_| RegistryFetchError::Failed("cannot read the file".into()))?;
            if bytes.len() as u64 > max {
                return Err(RegistryFetchError::TooLarge);
            }
            Ok(Bytes::from(bytes))
        })
        .await
        .map_err(|e| RegistryFetchError::Failed(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_against_index() {
        let https = Url::parse("https://example.dev/registry/v1/index.json").unwrap();
        assert_eq!(
            resolve(&https, "seo/0.3.0/seo.wasm").unwrap().as_str(),
            "https://example.dev/registry/v1/seo/0.3.0/seo.wasm"
        );
        assert_eq!(
            resolve(&https, "https://github.com/x/y.wasm")
                .unwrap()
                .as_str(),
            "https://github.com/x/y.wasm"
        );
        assert!(resolve(&https, "http://github.com/x").is_err());
        assert!(resolve(&https, "../x").is_err());
        let file = Url::parse("file:///srv/registry/index.json").unwrap();
        assert_eq!(
            resolve(&file, "seo.wasm").unwrap().as_str(),
            "file:///srv/registry/seo.wasm"
        );
        assert!(resolve(&file, "https://example.dev/x.wasm").is_err());
        assert!(resolve(&file, "/etc/passwd").is_err());
    }
}
