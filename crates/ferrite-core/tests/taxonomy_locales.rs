//! Terms and menus per locale (M6-1 Task 7, design 13 §2.5–§2.6), on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_core::{
    Actor, CoreError, MenuService, NewTermInput, OptionService, TaxonomyService, TermPlacement,
};
use ferrite_domain::{Menu, MenuItem, MenuItemId, MenuItemKind, NewUser, Role, UserId, UserStatus};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

async fn admin(db: &TestDb) -> Actor {
    let store: &dyn Store = db.store();
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
    let actor = Actor::new(id, Role::Admin);
    OptionService::new(db.shared_store())
        .set_many(
            &actor,
            &[(
                "site.locales".into(),
                Some(json!([{"tag": "en"}, {"tag": "de"}])),
            )],
        )
        .await
        .unwrap();
    actor
}

fn input(slug: &str) -> NewTermInput {
    NewTermInput {
        taxonomy: "category".into(),
        name: slug.into(),
        slug: Some(slug.into()),
        parent: None,
        description: String::new(),
    }
}

fn placed(locale: &str, of: Option<ferrite_domain::TermId>) -> TermPlacement {
    TermPlacement {
        locale: Some(locale.into()),
        translation_of: of,
    }
}

async fn term_slug_shared_across_locales(db: &TestDb) {
    let actor = admin(db).await;
    let svc = TaxonomyService::new(db.shared_store());
    let en = svc
        .create_localized_term(&actor, input("news"), TermPlacement::default())
        .await
        .unwrap();
    let de = svc
        .create_localized_term(&actor, input("news"), placed("de", Some(en.id)))
        .await
        .unwrap();
    assert_eq!((en.slug.as_str(), de.slug.as_str()), ("news", "news"));
    assert_eq!((en.locale.as_str(), de.locale.as_str()), ("en", "de"));
    assert_eq!(de.translation_group, en.id);
    let again = svc
        .create_localized_term(&actor, input("news"), placed("de", None))
        .await
        .unwrap();
    assert_eq!(again.slug, "news-2");
    assert_eq!(
        svc.list_terms("category", Some("de")).await.unwrap().len(),
        2
    );
    match svc
        .create_localized_term(&actor, input("x"), placed("de", Some(en.id)))
        .await
    {
        Err(CoreError::InUse(m)) => assert!(m.contains(&de.id.to_string()), "{m}"),
        other => panic!("{other:?}"),
    }
    let err = svc
        .create_localized_term(&actor, input("y"), placed("fr", None))
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

async fn term_parent_must_share_locale(db: &TestDb) {
    let actor = admin(db).await;
    let svc = TaxonomyService::new(db.shared_store());
    let en = svc
        .create_localized_term(&actor, input("parent"), TermPlacement::default())
        .await
        .unwrap();
    let mut child = input("child");
    child.parent = Some(en.id);
    let err = svc
        .create_localized_term(&actor, child, placed("de", None))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::InvalidField {
                field: "parent_id",
                ..
            }
        ),
        "{err:?}"
    );
}

async fn translate_term_maps_parent(db: &TestDb) {
    let actor = admin(db).await;
    let svc = TaxonomyService::new(db.shared_store());
    let parent = svc
        .create_localized_term(&actor, input("garden"), TermPlacement::default())
        .await
        .unwrap();
    let mut child = input("roses");
    child.parent = Some(parent.id);
    let child = svc
        .create_localized_term(&actor, child, TermPlacement::default())
        .await
        .unwrap();
    // Untranslated parent: top level.
    let lonely = svc.translate_term(&actor, child.id, "de").await.unwrap();
    assert_eq!(lonely.parent_id, None);
    assert_eq!(lonely.translation_group, child.translation_group);
    let parent_de = svc.translate_term(&actor, parent.id, "de").await.unwrap();
    let other = svc
        .create_localized_term(
            &actor,
            {
                let mut i = input("tulips");
                i.parent = Some(parent.id);
                i
            },
            TermPlacement::default(),
        )
        .await
        .unwrap();
    let mapped = svc.translate_term(&actor, other.id, "de").await.unwrap();
    assert_eq!(mapped.parent_id, Some(parent_de.id));
    assert_eq!(
        (mapped.slug.as_str(), mapped.locale.as_str()),
        ("tulips", "de")
    );
}

async fn term_events_carry_group_key(db: &TestDb) {
    let actor = admin(db).await;
    let store = db.store();
    let svc = TaxonomyService::new(db.shared_store());
    let before = store.read().events().max_seq().await.unwrap();
    let t = svc
        .create_localized_term(&actor, input("news"), TermPlacement::default())
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
    assert!(keys.contains(&format!("tgroup:{}", t.id)), "{keys:?}");
}

fn menu(name: &str, locale: &str, of: Option<&str>) -> Menu {
    Menu {
        name: name.into(),
        label: name.into(),
        items: vec![MenuItem {
            id: MenuItemId::new(),
            parent_id: None,
            position: 0,
            kind: MenuItemKind::Url,
            target_id: None,
            url: Some("/".into()),
            label: "Home".into(),
        }],
        locale: locale.into(),
        translation_of: of.map(str::to_owned),
    }
}

async fn menu_translation_must_point_at_default_menu(db: &TestDb) {
    let actor = admin(db).await;
    let svc = MenuService::new(db.shared_store());
    svc.put(&actor, &menu("primary", "", None)).await.unwrap();
    assert_eq!(svc.get("primary").await.unwrap().locale, "en");
    svc.put(&actor, &menu("side-de", "de", None)).await.unwrap();
    for (bad, field) in [
        (menu("x-de", "de", Some("nope")), "translation_of"),
        (menu("y-de", "de", Some("side-de")), "translation_of"),
        (menu("z", "en", Some("primary")), "translation_of"),
        (menu("w", "fr", None), "locale"),
    ] {
        let err = svc.put(&actor, &bad).await.unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidField { field: f, .. } if f == field),
            "{}: {err:?}",
            bad.name
        );
    }
}

async fn menu_translation_unique_per_locale(db: &TestDb) {
    let actor = admin(db).await;
    let svc = MenuService::new(db.shared_store());
    svc.put(&actor, &menu("primary", "en", None)).await.unwrap();
    svc.put(&actor, &menu("primary-de", "de", Some("primary")))
        .await
        .unwrap();
    // The same menu again is an update, not a clash.
    svc.put(&actor, &menu("primary-de", "de", Some("primary")))
        .await
        .unwrap();
    match svc
        .put(&actor, &menu("second-de", "de", Some("primary")))
        .await
    {
        Err(CoreError::InUse(m)) => assert!(m.contains("primary-de"), "{m}"),
        other => panic!("{other:?}"),
    }
}

async fn for_location_falls_back_to_default_menu(db: &TestDb) {
    let actor = admin(db).await;
    let svc = MenuService::new(db.shared_store());
    svc.put(&actor, &menu("primary", "en", None)).await.unwrap();
    assert_eq!(
        svc.for_location("primary", "de")
            .await
            .unwrap()
            .unwrap()
            .name,
        "primary"
    );
    svc.put(&actor, &menu("primary-de", "de", Some("primary")))
        .await
        .unwrap();
    assert_eq!(
        svc.for_location("primary", "de")
            .await
            .unwrap()
            .unwrap()
            .name,
        "primary-de"
    );
    assert_eq!(
        svc.for_location("primary", "en")
            .await
            .unwrap()
            .unwrap()
            .name,
        "primary"
    );
    assert!(svc.for_location("footer", "de").await.unwrap().is_none());
}

async fn deleting_default_menu_keeps_translations_listed(db: &TestDb) {
    let actor = admin(db).await;
    let svc = MenuService::new(db.shared_store());
    svc.put(&actor, &menu("primary", "en", None)).await.unwrap();
    svc.put(&actor, &menu("primary-de", "de", Some("primary")))
        .await
        .unwrap();
    svc.delete(&actor, "primary").await.unwrap();
    let list = svc.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "primary-de");
    assert_eq!(list[0].translation_of.as_deref(), Some("primary"));
}

/// Review fix: a menu translation's write and delete also flush the pages that showed the
/// default menu at that location (they depend on `menu:<of>`).
async fn menu_translation_events_name_the_translated_menu(db: &TestDb) {
    let actor = admin(db).await;
    let store = db.store();
    let svc = MenuService::new(db.shared_store());
    svc.put(&actor, &menu("primary", "en", None)).await.unwrap();
    svc.put(&actor, &menu("footer", "en", None)).await.unwrap();
    async fn keys_since(store: &dyn Store, before: i64) -> Vec<String> {
        store
            .read()
            .events()
            .since(before, 100)
            .await
            .unwrap()
            .into_iter()
            .flat_map(|e| e.keys)
            .collect()
    }
    let before = store.read().events().max_seq().await.unwrap();
    svc.put(&actor, &menu("nav-de", "de", Some("primary")))
        .await
        .unwrap();
    let keys = keys_since(store, before).await;
    assert!(keys.contains(&"menu:nav-de".to_owned()), "{keys:?}");
    assert!(keys.contains(&"menu:primary".to_owned()), "{keys:?}");
    // Moved to another location: the old and the new one are both flushed.
    let before = store.read().events().max_seq().await.unwrap();
    svc.put(&actor, &menu("nav-de", "de", Some("footer")))
        .await
        .unwrap();
    let keys = keys_since(store, before).await;
    assert!(keys.contains(&"menu:primary".to_owned()), "{keys:?}");
    assert!(keys.contains(&"menu:footer".to_owned()), "{keys:?}");
    let before = store.read().events().max_seq().await.unwrap();
    svc.delete(&actor, "nav-de").await.unwrap();
    let keys = keys_since(store, before).await;
    assert!(keys.contains(&"menu:footer".to_owned()), "{keys:?}");
}

backend_tests!(
    menu_translation_events_name_the_translated_menu,
    term_slug_shared_across_locales,
    term_parent_must_share_locale,
    translate_term_maps_parent,
    term_events_carry_group_key,
    menu_translation_must_point_at_default_menu,
    menu_translation_unique_per_locale,
    for_location_falls_back_to_default_menu,
    deleting_default_menu_keeps_translations_listed,
);
