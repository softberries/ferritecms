//! `SiteReader` on both backends: published-only reads (Review Focus 1), hydration,
//! paging, dependency keys, preview, and the batch-loading call bound.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::CountingStore;
use ferrite_core::{
    Actor, CoreError, FeedSource, FrontPage, PageItem, PermalinkRules, SiteReader,
    events::{archive_key, option_key, post_key, term_key},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, Media, NewContent, NewUser,
    Role, Term, TermId, Timestamp, User, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

/// The fixed "now" of every reader in these tests: 2026-06-15T12:00:00Z.
const NOW: i64 = 1_781_524_800_000_000;
const HOUR: i64 = 3_600_000_000;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_micros(NOW + offset).unwrap()
}

fn reader(store: Arc<dyn Store>) -> SiteReader {
    SiteReader::new(
        store,
        Arc::new(PermalinkRules::default()),
        Arc::new(|hash: &ArtifactHash, name: &str| format!("/_media/{hash}/{name}")),
        Arc::new(|| at(0)),
    )
}

async fn user(store: &dyn Store, login: &str, role: Role) -> User {
    let tx = store.begin().await.unwrap();
    let u = tx
        .users()
        .insert(&NewUser {
            id: UserId::new(),
            login: login.into(),
            email: format!("{login}@example.com"),
            display_name: format!("{login} display"),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    u
}

fn actor(u: &User) -> Actor {
    Actor {
        user: u.id,
        role: u.role,
    }
}

struct Item {
    kind: &'static str,
    slug: String,
    status: ContentStatus,
    published_at: Option<Timestamp>,
    parent: Option<ContentId>,
    fields: Value,
    body: Value,
    excerpt: String,
    terms: Vec<TermId>,
}

fn item(kind: &'static str, slug: &str, status: ContentStatus, published: Option<i64>) -> Item {
    Item {
        kind,
        slug: slug.into(),
        status,
        published_at: published.map(at),
        parent: None,
        fields: json!({}),
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": format!("needle {slug} body")}]}
        ]}),
        excerpt: String::new(),
        terms: Vec::new(),
    }
}

/// Inserts the item directly (any status / publish time), sets its terms and indexes
/// it for search.
async fn insert(store: &dyn Store, author: &User, it: Item) -> ContentId {
    let id = ContentId::new();
    let tx = store.begin().await.unwrap();
    tx.content()
        .insert(&NewContent {
            id,
            kind: it.kind.into(),
            status: it.status,
            slug: it.slug.clone(),
            parent_id: it.parent,
            author_id: author.id,
            title: format!("Needle {}", it.slug),
            excerpt: it.excerpt,
            body: it.body,
            body_html: None,
            fields: it.fields,
            menu_order: 0,
            published_at: it.published_at,
            scheduled_at: (it.status == ContentStatus::Scheduled).then(|| at(HOUR)),
        })
        .await
        .unwrap();
    tx.terms().set_for_item(id, &it.terms).await.unwrap();
    tx.search()
        .index(id, &format!("Needle {}", it.slug), "needle body")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn term(store: &dyn Store, taxonomy: &str, slug: &str) -> TermId {
    let id = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id,
            taxonomy: taxonomy.into(),
            slug: slug.into(),
            name: format!("{slug} name"),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    id
}

async fn set_option(store: &dyn Store, key: &str, value: Value) {
    store
        .read()
        .options()
        .set(key, &value, false)
        .await
        .unwrap();
}

/// A media item in `status` with one artifact and a `thumbnail` variant.
async fn media(store: &dyn Store, author: &User, status: ContentStatus) -> ContentId {
    let hash = ArtifactHash::parse(&format!("{:0>64}", format!("{:x}", rand_u64()))).unwrap();
    let thumb = ArtifactHash::parse(&format!("{:f>64}", format!("{:x}", rand_u64()))).unwrap();
    let id = insert(
        store,
        author,
        item("media", &format!("m-{}", rand_u64()), status, Some(-HOUR)),
    )
    .await;
    let repos = store.read();
    for h in [&hash, &thumb] {
        repos
            .artifacts()
            .insert_if_absent(&Artifact {
                hash: h.clone(),
                kind: ArtifactKind::Media,
                bytes: 10,
                mime: "image/png".into(),
                created_at: at(0),
            })
            .await
            .unwrap();
    }
    repos
        .media()
        .insert(&Media {
            item_id: id,
            artifact_hash: hash,
            mime: "image/png".into(),
            bytes: 10,
            width: Some(800),
            height: Some(600),
            alt: "a cat".into(),
            original_name: "cat.png".into(),
            variants: json!({"thumbnail": {"hash": thumb.as_str(), "w": 150, "h": 150, "mime": "image/png"}}),
        })
        .await
        .unwrap();
    id
}

fn rand_u64() -> u64 {
    uuid::Uuid::now_v7().as_u64_pair().1
}

fn slugs(items: &[PageItem]) -> Vec<&str> {
    items.iter().map(|p| p.item.slug.as_str()).collect()
}

fn parts(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| (*s).to_owned()).collect()
}

// ------------------------------------------------------------ Review Focus 1

/// Hidden statuses, each also given the shared term, author and a past publish time so
/// only the status (or the future publish time) can keep them out.
const HIDDEN: [(&str, ContentStatus, Option<i64>); 6] = [
    ("draft", ContentStatus::Draft, Some(-HOUR)),
    ("pending", ContentStatus::Pending, Some(-HOUR)),
    ("scheduled", ContentStatus::Scheduled, Some(HOUR)),
    ("private", ContentStatus::Private, Some(-HOUR)),
    ("trash", ContentStatus::Trash, Some(-HOUR)),
    ("future", ContentStatus::Published, Some(HOUR)),
];

struct LeakSet {
    author: User,
    news: TermId,
    visible: ContentId,
    hidden: Vec<ContentId>,
}

async fn leak_set(store: &dyn Store) -> LeakSet {
    let author = user(store, "writer", Role::Author).await;
    let news = term(store, "category", "news").await;
    let mut visible = item("post", "visible", ContentStatus::Published, Some(-HOUR));
    visible.terms = vec![news];
    let visible = insert(store, &author, visible).await;
    let mut other = item("post", "other", ContentStatus::Published, Some(-2 * HOUR));
    other.terms = vec![news];
    insert(store, &author, other).await;
    let mut hidden = Vec::new();
    for (slug, status, published) in HIDDEN {
        let mut it = item("post", slug, status, published);
        it.terms = vec![news];
        hidden.push(insert(store, &author, it).await);
        // The same statuses as pages.
        insert(
            store,
            &author,
            item("page", &format!("page-{slug}"), status, published),
        )
        .await;
    }
    LeakSet {
        author,
        news,
        visible,
        hidden,
    }
}

const SHOWN: [&str; 2] = ["visible", "other"];

pub async fn no_leak_single(db: &TestDb) {
    let store = db.store();
    let set = leak_set(store).await;
    let r = reader(db.shared_store());
    for (slug, _, _) in HIDDEN {
        assert!(
            r.single("post", &parts(&[slug]))
                .await
                .unwrap()
                .value
                .is_none(),
            "post {slug} leaked"
        );
        let page = format!("page-{slug}");
        assert!(
            r.single("page", &parts(&[&page]))
                .await
                .unwrap()
                .value
                .is_none(),
            "page {slug} leaked"
        );
    }
    let shown = r.single("post", &parts(&["visible"])).await.unwrap();
    let shown = shown.value.unwrap();
    assert_eq!(shown.item.id, set.visible);
    assert_eq!(shown.url, "/visible/");
    assert!(shown.html.contains("needle visible body"));
}

pub async fn no_leak_listings(db: &TestDb) {
    let store = db.store();
    let set = leak_set(store).await;
    let r = reader(db.shared_store());

    let archive = r.archive("post", 1).await.unwrap().value.unwrap();
    assert_eq!(slugs(&archive.items), SHOWN);
    assert_eq!(archive.total, 2);
    let pages = r.archive("page", 1).await.unwrap().value.unwrap();
    assert!(pages.items.is_empty() && pages.total == 0);

    let FrontPage::Posts(front) = r.front_page(1).await.unwrap().value.unwrap() else {
        panic!("expected the post listing");
    };
    assert_eq!(slugs(&front.items), SHOWN);

    let by_term = r.term_archive("category", "news", 1).await.unwrap().value;
    assert_eq!(slugs(&by_term.unwrap().items), SHOWN);

    let by_author = r.author_archive("writer", 1).await.unwrap().value;
    let by_author = by_author.unwrap();
    assert_eq!(slugs(&by_author.items), SHOWN);
    assert_eq!(by_author.total, 2);

    for month in [Some(6), None] {
        let by_date = r.date_archive(2026, month, 1).await.unwrap().value.unwrap();
        assert_eq!(slugs(&by_date.items), SHOWN, "month {month:?}");
        assert_eq!(by_date.total, 2);
    }

    // Search orders by rank, not date.
    let found = r.search("needle", 1).await.unwrap().value.unwrap();
    let mut found = slugs(&found.items);
    found.sort_unstable();
    assert_eq!(found, ["other", "visible"]);

    let feed = r
        .feed(&FeedSource::Kind("post".into()))
        .await
        .unwrap()
        .value;
    assert_eq!(slugs(&feed.unwrap()), SHOWN);
    let term_feed = r
        .feed(&FeedSource::Term {
            taxonomy: "category".into(),
            slug: "news".into(),
        })
        .await
        .unwrap()
        .value;
    assert_eq!(slugs(&term_feed.unwrap()), SHOWN);

    let sitemap = r.sitemap("post", None).await.unwrap().value;
    let urls: Vec<&str> = sitemap.entries.iter().map(|e| e.url.as_str()).collect();
    assert_eq!(urls, ["/visible/", "/other/"]);
    assert!(sitemap.next_cursor.is_none());
    assert!(
        r.sitemap("page", None)
            .await
            .unwrap()
            .value
            .entries
            .is_empty()
    );

    let visible = r
        .single("post", &parts(&["visible"]))
        .await
        .unwrap()
        .value
        .unwrap();
    let related = r.related(&visible, 10).await.unwrap().value;
    assert_eq!(slugs(&related), ["other"]);

    // Hidden items hold the term too: nothing of theirs shows up in the deps either.
    for id in &set.hidden {
        assert!(!archive_deps(&r).await.contains(&post_key(*id)));
    }
    let _ = (set.author, set.news);
}

async fn archive_deps(r: &SiteReader) -> Vec<String> {
    r.archive("post", 1).await.unwrap().deps.into_vec()
}

/// The search index is not trusted for status or publish time: an item that is still
/// indexed under its old (published) text but has since been unpublished, trashed or
/// future-dated never comes back.
pub async fn search_never_trusts_index(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let shown = insert(
        store,
        &author,
        item("post", "kept", ContentStatus::Published, Some(-HOUR)),
    )
    .await;
    for (slug, status, published) in HIDDEN {
        let id = insert(store, &author, item("post", slug, status, published)).await;
        store
            .read()
            .search()
            .index(id, "needle stale", "needle")
            .await
            .unwrap();
    }
    // Media is not a public type.
    insert(
        store,
        &author,
        item(
            "media",
            "needle-media",
            ContentStatus::Published,
            Some(-HOUR),
        ),
    )
    .await;
    let r = reader(db.shared_store());
    let found = r.search("needle", 1).await.unwrap().value.unwrap();
    let ids: Vec<ContentId> = found.items.iter().map(|p| p.item.id).collect();
    assert_eq!(ids, [shown]);
    assert_eq!(found.total, 1, "total counts public matches only");
    assert!(
        r.search("   ", 1)
            .await
            .unwrap()
            .value
            .unwrap()
            .items
            .is_empty()
    );
}

/// R9: one search walks at most 200 matches, so `total` is capped.
pub async fn search_total_is_capped(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    // One transaction, no terms: cheap enough for 205 rows.
    let tx = store.begin().await.unwrap();
    for i in 0..205 {
        let id = ContentId::new();
        tx.content()
            .insert(&NewContent {
                id,
                kind: "post".into(),
                status: ContentStatus::Published,
                slug: format!("bulk-{i}"),
                parent_id: None,
                author_id: author.id,
                title: format!("Bulk {i}"),
                excerpt: String::new(),
                body: json!({"type": "doc", "content": []}),
                body_html: None,
                fields: json!({}),
                menu_order: 0,
                published_at: Some(at(-HOUR)),
                scheduled_at: None,
            })
            .await
            .unwrap();
        tx.search().index(id, "haystack", "").await.unwrap();
    }
    tx.commit().await.unwrap();
    let r = reader(db.shared_store());
    let first = r.search("haystack", 1).await.unwrap().value.unwrap();
    assert_eq!(first.total, ferrite_core::site::SEARCH_CAP as u64);
    assert_eq!(first.total, 200);
    assert_eq!(first.items.len(), 10);
    // Page 20 is the last page within the cap; page 21 lies beyond it.
    let last = r.search("haystack", 20).await.unwrap().value.unwrap();
    assert_eq!(last.items.len(), 10);
    assert!(
        r.search("haystack", 21)
            .await
            .unwrap()
            .value
            .unwrap()
            .items
            .is_empty()
    );
}

pub async fn related_never_unpublished(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let a = term(store, "category", "a").await;
    let b = term(store, "tag", "b").await;
    let mut base = item("post", "base", ContentStatus::Published, Some(-HOUR));
    base.terms = vec![a, b];
    insert(store, &author, base).await;
    for (slug, status, published) in HIDDEN {
        let mut it = item("post", slug, status, published);
        it.terms = vec![a, b];
        insert(store, &author, it).await;
    }
    let mut via_b = item("post", "via-b", ContentStatus::Published, Some(-3 * HOUR));
    via_b.terms = vec![b];
    insert(store, &author, via_b).await;
    let mut via_a = item("post", "via-a", ContentStatus::Published, Some(-2 * HOUR));
    via_a.terms = vec![a];
    insert(store, &author, via_a).await;
    // A page sharing the term is not related to a post.
    let mut page = item("page", "via-page", ContentStatus::Published, Some(-HOUR));
    page.terms = vec![a];
    insert(store, &author, page).await;
    insert(
        store,
        &author,
        item("post", "unrelated", ContentStatus::Published, Some(-HOUR)),
    )
    .await;

    let r = reader(db.shared_store());
    let base = r
        .single("post", &parts(&["base"]))
        .await
        .unwrap()
        .value
        .unwrap();
    let related = r.related(&base, 10).await.unwrap();
    assert_eq!(slugs(&related.value), ["via-a", "via-b"]);
    assert_eq!(slugs(&r.related(&base, 1).await.unwrap().value), ["via-a"]);
    assert!(r.related(&base, 0).await.unwrap().value.is_empty());
    let lonely = r
        .single("post", &parts(&["unrelated"]))
        .await
        .unwrap()
        .value
        .unwrap();
    assert!(r.related(&lonely, 10).await.unwrap().value.is_empty());
}

// ------------------------------------------------------------ single

/// Pages resolve by their full ancestor chain; a draft parent does not hide a
/// published child (WordPress behaviour), but a wrong chain is not found.
pub async fn single_walks_page_path(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let about = insert(
        store,
        &author,
        item("page", "about", ContentStatus::Draft, None),
    )
    .await;
    let mut team = item("page", "team", ContentStatus::Published, Some(-HOUR));
    team.parent = Some(about);
    let team = insert(store, &author, team).await;
    let mut kid = item("page", "kid", ContentStatus::Published, Some(-HOUR));
    kid.parent = Some(team);
    let kid = insert(store, &author, kid).await;
    insert(
        store,
        &author,
        item("page", "team", ContentStatus::Published, Some(-HOUR)),
    )
    .await;

    let r = reader(db.shared_store());
    let got = r.single("page", &parts(&["about", "team"])).await.unwrap();
    let page = got.value.unwrap();
    assert_eq!(page.item.id, team);
    assert_eq!(page.url, "/about/team/");
    assert!(got.deps.contains(&post_key(team)));
    assert!(got.deps.contains(&post_key(about)), "ancestor key recorded");
    assert!(got.cacheable);

    let deep = r
        .single("page", &parts(&["about", "team", "kid"]))
        .await
        .unwrap();
    assert_eq!(deep.value.unwrap().item.id, kid);
    assert_eq!(
        r.single("page", &parts(&["about", "team", "kid"]))
            .await
            .unwrap()
            .value
            .unwrap()
            .url,
        "/about/team/kid/"
    );
    // Accepted consequence of the WP-style rule: the sitemap lists the published
    // child with its draft ancestor's slug, but not the ancestor.
    let sitemap = r.sitemap("page", None).await.unwrap().value;
    let mut urls: Vec<&str> = sitemap.entries.iter().map(|e| e.url.as_str()).collect();
    urls.sort_unstable();
    assert_eq!(urls, ["/about/team/", "/about/team/kid/", "/team/"]);
    // The draft ancestor itself is not public.
    assert!(
        r.single("page", &parts(&["about"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
    // Wrong chains.
    assert!(
        r.single("page", &parts(&["kid"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        r.single("page", &parts(&["team", "kid"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        r.single("page", &parts(&["about", "kid"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(r.single("page", &[]).await.unwrap().value.is_none());
    // Top-level `team` is a different page.
    let top = r
        .single("page", &parts(&["team"]))
        .await
        .unwrap()
        .value
        .unwrap();
    assert_ne!(top.item.id, team);
    // Posts are flat; other kinds' slugs do not resolve as posts.
    assert!(
        r.single("post", &parts(&["about", "team"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        r.single("post", &parts(&["team"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
    // Media items are not rendered as singles.
    insert(
        store,
        &author,
        item("media", "pic", ContentStatus::Published, Some(-HOUR)),
    )
    .await;
    assert!(
        r.single("media", &parts(&["pic"]))
            .await
            .unwrap()
            .value
            .is_none()
    );
}

/// Author, terms, featured image, inline media, excerpt and the keys recorded.
pub async fn single_hydrates(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let news = term(store, "category", "news").await;
    let rust = term(store, "tag", "rust").await;
    let pic = media(store, &author, ContentStatus::Published).await;
    let hidden_pic = media(store, &author, ContentStatus::Draft).await;
    let gone = ContentId::new();
    let draft_post = insert(
        store,
        &author,
        item("post", "wip", ContentStatus::Draft, None),
    )
    .await;
    let mut it = item("post", "full", ContentStatus::Published, Some(-HOUR));
    it.terms = vec![rust, news];
    it.fields = json!({
        "featured_image": pic.to_string(),
        "color": "red",
        "refs": [hidden_pic.to_string(), draft_post.to_string(), gone.to_string()],
        "nested": {"x": hidden_pic.to_string(), "n": 3}
    });
    it.body = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Hello world"}]},
        {"type": "image", "attrs": {"src": format!("ferrite:media/{pic}"), "alt": "inline"}},
        {"type": "image", "attrs": {"src": format!("ferrite:media/{hidden_pic}"), "alt": "draft"}},
        {"type": "image", "attrs": {"src": format!("ferrite:media/{gone}"), "alt": "gone"}}
    ]});
    let id = insert(store, &author, it).await;
    let mut draft_featured = item(
        "post",
        "draft-featured",
        ContentStatus::Published,
        Some(-HOUR),
    );
    draft_featured.fields = json!({"featured_image": hidden_pic.to_string()});
    draft_featured.excerpt = "Stored excerpt".into();
    insert(store, &author, draft_featured).await;

    let r = reader(db.shared_store());
    let got = r.single("post", &parts(&["full"])).await.unwrap();
    let p = got.value.unwrap();
    assert_eq!(p.item.id, id);
    let a = p.author.unwrap();
    assert_eq!(
        (a.id, a.login.as_str(), a.display_name.as_str()),
        (author.id, "writer", "writer display")
    );
    let terms: Vec<(&str, &str, &str)> = p
        .terms
        .iter()
        .map(|t| (t.taxonomy.as_str(), t.slug.as_str(), t.url.as_str()))
        .collect();
    assert_eq!(
        terms,
        [
            ("tag", "rust", "/tag/rust/"),
            ("category", "news", "/category/news/")
        ]
    );
    assert_eq!(p.terms[1].name, "news name");
    let f = p.featured.unwrap();
    assert_eq!(f.id, pic);
    assert!(f.url.starts_with("/_media/") && f.url.ends_with("/cat.png"));
    assert_eq!(
        (f.alt.as_str(), f.width, f.height),
        ("a cat", Some(800), Some(600))
    );
    assert_eq!(f.variants.len(), 1);
    assert_eq!(f.variants[0].name, "thumbnail");
    assert_eq!(
        (f.variants[0].width, f.variants[0].height),
        (Some(150), Some(150))
    );
    assert_eq!(p.html.matches("<img").count(), 1, "{}", p.html);
    assert!(p.html.contains(&format!("src=\"{}\"", f.url)));
    assert!(p.html.contains("alt=\"inline\""));
    assert_eq!(p.item.excerpt, "Hello world");
    assert_eq!(
        p.item.fields,
        json!({
            "featured_image": pic.to_string(),
            "color": "red",
            "refs": [gone.to_string()],
            "nested": {"n": 3}
        }),
        "references to non-public content are stripped"
    );
    for key in [post_key(id), term_key(news), term_key(rust), post_key(pic)] {
        assert!(got.deps.contains(&key), "missing {key}");
    }
    assert!(!got.deps.contains(&post_key(hidden_pic)));

    let other = r
        .single("post", &parts(&["draft-featured"]))
        .await
        .unwrap()
        .value
        .unwrap();
    assert!(
        other.featured.is_none(),
        "an unpublished media item is not shown"
    );
    assert_eq!(other.item.excerpt, "Stored excerpt");
    assert_eq!(
        other.item.fields,
        json!({}),
        "unresolved featured_image removed"
    );
}

// ------------------------------------------------------------ listings

pub async fn listing_pages_and_deps(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    for i in 0..25 {
        insert(
            store,
            &author,
            item(
                "post",
                &format!("p{i:02}"),
                ContentStatus::Published,
                Some(-(i + 1) * HOUR),
            ),
        )
        .await;
    }
    let r = reader(db.shared_store());
    let first = r.archive("post", 1).await.unwrap();
    assert_eq!(
        (
            first.value.as_ref().unwrap().page,
            first.value.as_ref().unwrap().per_page,
            first.value.as_ref().unwrap().total
        ),
        (1, 10, 25)
    );
    assert_eq!(first.value.as_ref().unwrap().items.len(), 10);
    assert_eq!(first.value.as_ref().unwrap().items[0].item.slug, "p00");
    for key in [archive_key("post"), option_key("reading.per_page")] {
        assert!(first.deps.contains(&key), "missing {key}");
    }
    for p in &first.value.as_ref().unwrap().items {
        assert!(first.deps.contains(&post_key(p.item.id)));
    }
    let third = r.archive("post", 3).await.unwrap().value.unwrap();
    assert_eq!(slugs(&third.items), ["p20", "p21", "p22", "p23", "p24"]);
    assert!(
        r.archive("post", 4)
            .await
            .unwrap()
            .value
            .unwrap()
            .items
            .is_empty()
    );
    // Deep paging is capped at 5 000 skipped rows (R6): page 501 skips exactly 5 000.
    assert!(
        r.archive("post", 501)
            .await
            .unwrap()
            .value
            .unwrap()
            .items
            .is_empty()
    );
    assert!(r.archive("post", 502).await.unwrap().value.is_none());
    assert!(r.front_page(502).await.unwrap().value.is_none());
    assert!(r.search("needle", 502).await.unwrap().value.is_none());
    assert!(
        r.date_archive(2026, None, 502)
            .await
            .unwrap()
            .value
            .is_none()
    );
    // Page 0 is page 1.
    assert_eq!(r.archive("post", 0).await.unwrap().value.unwrap().page, 1);

    set_option(store, "reading.per_page", json!(4)).await;
    let small = r.archive("post", 2).await.unwrap().value.unwrap();
    assert_eq!(
        (small.per_page, slugs(&small.items)),
        (4, vec!["p04", "p05", "p06", "p07"])
    );
    set_option(store, "reading.per_page", json!(500)).await;
    assert_eq!(
        r.archive("post", 1).await.unwrap().value.unwrap().per_page,
        100
    );
    set_option(store, "reading.per_page", json!(0)).await;
    assert_eq!(
        r.archive("post", 1).await.unwrap().value.unwrap().per_page,
        1
    );
    set_option(store, "reading.per_page", json!("lots")).await;
    assert_eq!(
        r.archive("post", 1).await.unwrap().value.unwrap().per_page,
        10
    );
}

/// Hidden items between public ones never count or shift pages.
pub async fn listing_with_interleaved_hidden(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let news = term(store, "category", "news").await;
    for i in 0..12i64 {
        let mut shown = item(
            "post",
            &format!("s{i:02}"),
            ContentStatus::Published,
            Some(-(2 * i + 1) * HOUR),
        );
        shown.terms = vec![news];
        insert(store, &author, shown).await;
        let (_, status, _) = HIDDEN[(i as usize) % HIDDEN.len()];
        let published = if status == ContentStatus::Published {
            HOUR
        } else {
            -(2 * i + 2) * HOUR
        };
        let mut hidden = item("post", &format!("h{i:02}"), status, Some(published));
        hidden.terms = vec![news];
        insert(store, &author, hidden).await;
    }
    set_option(store, "reading.per_page", json!(5)).await;
    let r = reader(db.shared_store());
    let expect: Vec<String> = (0..12).map(|i| format!("s{i:02}")).collect();
    let mut seen = Vec::new();
    for page in 1..=3 {
        // Page 1 and 2 have a next page, so their total comes from `count`.
        let l = r.archive("post", page).await.unwrap().value.unwrap();
        assert_eq!(l.total, 12, "page {page}");
        let t = r
            .term_archive("category", "news", page)
            .await
            .unwrap()
            .value
            .unwrap();
        assert_eq!(t.total, 12, "term page {page}");
        assert_eq!(slugs(&t.items), slugs(&l.items));
        seen.extend(l.items.iter().map(|p| p.item.slug.clone()));
    }
    assert_eq!(seen, expect);
}

pub async fn archives_by_term_author_date(db: &TestDb) {
    let store = db.store();
    let writer = user(store, "writer", Role::Author).await;
    let other = user(store, "other", Role::Author).await;
    let news = term(store, "category", "news").await;
    let mut may = item("post", "may", ContentStatus::Published, None);
    may.published_at = Some(Timestamp::from_micros(NOW - 30 * 24 * HOUR).unwrap());
    may.terms = vec![news];
    insert(store, &writer, may).await;
    let mut june = item("post", "june", ContentStatus::Published, Some(-HOUR));
    june.terms = vec![news];
    insert(store, &other, june).await;
    let mut last_year = item("post", "last-year", ContentStatus::Published, None);
    last_year.published_at = Some(Timestamp::from_micros(NOW - 400 * 24 * HOUR).unwrap());
    insert(store, &writer, last_year).await;
    // Pages are not part of author or date archives.
    insert(
        store,
        &writer,
        item("page", "writer-page", ContentStatus::Published, Some(-HOUR)),
    )
    .await;

    let r = reader(db.shared_store());
    let t = r.term_archive("category", "news", 1).await.unwrap();
    assert_eq!(slugs(&t.value.as_ref().unwrap().items), ["june", "may"]);
    assert!(t.deps.contains(&term_key(news)));
    assert!(t.deps.contains(&archive_key("post")));
    assert!(
        r.term_archive("category", "nope", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        r.term_archive("tag", "news", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );

    let t = t.value.unwrap();
    assert_eq!(t.total, 2);

    // R7: a user or tag with only non-public posts looks unknown.
    let drafter = user(store, "drafter", Role::Author).await;
    let secret = term(store, "tag", "secret").await;
    for (slug, status, published) in HIDDEN {
        let mut it = item("post", &format!("d-{slug}"), status, published);
        it.terms = vec![secret];
        insert(store, &drafter, it).await;
    }
    assert!(
        r.author_archive("drafter", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        r.term_archive("tag", "secret", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );
    let secret_feed = FeedSource::Term {
        taxonomy: "tag".into(),
        slug: "secret".into(),
    };
    assert!(r.feed(&secret_feed).await.unwrap().value.is_none());
    // Past the last page of a non-empty archive is an empty page, not None.
    let beyond = r.author_archive("writer", 9).await.unwrap().value.unwrap();
    assert!(beyond.items.is_empty() && beyond.total == 2);

    let w = r.author_archive("writer", 1).await.unwrap().value.unwrap();
    assert_eq!(slugs(&w.items), ["may", "last-year"]);
    assert!(r.author_archive("nobody", 1).await.unwrap().value.is_none());

    let june = r
        .date_archive(2026, Some(6), 1)
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(slugs(&june.items), ["june"]);
    let may = r
        .date_archive(2026, Some(5), 1)
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(slugs(&may.items), ["may"]);
    let year = r.date_archive(2026, None, 1).await.unwrap().value.unwrap();
    assert_eq!(slugs(&year.items), ["june", "may"]);
    let prev = r.date_archive(2025, None, 1).await.unwrap().value.unwrap();
    assert_eq!(slugs(&prev.items), ["last-year"]);
    for bad in [Some(0), Some(13)] {
        assert!(r.date_archive(2026, bad, 1).await.unwrap().value.is_none());
    }
    assert!(
        r.date_archive(i32::MAX, None, 1)
            .await
            .unwrap()
            .value
            .is_none()
    );
}

pub async fn front_page_option(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    insert(
        store,
        &author,
        item("post", "a-post", ContentStatus::Published, Some(-HOUR)),
    )
    .await;
    let home = insert(
        store,
        &author,
        item("page", "home", ContentStatus::Published, Some(-HOUR)),
    )
    .await;
    let draft = insert(
        store,
        &author,
        item("page", "wip", ContentStatus::Draft, None),
    )
    .await;
    let r = reader(db.shared_store());

    set_option(store, "reading.front_page", json!(home.to_string())).await;
    let got = r.front_page(1).await.unwrap();
    let FrontPage::Page(p) = got.value.unwrap() else {
        panic!("expected the static front page")
    };
    assert_eq!(p.item.id, home);
    assert!(got.deps.contains(&option_key("reading.front_page")));
    assert!(got.deps.contains(&post_key(home)));

    for unusable in [json!(draft.to_string()), json!("not-an-id"), json!(7)] {
        set_option(store, "reading.front_page", unusable).await;
        let got = r.front_page(1).await.unwrap();
        let FrontPage::Posts(l) = got.value.unwrap() else {
            panic!("expected the post listing")
        };
        assert_eq!(slugs(&l.items), ["a-post"]);
        assert!(got.deps.contains(&option_key("reading.front_page")));
    }
}

// ------------------------------------------------------------ preview

pub async fn preview_is_gated(db: &TestDb) {
    let store = db.store();
    let writer = user(store, "writer", Role::Contributor).await;
    let editor = user(store, "editor", Role::Editor).await;
    let stranger = user(store, "stranger", Role::Author).await;
    let r = reader(db.shared_store());
    for (slug, status, published) in HIDDEN {
        let id = insert(store, &writer, item("post", slug, status, published)).await;
        let got = r.preview(&actor(&editor), id).await;
        if status == ContentStatus::Trash {
            assert!(matches!(got, Err(CoreError::NotFound)), "{slug}");
            continue;
        }
        let got = got.unwrap();
        assert_eq!(got.value.item.id, id);
        assert!(!got.cacheable, "previews are never cached");
        assert!(matches!(
            r.preview(&actor(&stranger), id).await,
            Err(CoreError::Forbidden("edit_others"))
        ));
    }
    let own = insert(
        store,
        &writer,
        item("post", "own", ContentStatus::Draft, None),
    )
    .await;
    assert_eq!(
        r.preview(&actor(&writer), own).await.unwrap().value.item.id,
        own
    );
    assert!(matches!(
        r.preview(&actor(&editor), ContentId::new()).await,
        Err(CoreError::NotFound)
    ));
}

// ------------------------------------------------------------ batch loading

/// Store calls per listing do not grow with the number of items (no N+1).
pub async fn listing_call_count_is_constant(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let news = term(store, "category", "news").await;
    let pic = media(store, &author, ContentStatus::Published).await;
    let counting = Arc::new(CountingStore::new(db.shared_store()));
    let r = reader(counting.clone());

    // An older item that is always on a later page, so every listing needs its count.
    insert(
        store,
        &author,
        item("post", "oldest", ContentStatus::Published, Some(-10 * HOUR)),
    )
    .await;
    let mut calls = Vec::new();
    for batch in [3u32, 27] {
        for i in 0..batch {
            let mut it = item(
                "post",
                &format!("b{batch}-{i}"),
                ContentStatus::Published,
                Some(-HOUR),
            );
            it.terms = vec![news];
            it.fields = json!({"featured_image": pic.to_string()});
            insert(store, &author, it).await;
        }
        set_option(store, "reading.per_page", json!(batch)).await;
        counting.reset();
        let listing = r.archive("post", 1).await.unwrap().value.unwrap();
        assert_eq!(listing.items.len(), batch as usize);
        assert!(listing.total > u64::from(batch));
        assert!(listing.items.iter().all(|p| p.featured.is_some()));
        calls.push(counting.calls());
    }
    assert_eq!(calls[0], calls[1], "calls grow with item count");
    assert!(calls[0] <= 7, "{calls:?}");

    // An empty listing short-circuits all batch loads.
    counting.reset();
    assert!(
        r.archive("nothing", 1)
            .await
            .unwrap()
            .value
            .unwrap()
            .items
            .is_empty()
    );
    assert!(counting.calls() <= 3, "{}", counting.calls());
}

backend_tests!(
    no_leak_single,
    no_leak_listings,
    search_never_trusts_index,
    search_total_is_capped,
    related_never_unpublished,
    single_walks_page_path,
    single_hydrates,
    listing_pages_and_deps,
    listing_with_interleaved_hidden,
    archives_by_term_author_date,
    front_page_option,
    preview_is_gated,
    listing_call_count_is_constant,
);
