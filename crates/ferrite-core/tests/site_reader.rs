//! `SiteReader` on both backends: published-only reads (Review Focus 1), hydration,
//! paging, dependency keys, preview, and the batch-loading call bound.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, CoreError, FeedSource, FrontPage, PageItem, PermalinkRules, SiteRead, SiteReader,
    events::{archive_key, option_key, post_key, term_key, user_key},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, ContentType, Media, NewContent,
    NewUser, Role, Taxonomy, Term, TermId, Timestamp, User, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{CountingStore, TestDb, backend_tests};
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

/// The author archive's listing alone (`author_archive_page` without the author).
async fn author_archive(
    r: &SiteReader,
    login: &str,
    page: u32,
) -> Result<SiteRead<Option<ferrite_core::Listing>>, CoreError> {
    Ok(r.author_archive_page(login, page)
        .await?
        .map(|v| v.map(|(_, l)| l)))
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
    Actor::new(u.id, u.role)
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
            created_at: None,
            updated_at: None,
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
                orphaned_at: None,
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
            sanitized: false,
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

/// The menu reads (final review I3): `links` returns public items only and records
/// `post:<id>` for every id; `term_link` needs a public post and records its keys.
pub async fn no_leak_menu_links(db: &TestDb) {
    let store = db.store();
    let set = leak_set(store).await;
    let m = media(store, &set.author, ContentStatus::Published).await;
    let empty = term(store, "category", "empty").await;
    let mut only_draft = item("post", "only-draft", ContentStatus::Draft, Some(-HOUR));
    only_draft.terms = vec![empty];
    insert(store, &set.author, only_draft).await;
    let r = reader(db.shared_store());

    let mut ids = set.hidden.clone();
    ids.extend([m, set.visible, ContentId::new()]);
    let links = r.links(&ids).await.unwrap();
    assert_eq!(links.value.len(), 1, "{:?}", links.value);
    let link = &links.value[&set.visible];
    assert_eq!(
        (link.title.as_str(), link.url.as_str()),
        ("Needle visible", "/visible/")
    );
    for id in &ids {
        assert!(links.deps.contains(&post_key(*id)), "{id}");
    }
    assert!(links.deps.contains(&term_key(set.news)));
    assert!(r.links(&[]).await.unwrap().value.is_empty());

    let news = r.term_link(set.news).await.unwrap();
    assert_eq!(news.value.unwrap().url, "/category/news/");
    for t in [empty, TermId::new()] {
        let hidden = r.term_link(t).await.unwrap();
        assert!(hidden.value.is_none(), "{t}");
        assert!(hidden.deps.contains(&term_key(t)));
        assert!(hidden.deps.contains(&archive_key("post")));
    }
}

/// The id- and offset-based reads behind plugin `content.read` (Review Focus 1).
pub async fn no_leak_by_id_and_latest(db: &TestDb) {
    let store = db.store();
    let set = leak_set(store).await;
    let author = set.author.clone();
    let m = media(store, &author, ContentStatus::Published).await;
    let r = reader(db.shared_store());
    for id in set.hidden.iter().copied().chain([m]) {
        let read = r.by_id(id).await.unwrap();
        assert!(read.value.is_none(), "{id} leaked");
        // Recorded even while hidden: publishing it must refresh the page.
        assert!(read.deps.contains(&post_key(id)));
        assert!(!r.is_visible(id).await.unwrap(), "{id} visible");
    }
    let shown = r.by_id(set.visible).await.unwrap();
    assert!(shown.cacheable);
    let shown = shown.value.unwrap();
    assert_eq!(shown.item.slug, "visible");
    assert_eq!(shown.url, "/visible/");
    assert!(r.is_visible(set.visible).await.unwrap());
    let refused = r.by_id_where(set.visible, &|k| k == "page").await.unwrap();
    assert!(refused.value.is_none(), "type filter applies");
    assert!(refused.deps.contains(&post_key(set.visible)));
    assert!(
        r.by_id_where(set.visible, &|k| k == "post")
            .await
            .unwrap()
            .value
            .is_some()
    );
    assert!(!r.is_visible(ContentId::new()).await.unwrap());

    let latest = r.latest("post", 10, 0).await.unwrap();
    assert_eq!(slugs(&latest.value), SHOWN);
    assert!(latest.deps.contains(&archive_key("post")));
    for p in &latest.value {
        assert!(latest.deps.contains(&post_key(p.item.id)));
    }
    assert_eq!(
        slugs(&r.latest("post", 10, 1).await.unwrap().value),
        ["other"]
    );
    assert_eq!(
        slugs(&r.latest("post", 1, 0).await.unwrap().value),
        ["visible"]
    );
    assert!(r.latest("post", 10, 2).await.unwrap().value.is_empty());
    assert!(r.latest("page", 10, 0).await.unwrap().value.is_empty());
    assert!(r.latest("media", 10, 0).await.unwrap().value.is_empty());
    assert!(r.latest("post", 0, 0).await.unwrap().value.is_empty());
    // Past the paging cap: nothing, like deep archive pages.
    let deep = u32::try_from(ferrite_core::site::MAX_SKIP).unwrap() + 1;
    assert!(r.latest("post", 10, deep).await.unwrap().value.is_empty());
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

    let by_author = author_archive(&r, "writer", 1).await.unwrap().value;
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
                created_at: None,
                updated_at: None,
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
    for key in [
        post_key(id),
        term_key(news),
        term_key(rust),
        post_key(pic),
        user_key(author.id),
    ] {
        assert!(got.deps.contains(&key), "missing {key}");
    }
    // Hidden references are dependencies too: publishing one must refresh the page.
    assert!(got.deps.contains(&post_key(hidden_pic)));

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
        author_archive(&r, "drafter", 1)
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
    let beyond = author_archive(&r, "writer", 9)
        .await
        .unwrap()
        .value
        .unwrap();
    assert!(beyond.items.is_empty() && beyond.total == 2);

    let w = author_archive(&r, "writer", 1)
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(slugs(&w.items), ["may", "last-year"]);
    assert!(
        author_archive(&r, "nobody", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );

    // R30: the `_page` variants return the term/author with the same listing, only
    // when the plain archive exists (no oracle), recording their keys.
    let tp = r.term_archive_page("category", "news", 1).await.unwrap();
    let (news_term, l) = tp.value.unwrap();
    assert_eq!((news_term.id, news_term.slug.as_str()), (news, "news"));
    assert_eq!(news_term.url, "/category/news/");
    assert_eq!(slugs(&l.items), ["june", "may"]);
    assert!(tp.deps.contains(&term_key(news)));
    assert!(
        r.term_archive_page("tag", "secret", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );
    let ap = r.author_archive_page("writer", 1).await.unwrap();
    let (author, l) = ap.value.unwrap();
    assert_eq!((author.id, author.login.as_str()), (writer.id, "writer"));
    assert_eq!(slugs(&l.items), ["may", "last-year"]);
    assert!(ap.deps.contains(&user_key(writer.id)));
    for login in ["drafter", "nobody"] {
        assert!(
            r.author_archive_page(login, 1)
                .await
                .unwrap()
                .value
                .is_none(),
            "{login}"
        );
    }

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
    // R27: a date archive without public items is None, like an empty term (R7).
    for (y, m) in [(1999, None), (2026, Some(7)), (2026, Some(1))] {
        assert!(
            r.date_archive(y, m, 1).await.unwrap().value.is_none(),
            "{y} {m:?}"
        );
    }
    // Past the last page of a non-empty date archive: an empty page.
    let beyond = r.date_archive(2026, None, 9).await.unwrap().value.unwrap();
    assert!(beyond.items.is_empty() && beyond.total == 2);
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
        assert_eq!(got.value.item().item.id, id);
        assert_eq!(got.value.status(), status, "real status carried");
        assert!(!got.cacheable, "previews are never cached");
        // R17/R23 `can_read`: another author without `edit_others` sees none of the
        // hidden items, the future-dated published one included (not public yet).
        assert!(
            matches!(
                r.preview(&actor(&stranger), id).await,
                Err(CoreError::Forbidden("edit_others"))
            ),
            "{slug}"
        );
        // The owner previews it in every status.
        assert_eq!(
            r.preview(&actor(&writer), id)
                .await
                .unwrap()
                .value
                .item()
                .item
                .id,
            id,
            "{slug}"
        );
    }
    // The contributor previews nobody else's draft, but published items of others.
    let editors = insert(
        store,
        &editor,
        item("post", "editors-draft", ContentStatus::Draft, None),
    )
    .await;
    assert!(matches!(
        r.preview(&actor(&writer), editors).await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    let published = insert(
        store,
        &editor,
        item(
            "post",
            "editors-post",
            ContentStatus::Published,
            Some(-HOUR),
        ),
    )
    .await;
    assert_eq!(
        r.preview(&actor(&writer), published)
            .await
            .unwrap()
            .value
            .item()
            .item
            .id,
        published
    );
    let own = insert(
        store,
        &writer,
        item("post", "own", ContentStatus::Draft, None),
    )
    .await;
    assert_eq!(
        r.preview(&actor(&writer), own)
            .await
            .unwrap()
            .value
            .item()
            .item
            .id,
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

/// A page past the last one costs one count, not a cursor walk over the earlier pages
/// (final review I1): it is an empty page whose `total` is the count.
pub async fn past_last_page_is_one_count(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    // More rows than one walk step (200), so walking would take several list calls.
    for i in 0..201 {
        insert(
            store,
            &author,
            item(
                "post",
                &format!("p{i}"),
                ContentStatus::Published,
                Some(-HOUR),
            ),
        )
        .await;
    }
    set_option(store, "reading.per_page", json!(100)).await;
    let counting = Arc::new(CountingStore::new(db.shared_store()));
    let r = reader(counting.clone());

    counting.reset();
    let beyond = r.archive("post", 4).await.unwrap().value.unwrap();
    assert!(
        beyond.items.is_empty() && beyond.total == 201,
        "{}",
        beyond.total
    );
    assert!(
        counting.calls() <= 2,
        "per_page + count, got {}",
        counting.calls()
    );

    // The last page still walks to its rows and keeps the count as its total.
    let last = r.archive("post", 3).await.unwrap().value.unwrap();
    assert_eq!((last.items.len(), last.total), (1, 201));
    let middle = r.archive("post", 2).await.unwrap().value.unwrap();
    assert_eq!((middle.items.len(), middle.total), (100, 201));
}

/// `media(id)`: only published media items (R9/I4, the media rule); `post:<id>` is
/// recorded either way; non-media items are `None`.
pub async fn media_is_published_only(db: &TestDb) {
    let store = db.shared_store();
    let r = reader(store.clone());
    let author = user(store.as_ref(), "mia", Role::Author).await;
    let public = media(store.as_ref(), &author, ContentStatus::Published).await;
    let draft = media(store.as_ref(), &author, ContentStatus::Draft).await;
    let post = insert(
        store.as_ref(),
        &author,
        item("post", "not-media", ContentStatus::Published, Some(-HOUR)),
    )
    .await;

    let read = r.media(public).await.unwrap();
    assert!(read.deps.contains(&post_key(public)));
    let m = read.value.expect("published media");
    assert_eq!(m.id, public);
    assert_eq!(m.alt, "a cat");
    assert_eq!(m.width, Some(800));
    assert!(m.url.ends_with("/cat.png"), "{}", m.url);
    assert_eq!(m.variants.len(), 1);
    assert_eq!(m.variants[0].name, "thumbnail");

    let read = r.media(draft).await.unwrap();
    assert!(read.value.is_none(), "draft media must not resolve");
    assert!(read.deps.contains(&post_key(draft)));
    assert!(r.media(post).await.unwrap().value.is_none(), "not media");
    assert!(r.media(ContentId::new()).await.unwrap().value.is_none());
}

/// `media_file(hash)`: the original or a variant of a public media item resolves; a
/// draft's or an unknown hash does not; a hash shared with a draft still resolves
/// through the published row.
pub async fn media_file_is_published_only(db: &TestDb) {
    let store = db.shared_store();
    let r = reader(store.clone());
    let author = user(store.as_ref(), "mfa", Role::Author).await;
    let public = media(store.as_ref(), &author, ContentStatus::Published).await;
    let draft = media(store.as_ref(), &author, ContentStatus::Draft).await;
    let row = store.read().media().get(public).await.unwrap().unwrap();
    let thumb = ArtifactHash::parse(row.variants["thumbnail"]["hash"].as_str().unwrap()).unwrap();

    let f = r
        .media_file(&row.artifact_hash)
        .await
        .unwrap()
        .expect("original");
    assert_eq!(
        (f.item, f.mime.as_str(), f.name.as_str()),
        (public, "image/png", "cat.png")
    );
    assert_eq!(f.variant, None);
    let f = r.media_file(&thumb).await.unwrap().expect("variant");
    assert_eq!((f.item, f.variant.as_deref()), (public, Some("thumbnail")));
    assert_eq!(f.mime, "image/png");

    let hidden = store.read().media().get(draft).await.unwrap().unwrap();
    assert!(r.media_file(&hidden.artifact_hash).await.unwrap().is_none());
    let unknown = ArtifactHash::parse(&"9".repeat(64)).unwrap();
    assert!(r.media_file(&unknown).await.unwrap().is_none());

    // A draft media item with the same original file: still served (by the public one).
    let twin = insert(
        store.as_ref(),
        &author,
        item("media", "twin", ContentStatus::Draft, Some(-HOUR)),
    )
    .await;
    let mut shared = row.clone();
    shared.item_id = twin;
    store.read().media().insert(&shared).await.unwrap();
    let f = r
        .media_file(&row.artifact_hash)
        .await
        .unwrap()
        .expect("shared");
    assert_eq!(f.item, public);
}

/// `sitemap_files`: `ceil(public items / file length)` from one count; with a short
/// file length the files split at that size (`with_sitemap_file_len`).
pub async fn sitemap_files_count_and_split(db: &TestDb) {
    let store = db.shared_store();
    let r = reader(store.clone()).with_sitemap_file_len(2);
    let author = user(store.as_ref(), "smw", Role::Author).await;
    for slug in ["s1", "s2", "s3"] {
        insert(
            store.as_ref(),
            &author,
            item("post", slug, ContentStatus::Published, Some(-HOUR)),
        )
        .await;
    }
    insert(
        store.as_ref(),
        &author,
        item("post", "s-draft", ContentStatus::Draft, Some(-HOUR)),
    )
    .await;
    insert(
        store.as_ref(),
        &author,
        item("post", "s-later", ContentStatus::Published, Some(HOUR)),
    )
    .await;
    let files = r.sitemap_files("post").await.unwrap();
    assert_eq!(files.value, 2);
    assert!(files.deps.contains(&archive_key("post")));
    assert_eq!(r.sitemap_files("page").await.unwrap().value, 0);
    assert_eq!(r.sitemap_files("media").await.unwrap().value, 0);
    let first = r.sitemap("post", None).await.unwrap().value;
    assert_eq!(first.entries.len(), 2);
    let second = r
        .sitemap("post", first.next_cursor.as_deref())
        .await
        .unwrap()
        .value;
    assert_eq!(second.entries.len(), 1);
    assert!(second.next_cursor.is_none());
    // The default length: one file.
    assert_eq!(
        reader(store.clone())
            .sitemap_files("post")
            .await
            .unwrap()
            .value,
        1
    );
}

/// A `book` type with a `genre` taxonomy used by books only.
async fn book_genre(store: &dyn Store) {
    store
        .read()
        .types()
        .upsert(&ContentType {
            name: "book".into(),
            label: "Books".into(),
            hierarchical: false,
            has_archive: true,
            fields: json!({}),
            built_in: false,
            created_at: at(-HOUR),
        })
        .await
        .unwrap();
    store
        .read()
        .taxonomies()
        .upsert(&Taxonomy {
            name: "genre".into(),
            label: "Genres".into(),
            hierarchical: false,
            content_types: vec!["book".into()],
            built_in: false,
        })
        .await
        .unwrap();
}

/// P25: a term archive lists every public type of its taxonomy, so a taxonomy used
/// only by `book` has one; its keys cover each listed type.
pub async fn custom_type_term_archive(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    book_genre(store).await;
    let scifi = term(store, "genre", "scifi").await;
    let mut book = item("book", "dune", ContentStatus::Published, Some(-HOUR));
    book.terms = vec![scifi];
    insert(store, &author, book).await;
    // A post carrying the term is not of a type the taxonomy lists.
    let mut post = item("post", "stray", ContentStatus::Published, Some(-HOUR));
    post.terms = vec![scifi];
    insert(store, &author, post).await;
    let mut draft = item("book", "unwritten", ContentStatus::Draft, None);
    draft.terms = vec![scifi];
    insert(store, &author, draft).await;
    let r = reader(db.shared_store());

    let read = r.term_archive_page("genre", "scifi", 1).await.unwrap();
    let (t, listing) = read.value.unwrap();
    assert_eq!((t.id, t.url.as_str()), (scifi, "/genre/scifi/"));
    assert_eq!(slugs(&listing.items), ["dune"]);
    assert_eq!(listing.total, 1);
    assert!(read.deps.contains(&archive_key("book")));
    assert!(read.deps.contains(&term_key(scifi)));
    assert!(!read.deps.contains(&archive_key("post")));
    // The menu probe agrees.
    let link = r.term_link(scifi).await.unwrap();
    assert_eq!(link.value.unwrap().url, "/genre/scifi/");
    assert!(link.deps.contains(&archive_key("book")));

    // Both of two types are listed, newest first.
    store
        .read()
        .taxonomies()
        .upsert(&Taxonomy {
            name: "genre".into(),
            label: "Genres".into(),
            hierarchical: false,
            content_types: vec!["book".into(), "post".into(), "media".into()],
            built_in: false,
        })
        .await
        .unwrap();
    let read = r.term_archive("genre", "scifi", 1).await.unwrap();
    let listing = read.value.unwrap();
    let mut listed = slugs(&listing.items);
    listed.sort();
    assert_eq!(listed, ["dune", "stray"]);
    for k in ["book", "post"] {
        assert!(read.deps.contains(&archive_key(k)), "{k}");
    }
    // Media is never listed, so a media-only taxonomy has no archive.
    store
        .read()
        .taxonomies()
        .upsert(&Taxonomy {
            name: "genre".into(),
            label: "Genres".into(),
            hierarchical: false,
            content_types: vec!["media".into()],
            built_in: false,
        })
        .await
        .unwrap();
    assert!(
        r.term_archive("genre", "scifi", 1)
            .await
            .unwrap()
            .value
            .is_none()
    );
    assert!(r.term_link(scifi).await.unwrap().value.is_none());
}

/// P25: the theme's `query(type, term, limit)` reads exactly `limit` items of the
/// requested kind in one query, whatever `reading.per_page` says.
pub async fn term_items_respects_limit(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    book_genre(store).await;
    set_option(store, "reading.per_page", json!(10)).await;
    let news = term(store, "category", "news").await;
    let scifi = term(store, "genre", "scifi").await;
    for i in 0..35_i64 {
        let mut it = item(
            "post",
            &format!("p{i:02}"),
            ContentStatus::Published,
            Some(-HOUR - i),
        );
        it.terms = vec![news];
        insert(store, &author, it).await;
    }
    let mut it = item("post", "hidden", ContentStatus::Draft, None);
    it.terms = vec![news];
    insert(store, &author, it).await;
    let mut it = item("book", "dune", ContentStatus::Published, Some(-HOUR));
    it.terms = vec![scifi];
    insert(store, &author, it).await;

    let counting = Arc::new(CountingStore::new(db.shared_store()));
    let r = reader(counting.clone());
    let read = r.term_items("post", "category", "news", 30).await.unwrap();
    assert_eq!(read.value.len(), 30);
    let expected: Vec<String> = (0..30).map(|i| format!("p{i:02}")).collect();
    assert_eq!(slugs(&read.value), expected);
    assert!(read.deps.contains(&archive_key("post")));
    assert!(read.deps.contains(&term_key(news)));
    assert!(!read.deps.contains(&option_key("reading.per_page")));
    // Term, taxonomy, the one content query, then hydration (terms and users).
    assert!(counting.calls() <= 5, "{} calls", counting.calls());

    // Any kind the taxonomy lists works; another kind, an unknown term or taxonomy
    // returns nothing.
    let books = r.term_items("book", "genre", "scifi", 5).await.unwrap();
    assert_eq!(slugs(&books.value), ["dune"]);
    assert!(books.deps.contains(&archive_key("book")));
    let wrong = r.term_items("post", "genre", "scifi", 5).await.unwrap();
    assert!(wrong.value.is_empty());
    assert!(wrong.deps.contains(&archive_key("post")));
    assert!(wrong.deps.contains(&term_key(scifi)));
    for (tax, slug) in [("category", "nope"), ("nope", "news")] {
        let none = r.term_items("post", tax, slug, 5).await.unwrap();
        assert!(none.value.is_empty(), "{tax}/{slug}");
        assert!(none.deps.contains(&archive_key("post")));
    }
    // The limit is 1..=MAX_QUERY_LIMIT.
    for bad in [0, ferrite_core::site::MAX_QUERY_LIMIT + 1] {
        assert!(matches!(
            r.term_items("post", "category", "news", bad).await,
            Err(CoreError::InvalidField { .. })
        ));
    }
    let max = r
        .term_items(
            "post",
            "category",
            "news",
            ferrite_core::site::MAX_QUERY_LIMIT,
        )
        .await
        .unwrap();
    assert_eq!(max.value.len(), 35);
}

// ------------------------------------------------------------ inline images (R28)

fn new_hash() -> ArtifactHash {
    ArtifactHash::parse(&format!("{:0>64}", format!("{:x}", rand_u64()))).unwrap()
}

/// A published media item of `mime`, `name`, `size` and `variants` (name, w, h), with
/// artifact rows for every file; returns its id and the URLs of the original and of
/// each variant (in the order given).
async fn media_with(
    store: &dyn Store,
    author: &User,
    mime: &str,
    name: &str,
    size: Option<(i32, i32)>,
    variants: &[(&str, i32, i32)],
) -> (ContentId, String, Vec<String>) {
    let id = insert(
        store,
        author,
        item(
            "media",
            &format!("m-{}", rand_u64()),
            ContentStatus::Published,
            Some(-HOUR),
        ),
    )
    .await;
    let hash = new_hash();
    let mut rows = serde_json::Map::new();
    let mut urls = Vec::new();
    let repos = store.read();
    for h in std::iter::once(hash.clone()).chain(variants.iter().map(|_| new_hash())) {
        repos
            .artifacts()
            .insert_if_absent(&Artifact {
                hash: h.clone(),
                kind: ArtifactKind::Media,
                bytes: 10,
                mime: mime.into(),
                created_at: at(0),
                orphaned_at: None,
            })
            .await
            .unwrap();
        if h != hash {
            let (v, w, height) = variants[urls.len()];
            rows.insert(
                v.into(),
                json!({"hash": h.as_str(), "w": w, "h": height, "mime": mime}),
            );
            urls.push(format!("/_media/{h}/{name}"));
        }
    }
    repos
        .media()
        .insert(&Media {
            item_id: id,
            artifact_hash: hash.clone(),
            mime: mime.into(),
            bytes: 10,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            alt: String::new(),
            original_name: name.into(),
            variants: Value::Object(rows),
            sanitized: mime == "image/svg+xml",
        })
        .await
        .unwrap();
    (id, format!("/_media/{hash}/{name}"), urls)
}

/// A published post whose body is the given inline images (`ferrite:media/<id>`, alt).
async fn post_with_images(
    store: &dyn Store,
    author: &User,
    slug: &str,
    images: &[(ContentId, &str)],
) {
    let mut it = item("post", slug, ContentStatus::Published, Some(-HOUR));
    let nodes: Vec<Value> = images
        .iter()
        .map(|(id, alt)| json!({"type": "image", "attrs": {"src": format!("ferrite:media/{id}"), "alt": alt}}))
        .collect();
    it.body = json!({"type": "doc", "content": nodes});
    insert(store, author, it).await;
}

/// R28 follow-up: an inline raster image renders as the `image()` helper does: `src`
/// the widest file no wider than the theme's largest size, `srcset` every ready
/// variant plus the original (a crop of another shape is left out), `sizes`, the
/// media's dimensions, lazy loading. Its media item is a dependency of the page.
pub async fn inline_image_has_srcset(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let (photo, orig, v) = media_with(
        store,
        &author,
        "image/jpeg",
        "photo.jpg",
        Some((2000, 1500)),
        &[("large", 1600, 1200), ("medium", 768, 576)],
    )
    .await;
    let (cropped, orig2, v2) = media_with(
        store,
        &author,
        "image/png",
        "wide.png",
        Some((2000, 1500)),
        &[("hero", 1280, 640), ("medium", 768, 576)],
    )
    .await;
    post_with_images(store, &author, "pics", &[(photo, "a photo"), (cropped, "")]).await;

    let r = reader(db.shared_store());
    r.set_image_width(Some(1280));
    let got = r.single("post", &parts(&["pics"])).await.unwrap();
    let html = got.value.unwrap().html;
    let (large, medium) = (&v[0], &v[1]);
    assert!(
        html.contains(&format!(
            "<img src=\"{medium}\" alt=\"a photo\" srcset=\"{medium} 768w, {large} 1600w, {orig} 2000w\" \
             sizes=\"(max-width: 1280px) 100vw, 1280px\" width=\"2000\" height=\"1500\" \
             loading=\"lazy\" decoding=\"async\">"
        )),
        "{html}"
    );
    // The 2:1 `hero` crop of a 4:3 image is not a candidate.
    let medium2 = &v2[1];
    assert!(
        html.contains(&format!(
            "<img src=\"{medium2}\" alt=\"\" srcset=\"{medium2} 768w, {orig2} 2000w\""
        )),
        "{html}"
    );
    assert!(!html.contains(&v2[0]), "{html}");
    assert!(got.deps.contains(&post_key(photo)));
    assert!(got.deps.contains(&post_key(cropped)));

    // Without theme sizes the original is `src` and the largest size.
    r.set_image_width(None);
    let html = r
        .single("post", &parts(&["pics"]))
        .await
        .unwrap()
        .value
        .unwrap()
        .html;
    assert!(
        html.contains(&format!(
            "<img src=\"{orig}\" alt=\"a photo\" srcset=\"{medium} 768w, {large} 1600w, {orig} 2000w\" \
             sizes=\"(max-width: 2000px) 100vw, 2000px\""
        )),
        "{html}"
    );
}

/// R28 follow-up: an SVG, a non-image and an image whose variants are not ready keep
/// the plain `<img src alt>`.
pub async fn inline_svg_has_no_srcset(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let (svg, svg_url, _) =
        media_with(store, &author, "image/svg+xml", "logo.svg", None, &[]).await;
    let (pdf, pdf_url, _) =
        media_with(store, &author, "application/pdf", "doc.pdf", None, &[]).await;
    let (fresh, fresh_url, _) = media_with(store, &author, "image/png", "new.png", None, &[]).await;
    post_with_images(
        store,
        &author,
        "plain",
        &[(svg, "logo"), (pdf, "doc"), (fresh, "new")],
    )
    .await;
    let r = reader(db.shared_store());
    r.set_image_width(Some(1280));
    let html = r
        .single("post", &parts(&["plain"]))
        .await
        .unwrap()
        .value
        .unwrap()
        .html;
    assert_eq!(
        html,
        format!(
            "<img src=\"{svg_url}\" alt=\"logo\"><img src=\"{pdf_url}\" alt=\"doc\">\
             <img src=\"{fresh_url}\" alt=\"new\">"
        )
    );
}

/// An original narrower than the theme's largest size claims only its own width in
/// `sizes`, and is `src` itself.
pub async fn inline_image_narrower_than_theme(db: &TestDb) {
    let store = db.store();
    let author = user(store, "writer", Role::Author).await;
    let (small, orig, v) = media_with(
        store,
        &author,
        "image/png",
        "small.png",
        Some((800, 600)),
        &[("medium", 768, 576)],
    )
    .await;
    post_with_images(store, &author, "small", &[(small, "s")]).await;
    let r = reader(db.shared_store());
    r.set_image_width(Some(1280));
    let html = r
        .single("post", &parts(&["small"]))
        .await
        .unwrap()
        .value
        .unwrap()
        .html;
    let medium = &v[0];
    assert!(
        html.contains(&format!(
            "<img src=\"{orig}\" alt=\"s\" srcset=\"{medium} 768w, {orig} 800w\" \
             sizes=\"(max-width: 800px) 100vw, 800px\" width=\"800\" height=\"600\""
        )),
        "{html}"
    );
}

backend_tests!(
    inline_image_has_srcset,
    inline_image_narrower_than_theme,
    inline_svg_has_no_srcset,
    media_is_published_only,
    sitemap_files_count_and_split,
    media_file_is_published_only,
    no_leak_single,
    no_leak_by_id_and_latest,
    no_leak_menu_links,
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
    past_last_page_is_one_count,
    custom_type_term_archive,
    term_items_respects_limit,
);
