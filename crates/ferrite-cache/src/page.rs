//! Cache keys and cached pages.

use std::time::Duration;

use bytes::Bytes;

/// Who a cached page was rendered for. M1c-2 caches anonymous responses only;
/// logged-in requests bypass the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Anon,
}

/// Identity of a cached response: (host, path, normalised allowlisted query, variant).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub host: String,
    pub path: String,
    /// The router's normalised query (`NormalQuery`), empty when there is none.
    pub query: String,
    pub variant: Variant,
}

impl CacheKey {
    /// An anonymous key.
    pub fn anon(
        host: impl Into<String>,
        path: impl Into<String>,
        query: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            path: path.into(),
            query: query.into(),
            variant: Variant::Anon,
        }
    }

    fn weight(&self) -> usize {
        self.host.len() + self.path.len() + self.query.len()
    }
}

/// A rendered response together with the dependency keys recorded while rendering it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPage {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    /// Strong, quoted ETag of `body` ([`crate::etag_for`]).
    pub etag: String,
    /// Dependency keys (grammar in `ferrite_core::events`).
    pub deps: Vec<String>,
    /// Lifetime in the cache; `None` = until invalidated or evicted.
    pub ttl: Option<Duration>,
}

impl CachedPage {
    /// Whether the cache may store this page: not an error status (>= 400), no
    /// `Set-Cookie` header, and not a zero TTL.
    pub fn is_cacheable(&self) -> bool {
        self.status < 400
            && self.ttl != Some(Duration::ZERO)
            && !self
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
    }

    /// The bytes an entry occupies, as counted against `cache.max_bytes`: body, key,
    /// headers, ETag and dependency keys.
    pub(crate) fn weight(&self, key: &CacheKey) -> u32 {
        let headers: usize = self.headers.iter().map(|(n, v)| n.len() + v.len()).sum();
        let deps: usize = self.deps.iter().map(String::len).sum();
        let total = self.body.len() + key.weight() + headers + deps + self.etag.len();
        u32::try_from(total).unwrap_or(u32::MAX)
    }
}
