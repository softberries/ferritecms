//! Review Focus 2 at the cache level, on both backends: real content writes through
//! `ContentService`, pages cached under the dependency keys `SiteReader` records, one
//! `poll_once`, and every affected page is gone (and re-renders with the new data)
//! while unrelated pages stay cached. The HTTP-level version is in `ferrite-http`.
//!
//! Also (Postgres only, P3): with `listen`, a write on one node invalidates another
//! node's cache through `LISTEN/NOTIFY` long before its next poll.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use ferrite_cache::{CacheKey, CachedPage, InvalidationLoop, PageCache, etag_for};
use ferrite_core::{
    Actor, ContentDraft, ContentService, FeedSource, FrontPage, PageItem, PermalinkRules,
    SiteReader,
    events::{PLUGINS_KEY, THEME_KEY},
};
use ferrite_domain::{
    ArtifactHash, ContentItem, NewEvent, NewUser, Role, Term, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

async fn admin(store: &dyn Store) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor {
        user: id,
        role: Role::Admin,
    }
}

async fn category(store: &dyn Store, slug: &str) -> TermId {
    let id = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id,
            taxonomy: "category".into(),
            slug: slug.into(),
            name: slug.into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    id
}

fn draft(kind: &str, title: &str, slug: &str, terms: &[TermId]) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: Some(slug.into()),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: terms.to_vec(),
    }
}

async fn published(svc: &ContentService, a: &Actor, d: ContentDraft) -> ContentItem {
    let it = svc.create(a, d).await.unwrap();
    svc.publish(a, it.id, it.version).await.unwrap()
}

fn reader(store: Arc<dyn Store>) -> SiteReader {
    SiteReader::new(
        store,
        Arc::new(PermalinkRules::default()),
        Arc::new(|hash: &ArtifactHash, name: &str| format!("/_media/{hash}/{name}")),
        Arc::new(Timestamp::now),
    )
}

/// The public views under test.
#[derive(Clone, Copy, Debug)]
enum View {
    Single(&'static str, &'static str),
    Home,
    PostArchive,
    Category(&'static str),
    Feed,
}

fn cache_key(view: View) -> CacheKey {
    let path = match view {
        View::Single(_, slug) => format!("/{slug}/"),
        View::Home => "/".into(),
        View::PostArchive => "/post/".into(),
        View::Category(slug) => format!("/category/{slug}/"),
        View::Feed => "/feed/".into(),
    };
    CacheKey::anon("example.com", path, "")
}

fn titles(items: &[PageItem]) -> String {
    items
        .iter()
        .map(|i| i.item.title.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

/// "Renders" a view: the titles it shows as the body, plus the reader's deps and the
/// keys the HTTP layer adds to every page. `None` = 404.
async fn render(r: &SiteReader, view: View) -> Option<CachedPage> {
    let (body, deps) = match view {
        View::Single(kind, slug) => {
            let read = r.single(kind, &[slug.to_owned()]).await.unwrap();
            (read.value.map(|p| p.item.title), read.deps)
        }
        View::Home => {
            let read = r.front_page(1).await.unwrap();
            let body = match read.value {
                Some(FrontPage::Posts(l)) => Some(titles(&l.items)),
                Some(FrontPage::Page(p)) => Some(p.item.title),
                None => None,
            };
            (body, read.deps)
        }
        View::PostArchive => {
            let read = r.archive("post", 1).await.unwrap();
            (read.value.map(|l| titles(&l.items)), read.deps)
        }
        View::Category(slug) => {
            let read = r.term_archive("category", slug, 1).await.unwrap();
            (read.value.map(|l| titles(&l.items)), read.deps)
        }
        View::Feed => {
            let read = r.feed(&FeedSource::Kind("post".into())).await.unwrap();
            (read.value.map(|items| titles(&items)), read.deps)
        }
    };
    let body = body?;
    let mut deps = deps.into_vec();
    deps.push(THEME_KEY.to_owned());
    deps.push(PLUGINS_KEY.to_owned());
    Some(CachedPage {
        status: 200,
        headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
        etag: etag_for(body.as_bytes()),
        body: Bytes::from(body),
        deps,
        ttl: None,
    })
}

async fn fill(cache: &PageCache, r: &SiteReader, views: &[View]) {
    for &v in views {
        serve(cache, r, v)
            .await
            .unwrap_or_else(|| panic!("{v:?} renders"));
        assert!(cached_body(cache, v).await.is_some(), "{v:?} cached");
    }
}

async fn cached_body(cache: &PageCache, view: View) -> Option<String> {
    cache
        .get(&cache_key(view))
        .await
        .map(|p| String::from_utf8(p.body.to_vec()).unwrap())
}

/// What the HTTP layer does: the cached page, or a single-flight render that is then
/// cached. `None` = 404 (the render "fails" and nothing is cached).
async fn serve(cache: &PageCache, r: &SiteReader, view: View) -> Option<String> {
    let page = cache
        .get_or_render(cache_key(view), async {
            render(r, view).await.ok_or("not found")
        })
        .await
        .ok()?;
    Some(String::from_utf8(page.body.to_vec()).unwrap())
}

struct Site {
    svc: ContentService,
    actor: Actor,
    reader: SiteReader,
    cache: Arc<PageCache>,
    poller: InvalidationLoop,
    target: ContentItem,
    news: TermId,
}

const AFFECTED: [View; 5] = [
    View::Single("post", "hello"),
    View::Home,
    View::PostArchive,
    View::Category("news"),
    View::Feed,
];
/// Every term archive of posts depends on `archive:post` (coarse listing key, 06 §6),
/// so other categories are affected too; singles of other items are not.
const UNRELATED: [View; 2] = [
    View::Single("page", "about"),
    View::Single("post", "elsewhere"),
];

async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    let actor = admin(db.store()).await;
    let news = category(db.store(), "news").await;
    let other = category(db.store(), "other").await;
    let target = published(&svc, &actor, draft("post", "Hello", "hello", &[news])).await;
    published(
        &svc,
        &actor,
        draft("post", "Elsewhere", "elsewhere", &[other]),
    )
    .await;
    published(&svc, &actor, draft("page", "About", "about", &[])).await;

    // Boot order: the cursor is set before anything is cached.
    let cache = Arc::new(PageCache::new(64 << 20));
    let poller = InvalidationLoop::for_store(store.clone(), cache.clone())
        .await
        .unwrap();
    let reader = reader(store);
    fill(&cache, &reader, &AFFECTED).await;
    fill(&cache, &reader, &UNRELATED).await;
    assert_eq!(cache.len().await, 7);
    assert_eq!(
        cached_body(&cache, View::Single("post", "hello"))
            .await
            .as_deref(),
        Some("Hello")
    );
    Site {
        svc,
        actor,
        reader,
        cache,
        poller,
        target,
        news,
    }
}

async fn unrelated_still_cached(s: &Site) {
    for v in UNRELATED {
        assert!(
            cached_body(&s.cache, v).await.is_some(),
            "{v:?} was dropped"
        );
    }
}

pub async fn title_edit_refreshes_page_archives_home_and_feed(db: &TestDb) {
    let mut s = site(db).await;
    s.svc
        .update(
            &s.actor,
            s.target.id,
            s.target.version,
            draft("post", "Hello again", "hello", &[s.news]),
        )
        .await
        .unwrap();
    assert!(s.poller.poll_once().await.unwrap() > 0);
    assert_eq!(
        s.poller.last_seq(),
        db.store().read().events().max_seq().await.unwrap()
    );

    for v in AFFECTED {
        assert_eq!(cached_body(&s.cache, v).await, None, "{v:?} is stale");
    }
    unrelated_still_cached(&s).await;

    // Served again, every view shows the new title.
    for v in AFFECTED {
        let body = serve(&s.cache, &s.reader, v).await.unwrap();
        assert!(body.contains("Hello again"), "{v:?}: {body}");
    }
}

pub async fn trash_removes_it_everywhere(db: &TestDb) {
    let mut s = site(db).await;
    s.svc
        .trash(&s.actor, s.target.id, s.target.version)
        .await
        .unwrap();
    s.poller.poll_once().await.unwrap();

    for v in AFFECTED {
        assert_eq!(cached_body(&s.cache, v).await, None, "{v:?} is stale");
    }
    unrelated_still_cached(&s).await;

    assert_eq!(
        serve(&s.cache, &s.reader, View::Single("post", "hello")).await,
        None
    );
    // The category's only post is gone: its archive is a 404 now (R7).
    assert_eq!(
        serve(&s.cache, &s.reader, View::Category("news")).await,
        None
    );
    for v in [View::Home, View::PostArchive, View::Feed] {
        let body = serve(&s.cache, &s.reader, v).await.unwrap();
        assert!(!body.contains("Hello"), "{v:?}: {body}");
        assert!(body.contains("Elsewhere"), "{v:?}: {body}");
    }
}

backend_tests!(
    title_edit_refreshes_page_archives_home_and_feed,
    trash_removes_it_everywhere,
);

/// One node's cache and listening loop (poll interval 10 s, so only notices are fast).
async fn listening_node(
    store: Arc<dyn Store>,
    shutdown: &CancellationToken,
) -> (Arc<PageCache>, JoinHandle<()>) {
    let cache = Arc::new(PageCache::new(1 << 20));
    let handle = InvalidationLoop::for_store(store, cache.clone())
        .await
        .unwrap()
        .spawn(Duration::from_secs(10), true, shutdown.clone());
    (cache, handle)
}

fn plain_page(key: &str) -> CachedPage {
    CachedPage {
        status: 200,
        headers: vec![],
        body: Bytes::from_static(b"p"),
        etag: etag_for(b"p"),
        deps: vec![key.to_owned()],
        ttl: None,
    }
}

/// Caches a page depending on `dep` under `key` (the guarded path the HTTP layer uses).
async fn cache_page(cache: &PageCache, key: CacheKey, dep: &str) {
    let page = plain_page(dep);
    cache
        .get_or_render(key, async move { Ok::<_, ()>(page) })
        .await
        .unwrap();
}

async fn invalidate(store: &dyn Store, key: &str) {
    let tx = store.begin().await.unwrap();
    tx.events()
        .append(&NewEvent {
            kind: "invalidate".into(),
            keys: vec![key.to_owned()],
            origin_node: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Waits up to `within` for `key`'s page to leave `cache`.
async fn gone_within(cache: &PageCache, key: &CacheKey, within: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < within {
        if cache.get(key).await.is_none() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cache.get(key).await.is_none()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listen_invalidates_peer_cache_within_1s() {
    let Some(db) = TestDb::postgres().await else {
        return;
    };
    let store = db.shared_store();
    let peer = db.peer().await.expect("a peer store on Postgres");
    let shutdown = CancellationToken::new();
    let (own_cache, own_loop) = listening_node(store.clone(), &shutdown).await;
    let (peer_cache, peer_loop) = listening_node(peer, &shutdown).await;

    // The listeners subscribe asynchronously: wait until a notice gets through (the
    // first ticks have run, so only a notice can apply an event within 10 s).
    let warm = CacheKey::anon("example.com", "/warm/", "");
    let mut ready = false;
    for _ in 0..50 {
        cache_page(&peer_cache, warm.clone(), "warm").await;
        cache_page(&own_cache, warm.clone(), "warm").await;
        invalidate(store.as_ref(), "warm").await;
        if gone_within(&peer_cache, &warm, Duration::from_millis(200)).await
            && gone_within(&own_cache, &warm, Duration::from_millis(200)).await
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "listeners never became ready");

    for i in 0..5 {
        let dep = format!("post:{i}");
        let key = CacheKey::anon("example.com", format!("/{i}/"), "");
        let other = CacheKey::anon("example.com", format!("/other-{i}/"), "");
        cache_page(&peer_cache, key.clone(), &dep).await;
        cache_page(&peer_cache, other.clone(), "post:other").await;
        cache_page(&own_cache, key.clone(), &dep).await;
        invalidate(store.as_ref(), &dep).await;
        assert!(
            gone_within(&peer_cache, &key, Duration::from_secs(1)).await,
            "repetition {i}: the peer's page is still cached after 1 s"
        );
        assert!(
            gone_within(&own_cache, &key, Duration::from_secs(1)).await,
            "repetition {i}: the writer's own page is still cached after 1 s"
        );
        assert!(peer_cache.get(&other).await.is_some(), "only its keys");
    }

    // Subscriptions are dropped on the runtime, before the stores close.
    shutdown.cancel();
    own_loop.await.unwrap();
    peer_loop.await.unwrap();
    db.teardown().await;
}
