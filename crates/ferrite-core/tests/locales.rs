//! Site locales (M6-1 Task 5, design 13 §2.2): `locales::read`, the startup `settle`, and
//! `site.locales` / `site.locale` / `site.locale_texts` writes through
//! `OptionService::set_many`, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, ContentService, ContentTypeService, CoreError, NewType, OptionService,
    PermalinkRules,
    events::{ALL_KEY, OPTIONS_CHANGED, option_key},
    locales,
};
use ferrite_domain::{NewUser, Role, UserId, UserStatus, locale::SiteLocales};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

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
    Actor::new(id, Role::Admin)
}

fn draft(kind: &str, slug: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: slug.into(),
        slug: Some(slug.into()),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

fn content(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

fn options(db: &TestDb) -> OptionService {
    OptionService::new(db.shared_store())
}

fn set(key: &str, value: Value) -> Vec<(String, Option<Value>)> {
    vec![(key.to_owned(), Some(value))]
}

async fn stored(store: &dyn Store, key: &str) -> Option<Value> {
    store
        .read()
        .options()
        .get(key)
        .await
        .unwrap()
        .map(|e| e.value)
}

async fn locales_of(store: &dyn Store) -> Vec<String> {
    locales::read(store.read())
        .await
        .unwrap()
        .all()
        .map(|d| d.tag.clone())
        .collect()
}

/// `(locale)` of every content item and term, sorted.
async fn row_locales(db: &TestDb) -> Vec<String> {
    let mut out: Vec<String> = db
        .query_text("SELECT locale FROM content_items UNION ALL SELECT locale FROM terms")
        .await
        .into_iter()
        .flatten()
        .collect();
    out.sort();
    out
}

async fn max_seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

async fn events_since(store: &dyn Store, after: i64) -> Vec<(String, Vec<String>)> {
    store
        .read()
        .events()
        .since(after, 1000)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.keys))
        .collect()
}

async fn read_falls_back_to_site_locale_option(db: &TestDb) {
    let store = db.store();
    assert_eq!(
        locales::read(store.read()).await.unwrap(),
        SiteLocales::single("en")
    );
    store
        .read()
        .options()
        .set("site.locale", &json!("pt_BR.UTF-8"), true)
        .await
        .unwrap();
    assert_eq!(locales_of(store).await, ["pt-BR"]);
    // An unreadable site.locales falls back too.
    store
        .read()
        .options()
        .set("site.locales", &json!("nope"), true)
        .await
        .unwrap();
    assert_eq!(locales_of(store).await, ["pt-BR"]);
}

/// The first `settle` writes `site.locales` from the config and relabels the rows of a
/// one-locale site to it; later ones change nothing.
async fn settle_relabels(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    content(db)
        .create(&actor, draft("post", "hello"))
        .await
        .unwrap();
    // The migration's guess was `en`; the config says `de`.
    let report = locales::settle(store, "de").await.unwrap();
    assert_eq!(report.wrote_option.as_deref(), Some("de"));
    assert!(report.relabelled.items >= 1);
    assert_eq!(locales_of(store).await, ["de"]);
    assert!(row_locales(db).await.iter().all(|l| l == "de"));
    // Once: the option now exists, the config no longer matters.
    let again = locales::settle(store, "fr").await.unwrap();
    assert!(!again.changed(), "{again:?}");
    assert_eq!(locales_of(store).await, ["de"]);
}

async fn settle_uses_the_site_locale_option_before_the_config(db: &TestDb) {
    let store = db.store();
    store
        .read()
        .options()
        .set("site.locale", &json!("pl_PL.UTF-8"), true)
        .await
        .unwrap();
    locales::settle(store, "de").await.unwrap();
    assert_eq!(locales_of(store).await, ["pl-PL"]);
}

async fn settle_fills_blank_rows_on_multilingual_site(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let svc = content(db);
    svc.create(&actor, draft("post", "a")).await.unwrap();
    svc.create(&actor, draft("post", "b")).await.unwrap();
    store
        .read()
        .options()
        .set("site.locales", &json!([{"tag": "en"}, {"tag": "de"}]), true)
        .await
        .unwrap();
    // `b` is German; `a` was written by an older binary (blank locale, no group).
    db.exec_raw("UPDATE content_items SET locale = 'de' WHERE slug = 'b'")
        .await;
    db.exec_raw("UPDATE content_items SET locale = '', translation_group = NULL WHERE slug = 'a'")
        .await;
    let report = locales::settle(store, "fr").await.unwrap();
    assert_eq!(report.wrote_option, None);
    assert_eq!(report.groups, 1);
    assert_eq!(report.relabelled.items, 1);
    let rows = db
        .query_text("SELECT slug || ':' || locale FROM content_items ORDER BY slug")
        .await;
    assert_eq!(rows, [Some("a:en".to_owned()), Some("b:de".to_owned())]);
}

async fn settle_is_idempotent_and_emits_all_only_on_change(db: &TestDb) {
    let store = db.store();
    let before = max_seq(store).await;
    let first = locales::settle(store, "en").await.unwrap();
    assert!(first.changed());
    let events = events_since(store, before).await;
    assert!(
        events
            .iter()
            .any(|(k, keys)| k == "invalidate" && keys == &[ALL_KEY.to_owned()]),
        "{events:?}"
    );
    let mid = max_seq(store).await;
    let second = locales::settle(store, "en").await.unwrap();
    assert!(!second.changed());
    assert!(events_since(store, mid).await.is_empty());
}

async fn add_locale_with_clashing_prefix_refused(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    locales::settle(store, "en").await.unwrap();
    let svc = content(db);
    let page = svc.create(&actor, draft("page", "de")).await.unwrap();
    ContentTypeService::new(db.shared_store())
        .create(
            &actor,
            NewType {
                name: "faq".into(),
                label: "FAQ".into(),
                hierarchical: false,
                has_archive: true,
                fields: None,
            },
        )
        .await
        .unwrap();
    let opts = options(db);
    let refused = |tag: &str| {
        let opts = &opts;
        let actor = &actor;
        let tag = tag.to_owned();
        async move {
            opts.set_many(
                actor,
                &set("site.locales", json!([{"tag": "en"}, {"tag": tag}])),
            )
            .await
        }
    };
    for (tag, needle) in [
        ("de", page.id.to_string()),
        ("faq", "a content type".to_owned()),
        ("tag", "a taxonomy".to_owned()),
        ("api", "a reserved word".to_owned()),
    ] {
        match refused(tag).await {
            Err(CoreError::InvalidField { field, message }) => {
                assert_eq!(field, "site.locales");
                assert!(message.contains(&needle), "{tag}: {message}");
            }
            other => panic!("{tag}: {other:?}"),
        }
    }
    assert_eq!(locales_of(store).await, ["en"]);
    // A free prefix passes, and a type may no longer take it as its name.
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "en"}, {"tag": "fr"}])),
    )
    .await
    .unwrap();
    assert_eq!(locales_of(store).await, ["en", "fr"]);
    let err = ContentTypeService::new(db.shared_store())
        .create(
            &actor,
            NewType {
                name: "fr".into(),
                label: "Fr".into(),
                hierarchical: false,
                has_archive: false,
                fields: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::InvalidField { field: "name", .. }),
        "{err:?}"
    );
}

async fn remove_used_locale_conflicts_with_counts(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let opts = options(db);
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "en"}, {"tag": "de"}])),
    )
    .await
    .unwrap();
    content(db)
        .create(&actor, draft("post", "x"))
        .await
        .unwrap();
    db.exec_raw("UPDATE content_items SET locale = 'de' WHERE slug = 'x'")
        .await;
    let err = opts
        .set_many(&actor, &set("site.locales", json!([{"tag": "en"}])))
        .await
        .unwrap_err();
    match err {
        CoreError::InUse(m) => assert!(m.contains("de") && m.contains("1 content item"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(locales_of(store).await, ["en", "de"]);
    db.exec_raw("DELETE FROM content_items WHERE slug = 'x'")
        .await;
    opts.set_many(&actor, &set("site.locales", json!([{"tag": "en"}])))
        .await
        .unwrap();
    assert_eq!(locales_of(store).await, ["en"]);
}

async fn rename_single_locale_relabels(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    locales::settle(store, "en").await.unwrap();
    content(db)
        .create(&actor, draft("post", "x"))
        .await
        .unwrap();
    let opts = options(db);
    opts.set_many(&actor, &set("site.locales", json!([{"tag": "de"}])))
        .await
        .unwrap();
    assert!(row_locales(db).await.iter().all(|l| l == "de"));
    assert_eq!(stored(store, "site.locale").await, Some(json!("de")));
    // A `site.locale` write alone on a one-locale site renames too.
    opts.set_many(&actor, &set("site.locale", json!("fr_FR")))
        .await
        .unwrap();
    assert_eq!(locales_of(store).await, ["fr-FR"]);
    assert!(row_locales(db).await.iter().all(|l| l == "fr-FR"));
}

async fn reorder_keeps_rows(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let opts = options(db);
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "en"}, {"tag": "de"}])),
    )
    .await
    .unwrap();
    content(db)
        .create(&actor, draft("post", "x"))
        .await
        .unwrap();
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "de"}, {"tag": "en"}])),
    )
    .await
    .unwrap();
    assert_eq!(locales_of(store).await, ["de", "en"]);
    assert_eq!(stored(store, "site.locale").await, Some(json!("de")));
    let rows = db.query_text("SELECT locale FROM content_items").await;
    assert_eq!(rows, [Some("en".to_owned())]);
}

async fn site_locale_alone_on_multilingual_must_match_default(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let opts = options(db);
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "en"}, {"tag": "de"}])),
    )
    .await
    .unwrap();
    let err = opts
        .set_many(&actor, &set("site.locale", json!("de")))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "site.locale",
                ..
            }
        ),
        "{err:?}"
    );
    opts.set_many(&actor, &set("site.locale", json!("en")))
        .await
        .unwrap();
    // Both in one write must agree.
    let err = opts
        .set_many(
            &actor,
            &[
                (
                    "site.locales".into(),
                    Some(json!([{"tag": "de"}, {"tag": "en"}])),
                ),
                ("site.locale".into(), Some(json!("en"))),
            ],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "site.locale",
                ..
            }
        ),
        "{err:?}"
    );
    // site.locales cannot be cleared.
    let err = opts
        .set_many(&actor, &[("site.locales".into(), None)])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "site.locales",
                ..
            }
        ),
        "{err:?}"
    );
}

async fn locale_texts_keys_must_be_listed(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let opts = options(db);
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "en"}, {"tag": "de"}])),
    )
    .await
    .unwrap();
    for bad in [
        json!({"fr": {"title": "x"}}),
        json!({"en": {"title": "x"}}),
        json!({"de": {"slogan": "x"}}),
        json!({"de": {"title": "a\u{0}b"}}),
        json!(["de"]),
    ] {
        let err = opts
            .set_many(&actor, &set("site.locale_texts", bad.clone()))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::InvalidField {
                    field: "site.locale_texts",
                    ..
                }
            ),
            "{bad}: {err:?}"
        );
    }
    let texts = json!({"de": {"title": "Mein Garten", "description": "Über Pflanzen"}});
    opts.set_many(&actor, &set("site.locale_texts", texts.clone()))
        .await
        .unwrap();
    let t = locales::texts(store.read(), "de").await.unwrap().unwrap();
    assert_eq!(t.title.as_deref(), Some("Mein Garten"));
    // Removing `de` drops its texts in the same write.
    opts.set_many(&actor, &set("site.locales", json!([{"tag": "en"}])))
        .await
        .unwrap();
    assert_eq!(stored(store, "site.locale_texts").await, Some(json!({})));
}

async fn locale_write_emits_options_changed_and_all(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let before = max_seq(store).await;
    options(db)
        .set_many(
            &actor,
            &set("site.locales", json!([{"tag": "en"}, {"tag": "de"}])),
        )
        .await
        .unwrap();
    let events = events_since(store, before).await;
    assert!(
        events
            .iter()
            .any(|(k, keys)| k == OPTIONS_CHANGED && keys == &[option_key("site.locales")]),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|(k, keys)| k == "invalidate" && keys == &[ALL_KEY.to_owned()]),
        "{events:?}"
    );
}

/// Review fix: a new default moves its items to the root, so every prefix is checked
/// against the new default's top-level slugs: a German page `fr` would sit at `/fr/`.
async fn default_change_refuses_a_shadowed_prefix(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let opts = options(db);
    opts.set_many(
        &actor,
        &set(
            "site.locales",
            json!([{"tag": "en"}, {"tag": "de"}, {"tag": "fr"}]),
        ),
    )
    .await
    .unwrap();
    let page = content(db)
        .create_localized(&actor, draft("page", "fr"), Some("de".into()), None)
        .await
        .unwrap();
    assert_eq!((page.locale.as_str(), page.slug.as_str()), ("de", "fr"));
    let err = opts
        .set_many(
            &actor,
            &set(
                "site.locales",
                json!([{"tag": "de"}, {"tag": "en"}, {"tag": "fr"}]),
            ),
        )
        .await
        .unwrap_err();
    match err {
        CoreError::InvalidField { field, message } => {
            assert_eq!(field, "site.locales");
            assert!(message.contains(&page.id.to_string()), "{message}");
            assert!(message.contains("/fr/"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(locales_of(store).await, ["en", "de", "fr"]);
}

/// Review fix: media is shared by every locale and follows the default, and the lossy
/// down of 0014 never deletes it.
async fn default_change_relabels_media_and_down_keeps_it(db: &TestDb) {
    let store = db.store();
    let actor = admin(store).await;
    let opts = options(db);
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "en"}, {"tag": "de"}])),
    )
    .await
    .unwrap();
    let svc = content(db);
    svc.create(&actor, draft("post", "x")).await.unwrap();
    svc.create(&actor, draft("post", "pic")).await.unwrap();
    db.exec_raw("UPDATE content_items SET type = 'media' WHERE slug = 'pic'")
        .await;
    opts.set_many(
        &actor,
        &set("site.locales", json!([{"tag": "de"}, {"tag": "en"}])),
    )
    .await
    .unwrap();
    let media = db
        .query_text("SELECT locale FROM content_items WHERE type = 'media'")
        .await;
    assert_eq!(media, [Some("de".to_owned())]);
    let post = db
        .query_text("SELECT locale FROM content_items WHERE type = 'post'")
        .await;
    assert_eq!(post, [Some("en".to_owned())]);
    // Back below 0014 (lossy): the English post goes with the old default; media stays.
    store.migrate_down(13, true).await.unwrap();
    let types = db.query_text("SELECT type FROM content_items").await;
    assert_eq!(types, [Some("media".to_owned())]);
}

backend_tests!(
    default_change_refuses_a_shadowed_prefix,
    default_change_relabels_media_and_down_keeps_it,
    read_falls_back_to_site_locale_option,
    settle_relabels,
    settle_uses_the_site_locale_option_before_the_config,
    settle_fills_blank_rows_on_multilingual_site,
    settle_is_idempotent_and_emits_all_only_on_change,
    add_locale_with_clashing_prefix_refused,
    remove_used_locale_conflicts_with_counts,
    rename_single_locale_relabels,
    reorder_keeps_rows,
    site_locale_alone_on_multilingual_must_match_default,
    locale_texts_keys_must_be_listed,
    locale_write_emits_options_changed_and_all,
);
