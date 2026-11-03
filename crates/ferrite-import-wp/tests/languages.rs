//! M6-1 Task 17: languages and translation groups from a WXR file (design 13 §8.1).
//! The two fixtures are hand-written in the shapes Polylang's storage and WPML's
//! "Export and Import" documentation describe; no real export was available
//! (`tests/corpus/README.md`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferrite_core::{Actor, PermalinkRules, SwappablePermalinks, import::ImportService};
use ferrite_domain::{ContentItem, ContentQuery, NewUser, Role, UserId, UserStatus};
use ferrite_import_wp::run::{DirSource, ImportContext, ImportOptions, Report, run};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

struct Site {
    _media_dir: tempfile::TempDir,
    _work: tempfile::TempDir,
    ctx: ImportContext,
}

async fn site(db: &TestDb, locales: &[&str]) -> Site {
    let id = UserId::new();
    let tx = db.store().begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: "importer".into(),
            email: "a@example.test".into(),
            display_name: "Importer".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    let list: Vec<_> = locales
        .iter()
        .map(|t| json!({"tag": t, "label": t}))
        .collect();
    tx.options()
        .set("site.locales", &json!(list), true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let media_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let permalinks = Arc::new(SwappablePermalinks::new(PermalinkRules::default()));
    let ctx = ImportContext {
        import: Arc::new(ImportService::new(db.shared_store(), permalinks.clone())),
        permalinks,
        media_store: Arc::new(
            MediaStore::open(MediaConfig {
                backend: Backend::Local {
                    root: media_dir.path().join("media"),
                },
                prefix: String::new(),
                public: PublicUrlMode::ServeViaApp {
                    path_prefix: "/_media".into(),
                },
            })
            .unwrap(),
        ),
        actor: Actor::new(id, Role::Admin),
        work_dir: work.path().join("import"),
        media_source: Arc::new(DirSource::new(corpus("site-a-uploads"))),
        site_url: "http://localhost:8080".into(),
    };
    Site {
        _media_dir: media_dir,
        _work: work,
        ctx,
    }
}

async fn import(site: &Site, file: &str) -> Report {
    run(site.ctx.clone(), &corpus(file), ImportOptions::default())
        .await
        .unwrap()
}

async fn posts(store: &dyn Store) -> Vec<ContentItem> {
    store
        .read()
        .content()
        .list(&ContentQuery {
            kind: Some("post".into()),
            limit: 100,
            ..ContentQuery::default()
        })
        .await
        .unwrap()
        .items
}

fn by<'a>(items: &'a [ContentItem], title: &str) -> &'a ContentItem {
    items.iter().find(|i| i.title == title).unwrap()
}

pub async fn polylang_languages_and_groups_imported(db: &TestDb) {
    let site = site(db, &["en", "de"]).await;
    let report = import(&site, "polylang-shape.xml").await;
    let items = posts(db.store()).await;
    let (en, de) = (by(&items, "Hello"), by(&items, "Hallo"));
    assert_eq!(en.locale, "en");
    assert_eq!(de.locale, "de");
    // The same slug in two locales.
    assert_eq!((en.slug.as_str(), de.slug.as_str()), ("hello", "hello"));
    assert_eq!(de.translation_group, en.translation_group);
    assert_eq!(en.translation_group, en.id);
    assert_eq!(by(&items, "Alone").locale, "en");
    // `fr` is not one of the site's locales: skipped and reported.
    assert!(items.iter().all(|i| i.title != "Bonjour"));
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("locale_not_on_site fr")),
        "{:?}",
        report.notes
    );
    // An invalid language falls back to the default, reported.
    assert_eq!(by(&items, "Bad tag").locale, "en");
    assert!(
        report.notes.iter().any(|n| n.contains("invalid_locale")),
        "{:?}",
        report.notes
    );
    assert_eq!(
        report.counts["translations"].updated, 1,
        "{:?}",
        report.counts
    );
}

pub async fn polylang_taxonomies_not_registered(db: &TestDb) {
    let site = site(db, &["en", "de"]).await;
    import(&site, "polylang-shape.xml").await;
    let taxonomies = db.store().read().taxonomies().list().await.unwrap();
    let names: Vec<_> = taxonomies.iter().map(|t| t.name.as_str()).collect();
    for internal in [
        "language",
        "post_translations",
        "term_language",
        "term_translations",
    ] {
        assert!(!names.contains(&internal), "{names:?}");
    }
}

pub async fn wpml_meta_languages_and_groups(db: &TestDb) {
    let site = site(db, &["en", "de"]).await;
    import(&site, "wpml-shape.xml").await;
    let items = posts(db.store()).await;
    let (en, de) = (by(&items, "Morning"), by(&items, "Morgen"));
    assert_eq!((en.locale.as_str(), de.locale.as_str()), ("en", "de"));
    assert_eq!(de.translation_group, en.translation_group);
    let plain = by(&items, "Plain");
    assert_eq!(plain.locale, "en");
    assert_eq!(plain.translation_group, plain.id);
}

pub async fn plain_wpml_export_imports_in_default_locale(db: &TestDb) {
    // Site A carries no language data: everything lands in the default locale.
    let site = site(db, &["de", "en"]).await;
    import(&site, "site-a.xml").await;
    let items = posts(db.store()).await;
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i.locale == "de"));
}

pub async fn rerun_keeps_groups_and_edited_items(db: &TestDb) {
    let site = site(db, &["en", "de"]).await;
    import(&site, "polylang-shape.xml").await;
    let first = posts(db.store()).await;
    let again = import(&site, "polylang-shape.xml").await;
    let now = posts(db.store()).await;
    for item in &first {
        let n = now.iter().find(|i| i.id == item.id).unwrap();
        assert_eq!(
            n.translation_group, item.translation_group,
            "{}",
            item.title
        );
        assert_eq!(n.version, item.version, "{}", item.title);
    }
    assert_eq!(
        again.counts["translations"].unchanged, 2,
        "{:?}",
        again.counts
    );
}

backend_tests!(
    polylang_languages_and_groups_imported,
    polylang_taxonomies_not_registered,
    wpml_meta_languages_and_groups,
    plain_wpml_export_imports_in_default_locale,
    rerun_keeps_groups_and_edited_items,
);
