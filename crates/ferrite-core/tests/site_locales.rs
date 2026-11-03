//! `SiteReader` per locale (M6-1 Task 9, design 13 §5): `ReadLocale::All` adds no locale
//! predicate (D4), `in_locale` scopes singles, listings, search, feeds and the front
//! page, and `translations` / sitemap alternates read the translation groups.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    FeedSource, FrontPage, PermalinkRules, ReadLocale, SiteReader, events::option_key, locales,
};
use ferrite_domain::{
    ArtifactHash, ContentId, ContentStatus, NewContent, NewUser, Role, Term, TermId, Timestamp,
    User, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{CountingStore, TestDb, backend_tests};
use serde_json::json;

const NOW: i64 = 1_781_524_800_000_000;
const HOUR: i64 = 3_600_000_000;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_micros(NOW + offset).unwrap()
}

async fn rules(store: &dyn Store) -> PermalinkRules {
    PermalinkRules::default().with_locales(locales::read(store.read()).await.unwrap())
}

fn reader(store: Arc<dyn Store>, rules: PermalinkRules) -> SiteReader {
    SiteReader::new(
        store,
        Arc::new(rules),
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
            login: "ada".into(),
            email: "ada@example.com".into(),
            display_name: "Ada".into(),
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

async fn multilingual(store: &dyn Store) {
    store
        .read()
        .options()
        .set("site.locales", &json!([{"tag": "en"}, {"tag": "de"}]), true)
        .await
        .unwrap();
}

/// A published post (an hour ago) in `locale`, joining `group`.
async fn post(
    store: &dyn Store,
    author: &User,
    kind: &str,
    slug: &str,
    locale: &str,
    group: Option<ContentId>,
    terms: &[TermId],
) -> ContentId {
    let id = ContentId::new();
    let tx = store.begin().await.unwrap();
    tx.content()
        .insert(&NewContent {
            id,
            kind: kind.into(),
            status: ContentStatus::Published,
            slug: slug.into(),
            parent_id: None,
            author_id: author.id,
            title: format!("T {slug}"),
            excerpt: String::new(),
            body: json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": format!("needle {slug}")}]}
            ]}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: Some(at(-HOUR)),
            scheduled_at: None,
            created_at: None,
            updated_at: None,
            locale: locale.into(),
            translation_group: group,
        })
        .await
        .unwrap();
    tx.terms().set_for_item(id, terms).await.unwrap();
    tx.search()
        .index(id, &format!("T {slug}"), &format!("needle {slug}"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn term(store: &dyn Store, slug: &str, locale: &str, group: Option<TermId>) -> TermId {
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
            locale: locale.into(),
            translation_group: group.unwrap_or(id),
        })
        .await
        .unwrap();
    id
}

async fn all_issues_no_locale_predicate(db: &TestDb) {
    let store = db.shared_store();
    let a = author(store.as_ref()).await;
    let news = term(store.as_ref(), "news", "en", None).await;
    post(store.as_ref(), &a, "post", "hello", "en", None, &[news]).await;
    let counting = Arc::new(CountingStore::new(store.clone()));
    let r = reader(counting.clone(), PermalinkRules::default());
    assert_eq!(r.locale(), &ReadLocale::All);
    let one = r
        .single("post", &["hello".into()])
        .await
        .unwrap()
        .value
        .unwrap();
    r.front_page(1).await.unwrap();
    r.archive("post", 1).await.unwrap();
    r.term_archive("category", "news", 1).await.unwrap();
    r.search("needle", 1).await.unwrap();
    r.feed(&FeedSource::Kind("post".into())).await.unwrap();
    r.related(&one, 3).await.unwrap();
    r.latest("post", 3, 0, ferrite_core::site::ListOrder::Date)
        .await
        .unwrap();
    r.tree("page").await.unwrap();
    assert_eq!(
        counting.localized_reads(),
        0,
        "a one-locale read adds no locale"
    );
}

async fn only_filters_singles_listings_search_feeds_archives(db: &TestDb) {
    let store = db.shared_store();
    multilingual(store.as_ref()).await;
    let a = author(store.as_ref()).await;
    let en = post(store.as_ref(), &a, "post", "garden", "en", None, &[]).await;
    let de = post(store.as_ref(), &a, "post", "garden", "de", Some(en), &[]).await;
    let base = reader(store.clone(), rules(store.as_ref()).await);
    let de_r = base.in_locale(ReadLocale::Only("de".into()));
    let en_r = base.in_locale(ReadLocale::Only("en".into()));
    let got = de_r
        .single("post", &["garden".into()])
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!((got.item.id, got.url.as_str()), (de, "/de/garden/"));
    assert_eq!(got.item.locale, "de");
    let got = en_r
        .single("post", &["garden".into()])
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!((got.item.id, got.url.as_str()), (en, "/garden/"));
    let ids = |l: ferrite_core::Listing| l.items.into_iter().map(|i| i.item.id).collect::<Vec<_>>();
    assert_eq!(
        ids(de_r.archive("post", 1).await.unwrap().value.unwrap()),
        [de]
    );
    assert_eq!(
        ids(en_r.search("needle", 1).await.unwrap().value.unwrap()),
        [en]
    );
    let feed = de_r
        .feed(&FeedSource::Kind("post".into()))
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(feed.iter().map(|i| i.item.id).collect::<Vec<_>>(), [de]);
    let FrontPage::Posts(front) = de_r.front_page(1).await.unwrap().value.unwrap() else {
        panic!("a listing")
    };
    assert_eq!(ids(front), [de]);
}

async fn term_archive_finds_term_by_locale(db: &TestDb) {
    let store = db.shared_store();
    multilingual(store.as_ref()).await;
    let a = author(store.as_ref()).await;
    let en_t = term(store.as_ref(), "news", "en", None).await;
    let de_t = term(store.as_ref(), "news", "de", Some(en_t)).await;
    let en = post(store.as_ref(), &a, "post", "a", "en", None, &[en_t]).await;
    let de = post(store.as_ref(), &a, "post", "a", "de", Some(en), &[de_t]).await;
    let base = reader(store.clone(), rules(store.as_ref()).await);
    let (t, l) = base
        .in_locale(ReadLocale::Only("de".into()))
        .term_archive_page("category", "news", 1)
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(t.id, de_t);
    assert_eq!(l.items.iter().map(|i| i.item.id).collect::<Vec<_>>(), [de]);
}

async fn front_page_per_locale_and_fallback_to_listing(db: &TestDb) {
    let store = db.shared_store();
    multilingual(store.as_ref()).await;
    let a = author(store.as_ref()).await;
    let home = post(store.as_ref(), &a, "page", "home", "en", None, &[]).await;
    store
        .read()
        .options()
        .set("reading.front_page", &json!(home.to_string()), true)
        .await
        .unwrap();
    let base = reader(store.clone(), rules(store.as_ref()).await);
    let de_r = base.in_locale(ReadLocale::Only("de".into()));
    // No translation yet: the German post listing.
    assert!(matches!(
        de_r.front_page(1).await.unwrap().value,
        Some(FrontPage::Posts(_))
    ));
    let start = post(store.as_ref(), &a, "page", "start", "de", Some(home), &[]).await;
    let read = de_r.front_page(1).await.unwrap();
    assert!(
        read.deps.contains(&format!("group:{home}")),
        "{:?}",
        read.deps
    );
    let Some(FrontPage::Page(item)) = read.value else {
        panic!("the translation")
    };
    assert_eq!(item.item.id, start);
    assert!(de_r.is_front_page(&item.item).await.unwrap().value);
    let en_front = base
        .in_locale(ReadLocale::Only("en".into()))
        .front_page(1)
        .await
        .unwrap();
    assert!(matches!(en_front.value, Some(FrontPage::Page(p)) if p.item.id == home));
}

async fn translations_lists_public_members_in_site_order(db: &TestDb) {
    let store = db.shared_store();
    store
        .read()
        .options()
        .set(
            "site.locales",
            &json!([{"tag": "en"}, {"tag": "fr"}, {"tag": "de", "label": "Deutsch!"}]),
            true,
        )
        .await
        .unwrap();
    let a = author(store.as_ref()).await;
    let en = post(store.as_ref(), &a, "post", "x", "en", None, &[]).await;
    post(store.as_ref(), &a, "post", "x", "de", Some(en), &[]).await;
    // A French draft is not listed.
    let fr = post(store.as_ref(), &a, "post", "x", "fr", Some(en), &[]).await;
    let mut fr_item = store.read().content().get(fr).await.unwrap().unwrap();
    fr_item.status = ContentStatus::Draft;
    store.read().content().update(&fr_item).await.unwrap();
    let base = reader(store.clone(), rules(store.as_ref()).await);
    let item = base
        .in_locale(ReadLocale::Only("de".into()))
        .single("post", &["x".into()])
        .await
        .unwrap()
        .value
        .unwrap();
    let read = base.translations(&item).await.unwrap();
    assert!(read.deps.contains(&format!("group:{en}")));
    assert!(read.deps.contains(&option_key("site.locales")));
    let got: Vec<(String, String, bool)> = read
        .value
        .into_iter()
        .map(|l| (l.locale, l.url, l.current))
        .collect();
    assert_eq!(
        got,
        [
            ("en".to_owned(), "/x/".to_owned(), false),
            ("de".to_owned(), "/de/x/".to_owned(), true),
        ]
    );
}

async fn sitemap_entries_carry_alternates_only_when_multilingual(db: &TestDb) {
    let store = db.shared_store();
    let a = author(store.as_ref()).await;
    let en = post(store.as_ref(), &a, "post", "x", "en", None, &[]).await;
    let one = reader(store.clone(), rules(store.as_ref()).await);
    let page = one.sitemap("post", None).await.unwrap().value;
    assert!(page.entries.iter().all(|e| e.alternates.is_empty()));
    multilingual(store.as_ref()).await;
    post(store.as_ref(), &a, "post", "x", "de", Some(en), &[]).await;
    post(store.as_ref(), &a, "post", "solo", "en", None, &[]).await;
    let many = reader(store.clone(), rules(store.as_ref()).await);
    let page = many.sitemap("post", None).await.unwrap().value;
    let x = page.entries.iter().find(|e| e.url == "/x/").unwrap();
    assert_eq!(
        x.alternates,
        [
            ("en".to_owned(), "/x/".to_owned()),
            ("de".to_owned(), "/de/x/".to_owned())
        ]
    );
    let solo = page.entries.iter().find(|e| e.url == "/solo/").unwrap();
    assert!(solo.alternates.is_empty());
    assert!(page.entries.iter().any(|e| e.url == "/de/x/"));
}

async fn related_and_tree_stay_in_item_locale(db: &TestDb) {
    let store = db.shared_store();
    multilingual(store.as_ref()).await;
    let a = author(store.as_ref()).await;
    let t = term(store.as_ref(), "news", "en", None).await;
    let t_de = term(store.as_ref(), "news", "de", Some(t)).await;
    let one = post(store.as_ref(), &a, "post", "one", "de", None, &[t_de]).await;
    let two = post(store.as_ref(), &a, "post", "two", "de", None, &[t_de]).await;
    post(store.as_ref(), &a, "post", "three", "en", None, &[t]).await;
    post(store.as_ref(), &a, "page", "seite", "de", None, &[]).await;
    post(store.as_ref(), &a, "page", "page", "en", None, &[]).await;
    let de_r =
        reader(store.clone(), rules(store.as_ref()).await).in_locale(ReadLocale::Only("de".into()));
    let item = de_r
        .single("post", &["one".into()])
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(item.item.id, one);
    let rel = de_r.related(&item, 5).await.unwrap().value;
    assert_eq!(rel.iter().map(|i| i.item.id).collect::<Vec<_>>(), [two]);
    let tree = de_r.tree("page").await.unwrap().value.unwrap();
    assert_eq!(
        tree.nodes
            .iter()
            .map(|n| n.url.as_str())
            .collect::<Vec<_>>(),
        ["/de/seite/"]
    );
}

backend_tests!(
    all_issues_no_locale_predicate,
    only_filters_singles_listings_search_feeds_archives,
    term_archive_finds_term_by_locale,
    front_page_per_locale_and_fallback_to_listing,
    translations_lists_public_members_in_site_order,
    sitemap_entries_carry_alternates_only_when_multilingual,
    related_and_tree_stay_in_item_locale,
);
