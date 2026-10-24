//! The per-node page cache (06 §6).

use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use moka::{Expiry, future::Cache, notification::RemovalCause, ops::compute::Op};

use crate::{
    index::{Entry, Invalidated, State},
    page::{CacheKey, CachedPage},
};

/// Per-entry lifetime: the page's `ttl` (`None` = no expiry).
struct PageExpiry;

impl Expiry<CacheKey, Arc<Entry>> for PageExpiry {
    fn expire_after_create(
        &self,
        _key: &CacheKey,
        value: &Arc<Entry>,
        _created_at: Instant,
    ) -> Option<Duration> {
        value.page.ttl
    }

    fn expire_after_update(
        &self,
        _key: &CacheKey,
        value: &Arc<Entry>,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        // A replacement is a new page: its own ttl, not the old remaining time.
        value.page.ttl
    }
}

/// Why a single-flight render produced no cache entry.
enum Flight<E> {
    /// The render failed; nothing is cached and every waiter gets the error.
    Failed(Arc<E>),
    /// The page is served but not cached (error status, `Set-Cookie`, zero TTL, or
    /// an invalidation of its deps arrived while it rendered).
    Uncached(Arc<CachedPage>),
    /// The flight ran without a render future (cannot happen; see `get_or_render`).
    Lost,
}

/// Outcome of one single-flight attempt.
enum Flown {
    Page(Arc<CachedPage>),
    /// Another caller's entry, invalidated since; removed from moka.
    Invalid(Arc<CachedPage>),
    Lost,
}

/// A snapshot of [`PageCache`] counters (M2a-2b P20).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Pages held.
    pub entries: u64,
    /// Their total weight (body plus key, headers, ETag and dependency keys).
    pub bytes: u64,
    /// Lookups that found a valid page, since the cache was built.
    pub hits: u64,
    /// Lookups that did not (absent or invalidated), since the cache was built.
    pub misses: u64,
}

/// A size-bounded, per-node cache of rendered pages with a dependency index.
///
/// Correctness over hit rate: a page is dropped as soon as any of its dependency keys
/// is invalidated, and a page whose deps were invalidated while it rendered is never
/// stored (see the staleness guard in the index module).
pub struct PageCache {
    cache: Cache<CacheKey, Arc<Entry>>,
    state: State,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl PageCache {
    /// A cache holding at most `max_bytes` (config `cache.max_bytes`), weighed as body
    /// plus key, header, ETag and dependency-key bytes.
    pub fn new(max_bytes: u64) -> Self {
        let state = State::default();
        let listener_state = state.clone();
        let cache = Cache::builder()
            .max_capacity(max_bytes)
            .weigher(|key: &CacheKey, entry: &Arc<Entry>| entry.page.weight(key))
            .expire_after(PageExpiry)
            .eviction_listener(
                move |key: Arc<CacheKey>, entry: Arc<Entry>, _cause: RemovalCause| {
                    listener_state.evicted(&key, entry.id);
                },
            )
            .build();
        Self {
            cache,
            state,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// The cached page for `key`, if present and not invalidated.
    /// Counted as a hit or a miss ([`stats`](Self::stats)).
    pub async fn get(&self, key: &CacheKey) -> Option<Arc<CachedPage>> {
        let found = self.lookup(key).await;
        let counter = if found.is_some() {
            &self.hits
        } else {
            &self.misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
        found
    }

    async fn lookup(&self, key: &CacheKey) -> Option<Arc<CachedPage>> {
        let entry = self.cache.get(key).await?;
        if entry.is_valid() {
            return Some(entry.page.clone());
        }
        self.remove_if_id(key, entry.id).await;
        None
    }

    /// Entries, bytes (after running moka's pending maintenance) and the hit and miss
    /// counts of [`get`](Self::get) (every [`get_or_render`](Self::get_or_render) looks
    /// up once).
    pub async fn stats(&self) -> CacheStats {
        self.cache.run_pending_tasks().await;
        CacheStats {
            entries: self.cache.entry_count(),
            bytes: self.cache.weighted_size(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    /// Removes the entry under `key` only if it is still entry `id`, so a newer valid
    /// entry stored meanwhile survives.
    async fn remove_if_id(&self, key: &CacheKey, id: u64) {
        self.cache
            .entry_by_ref(key)
            .and_compute_with(|current| {
                let same = current.is_some_and(|e| e.value().id == id);
                async move { if same { Op::Remove } else { Op::Nop } }
            })
            .await;
    }

    /// Stores `page` unless it is not cacheable; returns whether it was stored.
    /// Test-only: it has no protection against an invalidation that arrived while the
    /// caller rendered the page. Production code uses [`PageCache::get_or_render`].
    #[cfg(test)]
    pub(crate) async fn insert(&self, key: CacheKey, page: CachedPage) -> bool {
        if !page.is_cacheable() {
            return false;
        }
        let guard = self.state.begin_render();
        let Some(entry) = self.state.register(&key, Arc::new(page), &guard) else {
            return false;
        };
        drop(guard);
        self.cache.insert(key, Arc::new(entry)).await;
        true
    }

    /// The cached page for `key`, or the result of `render` (single-flight: concurrent
    /// callers for the same key share one render). The rendered page is cached only if
    /// it is cacheable and none of its deps (nor `all`) were invalidated while it
    /// rendered; either way every waiter gets it. A render error is not cached and is
    /// returned to every waiter.
    ///
    /// If the flight hands back an entry invalidated since another caller rendered it,
    /// the entry is removed and the render is retried once through the guarded path;
    /// if that again yields an invalidated entry, `render` runs uncached.
    pub async fn get_or_render<F, E>(
        &self,
        key: CacheKey,
        render: F,
    ) -> Result<Arc<CachedPage>, Arc<E>>
    where
        F: Future<Output = Result<CachedPage, E>> + Send,
        E: Send + Sync + 'static,
    {
        self.render_guarded(key, render, false).await
    }

    /// [`get_or_render`](Self::get_or_render) for a shared error page: the single
    /// exception to the `>= 400` refusal. A `404` or `410` page is cached when `key` is
    /// [internal](CacheKey::internal) (one body shared by every missing URL, so random
    /// 404s render once); every other rule (no `Set-Cookie`, no zero TTL, the staleness
    /// guard, dependency invalidation) still applies. Under a request-path key this
    /// behaves exactly like `get_or_render`.
    pub async fn get_or_render_error<F, E>(
        &self,
        key: CacheKey,
        render: F,
    ) -> Result<Arc<CachedPage>, Arc<E>>
    where
        F: Future<Output = Result<CachedPage, E>> + Send,
        E: Send + Sync + 'static,
    {
        let error_page = key.is_internal();
        self.render_guarded(key, render, error_page).await
    }

    async fn render_guarded<F, E>(
        &self,
        key: CacheKey,
        render: F,
        error_page: bool,
    ) -> Result<Arc<CachedPage>, Arc<E>>
    where
        F: Future<Output = Result<CachedPage, E>> + Send,
        E: Send + Sync + 'static,
    {
        if let Some(page) = self.get(&key).await {
            return Ok(page);
        }
        // Kept outside the flight: when another caller's flight wins, ours is dropped
        // unpolled and the render is still here for the retry.
        let mut render = Some(render);
        let mut stale = None;
        for _ in 0..2 {
            match self.flight(&key, &mut render, error_page).await? {
                Flown::Page(page) => return Ok(page),
                Flown::Invalid(page) => stale = Some(page),
                Flown::Lost => break,
            }
        }
        match (render.take(), stale) {
            (Some(render), _) => render.await.map(Arc::new).map_err(Arc::new),
            // Unreachable: once our own render ran, its result was returned above.
            (None, Some(page)) => Ok(page),
            (None, None) => {
                tracing::error!(path = %key.path, "page render lost by the cache");
                Ok(Arc::new(CachedPage::unavailable()))
            }
        }
    }

    /// One single-flight attempt; `render` is taken if this caller's flight runs.
    /// `error_page`: a 404/410 may be stored (internal keys only, see
    /// [`get_or_render_error`](Self::get_or_render_error)).
    async fn flight<F, E>(
        &self,
        key: &CacheKey,
        render: &mut Option<F>,
        error_page: bool,
    ) -> Result<Flown, Arc<E>>
    where
        F: Future<Output = Result<CachedPage, E>> + Send,
        E: Send + Sync + 'static,
    {
        let state = self.state.clone();
        let init = async {
            let Some(render) = render.take() else {
                return Err(Flight::Lost);
            };
            let guard = state.begin_render();
            let page = Arc::new(render.await.map_err(|e| Flight::Failed(Arc::new(e)))?);
            let storable = if error_page && key.is_internal() {
                page.is_cacheable_error_page()
            } else {
                page.is_cacheable()
            };
            if !storable {
                return Err(Flight::Uncached(page));
            }
            match state.register(key, page.clone(), &guard) {
                Some(entry) => Ok(Arc::new(entry)),
                None => Err(Flight::Uncached(page)),
            }
        };
        let result = self.cache.try_get_with(key.clone(), init).await;
        let ran = render.is_none();
        match result {
            // Our own render is served even if invalidated after it registered: it
            // finished before that invalidation.
            Ok(entry) if entry.is_valid() || ran => Ok(Flown::Page(entry.page.clone())),
            Ok(entry) => {
                self.remove_if_id(key, entry.id).await;
                Ok(Flown::Invalid(entry.page.clone()))
            }
            Err(flight) => match &*flight {
                Flight::Failed(e) => Err(e.clone()),
                Flight::Uncached(page) => Ok(Flown::Page(page.clone())),
                Flight::Lost => Ok(Flown::Lost),
            },
        }
    }

    /// Drops every page that depends on any of `keys`; `all` drops everything.
    pub async fn invalidate(&self, keys: &[String]) {
        match self.state.invalidate(keys) {
            Invalidated::All => self.cache.invalidate_all(),
            Invalidated::Keys(hit) => {
                for key in hit {
                    self.cache.invalidate(&key).await;
                }
            }
        }
    }

    /// Drops every page.
    pub fn flush(&self) {
        self.state.flush();
        self.cache.invalidate_all();
    }

    /// Number of entries (after running moka's pending maintenance).
    pub async fn len(&self) -> u64 {
        self.cache.run_pending_tasks().await;
        self.cache.entry_count()
    }

    /// Whether the cache holds no entries (after running pending maintenance).
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Total weight in bytes (after running moka's pending maintenance).
    pub async fn weighted_size(&self) -> u64 {
        self.cache.run_pending_tasks().await;
        self.cache.weighted_size()
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    #[cfg(test)]
    pub(crate) fn moka(&self) -> &Cache<CacheKey, Arc<Entry>> {
        &self.cache
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use bytes::Bytes;
    use tokio::sync::Notify;

    use super::*;
    use crate::etag_for;

    fn key(path: &str) -> CacheKey {
        CacheKey::anon("example.com", path, "")
    }

    fn page(body: &str, deps: &[&str]) -> CachedPage {
        CachedPage {
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: Bytes::from(body.to_owned()),
            etag: etag_for(body.as_bytes()),
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
            ttl: None,
        }
    }

    /// Every registered key is in moka, every dep-map reference is registered, and
    /// every moka entry is registered.
    async fn assert_index_consistent(c: &PageCache) {
        c.moka().run_pending_tasks().await;
        let (keys, _deps, refs) = c.state().snapshot();
        assert!(refs.is_subset(&keys), "dangling dep references");
        let in_moka: HashSet<CacheKey> = c.moka().iter().map(|(k, _)| (*k).clone()).collect();
        assert_eq!(keys, in_moka, "index and moka disagree");
    }

    /// M2a-2b P20: every lookup is a hit or a miss; entries and bytes follow the
    /// pages held.
    #[tokio::test]
    async fn stats_count_hits_misses_and_size() {
        let c = PageCache::new(1 << 20);
        assert_eq!(c.stats().await, CacheStats::default());
        let render = || async { Ok::<_, std::convert::Infallible>(page("body", &["post:1"])) };
        c.get_or_render(key("/a/"), render()).await.unwrap();
        c.get_or_render(key("/a/"), render()).await.unwrap();
        c.get_or_render(key("/a/"), render()).await.unwrap();
        assert!(c.get(&key("/b/")).await.is_none());
        let s = c.stats().await;
        assert_eq!((s.entries, s.hits, s.misses), (1, 2, 2), "{s:?}");
        assert_eq!(s.bytes, c.weighted_size().await);
        assert!(s.bytes > 4);
        // An invalidated page is a miss.
        c.invalidate(&["post:1".to_owned()]).await;
        assert!(c.get(&key("/a/")).await.is_none());
        let s = c.stats().await;
        assert_eq!(
            (s.entries, s.bytes, s.hits, s.misses),
            (0, 0, 2, 3),
            "{s:?}"
        );
    }

    /// Per-user variants (P21): one entry per user next to the anonymous one, and
    /// dependency invalidation drops every variant of the page.
    #[tokio::test]
    async fn user_variants_are_separate_and_invalidated_together() {
        use crate::Variant;
        use ferrite_domain::UserId;
        let c = PageCache::new(1 << 20);
        let (a, b) = (UserId::new(), UserId::new());
        let ua = CacheKey::user("example.com", "/p/", "", a);
        let ub = CacheKey::user("example.com", "/p/", "", b);
        assert_eq!(ua.variant, Variant::User(a));
        assert_ne!(ua, ub);
        assert_ne!(ua, key("/p/"));
        assert!(!ua.is_internal());
        for (k, body) in [(key("/p/"), "anon"), (ua.clone(), "a"), (ub.clone(), "b")] {
            assert!(c.insert(k, page(body, &["post:1"])).await);
        }
        assert_eq!(&c.get(&ua).await.unwrap().body[..], b"a");
        assert_eq!(&c.get(&ub).await.unwrap().body[..], b"b");
        assert_eq!(&c.get(&key("/p/")).await.unwrap().body[..], b"anon");
        c.invalidate(&["post:1".to_owned()]).await;
        assert!(c.is_empty().await);
        assert_index_consistent(&c).await;
    }

    #[tokio::test]
    async fn refuses_errors_set_cookie_and_zero_ttl() {
        let c = PageCache::new(1 << 20);
        for status in [400, 404, 500, 503] {
            let mut p = page("x", &["post:1"]);
            p.status = status;
            assert!(!c.insert(key("/e/"), p).await, "status {status}");
        }
        let mut p = page("x", &[]);
        p.headers.push(("Set-Cookie".into(), "a=b".into()));
        assert!(!c.insert(key("/c/"), p).await);
        let mut p = page("x", &[]);
        p.ttl = Some(Duration::ZERO);
        assert!(!c.insert(key("/z/"), p).await);
        assert!(c.is_empty().await);

        // get_or_render serves an uncacheable page without caching it.
        let mut p = page("boom", &[]);
        p.status = 500;
        let got = c
            .get_or_render(key("/e/"), async { Ok::<_, ()>(p) })
            .await
            .unwrap();
        assert_eq!(got.status, 500);
        assert!(c.get(&key("/e/")).await.is_none());
        assert!(c.is_empty().await);
        assert_index_consistent(&c).await;
    }

    /// The one exception to the >= 400 refusal: a 404/410 under an internal key,
    /// through `get_or_render_error`. Path keys, other statuses and plain
    /// `get_or_render` still refuse.
    #[tokio::test]
    async fn internal_error_pages_only_through_get_or_render_error() {
        let c = PageCache::new(1 << 20);
        let status = |s: u16| {
            let mut p = page("err", &["menu:primary"]);
            p.status = s;
            p
        };
        assert!(CacheKey::internal("404").is_internal());
        assert!(!key("/404/").is_internal());
        assert_ne!(CacheKey::internal("404"), CacheKey::internal("410"));

        for s in [404, 410] {
            let k = CacheKey::internal(&s.to_string());
            let got = c
                .get_or_render_error(k.clone(), async { Ok::<_, ()>(status(s)) })
                .await
                .unwrap();
            assert_eq!(got.status, s);
            assert!(c.get(&k).await.is_some(), "{s} cached");
        }
        // Invalidated like any page.
        c.invalidate(&["menu:primary".to_owned()]).await;
        assert!(c.is_empty().await);

        // Not cached: a path key, a 500, plain get_or_render, a set-cookie, zero TTL.
        let cases: Vec<(CacheKey, CachedPage, bool)> = vec![
            (key("/missing/"), status(404), true),
            (CacheKey::internal("500"), status(500), true),
            (CacheKey::internal("404"), status(404), false),
            (
                CacheKey::internal("404"),
                {
                    let mut p = status(404);
                    p.headers.push(("set-cookie".into(), "a=b".into()));
                    p
                },
                true,
            ),
            (
                CacheKey::internal("404"),
                {
                    let mut p = status(404);
                    p.ttl = Some(Duration::ZERO);
                    p
                },
                true,
            ),
        ];
        for (k, p, error_api) in cases {
            let fut = async { Ok::<_, ()>(p) };
            let got = if error_api {
                c.get_or_render_error(k.clone(), fut).await
            } else {
                c.get_or_render(k.clone(), fut).await
            };
            assert!(got.is_ok());
            assert!(c.get(&k).await.is_none(), "{k:?}");
        }
        assert!(c.is_empty().await);
        assert_index_consistent(&c).await;
    }

    #[tokio::test]
    async fn invalidate_drops_exactly_the_dependents() {
        let c = PageCache::new(1 << 20);
        assert!(
            c.insert(key("/a/"), page("a", &["post:1", "archive:post"]))
                .await
        );
        assert!(
            c.insert(key("/b/"), page("b", &["post:2", "archive:post"]))
                .await
        );
        assert!(c.insert(key("/c/"), page("c", &["post:3"])).await);
        c.invalidate(&["post:1".into()]).await;
        assert!(c.get(&key("/a/")).await.is_none());
        assert!(c.get(&key("/b/")).await.is_some());
        assert_index_consistent(&c).await;

        c.invalidate(&["archive:post".into()]).await;
        assert!(c.get(&key("/b/")).await.is_none());
        assert!(c.get(&key("/c/")).await.is_some());
        assert_index_consistent(&c).await;
        let (_, deps, _) = c.state().snapshot();
        assert_eq!(deps, HashSet::from(["post:3".to_owned()]));

        c.invalidate(&["all".into()]).await;
        assert!(c.get(&key("/c/")).await.is_none());
        assert!(c.is_empty().await);
        assert_index_consistent(&c).await;
    }

    #[tokio::test]
    async fn flush_and_replacement_keep_the_index_clean() {
        let c = PageCache::new(1 << 20);
        assert!(c.insert(key("/a/"), page("a1", &["post:1"])).await);
        assert!(c.insert(key("/a/"), page("a2", &["post:2"])).await);
        assert_eq!(&c.get(&key("/a/")).await.unwrap().body[..], b"a2");
        assert_index_consistent(&c).await;
        let (_, deps, _) = c.state().snapshot();
        assert_eq!(deps, HashSet::from(["post:2".to_owned()]));
        // The old dep no longer reaches the new page.
        c.invalidate(&["post:1".into()]).await;
        assert!(c.get(&key("/a/")).await.is_some());

        c.flush();
        assert!(c.get(&key("/a/")).await.is_none());
        assert!(c.is_empty().await);
        assert_index_consistent(&c).await;
    }

    #[tokio::test]
    async fn size_eviction_leaves_no_dangling_index_entries() {
        let max = 8 * 1024;
        let c = PageCache::new(max);
        let body = "x".repeat(1000);
        for i in 0..64 {
            c.insert(
                key(&format!("/p{i}/")),
                page(&body, &[&format!("post:{i}"), "archive:post"]),
            )
            .await;
        }
        assert!(c.weighted_size().await <= max);
        let n = c.len().await;
        assert!(n > 0 && n < 64, "{n} entries");
        assert_index_consistent(&c).await;
        let (keys, deps, _) = c.state().snapshot();
        assert_eq!(keys.len() as u64, n);
        // One post key per surviving page plus the shared archive key.
        assert_eq!(deps.len(), keys.len() + 1);

        c.invalidate(&["archive:post".into()]).await;
        assert!(c.is_empty().await);
        assert_index_consistent(&c).await;
        let (keys, deps, refs) = c.state().snapshot();
        assert!(keys.is_empty() && deps.is_empty() && refs.is_empty());
    }

    #[tokio::test]
    async fn ttl_expires_entries() {
        let c = PageCache::new(1 << 20);
        let mut short = page("s", &["post:1"]);
        short.ttl = Some(Duration::from_millis(100));
        assert!(c.insert(key("/s/"), short).await);
        assert!(c.insert(key("/l/"), page("l", &["post:1"])).await);
        assert!(c.get(&key("/s/")).await.is_some());
        // moka's clock is real time, not tokio's.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(c.get(&key("/s/")).await.is_none());
        assert!(c.get(&key("/l/")).await.is_some());
        // moka removes expired entries in maintenance on its timer wheel (~1 s
        // granularity); the eviction listener then clears the index.
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.state().snapshot().0.len() > 1 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c.moka().run_pending_tasks().await;
        }
        assert_eq!(c.len().await, 1);
        assert_index_consistent(&c).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn get_or_render_is_single_flight() {
        let c = Arc::new(PageCache::new(1 << 20));
        let renders = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..32 {
            let (c, renders) = (c.clone(), renders.clone());
            tasks.push(tokio::spawn(async move {
                c.get_or_render(key("/a/"), async move {
                    renders.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok::<_, ()>(page("a", &["post:1"]))
                })
                .await
                .unwrap()
            }));
        }
        for t in tasks {
            assert_eq!(&t.await.unwrap().body[..], b"a");
        }
        assert_eq!(renders.load(Ordering::SeqCst), 1);
        assert!(c.get(&key("/a/")).await.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn render_error_reaches_every_waiter_and_is_not_cached() {
        let c = Arc::new(PageCache::new(1 << 20));
        let renders = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (c, renders) = (c.clone(), renders.clone());
            tasks.push(tokio::spawn(async move {
                c.get_or_render(key("/a/"), async move {
                    renders.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Err::<CachedPage, _>("template error")
                })
                .await
            }));
        }
        for t in tasks {
            assert_eq!(*t.await.unwrap().unwrap_err(), "template error");
        }
        assert_eq!(renders.load(Ordering::SeqCst), 1);
        assert!(c.is_empty().await);
        // The next request renders again.
        let ok = c
            .get_or_render(key("/a/"), async { Ok::<_, &str>(page("a", &[])) })
            .await
            .unwrap();
        assert_eq!(&ok.body[..], b"a");
        assert!(c.get(&key("/a/")).await.is_some());
    }

    /// Renders `/a/` with deps `post:1`, firing `during` while the render is blocked,
    /// and returns whether the result ended up cached.
    async fn render_with_concurrent(during: &[&str]) -> bool {
        let c = Arc::new(PageCache::new(1 << 20));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let task = {
            let (c, started, release) = (c.clone(), started.clone(), release.clone());
            tokio::spawn(async move {
                c.get_or_render(key("/a/"), async move {
                    started.notify_one();
                    release.notified().await;
                    Ok::<_, ()>(page("old title", &["post:1", "theme"]))
                })
                .await
                .unwrap()
            })
        };
        started.notified().await;
        let keys: Vec<String> = during.iter().map(|k| (*k).to_owned()).collect();
        c.invalidate(&keys).await;
        release.notify_one();
        let served = task.await.unwrap();
        assert_eq!(
            &served.body[..],
            b"old title",
            "the waiter still gets the page"
        );
        let cached = c.get(&key("/a/")).await.is_some();
        assert_index_consistent(&c).await;
        assert_eq!(c.state().log_len(), 0, "log pruned after the render");
        cached
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalidation_during_render_is_not_cached() {
        assert!(!render_with_concurrent(&["post:1"]).await);
        assert!(!render_with_concurrent(&["all"]).await);
        assert!(!render_with_concurrent(&["post:9", "theme"]).await);
        // Unrelated keys do not block caching.
        assert!(render_with_concurrent(&["post:2"]).await);
    }

    /// Stores an entry for `/a/` that is already invalidated, bypassing `get`.
    async fn plant_invalid(c: &PageCache) -> u64 {
        let guard = c.state().begin_render();
        let entry = c
            .state()
            .register(&key("/a/"), Arc::new(page("stale", &["post:1"])), &guard)
            .unwrap();
        drop(guard);
        let id = entry.id;
        c.invalidate(&["post:1".into()]).await;
        c.moka().insert(key("/a/"), Arc::new(entry)).await;
        id
    }

    #[tokio::test]
    async fn flight_returning_an_invalid_entry_is_retried() {
        let c = PageCache::new(1 << 20);
        plant_invalid(&c).await;
        let mut render = Some(async { Ok::<_, ()>(page("fresh", &["post:1"])) });
        // Another caller's (invalid) entry: not served, removed, our render kept.
        assert!(matches!(
            c.flight(&key("/a/"), &mut render, false).await,
            Ok(Flown::Invalid(_))
        ));
        assert!(render.is_some());
        assert!(!c.moka().contains_key(&key("/a/")));
        // The retry renders and caches.
        match c.flight(&key("/a/"), &mut render, false).await {
            Ok(Flown::Page(p)) => assert_eq!(&p.body[..], b"fresh"),
            _ => panic!("expected a page"),
        }
        assert!(render.is_none());
        assert_eq!(&c.get(&key("/a/")).await.unwrap().body[..], b"fresh");
        assert_index_consistent(&c).await;
    }

    #[tokio::test]
    async fn id_checked_removal_spares_a_newer_entry() {
        let c = PageCache::new(1 << 20);
        let old = plant_invalid(&c).await;
        assert!(c.insert(key("/a/"), page("new", &["post:2"])).await);
        c.remove_if_id(&key("/a/"), old).await;
        assert_eq!(&c.get(&key("/a/")).await.unwrap().body[..], b"new");
        assert_index_consistent(&c).await;
    }

    #[tokio::test]
    async fn invalidation_between_registration_and_store_is_never_served() {
        // The second window: registered, then invalidated, then moka stores it.
        let c = PageCache::new(1 << 20);
        let guard = c.state().begin_render();
        let entry = c
            .state()
            .register(&key("/a/"), Arc::new(page("a", &["post:1"])), &guard)
            .unwrap();
        drop(guard);
        c.invalidate(&["post:1".into()]).await;
        c.moka().insert(key("/a/"), Arc::new(entry)).await;
        assert!(c.get(&key("/a/")).await.is_none());
        assert!(c.is_empty().await, "an invalid entry is evicted on lookup");
        // get_or_render renders afresh instead of returning the invalid entry.
        let fresh = c
            .get_or_render(key("/a/"), async { Ok::<_, ()>(page("b", &["post:1"])) })
            .await
            .unwrap();
        assert_eq!(&fresh.body[..], b"b");
        assert_index_consistent(&c).await;
    }
}
