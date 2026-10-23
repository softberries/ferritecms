//! Cache keys and cached pages.

use std::time::Duration;

use bytes::Bytes;
use ferrite_domain::UserId;

/// First character of an internal key's path.
const INTERNAL: char = '\0';

/// Who a cached page was rendered for: anonymous visitors share [`Anon`](Self::Anon);
/// with `cache.logged_in = "per-user"` a logged-in user's pages are stored under
/// [`User`](Self::User) (P21), never shown to anyone else. Dependency invalidation
/// does not look at the variant: an edit drops every variant of a page.
///
/// **Invariant:** the user id alone is the key because no user-specific data reaches
/// the render (plugins see only `is_logged_in`; templates get no user, role or
/// capability). Per-user output added later must put role/capabilities into the key
/// (or bypass the cache) and invalidate `user:<id>` on role change and logout. Entries
/// are not evicted on logout: an ended session no longer resolves, so they are
/// unreachable until LRU or invalidation drops them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Anon,
    User(UserId),
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

    /// A per-user key: the page as `user` sees it (P21).
    pub fn user(
        host: impl Into<String>,
        path: impl Into<String>,
        query: impl Into<String>,
        user: UserId,
    ) -> Self {
        Self {
            variant: Variant::User(user),
            ..Self::anon(host, path, query)
        }
    }

    /// The key of a shared internal page (the 404 body, the 410 body): empty host,
    /// path `\0<name>`. Request paths never contain a NUL byte (the router rejects
    /// control characters), so it cannot collide with a URL.
    pub fn internal(name: &str) -> Self {
        Self::anon("", format!("{INTERNAL}{name}"), "")
    }

    /// Whether this is an [internal](Self::internal) key.
    pub fn is_internal(&self) -> bool {
        self.host.is_empty() && self.path.starts_with(INTERNAL)
    }

    fn weight(&self) -> usize {
        let variant = match self.variant {
            Variant::Anon => 0,
            Variant::User(_) => size_of::<UserId>(),
        };
        self.host.len() + self.path.len() + self.query.len() + variant
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
        self.status < 400 && self.ttl != Some(Duration::ZERO) && !self.has_set_cookie()
    }

    /// Whether a shared error page may be stored under an internal key: status 404 or
    /// 410, and otherwise the rules of [`is_cacheable`](Self::is_cacheable).
    pub(crate) fn is_cacheable_error_page(&self) -> bool {
        matches!(self.status, 404 | 410)
            && self.ttl != Some(Duration::ZERO)
            && !self.has_set_cookie()
    }

    fn has_set_cookie(&self) -> bool {
        self.headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
    }

    /// An uncacheable `503` with an empty body (a defensive fallback only).
    pub(crate) fn unavailable() -> Self {
        Self {
            status: 503,
            headers: Vec::new(),
            body: Bytes::new(),
            etag: crate::etag_for(b""),
            deps: Vec::new(),
            ttl: None,
        }
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
