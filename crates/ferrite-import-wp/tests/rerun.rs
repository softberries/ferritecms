//! Re-running an import (M3-1 Task 15, P3, P4): a second run changes nothing, a local
//! edit is kept, and a run that failed part-way resumes to the state of a clean run.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ferrite_core::{
    Actor, ContentDraft, ContentService, MenuService, OptionService, PermalinkRules,
    RedirectService, SwappablePermalinks, import::ImportService,
};
use ferrite_domain::{
    CommentQuery, ContentItem, ContentQuery, Menu, NewUser, RedirectQuery, Role, UserId, UserStatus,
};
use ferrite_import_wp::ImportError;
use ferrite_import_wp::run::{DirSource, ImportContext, ImportOptions, Report, StepHook, run};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

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

/// A copy of site A with `edits` (old, new) applied, each found exactly once.
fn edited(site: &Site, name: &str, edits: &[(&str, &str)]) -> PathBuf {
    let mut xml = std::fs::read_to_string(corpus("site-a.xml")).unwrap();
    for (from, to) in edits {
        assert_eq!(xml.matches(from).count(), 1, "{from}");
        xml = xml.replace(from, to);
    }
    let path = site.work.path().join(name);
    std::fs::write(&path, xml).unwrap();
    path
}

async fn option(store: &dyn Store, key: &str) -> Option<Value> {
    store
        .read()
        .options()
        .get(key)
        .await
        .unwrap()
        .map(|o| o.value)
}

fn noted(report: &Report, needle: &str) -> bool {
    report.notes.iter().any(|n| n.contains(needle))
}

const TITLE: &str = "<title>Corpus Site</title>";
const HOME: &str = "<title><![CDATA[Home]]></title>";

/// Options follow P4: an unchanged WXR value is skipped; a changed one is written only
/// while the site still holds what the import wrote, else the site's is kept (noted).
pub async fn rerun_options_follow_p4(db: &TestDb) {
    let site = site(db).await;
    let store = db.store();
    import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    assert_eq!(
        option(store, "site.title").await,
        Some(json!("Corpus Site"))
    );
    OptionService::new(db.shared_store())
        .set(&site.ctx.actor, "site.title", &json!("Mine"), true)
        .await
        .unwrap();
    // The same file: the local title stays, nothing to note.
    let report = import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    assert_eq!(option(store, "site.title").await, Some(json!("Mine")));
    assert!(!noted(&report, "site.title"), "{:?}", report.notes);
    // Changed in WordPress, edited here: kept, noted. The description, not edited here,
    // follows WordPress.
    let description = std::fs::read_to_string(corpus("site-a.xml")).unwrap();
    let old_description = description
        .split("<description>")
        .nth(1)
        .and_then(|d| d.split("</description>").next())
        .unwrap()
        .to_owned();
    let changed = edited(
        &site,
        "changed.xml",
        &[
            (TITLE, "<title>Corpus Site 2</title>"),
            (
                &format!("<description>{old_description}</description>"),
                "<description>New tagline</description>",
            ),
        ],
    );
    let report = import(&site, &changed, ImportOptions::default()).await;
    assert_eq!(option(store, "site.title").await, Some(json!("Mine")));
    assert!(noted(&report, "site.title"), "{:?}", report.notes);
    assert_eq!(
        option(store, "site.description").await,
        Some(json!("New tagline"))
    );
}

/// On the first import, a site with its own post structure keeps it (noted).
pub async fn permalinks_keep_the_sites_own_structure(db: &TestDb) {
    let site = site(db).await;
    OptionService::new(db.shared_store())
        .set(
            &site.ctx.actor,
            "permalink.post",
            &json!("/blog/%slug%/"),
            true,
        )
        .await
        .unwrap();
    site.ctx
        .permalinks
        .swap(PermalinkRules::from_options(Some("/blog/%slug%/"), None).unwrap());
    let report = import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    assert_eq!(
        option(db.store(), "permalink.post").await,
        Some(json!("/blog/%slug%/"))
    );
    assert_eq!(report.permalink_structure, None);
    assert!(noted(&report, "permalink.post"), "{:?}", report.notes);
}

/// A menu edited here is kept on a re-run (noted); one deleted here is made again.
pub async fn rerun_menus_follow_p4(db: &TestDb) {
    let site = site(db).await;
    let store = db.store();
    import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    let menus = MenuService::new(db.shared_store());
    let mut menu = store.read().menus().get("main").await.unwrap().unwrap();
    menu.items[0].label = "Start".into();
    menus.put(&site.ctx.actor, &menu).await.unwrap();
    let changed = edited(
        &site,
        "home.xml",
        &[(HOME, "<title><![CDATA[Homepage]]></title>")],
    );
    let report = import(&site, &changed, ImportOptions::default()).await;
    assert_eq!(report.counts["menus"].kept, 1, "{:?}", report.counts);
    assert!(
        noted(&report, "menu \"main\" was edited here"),
        "{:?}",
        report.notes
    );
    let menu = store.read().menus().get("main").await.unwrap().unwrap();
    assert_eq!(menu.items[0].label, "Start");

    menus.delete(&site.ctx.actor, "main").await.unwrap();
    let report = import(&site, &changed, ImportOptions::default()).await;
    assert_eq!(report.counts["menus"].created, 1, "{:?}", report.counts);
    let menu = store.read().menus().get("main").await.unwrap().unwrap();
    assert_eq!(menu.items[0].label, "Homepage");
    assert_eq!(menu.items.len(), 5);
}

/// A site menu of the same name is kept; the import's goes under `<name>-wp` (noted),
/// and stays there on a re-run.
pub async fn native_menu_of_the_same_name_is_kept(db: &TestDb) {
    let site = site(db).await;
    let store = db.store();
    let menus = MenuService::new(db.shared_store());
    menus
        .put(
            &site.ctx.actor,
            &Menu {
                name: "main".into(),
                label: "Native".into(),
                items: Vec::new(),
            },
        )
        .await
        .unwrap();
    let report = import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    assert!(noted(&report, "main-wp"), "{:?}", report.notes);
    let native = store.read().menus().get("main").await.unwrap().unwrap();
    assert_eq!((native.label.as_str(), native.items.len()), ("Native", 0));
    let imported = store.read().menus().get("main-wp").await.unwrap().unwrap();
    assert_eq!(imported.items.len(), 5);
    let again = import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    assert_eq!(again.counts["menus"].unchanged, 1, "{:?}", again.counts);
}

/// A menu target trashed here does not fail the run: its entry is left out (noted) and
/// its child moves up.
pub async fn menu_target_trashed_here(db: &TestDb) {
    let site = site(db).await;
    let store = db.store();
    import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    let about = by_kind_title(store, "page", "About").await;
    ContentService::new(db.shared_store(), site.ctx.permalinks.clone())
        .trash(&site.ctx.actor, about.id, about.version)
        .await
        .unwrap();
    let changed = edited(
        &site,
        "home.xml",
        &[(HOME, "<title><![CDATA[Homepage]]></title>")],
    );
    let report = import(&site, &changed, ImportOptions::default()).await;
    assert!(noted(&report, "entry 34 left out"), "{:?}", report.notes);
    let menu = store.read().menus().get("main").await.unwrap().unwrap();
    let labels: Vec<(&str, bool)> = menu
        .items
        .iter()
        .map(|i| (i.label.as_str(), i.parent_id.is_some()))
        .collect();
    assert_eq!(
        labels,
        [
            ("Homepage", false),
            ("Team", false),
            ("All news", false),
            ("Every core block", false)
        ]
    );
}

/// A redirect edited here is kept on a re-run (noted).
pub async fn rerun_keeps_an_edited_redirect(db: &TestDb) {
    let site = site(db).await;
    let store = db.store();
    import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    let row = store
        .read()
        .redirects()
        .get_by_source("/books/the-first-book/")
        .await
        .unwrap()
        .unwrap();
    RedirectService::new(db.shared_store())
        .update(
            &site.ctx.actor,
            row.id,
            &row.source_path,
            "/elsewhere/",
            301,
        )
        .await
        .unwrap();
    let report = import(&site, &corpus("site-a.xml"), ImportOptions::default()).await;
    let kept = store
        .read()
        .redirects()
        .get_by_source("/books/the-first-book/")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.target, "/elsewhere/");
    assert_eq!(report.counts["redirects"].kept, 1, "{:?}", report.counts);
    assert!(
        noted(
            &report,
            "redirect from /books/the-first-book/ was edited here"
        ),
        "{:?}",
        report.notes
    );
}

async fn by_kind_title(store: &dyn Store, kind: &str, title: &str) -> ContentItem {
    store
        .read()
        .content()
        .list(&ContentQuery {
            kind: Some(kind.into()),
            limit: 100,
            ..ContentQuery::default()
        })
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|i| i.title == title)
        .unwrap_or_else(|| panic!("no {kind} {title:?}"))
}

/// The unchanged counts of a re-run of a complete site A import.
const RERUN_UNCHANGED: [(&str, u64); 12] = [
    ("book", 2),
    ("comments", 4),
    ("media", 4),
    ("menus", 1),
    ("options", 3),
    ("page", 4),
    ("post", 8),
    ("redirects", 7),
    ("taxonomies", 1),
    ("terms", 9),
    ("types", 1),
    ("users", 4),
];

/// Fails a run of site A (one item per batch) after batch `n`, which must fall in step
/// `step`; the next run takes over and ends where a clean run does: the same rows, menu
/// and options, and a further run with a clean re-run's counts.
async fn resume_at(db: &TestDb, n: u32, step: &'static str) {
    let site = site(db).await;
    let store = db.store();
    let wxr = corpus("site-a.xml");
    let steps = Arc::new(Mutex::new(Vec::new()));
    let seen = steps.clone();
    let opts = ImportOptions {
        batch_items: Some(1),
        fail_after_batches: Some(n),
        step_hook: Some(StepHook::new(move |s| {
            seen.lock().unwrap().push(s.to_owned());
            Ok(())
        })),
        ..ImportOptions::default()
    };
    let err = run(site.ctx.clone(), &wxr, opts).await.unwrap_err();
    assert!(matches!(err, ImportError::Stopped), "{err:?}");
    assert_eq!(
        steps.lock().unwrap().last().map(String::as_str),
        Some(step),
        "batch {n} is not in {step}"
    );
    let opts = ImportOptions {
        takeover: true,
        ..ImportOptions::default()
    };
    import(&site, &wxr, opts).await;
    assert_eq!(rows(store).await, SITE_A);
    let menu = store.read().menus().get("main").await.unwrap().unwrap();
    let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Home", "About", "Team", "All news", "Every core block"]
    );
    assert_eq!(
        option(store, "site.title").await,
        Some(json!("Corpus Site"))
    );
    assert_eq!(
        option(store, "permalink.post").await,
        Some(json!("/%year%/%month%/%slug%/"))
    );
    let again = import(&site, &wxr, ImportOptions::default()).await;
    all_unchanged(&again);
    let unchanged: Vec<(&str, u64)> = again
        .counts
        .iter()
        .map(|(k, c)| (k.as_str(), c.unchanged))
        .collect();
    assert_eq!(unchanged, RERUN_UNCHANGED);
}

/// Failure points in pass 1 (a term batch), pass 3 (an item) and pass 4 (a comment).
pub async fn resume_after_failure_in_pass1(db: &TestDb) {
    resume_at(db, 6, "pass1").await;
}

pub async fn resume_after_failure_in_pass3(db: &TestDb) {
    resume_at(db, 30, "pass3").await;
}

pub async fn resume_after_failure_in_pass4(db: &TestDb) {
    resume_at(db, 43, "pass4").await;
}

backend_tests!(
    rerun_site_a_is_unchanged,
    rerun_keeps_a_local_edit,
    rerun_options_follow_p4,
    permalinks_keep_the_sites_own_structure,
    rerun_menus_follow_p4,
    native_menu_of_the_same_name_is_kept,
    menu_target_trashed_here,
    rerun_keeps_an_edited_redirect,
    resume_after_failure_in_pass1,
    resume_after_failure_in_pass3,
    resume_after_failure_in_pass4,
);
