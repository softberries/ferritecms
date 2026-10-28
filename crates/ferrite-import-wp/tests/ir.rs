//! The IR work file and the run driver (M3-1 Task 9, P5, P21), the driver on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ferrite_core::{Actor, PermalinkRules, SwappablePermalinks, import::ImportService};
use ferrite_domain::{ContentQuery, ImportRunState, NewUser, Role, UserId, UserStatus};
use ferrite_import_wp::ImportError;
use ferrite_import_wp::ir::Ir;
use ferrite_import_wp::model::{Author, Channel, Item, Record, Term};
use ferrite_import_wp::run::{DirSource, ImportContext, ImportOptions, StepHook, run};
use ferrite_import_wp::wxr;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use futures::TryStreamExt;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

fn parse_file(path: &Path) -> Vec<Record> {
    let mut records = Vec::new();
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    wxr::parse(file, |r| {
        records.push(r);
        Ok(())
    })
    .unwrap();
    records
}

#[tokio::test]
async fn ir_round_trip_of_site_a() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ir.sqlite");
    let records = parse_file(&corpus("site-a.xml"));

    let mut ir = Ir::create(&path).await.unwrap();
    for r in &records {
        ir.push(r).await.unwrap();
    }
    ir.flush().await.unwrap();
    ir.close().await;

    let ir = Ir::open(&path).await.unwrap();
    let mut channel = None;
    let (mut authors, mut terms, mut items) = (Vec::<Author>::new(), Vec::<Term>::new(), vec![]);
    for r in &records {
        match r {
            Record::Channel(c) => channel = Some(c.clone()),
            Record::Author(a) => authors.push(a.clone()),
            Record::Term(t) => terms.push(t.clone()),
            Record::Item(i) => items.push((**i).clone()),
        }
    }
    let channel: Channel = channel.unwrap();
    assert_eq!(ir.channel().await.unwrap(), channel);
    assert_eq!(channel.title, "Corpus Site");
    assert_eq!(ir.authors().await.unwrap(), authors);
    assert_eq!(ir.terms().await.unwrap(), terms);

    // By type, in file order.
    let posts: Vec<Item> = ir.items(&["post"]).try_collect().await.unwrap();
    assert_eq!(posts.len(), 8);
    let expected: Vec<Item> = items
        .iter()
        .filter(|i| i.post_type == "post" || i.post_type == "page")
        .cloned()
        .collect();
    let posts_and_pages: Vec<Item> = ir.items(&["page", "post"]).try_collect().await.unwrap();
    assert_eq!(posts_and_pages, expected);
    assert_eq!(posts_and_pages.len(), 12);
    let all: Vec<Item> = ir.items(&[]).try_collect().await.unwrap();
    assert_eq!(all, items);
    let none: Vec<Item> = ir.items(&["no_such_type"]).try_collect().await.unwrap();
    assert!(none.is_empty());

    // By id.
    let book = items.iter().find(|i| i.post_type == "book").unwrap();
    assert_eq!(ir.item(book.wp_id).await.unwrap().as_ref(), Some(book));
    assert_eq!(ir.item(-1).await.unwrap(), None);

    // Passes persist across a reopen.
    assert!(!ir.pass_done("pass1").await.unwrap());
    ir.mark_pass("pass1").await.unwrap();
    ir.mark_pass("pass1").await.unwrap();
    ir.close().await;
    let ir = Ir::open(&path).await.unwrap();
    assert!(ir.pass_done("pass1").await.unwrap());
    assert!(!ir.pass_done("pass2").await.unwrap());
}

#[tokio::test]
async fn open_refuses_a_missing_file_and_create_an_existing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ir.sqlite");
    assert!(Ir::open(&path).await.is_err());
    Ir::create(&path).await.unwrap().close().await;
    assert!(Ir::create(&path).await.is_err());
}

/// `push` commits every 1 000 records on its own (a second reader sees them before the
/// flush), and every record of a push across the boundary reads back in order.
#[tokio::test]
async fn push_commits_across_the_batch_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ir.sqlite");
    let mut ir = Ir::create(&path).await.unwrap();
    let items: Vec<Item> = (1..=2_345)
        .map(|n| Item {
            wp_id: n,
            post_type: if n % 2 == 0 { "post" } else { "page" }.into(),
            title: format!("Item {n}"),
            ..Item::default()
        })
        .collect();
    for (n, item) in items.iter().enumerate() {
        ir.push(&Record::Item(Box::new(item.clone())))
            .await
            .unwrap();
        if n + 1 == 1_500 {
            let reader = Ir::open(&path).await.unwrap();
            let seen: Vec<Item> = reader.items(&[]).try_collect().await.unwrap();
            assert_eq!(seen.len(), 1_000);
            reader.close().await;
        }
    }
    ir.flush().await.unwrap();
    ir.close().await;

    let ir = Ir::open(&path).await.unwrap();
    let all: Vec<Item> = ir.items(&[]).try_collect().await.unwrap();
    assert_eq!(all, items);
    let posts: Vec<Item> = ir.items(&["post"]).try_collect().await.unwrap();
    assert_eq!(posts.len(), 1_172);
    assert_eq!(ir.item(2_345).await.unwrap().unwrap().title, "Item 2345");
    ir.close().await;
}

// --- The driver -------------------------------------------------------------------------

struct Site {
    _media_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    ctx: ImportContext,
}

async fn admin(store: &dyn Store) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
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
    Actor::new(id, Role::Admin)
}

async fn site(db: &TestDb) -> Site {
    let media_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let media_store = MediaStore::open(MediaConfig {
        backend: Backend::Local {
            root: media_dir.path().join("media"),
        },
        prefix: String::new(),
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".into(),
        },
    })
    .unwrap();
    let permalinks = Arc::new(SwappablePermalinks::new(PermalinkRules::default()));
    let ctx = ImportContext {
        import: Arc::new(ImportService::new(db.shared_store(), permalinks.clone())),
        permalinks,
        media_store: Arc::new(media_store),
        actor: admin(db.store()).await,
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

/// A hook recording every step the driver runs and failing at `fail_at`.
fn recorder(fail_at: Option<&'static str>) -> (StepHook, Arc<Mutex<Vec<String>>>) {
    let steps = Arc::new(Mutex::new(Vec::new()));
    let seen = steps.clone();
    let hook = StepHook::new(move |step| {
        seen.lock().unwrap().push(step.to_owned());
        if Some(step) == fail_at {
            return Err(ImportError::Stopped);
        }
        Ok(())
    });
    (hook, steps)
}

/// The run folders (`<work>/import/*/`) left behind.
fn run_dirs(site: &Site) -> Vec<PathBuf> {
    match std::fs::read_dir(&site.ctx.work_dir) {
        Ok(dir) => dir.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

async fn content_rows(store: &dyn Store) -> u64 {
    store
        .read()
        .content()
        .count(&ContentQuery::default())
        .await
        .unwrap()
}

/// A failed run keeps its IR; the next run of the same file reuses it, skipping the
/// parse and the passes that finished, then finishes `done` and deletes the folder.
pub async fn resume_skips_parse_and_done_passes(db: &TestDb) {
    let site = site(db).await;
    let wxr = corpus("site-a.xml");

    let (hook, steps) = recorder(Some("pass2"));
    let opts = ImportOptions {
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    let err = run(site.ctx.clone(), &wxr, opts).await.unwrap_err();
    assert!(matches!(err, ImportError::Stopped), "{err:?}");
    assert_eq!(*steps.lock().unwrap(), ["parse", "pass1", "pass2"]);
    let runs = db.store().read().imports().list_runs(5).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, ImportRunState::Failed);
    assert_eq!(runs[0].source, "http://localhost:8099");
    let dirs = run_dirs(&site);
    assert_eq!(dirs, [site.ctx.work_dir.join(runs[0].id.to_string())]);
    let ir = Ir::open(&dirs[0].join("ir.sqlite")).await.unwrap();
    assert!(ir.pass_done("parse").await.unwrap());
    assert!(ir.pass_done("pass1").await.unwrap());
    assert!(!ir.pass_done("pass2").await.unwrap());
    ir.close().await;

    let (hook, steps) = recorder(None);
    let opts = ImportOptions {
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    run(site.ctx.clone(), &wxr, opts).await.unwrap();
    assert_eq!(*steps.lock().unwrap(), ["pass2", "pass3", "pass4", "fixup"]);
    let runs = db.store().read().imports().list_runs(5).await.unwrap();
    assert_eq!(runs.len(), 2);
    let done = runs
        .iter()
        .find(|r| r.state == ImportRunState::Done)
        .unwrap();
    assert!(!done.dry_run);
    assert!(run_dirs(&site).is_empty(), "{:?}", run_dirs(&site));
    drop(site.work);
}

/// A dry run is recorded as one, writes no content and leaves no folder.
pub async fn dry_run_starts_a_dry_run_and_writes_nothing(db: &TestDb) {
    let site = site(db).await;
    let before = content_rows(db.store()).await;
    let (hook, steps) = recorder(None);
    let opts = ImportOptions {
        dry_run: true,
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    let report = run(site.ctx.clone(), &corpus("site-a.xml"), opts)
        .await
        .unwrap();
    assert_eq!(
        *steps.lock().unwrap(),
        ["parse", "pass1", "pass2", "pass3", "pass4", "fixup"]
    );
    assert!(report.counts.is_empty());
    let runs = db.store().read().imports().list_runs(5).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].dry_run);
    assert_eq!(runs[0].state, ImportRunState::Done);
    assert_eq!(content_rows(db.store()).await, before);
    assert!(run_dirs(&site).is_empty());
}

/// A dry run rolls its passes back, so its IR records none of them (only the parse):
/// a later real run of the same file must not skip them.
pub async fn dry_run_marks_no_pass(db: &TestDb) {
    let site = site(db).await;
    let (hook, steps) = recorder(Some("fixup"));
    let opts = ImportOptions {
        dry_run: true,
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    let wxr = corpus("site-a.xml");
    let err = run(site.ctx.clone(), &wxr, opts).await.unwrap_err();
    assert!(matches!(err, ImportError::Stopped), "{err:?}");
    assert_eq!(
        *steps.lock().unwrap(),
        ["parse", "pass1", "pass2", "pass3", "pass4", "fixup"]
    );
    let dirs = run_dirs(&site);
    assert_eq!(dirs.len(), 1);
    let ir = Ir::open(&dirs[0].join("ir.sqlite")).await.unwrap();
    assert!(ir.pass_done("parse").await.unwrap());
    for pass in ["pass1", "pass2", "pass3", "pass4", "fixup"] {
        assert!(!ir.pass_done(pass).await.unwrap(), "{pass}");
    }
    ir.close().await;

    let (hook, steps) = recorder(None);
    let opts = ImportOptions {
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    run(site.ctx.clone(), &wxr, opts).await.unwrap();
    assert_eq!(
        *steps.lock().unwrap(),
        ["pass1", "pass2", "pass3", "pass4", "fixup"]
    );
}

/// A file that is not WXR fails before a run starts.
pub async fn a_broken_file_starts_no_run(db: &TestDb) {
    let site = site(db).await;
    let bad = site.work.path().join("bad.xml");
    std::fs::write(&bad, "<rss><channel>").unwrap();
    let err = run(site.ctx.clone(), &bad, ImportOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ImportError::Xml { .. } | ImportError::Missing { .. }),
        "{err:?}"
    );
    assert!(
        db.store()
            .read()
            .imports()
            .list_runs(5)
            .await
            .unwrap()
            .is_empty()
    );
}

backend_tests!(
    resume_skips_parse_and_done_passes,
    dry_run_starts_a_dry_run_and_writes_nothing,
    dry_run_marks_no_pass,
    a_broken_file_starts_no_run,
);
