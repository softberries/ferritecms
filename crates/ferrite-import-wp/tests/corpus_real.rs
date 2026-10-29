//! The WordPress theme-test files (GPL-2.0, not checked in, P15) imported into a fresh
//! SQLite site when `FERRITE_WP_CORPUS` names their directory; `SKIP` otherwise. Each
//! run must finish, count something, and leave every body editable in the admin. The
//! report counts are printed (run with `--nocapture`) for the M3-1 exit document.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferrite_core::blocks::is_editor_safe;
use ferrite_core::{Actor, PermalinkRules, SwappablePermalinks, import::ImportService};
use ferrite_domain::{ContentQuery, NewUser, Role, UserId, UserStatus};
use ferrite_import_wp::run::{DirSource, ImportContext, ImportOptions, run};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store_tests::TestDb;

/// The theme-test files of the corpus directory.
const FILES: [&str; 2] = ["themeunittestdata.wordpress.xml", "64-block-test-data.xml"];

fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("FERRITE_WP_CORPUS").map(PathBuf::from)
}

async fn import(file: &Path) {
    let db = TestDb::sqlite().await;
    let id = UserId::new();
    let tx = db.store().begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: "importer".into(),
            email: "importer@example.test".into(),
            display_name: "Importer".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let permalinks = Arc::new(SwappablePermalinks::new(PermalinkRules::default()));
    let ctx = ImportContext {
        import: Arc::new(ImportService::new(db.shared_store(), permalinks.clone())),
        permalinks,
        media_store: Arc::new(
            MediaStore::open(MediaConfig {
                backend: Backend::Local {
                    root: dir.path().join("media"),
                },
                prefix: String::new(),
                public: PublicUrlMode::ServeViaApp {
                    path_prefix: "/_media".into(),
                },
            })
            .unwrap(),
        ),
        actor: Actor::new(id, Role::Admin),
        work_dir: dir.path().join("import"),
        // No uploads: the theme-test files' attachments are reported missing.
        media_source: Arc::new(DirSource::new(dir.path().join("no-uploads"))),
        site_url: "http://localhost:8080".into(),
    };
    let report = run(ctx, file, ImportOptions::default()).await.unwrap();
    let name = file.file_name().unwrap().to_string_lossy();
    println!("== {name}");
    for (kind, c) in &report.counts {
        println!(
            "{kind}: created {} updated {} unchanged {} kept {} skipped {} failed {}",
            c.created, c.updated, c.unchanged, c.kept, c.skipped, c.failed
        );
    }
    println!(
        "missing media {}, refused redirects {}, unknown blocks {}, unknown shortcodes {}, \
         dropped formatting {:?}",
        report
            .totals
            .get("missing_media")
            .copied()
            .unwrap_or(report.missing_media.len() as u64),
        report.redirects.refused.len(),
        report.unknown_blocks.len(),
        report.unknown_shortcodes.len(),
        report.dropped_formatting
    );
    let created: u64 = report.counts.values().map(|c| c.created).sum();
    assert!(created > 0, "{name}: nothing imported");

    let items = db
        .store()
        .read()
        .content()
        .list(&ContentQuery {
            limit: 200,
            ..ContentQuery::default()
        })
        .await
        .unwrap();
    let mut seen = 0;
    let mut page = items;
    loop {
        for item in &page.items {
            assert!(
                is_editor_safe(&item.body),
                "{name}: the body of {:?} is not editor-safe",
                item.title
            );
            seen += 1;
        }
        let Some(cursor) = page.next_cursor.clone() else {
            break;
        };
        page = db
            .store()
            .read()
            .content()
            .list(&ContentQuery {
                limit: 200,
                cursor: Some(cursor),
                ..ContentQuery::default()
            })
            .await
            .unwrap();
    }
    println!("{name}: {seen} items checked editor-safe");
    assert!(seen > 0);
    db.teardown().await;
}

#[tokio::test]
async fn theme_test_files_import() {
    let Some(dir) = corpus_dir() else {
        println!("SKIP: FERRITE_WP_CORPUS not set");
        return;
    };
    for file in FILES {
        import(&dir.join(file)).await;
    }
}
