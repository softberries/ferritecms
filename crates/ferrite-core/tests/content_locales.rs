//! Content locales and translation groups (M6-1 Task 6, design 13 §2.3–§2.4): create in
//! a locale, translate, relocate and group joins, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, AdminQuery, ContentDraft, ContentService, CoreError, OptionService, PermalinkRules,
    Relocation, TaxonomyService, TranslateNote,
    content::{Seo, SeoPatch},
    locales,
};
use ferrite_domain::{NewUser, Role, TermId, UserId, UserStatus};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

async fn user(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, role)
}

fn draft(kind: &str, title: &str, slug: Option<&str>) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: slug.map(str::to_owned),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": title}]}]}),
        fields: json!({}),
        menu_order: 3,
        terms: Vec::new(),
        seo: None,
    }
}

/// A multilingual site (`en`, `de`) and its content service with matching rules.
async fn multilingual(db: &TestDb) -> (Actor, ContentService) {
    let store = db.store();
    let admin = user(store, Role::Admin).await;
    OptionService::new(db.shared_store())
        .set_many(
            &admin,
            &[(
                "site.locales".into(),
                Some(json!([{"tag": "en"}, {"tag": "de"}])),
            )],
        )
        .await
        .unwrap();
    let rules = PermalinkRules::default().with_locales(locales::read(store.read()).await.unwrap());
    (
        admin,
        ContentService::new(db.shared_store(), Arc::new(rules)),
    )
}

async fn term_in(
    db: &TestDb,
    actor: &Actor,
    slug: &str,
    locale: &str,
    group: Option<TermId>,
) -> TermId {
    let t = TaxonomyService::new(db.shared_store())
        .create_term(actor, "category", slug, Some(slug.into()), None, "")
        .await
        .unwrap();
    let group = group.unwrap_or(t.id);
    let id = |u: uuid::Uuid| {
        if db.is_sqlite() {
            format!("x'{}'", u.simple())
        } else {
            format!("'{u}'")
        }
    };
    db.exec_raw(&format!(
        "UPDATE terms SET locale = '{locale}', translation_group = {} WHERE id = {}",
        id(group.as_uuid()),
        id(t.id.as_uuid())
    ))
    .await;
    t.id
}

async fn create_defaults_to_default_locale(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let item = svc
        .create(&admin, draft("post", "Hello", None))
        .await
        .unwrap();
    assert_eq!(item.locale, "en");
    assert_eq!(item.translation_group, item.id);
    let de = svc
        .create_localized(
            &admin,
            draft("post", "Hallo", None),
            Some("de".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(de.locale, "de");
}

async fn create_in_unlisted_locale_refused(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let err = svc
        .create_localized(
            &admin,
            draft("post", "Bonjour", None),
            Some("fr".into()),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "locale",
                ..
            }
        ),
        "{err:?}"
    );
}

async fn same_slug_in_two_locales(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let en = svc
        .create(&admin, draft("page", "About", Some("about")))
        .await
        .unwrap();
    let de = svc
        .create_localized(
            &admin,
            draft("page", "Über", Some("about")),
            Some("de".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!((en.slug.as_str(), de.slug.as_str()), ("about", "about"));
    let de2 = svc
        .create_localized(
            &admin,
            draft("page", "Über 2", Some("about")),
            Some("de".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(de2.slug, "about-2");
}

async fn prefix_slug_refused_for_top_level_default_item(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let err = svc
        .create(&admin, draft("page", "De", Some("de")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::InvalidField { field: "slug", .. }),
        "{err:?}"
    );
    let generated = svc.create(&admin, draft("page", "de", None)).await.unwrap();
    assert_eq!(generated.slug, "de-2");
    // In German, `de` is just a slug (`/de/de/`).
    let german = svc
        .create_localized(&admin, draft("page", "de", None), Some("de".into()), None)
        .await
        .unwrap();
    assert_eq!(german.slug, "de");
}

async fn translate_copies_fields_seo_and_maps_terms_and_parent(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let parent = svc
        .create(&admin, draft("page", "Garden", Some("garden")))
        .await
        .unwrap();
    let parent_de = svc.translate(&admin, parent.id, "de").await.unwrap().item;
    let mut child = draft("page", "Roses", Some("roses"));
    child.parent_id = Some(parent.id);
    child.seo = Some(SeoPatch {
        title: Some(Some("Roses SEO".into())),
        ..SeoPatch::default()
    });
    let child = svc.create(&admin, child).await.unwrap();
    let done = svc.translate(&admin, child.id, "de").await.unwrap();
    assert!(done.notes.is_empty(), "{:?}", done.notes);
    let t = done.item;
    assert_eq!(t.locale, "de");
    assert_eq!(t.translation_group, child.translation_group);
    assert_eq!(t.parent_id, Some(parent_de.id));
    assert_eq!(
        (t.title.as_str(), t.slug.as_str(), t.menu_order),
        ("Roses", "roses", 3)
    );
    assert_eq!(t.body, child.body);
    assert_eq!(t.status, ferrite_domain::ContentStatus::Draft);
    let seo = Seo::load(db.store().read(), t.id).await.unwrap();
    assert_eq!(seo.title.as_deref(), Some("Roses SEO"));

    let news = term_in(db, &admin, "news", "en", None).await;
    let news_de = term_in(db, &admin, "nachrichten", "de", Some(news)).await;
    let mut post = draft("post", "Story", None);
    post.terms = vec![news];
    let post = svc.create(&admin, post).await.unwrap();
    let t = svc.translate(&admin, post.id, "de").await.unwrap();
    let terms = db.store().read().terms().for_item(t.item.id).await.unwrap();
    assert_eq!(terms.iter().map(|t| t.id).collect::<Vec<_>>(), [news_de]);
}

async fn translate_notes_untranslated_parent_and_terms(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let parent = svc
        .create(&admin, draft("page", "Garden", None))
        .await
        .unwrap();
    let mut child = draft("page", "Roses", None);
    child.parent_id = Some(parent.id);
    let child = svc.create(&admin, child).await.unwrap();
    let t = svc.translate(&admin, child.id, "de").await.unwrap();
    assert_eq!(t.notes, [TranslateNote::ParentUntranslated]);
    assert_eq!(t.item.parent_id, None);

    let tag = term_in(db, &admin, "solo", "en", None).await;
    let mut post = draft("post", "P", None);
    post.terms = vec![tag];
    let post = svc.create(&admin, post).await.unwrap();
    let t = svc.translate(&admin, post.id, "de").await.unwrap();
    assert_eq!(t.notes, [TranslateNote::TermUntranslated(tag)]);
    assert_eq!(t.notes[0].code(), format!("term_untranslated:{tag}"));
}

async fn translate_twice_conflicts_with_existing_id(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let post = svc.create(&admin, draft("post", "P", None)).await.unwrap();
    let first = svc.translate(&admin, post.id, "de").await.unwrap().item;
    match svc.translate(&admin, post.id, "de").await {
        Err(CoreError::InUse(m)) => assert!(m.contains(&first.id.to_string()), "{m}"),
        other => panic!("{other:?}"),
    }
    // Translating into the source's own locale is refused too.
    let err = svc.translate(&admin, first.id, "de").await.unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "locale",
                ..
            }
        ),
        "{err:?}"
    );
}

async fn contributor_translates_published_post_as_own_draft(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let post = svc
        .create(&admin, draft("post", "Public", None))
        .await
        .unwrap();
    svc.publish(&admin, post.id, post.version).await.unwrap();
    let contributor = user(db.store(), Role::Contributor).await;
    let t = svc
        .translate(&contributor, post.id, "de")
        .await
        .unwrap()
        .item;
    assert_eq!(t.author_id, contributor.user);
    assert_eq!(t.status, ferrite_domain::ContentStatus::Draft);
}

async fn translate_unreadable_source_not_found(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let private_draft = svc
        .create(&admin, draft("post", "Secret", None))
        .await
        .unwrap();
    let contributor = user(db.store(), Role::Contributor).await;
    let err = svc
        .translate(&contributor, private_draft.id, "de")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::NotFound), "{err:?}");
}

async fn relocate_with_children_refused(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let parent = svc
        .create(&admin, draft("page", "Parent", None))
        .await
        .unwrap();
    let mut child = draft("page", "Child", None);
    child.parent_id = Some(parent.id);
    svc.create(&admin, child).await.unwrap();
    let err = svc
        .relocate(
            &admin,
            parent.id,
            parent.version,
            Relocation {
                locale: Some("de".into()),
                translation_of: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
}

async fn relocate_moves_locale_and_reallocates_slug(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    svc.create_localized(
        &admin,
        draft("post", "X", Some("story")),
        Some("de".into()),
        None,
    )
    .await
    .unwrap();
    let en = svc
        .create(&admin, draft("post", "Story", Some("story")))
        .await
        .unwrap();
    let moved = svc
        .relocate(
            &admin,
            en.id,
            en.version,
            Relocation {
                locale: Some("de".into()),
                translation_of: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (moved.locale.as_str(), moved.slug.as_str()),
        ("de", "story-2")
    );
    assert_eq!(moved.version, en.version + 1);
    let err = svc
        .relocate(&admin, en.id, en.version, Relocation::default())
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "{err:?}");
}

async fn join_group_refuses_taken_locale(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let en = svc.create(&admin, draft("post", "A", None)).await.unwrap();
    let de = svc.translate(&admin, en.id, "de").await.unwrap().item;
    let other_de = svc
        .create_localized(&admin, draft("post", "B", None), Some("de".into()), None)
        .await
        .unwrap();
    match svc
        .relocate(
            &admin,
            other_de.id,
            other_de.version,
            Relocation {
                locale: None,
                translation_of: Some(Some(en.id)),
            },
        )
        .await
    {
        Err(CoreError::InUse(m)) => assert!(m.contains(&de.id.to_string()), "{m}"),
        other => panic!("{other:?}"),
    }
    match svc
        .create_localized(
            &admin,
            draft("post", "C", None),
            Some("de".into()),
            Some(en.id),
        )
        .await
    {
        Err(CoreError::InUse(_)) => {}
        other => panic!("{other:?}"),
    }
}

async fn leave_group_becomes_own_group(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let en = svc.create(&admin, draft("post", "A", None)).await.unwrap();
    let de = svc.translate(&admin, en.id, "de").await.unwrap().item;
    let left = svc
        .relocate(
            &admin,
            de.id,
            de.version,
            Relocation {
                locale: None,
                translation_of: Some(None),
            },
        )
        .await
        .unwrap();
    assert_eq!(left.translation_group, de.id);
    // And it can join again.
    let back = svc
        .relocate(
            &admin,
            de.id,
            left.version,
            Relocation {
                locale: None,
                translation_of: Some(Some(en.id)),
            },
        )
        .await
        .unwrap();
    assert_eq!(back.translation_group, en.translation_group);
}

async fn terms_of_other_locale_refused_on_multilingual_site(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let de_term = term_in(db, &admin, "garten", "de", None).await;
    let mut post = draft("post", "P", None);
    post.terms = vec![de_term];
    let err = svc.create(&admin, post).await.unwrap_err();
    assert!(
        matches!(err, CoreError::InvalidField { field: "terms", .. }),
        "{err:?}"
    );
}

async fn writes_emit_group_keys(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let store = db.store();
    let en = svc.create(&admin, draft("post", "A", None)).await.unwrap();
    let before = store.read().events().max_seq().await.unwrap();
    let de = svc.translate(&admin, en.id, "de").await.unwrap().item;
    let keys: Vec<String> = store
        .read()
        .events()
        .since(before, 100)
        .await
        .unwrap()
        .into_iter()
        .flat_map(|e| e.keys)
        .collect();
    assert!(keys.contains(&format!("group:{}", en.id)), "{keys:?}");
    let before = store.read().events().max_seq().await.unwrap();
    svc.relocate(
        &admin,
        de.id,
        de.version,
        Relocation {
            locale: None,
            translation_of: Some(None),
        },
    )
    .await
    .unwrap();
    let keys: Vec<String> = store
        .read()
        .events()
        .since(before, 100)
        .await
        .unwrap()
        .into_iter()
        .flat_map(|e| e.keys)
        .collect();
    assert!(
        keys.contains(&format!("group:{}", en.id)),
        "old group: {keys:?}"
    );
    assert!(
        keys.contains(&format!("group:{}", de.id)),
        "new group: {keys:?}"
    );
}

async fn admin_list_filters_by_locale_and_returns_translations(db: &TestDb) {
    let (admin, svc) = multilingual(db).await;
    let en = svc.create(&admin, draft("post", "A", None)).await.unwrap();
    let de = svc.translate(&admin, en.id, "de").await.unwrap().item;
    let mut q = AdminQuery::new("post");
    q.locale = Some("de".into());
    let page = svc.list(&admin, &q).await.unwrap();
    assert_eq!(page.items.iter().map(|i| i.id).collect::<Vec<_>>(), [de.id]);
    let counts = svc.counts(&admin, "post", Some("en")).await.unwrap();
    assert_eq!(counts.draft, 1);
    let tr = svc
        .translations(&admin, std::slice::from_ref(&en))
        .await
        .unwrap();
    let refs = &tr[&en.id];
    assert_eq!(refs.len(), 1);
    assert_eq!((refs[0].id, refs[0].locale.as_str()), (de.id, "de"));
    // A contributor does not see another user's draft translation.
    let contributor = user(db.store(), Role::Contributor).await;
    let tr = svc.translations(&contributor, &[en]).await.unwrap();
    assert!(tr.values().all(Vec::is_empty));
}

backend_tests!(
    create_defaults_to_default_locale,
    create_in_unlisted_locale_refused,
    same_slug_in_two_locales,
    prefix_slug_refused_for_top_level_default_item,
    translate_copies_fields_seo_and_maps_terms_and_parent,
    translate_notes_untranslated_parent_and_terms,
    translate_twice_conflicts_with_existing_id,
    contributor_translates_published_post_as_own_draft,
    translate_unreadable_source_not_found,
    relocate_with_children_refused,
    relocate_moves_locale_and_reallocates_slug,
    join_group_refuses_taken_locale,
    leave_group_becomes_own_group,
    terms_of_other_locale_refused_on_multilingual_site,
    writes_emit_group_keys,
    admin_list_filters_by_locale_and_returns_translations,
);
