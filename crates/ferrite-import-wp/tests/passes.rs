//! Passes 1 and 2 of the import (M3-1 Task 13): authors, taxonomies, terms and custom
//! types with their ACF fields, then attachments and the media URL index, on both
//! backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ferrite_core::fields::{FieldDef, FieldKind, schema_of};
use ferrite_core::{Actor, PermalinkRules, SwappablePermalinks, import::ImportService};
use ferrite_domain::{
    ContentId, ContentQuery, ContentStatus, MEDIA_KIND, NewUser, Role, Taxonomy, Term, TermId,
    UserId, UserStatus,
};
use ferrite_import_wp::ImportError;
use ferrite_import_wp::ir::Ir;
use ferrite_import_wp::run::{
    ByteStream, Counts, DirSource, ImportContext, ImportOptions, MediaSource, MissingMedia,
    MustReset, Report, StepHook, UnmappedAcf, run,
};
use ferrite_import_wp::urls::MediaIndex;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

struct Site {
    media_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    ctx: ImportContext,
}

/// The importing admin: e-mail `a@example.test`, the WXR `admin`'s (R2).
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

async fn site(db: &TestDb, media: Arc<dyn MediaSource>) -> Site {
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
        media_source: media,
        site_url: "http://localhost:8080".into(),
    };
    Site {
        media_dir,
        work,
        ctx,
    }
}

fn site_a_uploads() -> Arc<dyn MediaSource> {
    Arc::new(DirSource::new(corpus("site-a-uploads")))
}

/// Runs `wxr` up to and including `step`, then finishes the run.
async fn run_until(site: &Site, wxr: &Path, step: &'static str) -> Report {
    let opts = ImportOptions {
        stop_after: Some(step),
        ..ImportOptions::default()
    };
    run(site.ctx.clone(), wxr, opts).await.unwrap()
}

fn counts(report: &Report, kind: &str) -> Counts {
    report.counts.get(kind).copied().unwrap_or_default()
}

async fn term(store: &dyn Store, taxonomy: &str, slug: &str) -> Term {
    store
        .read()
        .terms()
        .get_by_slug(taxonomy, slug)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no term {taxonomy}/{slug}"))
}

async fn taxonomy(store: &dyn Store, name: &str) -> Option<Taxonomy> {
    store.read().taxonomies().get(name).await.unwrap()
}

fn field(name: &str, kind: FieldKind) -> FieldDef {
    FieldDef {
        name: name.into(),
        label: String::new(),
        kind,
        required: false,
        options: Vec::new(),
        fields: Vec::new(),
        types: Vec::new(),
    }
}

/// The fields without their labels, which are asserted apart.
fn shapes(defs: &[FieldDef]) -> Vec<FieldDef> {
    defs.iter()
        .map(|d| FieldDef {
            label: String::new(),
            fields: shapes(&d.fields),
            ..d.clone()
        })
        .collect()
}

/// Site A, pass 1: the authors, both term trees, the `genre` taxonomy and the `book`
/// type with its ACF fields (`quotes` unmapped, R3).
pub async fn pass1_site_a(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let store = db.store();
    let report = run_until(&site, &corpus("site-a.xml"), "pass1").await;

    // Users (P9, R2): `admin` is the importing admin by e-mail; the rest are new
    // authors without a password who must set one.
    assert!(
        store
            .read()
            .users()
            .get_by_login("admin")
            .await
            .unwrap()
            .is_none()
    );
    for (login, email, name) in [
        ("cass", "cass@example.test", "Cass Contributor"),
        ("erin", "erin@example.test", "Erin Editor"),
        ("arlo", "arlo@example.test", "Arlo Author"),
    ] {
        let user = store
            .read()
            .users()
            .get_by_login(login)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.email, email);
        assert_eq!(user.display_name, name);
        assert_eq!(user.role, Role::Author);
        assert!(user.must_reset);
        assert!(user.password_hash.is_none());
    }
    assert_eq!(
        counts(&report, "users"),
        Counts {
            created: 3,
            unchanged: 1,
            ..Counts::default()
        }
    );
    let mut reset: Vec<&str> = report
        .users_must_reset
        .iter()
        .map(|u| u.login.as_str())
        .collect();
    reset.sort_unstable();
    assert_eq!(reset, ["arlo", "cass", "erin"]);
    assert!(report.users_must_reset.iter().all(|u| u.how == "created"));

    // Terms, parents first; the menu term waits for the menus pass.
    let news = term(store, "category", "news").await;
    let local = term(store, "category", "local").await;
    assert_eq!(local.name, "Local News");
    assert_eq!(local.parent_id, Some(news.id));
    assert_eq!(
        term(store, "category", "guides").await.description,
        "How-to articles"
    );
    term(store, "tag", "self-hosting").await;
    let genre = taxonomy(store, "genre").await.unwrap();
    assert!(genre.hierarchical);
    assert_eq!(genre.content_types, ["book"]);
    assert_eq!(genre.label, "Genre");
    let fantasy = term(store, "genre", "fantasy").await;
    assert_eq!(
        term(store, "genre", "high-fantasy").await.parent_id,
        Some(fantasy.id)
    );
    assert!(taxonomy(store, "nav_menu").await.is_none());
    let terms = counts(&report, "terms");
    assert_eq!(terms.skipped, 1, "{terms:?}");
    assert_eq!(terms.created + terms.unchanged, 9, "{terms:?}");

    // The `book` type, its fields from the ACF export (P16).
    let book = store.read().types().get("book").await.unwrap().unwrap();
    assert_eq!(book.label, "Book");
    assert!(!book.hierarchical);
    assert!(book.has_archive);
    let defs = schema_of(&book.fields).unwrap().unwrap();
    let select = FieldDef {
        options: vec!["hardback".into(), "paperback".into()],
        ..field("format", FieldKind::Select)
    };
    let related = FieldDef {
        fields: vec![field("item", FieldKind::Relation)],
        ..field("related", FieldKind::Repeater)
    };
    assert_eq!(
        shapes(&defs),
        [
            field("isbn", FieldKind::Text),
            field("pages", FieldKind::Number),
            field("in_print", FieldKind::Bool),
            field("released", FieldKind::Date),
            field("blurb", FieldKind::Text),
            field("cover", FieldKind::Media),
            select,
            field("site", FieldKind::Text),
            field("review", FieldKind::RichText),
            related,
        ]
    );
    assert_eq!(defs[0].label, "ISBN");
    assert_eq!(defs[2].label, "In print");
    // R3: no sub-field rows were exported for the repeater.
    assert_eq!(
        report.unmapped_acf,
        [UnmappedAcf {
            field: "quotes".into(),
            kind: "repeater".into(),
            items: 1,
        }]
    );
    assert_eq!(counts(&report, "types").created, 1);
    assert_eq!(counts(&report, "taxonomies").created, 1);

    // Again: everything is matched, nothing is created twice.
    let again = run_until(&site, &corpus("site-a.xml"), "pass1").await;
    assert_eq!(
        counts(&again, "users"),
        Counts {
            unchanged: 4,
            ..Counts::default()
        }
    );
    let terms = counts(&again, "terms");
    assert_eq!((terms.created, terms.unchanged), (0, 9), "{terms:?}");
    assert!(again.users_must_reset.is_empty());
    assert_eq!(counts(&again, "types").created, 0);
    drop(site.work);
}

/// A tiny WXR file with what the corpus lacks: authors with a broken or missing e-mail
/// or a login core refuses, a percent-encoded slug, a term already on the site, a parent
/// cycle and a parent that is not in the file.
const EDGE_WXR: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:excerpt="http://wordpress.org/export/1.2/excerpt/" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:wp="http://wordpress.org/export/1.2/">
<channel>
<title>Edges</title>
<link>http://edge.test</link>
<description></description>
<wp:wxr_version>1.2</wp:wxr_version>
<wp:base_site_url>http://edge.test</wp:base_site_url>
<wp:base_blog_url>http://edge.test</wp:base_blog_url>
<wp:author><wp:author_id>1</wp:author_id><wp:author_login>broken</wp:author_login><wp:author_email>not-an-email</wp:author_email><wp:author_display_name><![CDATA[Broken Mail]]></wp:author_display_name></wp:author>
<wp:author><wp:author_id>3</wp:author_id><wp:author_login>ann-smith</wp:author_login><wp:author_email></wp:author_email><wp:author_display_name><![CDATA[Ann S.]]></wp:author_display_name></wp:author>
<wp:author><wp:author_id>2</wp:author_id><wp:author_login>Ann Smith</wp:author_login><wp:author_email></wp:author_email><wp:author_display_name><![CDATA[Ann]]></wp:author_display_name></wp:author>
<wp:category><wp:term_id>1</wp:term_id><wp:category_nicename>%d0%b0%d0%b1</wp:category_nicename><wp:category_parent/><wp:cat_name><![CDATA[Аб]]></wp:cat_name></wp:category>
<wp:category><wp:term_id>2</wp:term_id><wp:category_nicename>kept</wp:category_nicename><wp:category_parent>%d0%b0%d0%b1</wp:category_parent><wp:cat_name><![CDATA[Kept from WP]]></wp:cat_name></wp:category>
<wp:category><wp:term_id>3</wp:term_id><wp:category_nicename>ring-a</wp:category_nicename><wp:category_parent>ring-b</wp:category_parent><wp:cat_name><![CDATA[Ring A]]></wp:cat_name></wp:category>
<wp:category><wp:term_id>4</wp:term_id><wp:category_nicename>ring-b</wp:category_nicename><wp:category_parent>ring-a</wp:category_parent><wp:cat_name><![CDATA[Ring B]]></wp:cat_name></wp:category>
<wp:category><wp:term_id>5</wp:term_id><wp:category_nicename>orphan</wp:category_nicename><wp:category_parent>gone</wp:category_parent><wp:cat_name><![CDATA[Orphan]]></wp:cat_name></wp:category>
</channel>
</rss>
"#;

/// Authors with a bad e-mail get a placeholder unique per author; percent-encoded slugs are decoded; a
/// site term with the same slug is reused with its own parent; a cycle or a missing
/// parent drops the parent and is reported.
pub async fn pass1_edge_cases(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let store = db.store();
    // A site term `kept` without a parent, which the WXR would put under `аб`.
    let svc = ferrite_core::TaxonomyService::new(db.shared_store());
    let mine = svc
        .create_term(
            &site.ctx.actor,
            "category",
            "Mine",
            Some("kept".into()),
            None,
            "",
        )
        .await
        .unwrap();
    let wxr = site.work.path().join("edge.xml");
    std::fs::write(&wxr, EDGE_WXR).unwrap();
    let report = run_until(&site, &wxr, "pass1").await;

    let user = async |login: &str| {
        store
            .read()
            .users()
            .get_by_login(login)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no user {login}"))
    };
    assert_eq!(user("broken").await.email, "wp1.broken@users.invalid");
    // Two WP authors whose logins normalise alike and who have no e-mail stay two
    // accounts: the placeholder carries the WP author id.
    let first = user("ann-smith").await;
    let second = user("ann-smith-2").await;
    assert_ne!(first.id, second.id);
    assert_eq!(first.email, "wp3.ann-smith@users.invalid");
    assert_eq!(second.email, "wp2.ann-smith@users.invalid");
    assert_eq!(counts(&report, "users").created, 3);
    let mut reset = report.users_must_reset.clone();
    reset.sort_by(|a, b| a.login.cmp(&b.login));
    let placeholder = |login: &str, email: &str| MustReset {
        login: login.into(),
        email: email.into(),
        how: "created with a placeholder e-mail".into(),
    };
    // The logins core stored, `-2` included.
    assert_eq!(
        reset,
        [
            placeholder("ann-smith", "wp3.ann-smith@users.invalid"),
            placeholder("ann-smith-2", "wp2.ann-smith@users.invalid"),
            placeholder("broken", "wp1.broken@users.invalid"),
        ]
    );

    let ab = term(store, "category", "аб").await;
    assert_eq!(ab.name, "Аб");
    let kept = term(store, "category", "kept").await;
    assert_eq!(kept.id, mine.id);
    assert_eq!(kept.name, "Mine");
    assert_eq!(kept.parent_id, None);
    let a = term(store, "category", "ring-a").await;
    let b = term(store, "category", "ring-b").await;
    assert!(
        a.parent_id.is_none() || b.parent_id.is_none(),
        "a cycle must be broken: {a:?} {b:?}"
    );
    assert_eq!(term(store, "category", "orphan").await.parent_id, None);
    let notes = report.notes.join("\n");
    assert!(notes.contains("cycle"), "{notes}");
    assert!(notes.contains("orphan"), "{notes}");
    let terms = counts(&report, "terms");
    assert_eq!((terms.created, terms.unchanged), (4, 1), "{terms:?}");
}

/// Site A, pass 2: four media items with their titles and alt texts, a URL index that
/// knows every size of each, and the files missing from the uploads folder reported.
pub async fn pass2_site_a(db: &TestDb) {
    let mut site = site(db, site_a_uploads()).await;
    let store = db.store();
    let report = run_until(&site, &corpus("site-a.xml"), "pass2").await;
    assert_eq!(
        counts(&report, "media"),
        Counts {
            created: 4,
            ..Counts::default()
        }
    );
    assert!(report.missing_media.is_empty());
    assert_eq!(files_under(&site.media_dir.path().join("media")), 4);
    let page = store
        .read()
        .content()
        .list(&ContentQuery {
            kind: Some(MEDIA_KIND.into()),
            ..ContentQuery::default()
        })
        .await
        .unwrap();
    let mut seen = Vec::new();
    for item in &page.items {
        assert_eq!(item.status, ContentStatus::Published);
        assert_eq!(item.author_id, site.ctx.actor.user);
        let media = store.read().media().get(item.id).await.unwrap().unwrap();
        assert_eq!(media.mime, "image/png");
        seen.push((item.title.clone(), media.alt));
    }
    seen.sort();
    assert_eq!(
        seen,
        [
            ("Cover".to_owned(), "A purple cover".to_owned()),
            ("Hero".to_owned(), "A red hero banner".to_owned()),
            ("Square-one".to_owned(), "Blue square".to_owned()),
            ("Square-two".to_owned(), String::new()),
        ]
    );
    assert_eq!(
        store
            .read()
            .meta()
            .get(page.items[0].id, "import.wp.id")
            .await
            .unwrap()
            .map(|v| v.is_i64()),
        Some(true)
    );

    // Again, with no files at all: every attachment is unchanged and no file is read
    // (a read would find nothing and report it missing).
    site.ctx.media_source = Arc::new(NoFiles);
    let again = run_until(&site, &corpus("site-a.xml"), "pass2").await;
    assert_eq!(
        counts(&again, "media"),
        Counts {
            unchanged: 4,
            ..Counts::default()
        }
    );
}

/// The URL index survives in the IR of a stopped run (P14, P21): every size file and the
/// original resolve to the item, host-insensitively.
pub async fn pass2_media_index_is_kept_in_the_ir(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let opts = ImportOptions {
        step_hook: Some(StepHook::new(|step| {
            if step == "pass3" {
                return Err(ImportError::Stopped);
            }
            Ok(())
        })),
        ..ImportOptions::default()
    };
    let err = run(site.ctx.clone(), &corpus("site-a.xml"), opts)
        .await
        .unwrap_err();
    assert!(matches!(err, ImportError::Stopped), "{err:?}");
    let dir = std::fs::read_dir(&site.ctx.work_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let ir = Ir::open(&dir.join("ir.sqlite")).await.unwrap();
    let index = MediaIndex::load(&ir).await.unwrap();
    ir.close().await;

    let hero = index
        .resolve("http://localhost:8099/wp-content/uploads/2026/10/hero.png")
        .unwrap()
        .clone();
    let media = db
        .store()
        .read()
        .media()
        .get(ContentId::from_uuid(hero.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        hero.url,
        site.ctx
            .media_store
            .public_url(&media.artifact_hash, "hero.png")
    );
    for form in [
        "http://localhost:8099/wp-content/uploads/2026/10/hero-768x432.png",
        "HTTPS://LOCALHOST:8099/wp-content/uploads/2026/10/hero-150x150.png",
        "https://cdn.example.test/wp-content/uploads/2026/10/hero-1536x864.png?ver=2",
        "/wp-content/uploads/2026/10/hero-300x169.png",
        "//localhost:8099/wp-content/uploads/2026/10/hero-1024x576.png#x",
    ] {
        assert_eq!(index.resolve(form), Some(&hero), "{form}");
    }
    let cover = index
        .resolve("http://localhost:8099/wp-content/uploads/2026/10/cover-200x300.png")
        .unwrap();
    assert_ne!(cover.id, hero.id);
    assert_eq!(
        index.resolve("http://localhost:8099/wp-content/uploads/2026/10/hero-1x1.png"),
        None
    );
    assert_eq!(index.len(), 6 + 4 + 4 + 3);
}

/// A source without any file.
struct NoFiles;

#[async_trait]
impl MediaSource for NoFiles {
    async fn open(&self, _: &str, _: &str, _: u64) -> Result<Option<ByteStream>, ImportError> {
        Ok(None)
    }
}

/// A source that misses one file.
struct Without {
    inner: DirSource,
    missing: &'static str,
}

#[async_trait]
impl MediaSource for Without {
    async fn open(
        &self,
        rel_path: &str,
        url: &str,
        max: u64,
    ) -> Result<Option<ByteStream>, ImportError> {
        if rel_path == self.missing {
            return Ok(None);
        }
        self.inner.open(rel_path, url, max).await
    }
}

/// A file neither in the folder nor fetchable is reported and skipped (P13).
pub async fn pass2_missing_file_is_reported(db: &TestDb) {
    let source = Arc::new(Without {
        inner: DirSource::new(corpus("site-a-uploads")),
        missing: "2026/10/square-two.png",
    });
    let site = site(db, source).await;
    let report = run_until(&site, &corpus("site-a.xml"), "pass2").await;
    assert_eq!(
        counts(&report, "media"),
        Counts {
            created: 3,
            skipped: 1,
            ..Counts::default()
        }
    );
    assert_eq!(
        report.missing_media,
        [MissingMedia {
            wp_id: 6,
            path: "2026/10/square-two.png".into(),
            url: "http://localhost:8099/wp-content/uploads/2026/10/square-two.png".into(),
        }]
    );
}

/// A dry run of passes 1 and 2 writes nothing and still gets through both (later
/// batches do not depend on rolled-back rows).
pub async fn pass1_and_pass2_dry_run(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let opts = ImportOptions {
        dry_run: true,
        stop_after: Some("pass2"),
        ..ImportOptions::default()
    };
    let report = run(site.ctx.clone(), &corpus("site-a.xml"), opts)
        .await
        .unwrap();
    assert_eq!(counts(&report, "media").created, 4);
    assert_eq!(counts(&report, "users").created, 3);
    let store = db.store();
    assert!(store.read().types().get("book").await.unwrap().is_none());
    assert!(taxonomy(store, "genre").await.is_none());
    assert!(
        store
            .read()
            .users()
            .get_by_login("cass")
            .await
            .unwrap()
            .is_none()
    );
    let none: Option<TermId> = store
        .read()
        .terms()
        .get_by_slug("category", "local")
        .await
        .unwrap()
        .map(|t| t.id);
    assert_eq!(none, None);
    // The files were staged into a throwaway store, not the site's.
    assert_eq!(files_under(&site.media_dir.path().join("media")), 0);
    assert!(
        std::fs::read_dir(&site.ctx.work_dir).map_or(true, |mut d| d.next().is_none()),
        "the run folder is gone"
    );
}

/// Regular files under `dir` (0 when it does not exist).
fn files_under(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .map(|e| e.unwrap().path())
        .map(|p| if p.is_dir() { files_under(&p) } else { 1 })
        .sum()
}

backend_tests!(
    pass1_site_a,
    pass1_edge_cases,
    pass1_and_pass2_dry_run,
    pass2_site_a,
    pass2_media_index_is_kept_in_the_ir,
    pass2_missing_file_is_reported,
);
