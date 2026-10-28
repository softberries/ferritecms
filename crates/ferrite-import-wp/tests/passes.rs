//! Passes 1 and 2 of the import (M3-1 Task 13): authors, taxonomies, terms and custom
//! types with their ACF fields, then attachments and the media URL index, on both
//! backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ferrite_core::content::{Robots, Seo};
use ferrite_core::fields::{FieldDef, FieldKind, schema_of};
use ferrite_core::jobs::PUBLISH_SCHEDULED_KIND;
use ferrite_core::{
    Actor, ContentDraft, ContentService, PermalinkRules, SwappablePermalinks, import::ImportService,
};
use ferrite_domain::{
    Comment, CommentQuery, CommentStatus, ContentId, ContentItem, ContentQuery, ContentStatus,
    JobQuery, MEDIA_KIND, MenuItemKind, NewUser, Role, Taxonomy, Term, TermId, Timestamp, UserId,
    UserStatus,
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
use serde_json::{Value, json};
use uuid::Uuid;

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

// --- Pass 3: content -------------------------------------------------------------------

/// Runs every step of `wxr`.
async fn run_all(site: &Site, wxr: &Path) -> Report {
    run(site.ctx.clone(), wxr, ImportOptions::default())
        .await
        .unwrap()
}

/// The item of `kind` titled `title` (any status).
async fn by_title(store: &dyn Store, kind: &str, title: &str) -> ContentItem {
    find_title(store, kind, title)
        .await
        .unwrap_or_else(|| panic!("no {kind} titled {title:?}"))
}

async fn find_title(store: &dyn Store, kind: &str, title: &str) -> Option<ContentItem> {
    store
        .read()
        .content()
        .list(&ContentQuery {
            kind: Some(kind.into()),
            limit: 200,
            ..ContentQuery::default()
        })
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|i| i.title == title)
}

async fn meta_of(store: &dyn Store, id: ContentId, key: &str) -> Option<Value> {
    store.read().meta().get(id, key).await.unwrap()
}

fn at(rfc3339: &str) -> Timestamp {
    serde_json::from_value(json!(rfc3339)).unwrap()
}

/// Every link `href` in a block tree.
fn hrefs(node: &Value, out: &mut Vec<String>) {
    if let Some(marks) = node.get("marks").and_then(Value::as_array) {
        for m in marks {
            if let Some(h) = m.pointer("/attrs/href").and_then(Value::as_str) {
                out.push(h.to_owned());
            }
        }
    }
    if let Some(children) = node.get("content").and_then(Value::as_array) {
        for c in children {
            hrefs(c, out);
        }
    }
}

/// Site A, pass 3 and the fix-up: statuses and dates (P7), authors, terms, the featured
/// image, SEO (P10), meta, ACF values (P16) and links to items written later (P14).
pub async fn pass3_site_a(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let store = db.store();
    let report = run_all(&site, &corpus("site-a.xml")).await;
    let user = |login: &'static str| async move {
        store
            .read()
            .users()
            .get_by_login(login)
            .await
            .unwrap()
            .unwrap()
            .id
    };
    let media = |title: &'static str| async move { by_title(store, MEDIA_KIND, title).await.id };

    let post = by_title(store, "post", "Every core block").await;
    assert_eq!(post.status, ContentStatus::Published);
    assert_eq!(post.slug, "every-core-block");
    assert_eq!(post.published_at, Some(at("2024-03-15T10:30:00Z")));
    assert_eq!(post.created_at, at("2024-03-15T10:30:00Z"));
    assert_eq!(post.author_id, user("arlo").await);
    let terms = store.read().terms().for_item(post.id).await.unwrap();
    let mut slugs: Vec<&str> = terms.iter().map(|t| t.slug.as_str()).collect();
    slugs.sort_unstable();
    assert_eq!(slugs, ["local", "news", "rust", "wasm"]);
    assert_eq!(
        post.fields["featured_image"],
        json!(media("Hero").await.to_string())
    );
    let seo = Seo::load(store.read(), post.id).await.unwrap();
    assert_eq!(
        seo,
        Seo {
            title: Some("Every core block - Corpus Site".into()),
            description: Some("Yoast description for the block post.".into()),
            canonical: Some("https://canonical.example.org/blocks/".into()),
            robots: vec![
                Robots::NoArchive,
                Robots::NoFollow,
                Robots::NoIndex,
                Robots::NoSnippet
            ],
        }
    );
    assert_eq!(
        meta_of(store, post.id, "wp.some_unrelated_meta").await,
        Some(json!("kept as wp.*"))
    );
    for dropped in ["wp._pingme", "wp._encloseme", "wp._thumbnail_id"] {
        assert_eq!(meta_of(store, post.id, dropped).await, None, "{dropped}");
    }
    assert_eq!(
        meta_of(store, post.id, "import.wp.id").await,
        Some(json!(7))
    );
    // Links to the about and team pages, written after the post, point at their new
    // paths (the fix-up).
    let about = by_title(store, "page", "About").await;
    let team = by_title(store, "page", "Team").await;
    assert_eq!(team.parent_id, Some(about.id));
    assert_eq!(team.menu_order, 2);
    let mut links = Vec::new();
    hrefs(&post.body, &mut links);
    assert!(links.contains(&"/about/".to_owned()), "{links:?}");
    assert!(links.contains(&"/about/team/".to_owned()), "{links:?}");
    assert!(
        !links
            .iter()
            .any(|l| l.contains("localhost:8099/?page_id") || l == "http://localhost:8099/about/"),
        "{links:?}"
    );
    assert!(
        links.contains(&"https://example.org/".to_owned()),
        "{links:?}"
    );

    let future = by_title(store, "post", "From the future").await;
    assert_eq!(future.status, ContentStatus::Scheduled);
    assert_eq!(future.scheduled_at, Some(at("2030-06-01T12:00:00Z")));
    let jobs = store
        .read()
        .jobs()
        .list(&JobQuery {
            status: None,
            kind: Some(PUBLISH_SCHEDULED_KIND.into()),
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap()
        .items;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].run_at, at("2030-06-01T12:00:00Z"));
    assert_eq!(
        by_title(store, "post", "Private thoughts").await.status,
        ContentStatus::Private
    );
    let gone = by_title(store, "post", "Gone soon").await;
    assert_eq!(gone.status, ContentStatus::Trash);
    assert_eq!(gone.slug, "gone-soon");
    let draft = by_title(store, "post", "A draft").await;
    assert_eq!(draft.status, ContentStatus::Draft);
    assert_eq!(draft.author_id, user("cass").await);
    assert_eq!(
        by_title(store, "post", "Pending review").await.status,
        ContentStatus::Pending
    );
    let sticky = by_title(store, "post", "Sticky note").await;
    assert_eq!(
        meta_of(store, sticky.id, "wp.sticky").await,
        Some(json!(true))
    );
    let sample = by_title(store, "page", "Sample Page").await;
    assert_eq!(
        meta_of(store, sample.id, "wp.page_template").await,
        Some(json!("default"))
    );

    // The book's ACF values; the relation to the second book is fixed up.
    let first = by_title(store, "book", "The First Book").await;
    let second = by_title(store, "book", "The Second Book").await;
    let fields = &first.fields;
    assert_eq!(fields["isbn"], json!("978-3-16-148410-0"));
    assert_eq!(fields["pages"], json!(412));
    assert_eq!(fields["in_print"], json!(true));
    assert_eq!(fields["released"], json!("2022-05-05T00:00:00Z"));
    assert_eq!(fields["blurb"], json!("Line one\nLine two"));
    assert_eq!(fields["cover"], json!(media("Cover").await.to_string()));
    assert_eq!(fields["format"], json!("hardback"));
    assert_eq!(fields["site"], json!("https://books.example.org/first"));
    assert_eq!(fields["review"]["type"], json!("doc"));
    assert_eq!(fields["related"], json!([{"item": second.id.to_string()}]));
    assert_eq!(
        meta_of(store, first.id, "wp.quotes").await,
        Some(json!(
            r#"a:2:{i:0;a:1:{s:4:"text";s:9:"Quote one";}i:1;a:1:{s:4:"text";s:9:"Quote two";}}"#
        ))
    );
    for acf in ["wp.isbn", "wp._isbn", "wp.related", "wp._quotes"] {
        assert_eq!(meta_of(store, first.id, acf).await, None, "{acf}");
    }
    let terms = store.read().terms().for_item(first.id).await.unwrap();
    assert_eq!(
        terms.iter().map(|t| t.slug.as_str()).collect::<Vec<_>>(),
        ["fantasy"]
    );

    assert_eq!(counts(&report, "post").created, 8);
    assert_eq!(counts(&report, "page").created, 4);
    assert_eq!(counts(&report, "book").created, 2);

    // Again: nothing changes.
    let again = run_all(&site, &corpus("site-a.xml")).await;
    for kind in ["post", "page", "book"] {
        let c = counts(&again, kind);
        assert_eq!((c.created, c.updated, c.kept), (0, 0, 0), "{kind}: {c:?}");
    }
    assert_eq!(counts(&again, "post").unchanged, 8);
    let post_again = by_title(store, "post", "Every core block").await;
    assert_eq!(post_again.version, post.version);
}

/// Site B: Rank Math SEO (P10) and the primary category first, as `%category%` needs.
pub async fn pass3_site_b(db: &TestDb) {
    let site = site(db, Arc::new(DirSource::new(corpus("site-b-uploads")))).await;
    let store = db.store();
    run_all(&site, &corpus("site-b.xml")).await;
    let harbour = by_title(store, "post", "Harbour walk").await;
    let seo = Seo::load(store.read(), harbour.id).await.unwrap();
    assert_eq!(seo.title.as_deref(), Some("Harbour walk - Classic Corpus"));
    assert_eq!(
        seo.description.as_deref(),
        Some("Rank Math description for the harbour walk.")
    );
    assert_eq!(
        seo.canonical.as_deref(),
        Some("https://canonical.example.org/harbour/")
    );
    assert_eq!(seo.robots, [Robots::NoFollow, Robots::NoIndex]);
    let terms = store.read().terms().for_item(harbour.id).await.unwrap();
    let first_category = terms.iter().find(|t| t.taxonomy == "category").unwrap();
    assert_eq!(first_category.slug, "travel");
    assert!(
        terms
            .iter()
            .any(|t| t.taxonomy == "tag" && t.slug == "europe")
    );
    assert_eq!(
        harbour.author_id,
        store
            .read()
            .users()
            .get_by_login("wren")
            .await
            .unwrap()
            .unwrap()
            .id
    );
    let cafe = by_title(store, "post", "Café review — “quotes” & ampersands").await;
    let seo = Seo::load(store.read(), cafe.id).await.unwrap();
    assert!(seo.robots.is_empty(), "{seo:?}");
    assert_eq!(
        meta_of(store, harbour.id, "wp.rank_math_focus_keyword").await,
        Some(json!("harbour,walk"))
    );
    assert_eq!(meta_of(store, harbour.id, "wp.rank_math_title").await, None);
}

/// The run's IR file (one run folder under the work dir).
fn ir_file(site: &Site) -> PathBuf {
    let mut dirs = std::fs::read_dir(&site.ctx.work_dir).unwrap();
    let dir = dirs.next().unwrap().unwrap().path();
    assert!(dirs.next().is_none(), "one run folder");
    dir.join("ir.sqlite")
}

/// The WP ids in IR table `table` (`content_paths` or `fixups`).
async fn ir_ids(file: &Path, table: &str) -> Vec<i64> {
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(file))
            .await
            .unwrap();
    let sql = match table {
        "content_paths" => "SELECT wp_id FROM content_paths ORDER BY wp_id",
        "fixups" => "SELECT wp_id FROM fixups ORDER BY wp_id",
        other => panic!("not an IR id table: {other}"),
    };
    let ids = sqlx::query_scalar(sql).fetch_all(&pool).await.unwrap();
    pool.close().await;
    ids
}

/// Runs `f` to completion from a sync step hook (multi-threaded runtime only).
fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

/// A run that stops right after the batch with "Every core block" committed (its links
/// name pages written later) still has the item in the fix-up queue, and the resumed
/// run fixes its links.
pub async fn pass3_fixup_queue_survives_a_crash(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let store = db.store();
    // One item per batch: 4 users, 9 term batches, 4 × 2 media batches, the permalink
    // structure, 4 attachment lookups, "Hello world!" (27), then "Every core block"
    // (28).
    let opts = ImportOptions {
        batch_items: Some(1),
        fail_after_batches: Some(28),
        ..ImportOptions::default()
    };
    let err = run(site.ctx.clone(), &corpus("site-a.xml"), opts)
        .await
        .unwrap_err();
    assert!(matches!(err, ImportError::Stopped), "{err:?}");
    let post = by_title(store, "post", "Every core block").await;
    let mut links = Vec::new();
    hrefs(&post.body, &mut links);
    assert!(
        links.contains(&"http://localhost:8099/about/".to_owned()),
        "not fixed yet: {links:?}"
    );
    assert_eq!(ir_ids(&ir_file(&site), "fixups").await, [7]);
    // The run stopped right after that batch: nothing after it was written, so the test
    // cannot pass on a wrong batch count.
    for (kind, title) in [
        ("post", "A draft"),
        ("page", "Sample Page"),
        ("page", "About"),
    ] {
        assert!(
            find_title(store, kind, title).await.is_none(),
            "{title} was written before the crash"
        );
    }

    run_all(&site, &corpus("site-a.xml")).await;
    let post = by_title(store, "post", "Every core block").await;
    let mut links = Vec::new();
    hrefs(&post.body, &mut links);
    assert!(links.contains(&"/about/".to_owned()), "{links:?}");
    assert!(links.contains(&"/about/team/".to_owned()), "{links:?}");
}

/// A dry run through pass 3 and the fix-up writes no content, no jobs and nothing to
/// the IR's path and fix-up tables (seen at the fix-up's start, before the folder goes).
pub async fn pass3_dry_run_writes_nothing(db: Arc<TestDb>) {
    let site = site(&db, site_a_uploads()).await;
    let store = db.store();
    let seen = Arc::new(std::sync::Mutex::new(None));
    let hook = {
        let seen = seen.clone();
        let work = site.ctx.work_dir.clone();
        StepHook::new(move |step| {
            if step == "fixup" {
                let dir = std::fs::read_dir(&work).unwrap().next().unwrap().unwrap();
                let file = dir.path().join("ir.sqlite");
                let rows = block_on(async {
                    (
                        ir_ids(&file, "content_paths").await,
                        ir_ids(&file, "fixups").await,
                    )
                });
                *seen.lock().unwrap() = Some(rows);
            }
            Ok(())
        })
    };
    let opts = ImportOptions {
        dry_run: true,
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    let report = run(site.ctx.clone(), &corpus("site-a.xml"), opts)
        .await
        .unwrap();
    assert_eq!(counts(&report, "post").created, 8, "counted as if written");
    assert!(report.notes.iter().any(|n| n.starts_with("dry run:")));
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.contains("was not imported; imported without"))
    );
    assert_eq!(
        *seen.lock().unwrap(),
        Some((Vec::new(), Vec::new())),
        "no paths and no fix-ups in the IR"
    );
    let rows = store
        .read()
        .content()
        .count(&ContentQuery::default())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let jobs = store
        .read()
        .jobs()
        .list(&JobQuery {
            status: None,
            kind: None,
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap()
        .items;
    assert!(jobs.is_empty(), "{jobs:?}");
}

/// An item edited here between pass 3 and the fix-up is kept as edited, with a note.
pub async fn fixup_keeps_an_item_edited_meanwhile(db: Arc<TestDb>) {
    let site = site(&db, site_a_uploads()).await;
    let store = db.shared_store();
    let hook = {
        let store = store.clone();
        let actor = site.ctx.actor;
        let permalinks = site.ctx.permalinks.clone();
        StepHook::new(move |step| {
            if step == "fixup" {
                block_on(async {
                    let post = by_title(store.as_ref(), "post", "Every core block").await;
                    ContentService::new(store.clone(), permalinks.clone())
                        .update(
                            &actor,
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
                });
            }
            Ok(())
        })
    };
    let opts = ImportOptions {
        step_hook: Some(hook),
        ..ImportOptions::default()
    };
    let report = run(site.ctx.clone(), &corpus("site-a.xml"), opts)
        .await
        .unwrap();
    let fixups = counts(&report, "fixups");
    assert_eq!(fixups.kept, 1, "{fixups:?}");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n == "item 7 was edited here before its links were fixed; kept as edited"),
        "{:?}",
        report.notes
    );
    let post = by_title(db.store(), "post", "Edited here").await;
    let mut links = Vec::new();
    hrefs(&post.body, &mut links);
    assert!(
        links.contains(&"http://localhost:8099/about/".to_owned()),
        "{links:?}"
    );
    // The book's relation was still fixed.
    assert!(fixups.updated >= 1, "{fixups:?}");
}

// --- Pass 4: comments, menus, permalinks, redirects, options ----------------------------

async fn comments_of(store: &dyn Store, item: ContentId) -> Vec<Comment> {
    let mut list = store
        .read()
        .comments()
        .list(&CommentQuery {
            status: None,
            item: Some(item),
            limit: 100,
            cursor: None,
        })
        .await
        .unwrap()
        .items;
    list.sort_by_key(|c| c.created_at);
    list
}

async fn redirect_to(store: &dyn Store, source: &str) -> Option<String> {
    store
        .read()
        .redirects()
        .get_by_source(source)
        .await
        .unwrap()
        .map(|r| r.target)
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

/// A menu item as asserted: label, kind, target, URL, whether it has a parent.
type MenuShape<'a> = (&'a str, MenuItemKind, Option<Uuid>, Option<&'a str>, bool);

/// Site A after pass 4: the inferred permalinks (P17), comments (P18, R4), the menu
/// (P19), redirects (P17) and the site options (P20).
pub async fn pass4_site_a(db: &TestDb) {
    let site = site(db, site_a_uploads()).await;
    let store = db.store();
    let report = run_all(&site, &corpus("site-a.xml")).await;

    assert_eq!(
        report.permalink_structure.as_deref(),
        Some("/%year%/%month%/%slug%/")
    );
    assert_eq!(
        option(store, "permalink.post").await,
        Some(json!("/%year%/%month%/%slug%/"))
    );

    // R4: WordPress does not export spam; the post has 3 comments.
    let post = by_title(store, "post", "Every core block").await;
    let comments = comments_of(store, post.id).await;
    assert_eq!(comments.len(), 3, "{comments:?}");
    let visitor = &comments[0];
    assert_eq!(visitor.author_name, "Visitor One");
    assert_eq!(visitor.status, CommentStatus::Approved);
    assert_eq!(visitor.parent_id, None);
    assert_eq!(visitor.created_at, at("2024-03-16T11:00:00Z"));
    let reply = &comments[1];
    assert_eq!(reply.body, "Thanks!");
    assert_eq!(reply.status, CommentStatus::Approved);
    assert_eq!(reply.parent_id, Some(visitor.id));
    let arlo = store
        .read()
        .users()
        .get_by_login("arlo")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.user_id, Some(arlo.id));
    assert_eq!(comments[2].status, CommentStatus::Pending);
    assert!(comments.iter().all(|c| c.status != CommentStatus::Spam));
    let hello = by_title(store, "post", "Hello world!").await;
    assert_eq!(
        comments_of(store, hello.id).await[0].body,
        "Hi, this is a comment.\n\nTo get started with moderating, editing, and deleting comments, \
         please visit the Comments screen in the dashboard.\n\nCommenter avatars come from Gravatar."
    );
    assert_eq!(counts(&report, "comments").created, 4);

    // The menu: Home (url) / About (item) > Team (item) / All news (term) / Every core
    // block (item).
    let menu = store.read().menus().get("main").await.unwrap().unwrap();
    assert_eq!(menu.label, "Main");
    let about = by_title(store, "page", "About").await;
    let team = by_title(store, "page", "Team").await;
    let news = term(store, "category", "news").await;
    let shape: Vec<MenuShape<'_>> = menu
        .items
        .iter()
        .map(|i| {
            (
                i.label.as_str(),
                i.kind,
                i.target_id,
                i.url.as_deref(),
                i.parent_id.is_some(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("Home", MenuItemKind::Url, None, Some("/"), false),
            (
                "About",
                MenuItemKind::Item,
                Some(about.id.as_uuid()),
                None,
                false
            ),
            (
                "Team",
                MenuItemKind::Item,
                Some(team.id.as_uuid()),
                None,
                true
            ),
            (
                "All news",
                MenuItemKind::Term,
                Some(news.id.as_uuid()),
                None,
                false
            ),
            (
                "Every core block",
                MenuItemKind::Item,
                Some(post.id.as_uuid()),
                None,
                false
            ),
        ]
    );
    assert_eq!(menu.items[2].parent_id, Some(menu.items[1].id));
    let tops: Vec<i32> = menu
        .items
        .iter()
        .filter(|i| i.parent_id.is_none())
        .map(|i| i.position)
        .collect();
    assert_eq!(tops, [0, 1, 2, 3]);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("opened in a new tab")),
        "_blank is dropped and counted: {:?}",
        report.notes
    );

    // Redirects: moved items, attachment pages, nested category paths; none where the
    // path did not change.
    assert_eq!(
        redirect_to(store, "/books/the-first-book/")
            .await
            .as_deref(),
        Some("/book/the-first-book/")
    );
    let media_store = &site.ctx.media_store;
    let media_url = |title: &'static str| async move {
        let id = by_title(store, MEDIA_KIND, title).await.id;
        let m = store.read().media().get(id).await.unwrap().unwrap();
        media_store.public_url(&m.artifact_hash, &m.original_name)
    };
    assert_eq!(
        redirect_to(store, "/hero/").await,
        Some(media_url("Hero").await)
    );
    assert_eq!(
        redirect_to(store, "/books/the-first-book/cover/").await,
        Some(media_url("Cover").await)
    );
    assert_eq!(
        redirect_to(store, "/category/news/local/").await.as_deref(),
        Some("/category/local/")
    );
    for same in [
        "/tag/rust/",
        "/category/news/",
        "/2024/03/every-core-block/",
        "/about/",
    ] {
        assert_eq!(redirect_to(store, same).await, None, "{same}");
    }

    assert_eq!(
        option(store, "site.title").await,
        Some(json!("Corpus Site"))
    );
    assert!(option(store, "site.description").await.is_some());
}

/// Site B: `%category%` permalinks, a pingback skipped and counted, comment HTML as plain
/// text, and a custom tag base redirected to core's.
pub async fn pass4_site_b(db: &TestDb) {
    let site = site(db, Arc::new(DirSource::new(corpus("site-b-uploads")))).await;
    let store = db.store();
    let opts = ImportOptions {
        tag_base: "topic".into(),
        ..ImportOptions::default()
    };
    let report = run(site.ctx.clone(), &corpus("site-b.xml"), opts)
        .await
        .unwrap();
    assert_eq!(
        report.permalink_structure.as_deref(),
        Some("/%category%/%slug%/")
    );
    let harbour = by_title(store, "post", "Harbour walk").await;
    let comments = comments_of(store, harbour.id).await;
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].body, "Lovely photos!\n\nSecond paragraph bold.");
    let c = counts(&report, "comments");
    assert_eq!((c.created, c.skipped), (1, 1), "{c:?}");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n == "1 pingbacks and trackbacks were skipped"),
        "{:?}",
        report.notes
    );
    assert_eq!(
        redirect_to(store, "/topic/europe/").await.as_deref(),
        Some("/tag/europe/")
    );
    // The post keeps its old URL: no redirect.
    assert_eq!(redirect_to(store, "/travel/harbour-walk/").await, None);
    assert_eq!(
        option(store, "site.title").await,
        Some(json!("Classic Corpus"))
    );
}

backend_tests!(
    pass1_site_a,
    pass1_edge_cases,
    pass1_and_pass2_dry_run,
    pass2_site_a,
    pass2_media_index_is_kept_in_the_ir,
    pass2_missing_file_is_reported,
    pass3_site_a,
    pass3_site_b,
    pass3_fixup_queue_survives_a_crash,
    pass4_site_a,
    pass4_site_b,
);

ferrite_store_tests::backend_tests_spawning!(
    pass3_dry_run_writes_nothing,
    fixup_keeps_an_item_edited_meanwhile,
);
