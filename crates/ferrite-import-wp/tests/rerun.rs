//! Re-running an import (M3-1 Task 15, P3, P4): a second run changes nothing, a local
//! edit is kept, and a run that failed part-way resumes to the state of a clean run.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, ContentService, PermalinkRules, SwappablePermalinks, import::ImportService,
};
use ferrite_domain::{
    CommentQuery, ContentItem, ContentQuery, NewUser, RedirectQuery, Role, UserId, UserStatus,
};
use ferrite_import_wp::ImportError;
use ferrite_import_wp::run::{DirSource, ImportContext, ImportOptions, Report, run};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

struct Site {
    _media_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    ctx: ImportContext,
}

async fn site(db: &TestDb) -> Site {
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
        work,
        ctx,
    }
}

async fn import(site: &Site, wxr: &Path, opts: ImportOptions) -> Report {
    run(site.ctx.clone(), wxr, opts).await.unwrap()
}

/// What the site holds after importing site A: content items (media included),
/// comments, redirects and terms of the imported taxonomies.
#[derive(Debug, PartialEq, Eq)]
struct Rows {
    content: u64,
    comments: usize,
    redirects: usize,
    terms: usize,
}

async fn rows(store: &dyn Store) -> Rows {
    let r = store.read();
    let mut terms = 0;
    for taxonomy in ["category", "tag", "genre"] {
        terms += r.terms().list(taxonomy).await.unwrap().len();
    }
    Rows {
        content: r.content().count(&ContentQuery::default()).await.unwrap(),
        comments: r
            .comments()
            .list(&CommentQuery {
                status: None,
                item: None,
                limit: 100,
                cursor: None,
            })
            .await
            .unwrap()
            .items
            .len(),
        redirects: r
            .redirects()
            .list(&RedirectQuery {
                limit: 100,
                ..RedirectQuery::default()
            })
            .await
            .unwrap()
            .items
            .len(),
        terms,
    }
}

/// A clean import of site A: 8 posts, 4 pages, 2 books and 4 media items; 4 comments;
/// the redirects of the 2 moved books, 4 attachment pages and the nested category; 9
/// terms.
const SITE_A: Rows = Rows {
    content: 18,
    comments: 4,
    redirects: 7,
    terms: 9,
};

/// Nothing was created, updated or kept in any kind.
fn all_unchanged(report: &Report) {
    for (kind, c) in &report.counts {
        assert_eq!(
            (c.created, c.updated, c.kept),
            (0, 0, 0),
            "{kind} changed on a re-run: {c:?}"
        );
    }
}

async fn by_title(store: &dyn Store, title: &str) -> ContentItem {
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
        .into_iter()
        .find(|i| i.title == title)
        .unwrap_or_else(|| panic!("no post {title:?}"))
}

/// A second import of the same file is all `unchanged` and adds no rows.
pub async fn rerun_site_a_is_unchanged(db: &TestDb) {
    let site = site(db).await;
    let wxr = corpus("site-a.xml");
    import(&site, &wxr, ImportOptions::default()).await;
    assert_eq!(rows(db.store()).await, SITE_A);
    let again = import(&site, &wxr, ImportOptions::default()).await;
    all_unchanged(&again);
    assert_eq!(again.counts["post"].unchanged, 8);
    assert_eq!(again.counts["comments"].unchanged, 4);
    assert_eq!(again.counts["menus"].unchanged, 1);
    assert_eq!(again.counts["redirects"].unchanged, 7);
    assert!(!again.counts.contains_key("fixups"), "{:?}", again.counts);
    assert_eq!(rows(db.store()).await, SITE_A);
}

/// A post edited here, then changed in WordPress, is kept as edited and noted (P4).
pub async fn rerun_keeps_a_local_edit(db: &TestDb) {
    let site = site(db).await;
    let wxr = corpus("site-a.xml");
    import(&site, &wxr, ImportOptions::default()).await;
    let store = db.shared_store();
    let post = by_title(store.as_ref(), "Sticky note").await;
    ContentService::new(store.clone(), site.ctx.permalinks.clone())
        .update(
            &site.ctx.actor,
            post.id,
            post.version,
            ContentDraft {
                kind: "post".into(),
                title: "Edited here".into(),
                slug: None,
                parent_id: None,
                excerpt: None,
                body: post.body.clone(),
                fields: post.fields.clone(),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    let changed = site.work.path().join("site-a-changed.xml");
    let xml = std::fs::read_to_string(&wxr).unwrap();
    let from = "<title><![CDATA[Sticky note]]></title>";
    assert_eq!(xml.matches(from).count(), 1);
    std::fs::write(
        &changed,
        xml.replace(from, "<title><![CDATA[Sticky note, revised]]></title>"),
    )
    .unwrap();

    let report = import(&site, &changed, ImportOptions::default()).await;
    let posts = report.counts["post"];
    assert_eq!(
        (posts.kept, posts.updated, posts.unchanged),
        (1, 0, 7),
        "{posts:?}"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n == "item 17 was edited here since the last import; kept"),
        "{:?}",
        report.notes
    );
    let kept = store.read().content().get(post.id).await.unwrap().unwrap();
    assert_eq!(kept.title, "Edited here");
}

/// A run that fails after its second batch (3 items each) is taken over by the next,
/// which ends where a clean run does; a third run changes nothing.
pub async fn resume_after_failure(db: &TestDb) {
    let site = site(db).await;
    let wxr = corpus("site-a.xml");
    let opts = ImportOptions {
        batch_items: Some(3),
        fail_after_batches: Some(2),
        ..ImportOptions::default()
    };
    let err = run(site.ctx.clone(), &wxr, opts).await.unwrap_err();
    assert!(matches!(err, ImportError::Stopped), "{err:?}");
    let opts = ImportOptions {
        takeover: true,
        ..ImportOptions::default()
    };
    import(&site, &wxr, opts).await;
    assert_eq!(rows(db.store()).await, SITE_A);
    let again = import(&site, &wxr, ImportOptions::default()).await;
    all_unchanged(&again);
    assert_eq!(rows(db.store()).await, SITE_A);
}

backend_tests!(
    rerun_site_a_is_unchanged,
    rerun_keeps_a_local_edit,
    resume_after_failure,
);
