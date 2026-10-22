//! Review Focus 2 at the cache level, on both backends: real content writes through
//! `ContentService`, pages cached under the dependency keys `SiteReader` records, one
//! `poll_once`, and every affected page is gone (and re-renders with the new data)
//! while unrelated pages stay cached. The HTTP-level version is in `ferrite-http`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use bytes::Bytes;
use ferrite_cache::{CacheKey, CachedPage, InvalidationLoop, PageCache, etag_for};
use ferrite_core::{
    Actor, ContentDraft, ContentService, FeedSource, FrontPage, PageItem, PermalinkRules,
    SiteReader,
    events::{PLUGINS_KEY, THEME_KEY},
};
use ferrite_domain::{
    ArtifactHash, ContentItem, NewUser, Role, Term, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

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
        let page = render(r, v)
            .await
            .unwrap_or_else(|| panic!("{v:?} renders"));
        assert!(cache.insert(cache_key(v), page).await);
    }
}

async fn cached_body(cache: &PageCache, view: View) -> Option<String> {
    cache
        .get(&cache_key(view))
        .await
        .map(|p| String::from_utf8(p.body.to_vec()).unwrap())
}

/// The cached body if present, else a fresh render that is then cached (what the
/// HTTP layer does on a miss).
async fn serve(cache: &PageCache, r: &SiteReader, view: View) -> Option<String> {
    if let Some(body) = cached_body(cache, view).await {
        return Some(body);
    }
    let page = render(r, view).await?;
    let body = String::from_utf8(page.body.to_vec()).unwrap();
    cache.insert(cache_key(view), page).await;
    Some(body)
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
