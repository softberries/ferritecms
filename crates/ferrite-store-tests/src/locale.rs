//! Contract tests for locales and translation groups (M6-1, design 13 §3).

use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentStatus, Menu, NewContent, SearchQuery, Term,
    TermId, Timestamp, User,
};
use ferrite_store::{LocaleUsage, MAX_GROUPS, Store, StoreError};

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

/// A draft post in `locale`, joining `group` (`None`: its own group).
fn post_in(author: &User, slug: &str, locale: &str, group: Option<ContentId>) -> NewContent {
    NewContent {
        locale: locale.into(),
        translation_group: group,
        ..new_post(author, slug)
    }
}

async fn insert(store: &dyn Store, item: &NewContent) -> ContentItem {
    store.read().content().insert(item).await.unwrap()
}

fn published(mut n: NewContent, micros: i64) -> NewContent {
    n.status = ContentStatus::Published;
    n.published_at = Some(Timestamp::from_micros(micros).unwrap());
    n
}

fn term(slug: &str, locale: &str, group: Option<TermId>) -> Term {
    let id = TermId::new();
    Term {
        id,
        taxonomy: "category".into(),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        parent_id: None,
        locale: locale.into(),
        translation_group: group.unwrap_or(id),
    }
}

/// An insert without a group makes the item its own group; with one, it joins it.
pub async fn insert_defaults_group_to_id(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = insert(store, &post_in(&user, "a", "en", None)).await;
    assert_eq!(a.translation_group, a.id);
    assert_eq!(a.locale, "en");
    let b = insert(store, &post_in(&user, "b", "de", Some(a.id))).await;
    assert_eq!(b.translation_group, a.id);
    assert_eq!(store.read().content().get(b.id).await.unwrap(), Some(b));
}

/// `ContentQuery.locale` filters `list`, `count` and `file_bounds` alike.
pub async fn locale_filter_on_list_count_and_file_bounds(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    for (i, (slug, locale)) in [("e1", "en"), ("e2", "en"), ("d1", "de")]
        .iter()
        .enumerate()
    {
        insert(
            store,
            &published(
                post_in(&user, slug, locale, None),
                1_000_000 * (i as i64 + 1),
            ),
        )
        .await;
    }
    let q = |locale: Option<&str>| ContentQuery {
        kind: Some("post".into()),
        locale: locale.map(str::to_owned),
        limit: 50,
        ..Default::default()
    };
    let content = store.read().content();
    assert_eq!(content.list(&q(None)).await.unwrap().items.len(), 3);
    let de = content.list(&q(Some("de"))).await.unwrap().items;
    assert_eq!(
        de.iter().map(|i| i.slug.as_str()).collect::<Vec<_>>(),
        ["d1"]
    );
    assert_eq!(content.count(&q(Some("en"))).await.unwrap(), 2);
    assert_eq!(content.count(&q(Some("fr"))).await.unwrap(), 0);
    let bounds = |locale| async move {
        let mut q = q(locale);
        q.order = ferrite_domain::ContentOrder::PublishedDesc;
        content.file_bounds(&q, 1).await.unwrap().len()
    };
    assert_eq!(bounds(None).await, 3);
    assert_eq!(bounds(Some("en")).await, 2);
}

/// Media items are shared across locales: a locale filter never hides them.
pub async fn media_ignores_locale_filter(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let mut media = post_in(&user, "pic", "en", None);
    media.kind = "media".into();
    insert(store, &media).await;
    insert(store, &post_in(&user, "p", "en", None)).await;
    let content = store.read().content();
    let only_media = ContentQuery {
        kind: Some("media".into()),
        locale: Some("de".into()),
        ..Default::default()
    };
    assert_eq!(content.count(&only_media).await.unwrap(), 1);
    let any_type = ContentQuery {
        locale: Some("de".into()),
        ..Default::default()
    };
    assert_eq!(
        content.count(&any_type).await.unwrap(),
        1,
        "the media item only"
    );
    let mixed = ContentQuery {
        kinds: vec!["post".into(), "media".into()],
        locale: Some("en".into()),
        ..Default::default()
    };
    assert_eq!(content.count(&mixed).await.unwrap(), 2);
}

/// `get_by_slug` with a locale finds only that locale's item; without, any.
pub async fn get_by_slug_with_and_without_locale(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let de = insert(store, &post_in(&user, "about", "de", None)).await;
    let content = store.read().content();
    assert_eq!(
        content
            .get_by_slug("post", Some("de"), None, "about")
            .await
            .unwrap()
            .map(|i| i.id),
        Some(de.id)
    );
    assert!(
        content
            .get_by_slug("post", Some("en"), None, "about")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        content
            .get_by_slug("post", None, None, "about")
            .await
            .unwrap()
            .is_some()
    );
}

/// Two locales may hold the same slug at the same level; one locale may not twice.
pub async fn same_slug_two_locales(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    insert(store, &post_in(&user, "about", "en", None)).await;
    insert(store, &post_in(&user, "about", "de", None)).await;
    let err = store
        .read()
        .content()
        .insert(&post_in(&user, "about", "de", None))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::UniqueViolation { constraint } if constraint == "content_items_slug_uq"),
        "{err:?}"
    );
    let content = store.read().content();
    let slugs = content
        .slugs_with_prefix("post", Some("de"), None, "about")
        .await
        .unwrap();
    assert_eq!(slugs, ["about"]);
    let all = content
        .slugs_with_prefix("post", None, None, "about")
        .await
        .unwrap();
    assert_eq!(all, ["about"], "distinct across locales");
}

/// `group_members` returns every member of each group in one call, by group then
/// locale, and refuses more than `MAX_GROUPS` groups.
pub async fn group_members_batches_and_caps(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = insert(store, &post_in(&user, "a", "en", None)).await;
    let a_de = insert(store, &post_in(&user, "a-de", "de", Some(a.id))).await;
    let b = insert(store, &post_in(&user, "b", "en", None)).await;
    let alone = insert(store, &post_in(&user, "c", "en", None)).await;
    let content = store.read().content();
    let got = content.group_members(&[a.id, b.id]).await.unwrap();
    let ids: Vec<_> = got
        .iter()
        .map(|r| (r.group, r.id, r.locale.as_str()))
        .collect();
    let mut want = vec![
        (a.id, a_de.id, "de"),
        (a.id, a.id, "en"),
        (b.id, b.id, "en"),
    ];
    want.sort_by_key(|(g, _, l)| (g.as_uuid(), *l));
    assert_eq!(ids, want);
    assert!(got.iter().all(|r| r.id != alone.id));
    assert_eq!(got[0].kind, "post");
    assert!(content.group_members(&[]).await.unwrap().is_empty());
    let many = vec![ContentId::new(); MAX_GROUPS + 1];
    assert!(matches!(
        content.group_members(&many).await,
        Err(StoreError::Invalid(_))
    ));
    // A row an older binary wrote (no group) is the group of its own id.
    db.exec_raw("UPDATE content_items SET translation_group = NULL WHERE slug = 'c'")
        .await;
    let got = content.group_members(&[alone.id]).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].id, got[0].group), (alone.id, alone.id));
    assert_eq!(
        content
            .get(alone.id)
            .await
            .unwrap()
            .unwrap()
            .translation_group,
        alone.id
    );
}

/// `relocate` checks the version, moves the locale, group and slug, and refuses a
/// second member of one locale in a group.
pub async fn relocate_checks_version_and_group_uniqueness(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let a = insert(store, &post_in(&user, "a", "en", None)).await;
    let b = insert(store, &post_in(&user, "b", "en", None)).await;
    let c = insert(store, &post_in(&user, "c", "en", None)).await;
    let content = store.read().content();
    let err = content
        .relocate(b.id, "de", a.id, "b", b.version + 5)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Conflict { current_version } if current_version == b.version)
    );
    let moved = content
        .relocate(b.id, "de", a.id, "b-de", b.version)
        .await
        .unwrap();
    assert_eq!(
        (
            moved.locale.as_str(),
            moved.translation_group,
            moved.slug.as_str()
        ),
        ("de", a.id, "b-de")
    );
    assert_eq!(moved.version, b.version + 1);
    let err = content
        .relocate(c.id, "de", a.id, "c", c.version)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::UniqueViolation { constraint } if constraint == "content_items_translation_uq"),
        "{err:?}"
    );
    assert!(matches!(
        content.relocate(ContentId::new(), "de", a.id, "x", 1).await,
        Err(StoreError::NotFound)
    ));
}

/// Term slugs are unique per (taxonomy, locale); lookups and lists take a locale.
pub async fn term_slug_per_locale(db: &TestDb) {
    let store = db.store();
    let terms = store.read().terms();
    let en = term("news", "en", None);
    terms.insert(&en).await.unwrap();
    let de = term("news", "de", Some(en.id));
    terms.insert(&de).await.unwrap();
    let err = terms.insert(&term("news", "de", None)).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::UniqueViolation { constraint } if constraint == "terms_taxonomy_slug_uq"),
        "{err:?}"
    );
    assert_eq!(
        terms
            .get_by_slug("category", Some("de"), "news")
            .await
            .unwrap(),
        Some(de.clone())
    );
    assert!(
        terms
            .get_by_slug("category", None, "news")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        terms.list("category", Some("de")).await.unwrap(),
        vec![de.clone()]
    );
    assert_eq!(terms.list("category", None).await.unwrap().len(), 2);
    let group = terms.group_members(&[en.id]).await.unwrap();
    assert_eq!(group.len(), 2);
    assert!(matches!(
        terms
            .group_members(&vec![TermId::new(); MAX_GROUPS + 1])
            .await,
        Err(StoreError::Invalid(_))
    ));
    // `update` writes the locale and group too.
    let mut moved = de.clone();
    moved.locale = "fr".into();
    terms.update(&moved).await.unwrap();
    assert_eq!(terms.get(de.id).await.unwrap(), Some(moved));
}

fn menu(name: &str, locale: &str, of: Option<&str>) -> Menu {
    Menu {
        name: name.into(),
        label: name.into(),
        items: Vec::new(),
        locale: locale.into(),
        translation_of: of.map(str::to_owned),
    }
}

/// `get_translation` finds the menu translating a name into a locale; `list` reports
/// both; a second translation of one menu into one locale is refused.
pub async fn menu_translation_lookup(db: &TestDb) {
    let store = db.store();
    let menus = store.read().menus();
    menus.put(&menu("primary", "en", None)).await.unwrap();
    menus
        .put(&menu("primary-de", "de", Some("primary")))
        .await
        .unwrap();
    let got = menus
        .get_translation("primary", "de")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.name, "primary-de");
    assert_eq!(got.translation_of.as_deref(), Some("primary"));
    assert!(
        menus
            .get_translation("primary", "fr")
            .await
            .unwrap()
            .is_none()
    );
    let list = menus.list().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].locale, "de");
    assert_eq!(list[1].translation_of.as_deref(), Some("primary"));
    let err = menus
        .put(&menu("primary-de2", "de", Some("primary")))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::UniqueViolation { constraint } if constraint == "menus_translation_uq"),
        "{err:?}"
    );
}

/// `usage` counts items (not media), terms and menus of one locale.
pub async fn locale_usage_counts(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    insert(store, &post_in(&user, "a", "de", None)).await;
    let mut media = post_in(&user, "m", "de", None);
    media.kind = "media".into();
    insert(store, &media).await;
    store
        .read()
        .terms()
        .insert(&term("t", "de", None))
        .await
        .unwrap();
    store
        .read()
        .menus()
        .put(&menu("m-de", "de", None))
        .await
        .unwrap();
    assert_eq!(
        store.read().locales().usage("de").await.unwrap(),
        LocaleUsage {
            items: 1,
            terms: 1,
            menus: 1
        }
    );
    assert!(store.read().locales().usage("fr").await.unwrap().is_empty());
}

/// `relabel_media` moves media items only (a new default locale).
pub async fn relabel_media_moves_only_media(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let mut media = post_in(&user, "pic", "en", None);
    media.kind = "media".into();
    let media_id = media.id;
    insert(store, &media).await;
    let post = post_in(&user, "p", "en", None);
    let post_id = post.id;
    insert(store, &post).await;
    let locales = store.read().locales();
    assert_eq!(locales.relabel_media("de").await.unwrap(), 1);
    assert_eq!(locales.relabel_media("de").await.unwrap(), 0);
    let content = store.read().content();
    assert_eq!(content.get(media_id).await.unwrap().unwrap().locale, "de");
    assert_eq!(content.get(post_id).await.unwrap().unwrap().locale, "en");
}

/// `relabel_all` moves every row to one locale; `relabel_blank` only blank ones.
pub async fn relabel_all_and_blank(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    insert(store, &post_in(&user, "a", "en", None)).await;
    insert(store, &post_in(&user, "b", "", None)).await;
    store
        .read()
        .terms()
        .insert(&term("t", "", None))
        .await
        .unwrap();
    store
        .read()
        .menus()
        .put(&menu("m", "", None))
        .await
        .unwrap();
    let locales = store.read().locales();
    assert_eq!(
        locales.relabel_blank("en").await.unwrap(),
        LocaleUsage {
            items: 1,
            terms: 1,
            menus: 1
        }
    );
    assert!(locales.relabel_blank("en").await.unwrap().is_empty());
    let all = locales.relabel_all("de").await.unwrap();
    assert_eq!((all.items, all.terms, all.menus), (2, 1, 1));
    assert_eq!(locales.usage("de").await.unwrap().items, 2);
    assert!(locales.relabel_all("de").await.unwrap().is_empty());
}

/// `fill_groups` sets the group of rows an older binary wrote without one.
pub async fn fill_groups_sets_null_groups(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    insert(store, &post_in(&user, "a", "en", None)).await;
    store
        .read()
        .terms()
        .insert(&term("t", "en", None))
        .await
        .unwrap();
    db.exec_raw("UPDATE content_items SET translation_group = NULL")
        .await;
    db.exec_raw("UPDATE terms SET translation_group = NULL")
        .await;
    assert_eq!(store.read().locales().fill_groups().await.unwrap(), 2);
    assert_eq!(store.read().locales().fill_groups().await.unwrap(), 0);
    let nulls = db
        .query_text("SELECT slug FROM content_items WHERE translation_group IS NULL")
        .await;
    assert!(nulls.is_empty());
}

/// A search with a locale finds only that locale's items.
pub async fn search_locale_filter(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let en = insert(store, &post_in(&user, "en", "en", None)).await;
    let de = insert(store, &post_in(&user, "de", "de", None)).await;
    let search = store.read().search();
    search.index(en.id, "Garden", "roses").await.unwrap();
    search.index(de.id, "Garten", "roses").await.unwrap();
    let q = |locale: Option<&str>| SearchQuery {
        q: "roses".into(),
        kinds: vec!["post".into()],
        statuses: Vec::new(),
        readable_by: None,
        locale: locale.map(str::to_owned),
        limit: 10,
        cursor: None,
    };
    assert_eq!(search.search(&q(None)).await.unwrap().items.len(), 2);
    assert_eq!(
        search.search(&q(Some("de"))).await.unwrap().items,
        vec![de.id]
    );
    let mut every_type = q(Some("en"));
    every_type.kinds.clear();
    assert_eq!(search.search(&every_type).await.unwrap().items, vec![en.id]);
}
