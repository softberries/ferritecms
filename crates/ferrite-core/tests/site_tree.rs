//! `SiteReader::tree` and `ListOrder` on both backends (M4-3b P56, P58): the tree has
//! exactly the sitemap's items and paths (the front page as `/`, M4-4 P67), comes in
//! `menu_order` order, records `archive:<kind>` and `option:reading.front_page`, and
//! stops at `MAX_TREE_NODES`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeSet, sync::Arc};

use ferrite_core::{
    DepKeys, PermalinkRules, SiteReader,
    events::{archive_key, option_key, post_key},
    site::{FRONT_PAGE_OPTION, ListOrder, MAX_TREE_NODES},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, ContentType, Media, NewContent,
    NewUser, Role, Term, TermId, Timestamp, User, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

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

async fn author(store: &dyn Store) -> User {
    let tx = store.begin().await.unwrap();
    let u = tx
        .users()
        .insert(&NewUser {
            id: UserId::new(),
            login: "ana".into(),
            email: "ana@example.com".into(),
            display_name: "Ana".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    u
}

struct Row<'a> {
    kind: &'a str,
    slug: String,
    title: String,
    status: ContentStatus,
    published: i64,
    parent: Option<ContentId>,
    menu_order: i32,
    excerpt: &'a str,
    fields: serde_json::Value,
}

fn row<'a>(kind: &'a str, slug: &str, menu_order: i32) -> Row<'a> {
    Row {
        kind,
        slug: slug.into(),
        title: format!("Title {slug}"),
        status: ContentStatus::Published,
        published: -HOUR,
        parent: None,
        menu_order,
        excerpt: "",
        fields: json!({}),
    }
}

/// Inserts `rows` in one transaction; returns their ids in order.
async fn insert(store: &dyn Store, author: &User, rows: Vec<Row<'_>>) -> Vec<ContentId> {
    let tx = store.begin().await.unwrap();
    let mut ids = Vec::with_capacity(rows.len());
    for r in rows {
        let id = ContentId::new();
        tx.content()
            .insert(&NewContent {
                id,
                kind: r.kind.into(),
                status: r.status,
                slug: r.slug,
                parent_id: r.parent,
                author_id: author.id,
                title: r.title,
                excerpt: r.excerpt.into(),
                body: json!({"type": "doc", "content": []}),
                body_html: None,
                fields: r.fields,
                menu_order: r.menu_order,
                published_at: Some(at(r.published)),
                scheduled_at: None,
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();
        ids.push(id);
    }
    tx.commit().await.unwrap();
    ids
}

async fn one(store: &dyn Store, author: &User, r: Row<'_>) -> ContentId {
    insert(store, author, vec![r]).await[0]
}

fn child<'a>(slug: &str, parent: ContentId, menu_order: i32) -> Row<'a> {
    let mut r = row("page", slug, menu_order);
    r.parent = Some(parent);
    r
}

/// Pages A (order 2) and B (order 1) with children, a draft D with a published child:
/// the tree's URLs are the sitemap's, in `menu_order` order, with parents set.
pub async fn tree_matches_the_sitemap(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    let a = one(store, &ana, row("page", "a", 2)).await;
    let b = one(store, &ana, row("page", "b", 1)).await;
    let a1 = one(store, &ana, child("a1", a, 0)).await;
    let a2 = one(store, &ana, child("a2", a, 3)).await;
    let b1 = one(store, &ana, child("b1", b, 1)).await;
    let mut d = row("page", "d", 0);
    d.status = ContentStatus::Draft;
    let d = one(store, &ana, d).await;
    let d1 = one(store, &ana, child("d1", d, 5)).await;
    // A future-dated page and a post are not in the page tree.
    let mut future = row("page", "future", 0);
    future.published = HOUR;
    one(store, &ana, future).await;
    one(store, &ana, row("post", "a-post", 0)).await;

    let r = reader(db.shared_store());
    let tree = r.tree("page").await.unwrap().value.expect("a page tree");
    assert_eq!(tree.kind, "page");
    assert!(!tree.truncated);

    let sitemap = r.sitemap("page", None).await.unwrap().value;
    let want: BTreeSet<String> = sitemap.entries.iter().map(|e| e.url.clone()).collect();
    let got: BTreeSet<String> = tree.nodes.iter().map(|n| n.url.clone()).collect();
    assert_eq!(got, want);
    assert_eq!(tree.nodes.len(), 6, "{:?}", tree.nodes);
    assert!(got.contains("/a/a1/") && got.contains("/d/d1/"), "{got:?}");

    let orders: Vec<i32> = tree.nodes.iter().map(|n| n.menu_order).collect();
    let mut sorted = orders.clone();
    sorted.sort_unstable();
    assert_eq!(orders, sorted, "menu_order ascending");

    let node = |id| tree.nodes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(node(a).parent, None);
    assert_eq!(node(b).parent, None);
    assert_eq!(node(a1).parent, Some(a));
    assert_eq!(node(a2).parent, Some(a));
    assert_eq!(node(b1).parent, Some(b));
    // The draft parent is not a node; its published child keeps the parent id.
    assert!(tree.nodes.iter().all(|n| n.id != d));
    assert_eq!(node(d1).parent, Some(d));
    assert_eq!(node(a1).title, "Title a1");
    assert_eq!(node(a1).url, "/a/a1/");
    assert_eq!(node(a2).menu_order, 3);
}

/// R-b1: a node carries the stored excerpt and the featured media id, the id only
/// while it names a public media item (R8); every featured id checked is a dependency.
pub async fn tree_nodes_carry_excerpt_and_featured(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    let shown = one(store, &ana, row("media", "shown", 0)).await;
    let mut draft_media = row("media", "draft-media", 0);
    draft_media.status = ContentStatus::Draft;
    let draft_media = one(store, &ana, draft_media).await;
    let a_post = one(store, &ana, row("post", "a-post", 0)).await;
    let missing = ContentId::new();
    let featured = |id: ContentId| json!({"featured_image": id.to_string()});

    let mut r1 = row("page", "with", 0);
    r1.excerpt = "Stored excerpt";
    r1.fields = featured(shown);
    let with = one(store, &ana, r1).await;
    let mut r2 = row("page", "bad", 1);
    r2.fields = json!({"featured_image": "not-a-uuid"});
    let bad = one(store, &ana, r2).await;
    let plain = one(store, &ana, row("page", "plain", 2)).await;
    let mut r3 = row("page", "drafted", 3);
    r3.fields = featured(draft_media);
    let drafted = one(store, &ana, r3).await;
    let mut r4 = row("page", "not-media", 4);
    r4.fields = featured(a_post);
    let not_media = one(store, &ana, r4).await;
    let mut r5 = row("page", "gone", 5);
    r5.fields = featured(missing);
    let gone = one(store, &ana, r5).await;

    let r = reader(db.shared_store());
    let read = r.tree("page").await.unwrap();
    let tree = read.value.unwrap();
    let node = |id| tree.nodes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(node(with).excerpt, "Stored excerpt");
    assert_eq!(node(with).featured, Some(shown));
    assert_eq!(node(bad).featured, None);
    assert_eq!(node(plain).featured, None);
    assert_eq!(node(plain).excerpt, "");
    assert_eq!(node(drafted).featured, None, "draft media is not shown");
    assert_eq!(node(not_media).featured, None, "a post is not media");
    assert_eq!(node(gone).featured, None);
    for id in [shown, draft_media, a_post, missing] {
        assert!(read.deps.contains(&post_key(id)), "{id}");
    }
    assert!(read.deps.contains(&archive_key("page")));

    // Publishing the media shows it.
    let tx = store.begin().await.unwrap();
    let mut m = tx.content().get(draft_media).await.unwrap().unwrap();
    m.status = ContentStatus::Published;
    m.published_at = Some(at(-HOUR));
    tx.content().update(&m).await.unwrap();
    tx.commit().await.unwrap();
    let read = r.tree("page").await.unwrap();
    let tree = read.value.unwrap();
    let node = |id| tree.nodes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(node(drafted).featured, Some(draft_media));
    assert!(read.deps.contains(&post_key(draft_media)));
}

/// A media row for content item `id`: an 800×600 original and a `card` variant.
async fn media_row(store: &dyn Store, id: ContentId, n: u8) {
    let hash = ArtifactHash::parse(&format!("{n:0>64}")).unwrap();
    let card = ArtifactHash::parse(&format!("{n:f>64}")).unwrap();
    let repos = store.read();
    for h in [&hash, &card] {
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
            variants: json!({"card": {"hash": card.as_str(), "w": 400, "h": 300, "mime": "image/png"}}),
            sanitized: false,
        })
        .await
        .unwrap();
}

/// M4-4: the tree carries the public featured media it checked, exactly as
/// `SiteReader::media` (the theme's `image()` bridge read) gives it, so card images
/// cost no bridge call; draft media is not carried.
pub async fn tree_carries_the_public_featured_media(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    let shown = one(store, &ana, row("media", "shown", 0)).await;
    media_row(store, shown, 1).await;
    let mut hidden = row("media", "hidden", 0);
    hidden.status = ContentStatus::Draft;
    let hidden = one(store, &ana, hidden).await;
    media_row(store, hidden, 2).await;
    let featured = |id: ContentId| json!({"featured_image": id.to_string()});
    let mut a = row("page", "a", 0);
    a.fields = featured(shown);
    one(store, &ana, a).await;
    let mut b = row("page", "b", 1);
    b.fields = featured(hidden);
    one(store, &ana, b).await;
    let mut c = row("page", "c", 2);
    c.fields = featured(shown);
    one(store, &ana, c).await;

    let r = reader(db.shared_store());
    let tree = r.tree("page").await.unwrap().value.unwrap();
    let want = r.media(shown).await.unwrap().value.expect("public media");
    assert_eq!(want.variants.len(), 1);
    assert_eq!(tree.media.len(), 1, "{:?}", tree.media.keys());
    assert_eq!(tree.media.get(&shown), Some(&want));
    assert!(!tree.media.contains_key(&hidden));
}

pub async fn tree_of_media_or_unknown_is_none(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    one(store, &ana, row("page", "a", 0)).await;
    let mut draft = row("event", "e", 0);
    draft.status = ContentStatus::Draft;
    store
        .read()
        .types()
        .upsert(&ContentType {
            name: "event".into(),
            label: "Events".into(),
            hierarchical: false,
            has_archive: true,
            fields: json!({}),
            built_in: false,
            created_at: at(-HOUR),
        })
        .await
        .unwrap();
    one(store, &ana, draft).await;

    let r = reader(db.shared_store());
    for kind in ["media", "nope", "event"] {
        let read = r.tree(kind).await.unwrap();
        assert!(read.value.is_none(), "{kind}");
        assert!(read.deps.contains(&archive_key(kind)), "{kind}");
    }
}

pub async fn tree_deps_are_archive_kind(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    let a = one(store, &ana, row("page", "a", 0)).await;
    one(store, &ana, child("a1", a, 0)).await;

    let read = reader(db.shared_store()).tree("page").await.unwrap();
    assert!(read.cacheable);
    let mut want = DepKeys::default();
    want.insert(archive_key("page"));
    want.insert(option_key(FRONT_PAGE_OPTION));
    assert_eq!(read.deps, want);
}

async fn set_front_page(store: &dyn Store, value: serde_json::Value) {
    store
        .read()
        .options()
        .set(FRONT_PAGE_OPTION, &value, false)
        .await
        .unwrap();
}

/// M4-4 P67: the front page's node links `/` (its permalink answers 301 to `/`); its
/// children keep their paths, and every other type's tree is unchanged.
pub async fn the_tree_names_the_front_page_root(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    let home = one(store, &ana, row("page", "home", 0)).await;
    let x = one(store, &ana, child("x", home, 0)).await;
    let other = one(store, &ana, row("page", "other", 1)).await;
    let post = one(store, &ana, row("post", "home", 0)).await;
    let r = reader(db.shared_store());
    let url = |tree: &ferrite_core::site::ContentTree, id| {
        tree.nodes.iter().find(|n| n.id == id).unwrap().url.clone()
    };

    // No front page: every node has its permalink.
    let tree = r.tree("page").await.unwrap().value.unwrap();
    assert_eq!(url(&tree, home), "/home/");

    set_front_page(store, json!(home.to_string())).await;
    let read = r.tree("page").await.unwrap();
    assert!(read.deps.contains(&option_key(FRONT_PAGE_OPTION)));
    let tree = read.value.unwrap();
    assert_eq!(url(&tree, home), "/");
    assert_eq!(url(&tree, x), "/home/x/", "a child keeps its path");
    assert_eq!(url(&tree, other), "/other/");
    let posts = r.tree("post").await.unwrap().value.unwrap();
    assert_ne!(
        url(&posts, post),
        "/",
        "a post sharing the slug is not the front page"
    );

    // Another page as the front page: `home` gets its path back.
    set_front_page(store, json!(other.to_string())).await;
    let tree = r.tree("page").await.unwrap().value.unwrap();
    assert_eq!(url(&tree, home), "/home/");
    assert_eq!(url(&tree, other), "/");

    // A value that names no item, or is not an id, changes nothing.
    for value in [json!(ContentId::new().to_string()), json!("nope"), json!(7)] {
        set_front_page(store, value.clone()).await;
        let tree = r.tree("page").await.unwrap().value.unwrap();
        assert!(tree.nodes.iter().all(|n| n.url != "/"), "{value}");
    }
}

/// 1001 published pages: the first 1000 in `menu_order` come back, marked truncated.
pub async fn tree_truncates_at_1000(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    let n = MAX_TREE_NODES + 1;
    let rows = (0..n)
        .map(|i| row("page", &format!("p{i}"), i32::try_from(i).unwrap()))
        .collect();
    let ids = insert(store, &ana, rows).await;

    let tree = reader(db.shared_store())
        .tree("page")
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(MAX_TREE_NODES, 1000);
    assert_eq!(tree.nodes.len(), MAX_TREE_NODES);
    assert!(tree.truncated);
    assert!(tree.nodes.iter().all(|node| node.id != ids[n - 1]));
    assert_eq!(tree.nodes[999].menu_order, 999);

    // Exactly 1000 is not truncated.
    let last = ids[n - 1];
    let tx = store.begin().await.unwrap();
    let mut item = tx.content().get(last).await.unwrap().unwrap();
    item.status = ContentStatus::Trash;
    tx.content().update(&item).await.unwrap();
    tx.commit().await.unwrap();
    let tree = reader(db.shared_store())
        .tree("page")
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(tree.nodes.len(), MAX_TREE_NODES);
    assert!(!tree.truncated);
}

/// `ListOrder::Menu` lists by `menu_order` (then title); `ListOrder::Date` is newest
/// first as before, for `latest` and `term_items`.
pub async fn query_menu_order(db: &TestDb) {
    let store = db.store();
    let ana = author(store).await;
    store
        .read()
        .types()
        .upsert(&ContentType {
            name: "service".into(),
            label: "Services".into(),
            hierarchical: false,
            has_archive: true,
            fields: json!({}),
            built_in: false,
            created_at: at(-HOUR),
        })
        .await
        .unwrap();
    // menu_order 3, 1, 2; published newest to oldest in that order.
    let mut rows = Vec::new();
    for (i, (slug, order)) in [("three", 3), ("one", 1), ("two", 2)].iter().enumerate() {
        let mut r = row("service", slug, *order);
        r.published = -HOUR * (i64::try_from(i).unwrap() + 1);
        rows.push(r);
    }
    // A tie on menu_order 2 is broken by title, case-insensitively.
    let mut tie = row("service", "also-two", 2);
    tie.title = "a tie".into();
    tie.published = -HOUR * 10;
    rows.push(tie);
    insert(store, &ana, rows).await;

    let r = reader(db.shared_store());
    let slugs = |items: &[ferrite_core::PageItem]| {
        items
            .iter()
            .map(|p| p.item.slug.clone())
            .collect::<Vec<_>>()
    };
    let menu = r.latest("service", 10, 0, ListOrder::Menu).await.unwrap();
    assert_eq!(slugs(&menu.value), ["one", "also-two", "two", "three"]);
    assert!(menu.deps.contains(&archive_key("service")));
    let date = r.latest("service", 10, 0, ListOrder::Date).await.unwrap();
    assert_eq!(slugs(&date.value), ["three", "one", "two", "also-two"]);
    let menu = r.latest("service", 2, 1, ListOrder::Menu).await.unwrap();
    assert_eq!(slugs(&menu.value), ["also-two", "two"]);

    // `term_items` takes the same order.
    let news = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id: news,
            taxonomy: "category".into(),
            slug: "news".into(),
            name: "News".into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    let mut rows = Vec::new();
    for (i, (slug, order)) in [("p3", 3), ("p1", 1), ("p2", 2)].iter().enumerate() {
        let mut r = row("post", slug, *order);
        r.published = -HOUR * (i64::try_from(i).unwrap() + 1);
        rows.push(r);
    }
    let ids = insert(store, &ana, rows).await;
    for id in ids {
        store
            .read()
            .terms()
            .set_for_item(id, &[news])
            .await
            .unwrap();
    }
    let menu = r
        .term_items("post", "category", "news", 10, ListOrder::Menu)
        .await
        .unwrap();
    assert_eq!(slugs(&menu.value), ["p1", "p2", "p3"]);
    let date = r
        .term_items("post", "category", "news", 10, ListOrder::Date)
        .await
        .unwrap();
    assert_eq!(slugs(&date.value), ["p3", "p1", "p2"]);
}

backend_tests!(
    tree_matches_the_sitemap,
    tree_nodes_carry_excerpt_and_featured,
    tree_carries_the_public_featured_media,
    tree_of_media_or_unknown_is_none,
    tree_deps_are_archive_kind,
    the_tree_names_the_front_page_root,
    tree_truncates_at_1000,
    query_menu_order,
);
