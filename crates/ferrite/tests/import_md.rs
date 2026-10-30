//! `ferrite import markdown` (M4-3b Task 6, P62): the CLI end to end on both backends,
//! re-runs, folder escapes, a dry run and a busy run.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use ferrite::{OpenMode, open_store};
use ferrite_config::Config;
use ferrite_core::{Actor, ContentDraft, ContentService, PermalinkRules};
use ferrite_domain::{
    ContentItem, ContentQuery, ContentStatus, ImportRun, ImportRunId, ImportRunState, MEDIA_KIND,
    NewUser, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::TestDb;
use serde_json::{Value, json};

const SITE_URL: &str = "http://127.0.0.1:8080";
const SITE_KEY: &str = "abababababababababababababababababababababababababababababababab";

/// A site the CLI imports into: a data folder, the environment its commands get, and
/// (Postgres) the test database holding it.
struct Site {
    dir: tempfile::TempDir,
    env: Vec<(String, String)>,
    _db: Option<TestDb>,
}

impl Site {
    async fn sqlite() -> Site {
        let site = Site {
            dir: tempfile::tempdir().unwrap(),
            env: Vec::new(),
            _db: None,
        };
        let mut init = site.cmd();
        init.args(["init", "--url", SITE_URL, "--data"])
            .arg(site.data());
        assert!(run(init).await.status.success());
        let mut user = site.cmd();
        user.args([
            "user",
            "create",
            "--login",
            "importer",
            "--email",
            "a@example.test",
            "--data",
        ])
        .arg(site.data());
        assert!(run(user).await.status.success());
        site
    }

    async fn postgres() -> Option<Site> {
        let db = TestDb::postgres().await?;
        let (url, schema) = db.pg_location().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ferrite.toml"),
            format!(
                "[site]\nurl = \"{SITE_URL}/\"\n[server]\nlisten = \"127.0.0.1:0\"\n\
                 [database]\nurl = \"{url}\"\nschema = \"{schema}\"\n\
                 [media]\nlocal_shared = true\n[cluster]\nmode = \"cluster\"\n"
            ),
        )
        .unwrap();
        admin(db.store()).await;
        Some(Site {
            dir,
            env: vec![("FERRITE_AUTH__SECRET_KEY".into(), SITE_KEY.into())],
            _db: Some(db),
        })
    }

    fn data(&self) -> &Path {
        self.dir.path()
    }

    fn cmd(&self) -> Command {
        let mut cmd = cargo_bin_cmd!("ferrite");
        cmd.env_clear();
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }

    /// `ferrite import markdown <dir> --data <data> <extra…>`.
    async fn import(&self, dir: &Path, extra: &[&str]) -> Output {
        let mut cmd = self.cmd();
        cmd.args(["import", "markdown"])
            .arg(dir)
            .arg("--data")
            .arg(self.data())
            .args(extra);
        run(cmd).await
    }

    fn config(&self) -> Config {
        ferrite_config::load(self.data(), self.env.clone()).unwrap()
    }

    async fn store(&self) -> Arc<dyn Store> {
        open_store(&self.config(), OpenMode::MustExist)
            .await
            .unwrap()
    }
}

async fn run(mut cmd: Command) -> Output {
    tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn admin(store: &dyn Store) -> UserId {
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
    id
}

/// Runs a test on SQLite and on Postgres (skipped without `FERRITE_TEST_PG_URL`).
macro_rules! both {
    ($($name:ident),* $(,)?) => {
        mod sqlite {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    super::$name(super::Site::sqlite().await).await;
                }
            )*
        }
        mod postgres {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    let Some(site) = super::Site::postgres().await else {
                        return;
                    };
                    super::$name(site).await;
                }
            )*
        }
    };
}

both!(
    imports_a_tree,
    a_rerun_with_nothing_changed_writes_nothing,
    an_edited_file_updates,
    an_admin_edit_is_kept,
    a_renamed_file_makes_a_new_page,
    a_new_index_reparents_its_folder,
    a_changed_slug_updates_the_links_to_it,
    duplicate_sibling_slugs_get_their_own_links,
    outside_dir_is_never_read,
    bad_front_matter_skips_the_file_exit_2,
    dry_run_writes_nothing,
    busy_exit_3,
);

/// A 2×2 PNG.
fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(2, 2, image::Rgb([200, 30, 30]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The docs tree of Task 5's fixture, without its bad file: seven pages (one a
/// synthesized section) and one image.
fn tree() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("docs");
    write(
        &dir,
        "index.md",
        b"+++\ntitle = \"Welcome\"\ndescription = \"The docs.\"\n+++\n# FerriteCMS docs\n\n\
          Start with [installing](01-install.md), then [x](02-configure.md#sec).\n",
    );
    write(
        &dir,
        "01-install.md",
        b"# Install\n\nDownload the binary.\n\n![Logo](assets/logo.png)\n",
    );
    write(
        &dir,
        "02-configure.md",
        b"# Configure\n\n## Sec\n\nEdit `ferrite.toml`.\n",
    );
    write(
        &dir,
        "03-reference/api.md",
        b"# API\n\nSee [configure](../02-configure.md#sec).\n",
    );
    write(
        &dir,
        "guides/README.md",
        b"+++\norder = 4\n+++\n# Guides\n\nHow-to guides.\n",
    );
    write(
        &dir,
        "guides/writing.md",
        b"+++\nslug = \"write\"\ndraft = true\n+++\n# Writing\n\nBack to [install](../01-install.md).\n",
    );
    write(&dir, "assets/logo.png", &png());
    (root, dir)
}

/// Every page of the site (any status) by slug.
async fn pages(site: &Site) -> HashMap<String, ContentItem> {
    let store = site.store().await;
    let items = store
        .read()
        .content()
        .list(&ContentQuery {
            kind: Some("page".into()),
            limit: 100,
            ..ContentQuery::default()
        })
        .await
        .unwrap()
        .items;
    store.close().await;
    items.into_iter().map(|i| (i.slug.clone(), i)).collect()
}

async fn media_count(site: &Site) -> u64 {
    let store = site.store().await;
    let n = store
        .read()
        .content()
        .count(&ContentQuery {
            kind: Some(MEDIA_KIND.into()),
            ..ContentQuery::default()
        })
        .await
        .unwrap();
    store.close().await;
    n
}

/// The `href` of every link mark in a body, in order.
fn hrefs(body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if m.get("type") == Some(&json!("link"))
                    && let Some(h) = m.get("attrs").and_then(|a| a.get("href"))
                {
                    out.push(h.as_str().unwrap_or_default().to_owned());
                }
                for v in m.values() {
                    walk(v, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(body, &mut out);
    out
}

/// The `src` of every image node in a body.
fn image_srcs(body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if m.get("type") == Some(&json!("image"))
                    && let Some(s) = m.get("attrs").and_then(|a| a.get("src"))
                {
                    out.push(s.as_str().unwrap_or_default().to_owned());
                }
                m.values().for_each(|v| walk(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(body, &mut out);
    out
}

/// The revision count of every page, by slug.
async fn revisions(site: &Site) -> HashMap<String, (usize, i64)> {
    let pages = pages(site).await;
    let store = site.store().await;
    let mut out = HashMap::new();
    for (slug, item) in pages {
        let n = store
            .read()
            .revisions()
            .list(item.id, 100)
            .await
            .unwrap()
            .len();
        out.insert(slug, (n, item.version));
    }
    store.close().await;
    out
}

async fn imports_a_tree(site: Site) {
    let (_root, dir) = tree();
    let out = site.import(&dir, &["--front-page"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let o = stdout(&out);
    assert!(
        o.contains("created 7, updated 0, unchanged 0, kept (edited here) 0, media 1"),
        "{o}"
    );
    assert!(o.contains("front page: index.md"), "{o}");
    assert!(o.contains("synthesized  03-reference  "), "{o}");

    let p = pages(&site).await;
    let (home, install, configure, reference, api, guides, writing) = (
        &p["home"],
        &p["install"],
        &p["configure"],
        &p["reference"],
        &p["api"],
        &p["guides"],
        &p["write"],
    );
    assert_eq!(p.len(), 7, "{:?}", p.keys());
    assert_eq!(home.title, "Welcome");
    assert_eq!(home.excerpt, "The docs.");
    assert_eq!(install.title, "Install");
    assert_eq!(reference.title, "Reference");
    for (item, parent, order) in [
        (home, None, 0),
        (install, None, 1),
        (configure, None, 2),
        (reference, None, 3),
        (api, Some(reference.id), 0),
        (guides, None, 4),
        (writing, Some(guides.id), 0),
    ] {
        assert_eq!(
            (item.parent_id, item.menu_order),
            (parent, order),
            "{}",
            item.slug
        );
    }
    assert_eq!(writing.status, ContentStatus::Draft);
    assert_eq!(install.status, ContentStatus::Published);

    let store = site.store().await;
    let r = store.read();
    // The front page, the SEO description and the id meta.
    let front = r
        .options()
        .get("reading.front_page")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(front.value, json!(home.id.to_string()));
    assert_eq!(
        r.meta().get(home.id, "seo.description").await.unwrap(),
        Some(json!("The docs."))
    );
    assert_eq!(
        r.meta().get(home.id, "import.md.id").await.unwrap(),
        Some(json!(ferrite_import_md::map_id("index.md")))
    );
    // The image is a media item, and the body points at it.
    let srcs = image_srcs(&install.body);
    assert_eq!(srcs.len(), 1, "{}", install.body);
    let media_id = srcs[0].strip_prefix("ferrite:media/").unwrap();
    let media = r
        .content()
        .get(media_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(media.kind, MEDIA_KIND);
    store.close().await;
    drop(store);
    // Links point at the pages' real paths, keeping the fragment.
    assert_eq!(hrefs(&home.body), ["/install/", "/configure/#sec"]);
    assert_eq!(hrefs(&api.body), ["/configure/#sec"]);
    assert_eq!(hrefs(&writing.body), ["/install/"]);
    // The first H1 is the title and is not in the body; the next heading stays h2.
    assert!(!configure.body.to_string().contains("\"Configure\""));
    assert_eq!(configure.body["content"][0]["attrs"]["level"], 2);
}

async fn a_rerun_with_nothing_changed_writes_nothing(site: Site) {
    let (_root, dir) = tree();
    let out = site.import(&dir, &["--front-page"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let before = revisions(&site).await;
    let out = site.import(&dir, &["--front-page"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("created 0, updated 0, unchanged 7, kept (edited here) 0, media 0"),
        "{}",
        text(&out)
    );
    // No write at all: neither a revision nor a version.
    assert_eq!(revisions(&site).await, before);
    assert_eq!(media_count(&site).await, 1);
}

async fn an_edited_file_updates(site: Site) {
    let (_root, dir) = tree();
    assert_eq!(site.import(&dir, &[]).await.status.code(), Some(0));
    write(
        &dir,
        "02-configure.md",
        b"# Configure\n\n## Sec\n\nEdit the new file.\n",
    );
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("created 0, updated 1, unchanged 6"),
        "{}",
        text(&out)
    );
    let p = pages(&site).await;
    assert!(
        p["configure"]
            .body
            .to_string()
            .contains("Edit the new file."),
        "{}",
        p["configure"].body
    );
}

async fn an_admin_edit_is_kept(site: Site) {
    let (_root, dir) = tree();
    assert_eq!(site.import(&dir, &[]).await.status.code(), Some(0));
    let install = pages(&site).await.remove("install").unwrap();
    let store = site.store().await;
    let user = store
        .read()
        .users()
        .get_by_login("importer")
        .await
        .unwrap()
        .unwrap();
    let body = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Edited here."}]}
    ]});
    ContentService::new(store.clone(), Arc::new(PermalinkRules::default()))
        .update(
            &Actor::new(user.id, Role::Admin),
            install.id,
            install.version,
            ContentDraft {
                kind: install.kind.clone(),
                title: install.title.clone(),
                slug: None,
                parent_id: install.parent_id,
                excerpt: None,
                body: body.clone(),
                fields: install.fields.clone(),
                menu_order: install.menu_order,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    store.close().await;
    drop(store);
    // Unchanged file: the edit stays and the link pass leaves it alone.
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(pages(&site).await["install"].body, body);
    // Changed file: kept as edited here.
    write(&dir, "01-install.md", b"# Install\n\nA new text.\n");
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("updated 0, unchanged 6, kept (edited here) 1"),
        "{}",
        text(&out)
    );
    assert_eq!(pages(&site).await["install"].body, body);
}

async fn a_renamed_file_makes_a_new_page(site: Site) {
    let (_root, dir) = tree();
    assert_eq!(site.import(&dir, &[]).await.status.code(), Some(0));
    std::fs::rename(
        dir.join("03-reference/api.md"),
        dir.join("03-reference/rest.md"),
    )
    .unwrap();
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let o = stdout(&out);
    assert!(o.contains("created 1, updated 0, unchanged 6"), "{o}");
    assert!(o.contains("not-in-source  /reference/api/  "), "{o}");
    let p = pages(&site).await;
    assert_eq!(p["rest"].parent_id, Some(p["reference"].id));
    // The old page stays.
    assert_eq!(p["api"].parent_id, Some(p["reference"].id));
}

/// Ruling R-b6: a new `index.md` in a folder that had a synthesized page becomes the
/// parent of the folder's files, which are updated to move under it.
async fn a_new_index_reparents_its_folder(site: Site) {
    let (_root, dir) = tree();
    assert_eq!(site.import(&dir, &[]).await.status.code(), Some(0));
    write(
        &dir,
        "03-reference/index.md",
        b"# Reference\n\nAll of it.\n",
    );
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let o = stdout(&out);
    assert!(o.contains("created 1, updated 1, unchanged 5"), "{o}");
    // The synthesized page stays, so the new one gets the next free slug.
    assert!(o.contains("not-in-source  /reference/  "), "{o}");
    assert!(
        o.contains(
            "slug-changed  03-reference/index.md  its slug was taken, the page got slug reference-2"
        ),
        "{o}"
    );
    let p = pages(&site).await;
    let index = &p["reference-2"];
    assert_eq!(index.title, "Reference");
    assert_eq!(p["api"].parent_id, Some(index.id));
    assert_eq!(p["api"].title, "API");
}

/// Ruling R-b6: a page whose target changed its slug gets its links rewritten, though its
/// own file did not change.
async fn a_changed_slug_updates_the_links_to_it(site: Site) {
    let (_root, dir) = tree();
    assert_eq!(site.import(&dir, &[]).await.status.code(), Some(0));
    write(
        &dir,
        "02-configure.md",
        b"+++\nslug = \"settings\"\n+++\n# Configure\n\n## Sec\n\nEdit `ferrite.toml`.\n",
    );
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("created 0, updated 1, unchanged 6"),
        "{}",
        text(&out)
    );
    let p = pages(&site).await;
    assert_eq!(hrefs(&p["home"].body), ["/install/", "/settings/#sec"]);
    assert_eq!(hrefs(&p["api"].body), ["/settings/#sec"]);
    // A fix-up write is the import's own: the next run sees nothing edited here.
    let before = revisions(&site).await;
    let out = site.import(&dir, &[]).await;
    assert!(
        stdout(&out).contains("created 0, updated 0, unchanged 7, kept (edited here) 0"),
        "{}",
        text(&out)
    );
    assert_eq!(revisions(&site).await, before);
}

/// Siblings whose names give one slug: the later one gets core's `-N`, reported, and
/// links to each still reach the right page.
async fn duplicate_sibling_slugs_get_their_own_links(site: Site) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("dups");
    write(
        &dir,
        "index.md",
        b"# Home\n\n[a](01-install.md) [b](install.md) [c](guides.md) [d](guides/) [e](guides)\n",
    );
    write(&dir, "01-install.md", b"# Numbered install\n");
    write(&dir, "install.md", b"# Plain install\n");
    write(&dir, "guides.md", b"# Guides file\n");
    write(&dir, "guides/index.md", b"# Guides folder\n");
    write(&dir, "guides/one.md", b"# One\n");
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let o = stdout(&out);
    assert!(o.contains("slug-changed  install.md  "), "{o}");
    assert!(o.contains("slug-changed  guides.md  "), "{o}");
    let p = pages(&site).await;
    assert_eq!(p["install"].title, "Numbered install");
    assert_eq!(p["install-2"].title, "Plain install");
    assert_eq!(p["guides"].title, "Guides folder");
    assert_eq!(p["guides-2"].title, "Guides file");
    assert_eq!(
        hrefs(&p["home"].body),
        [
            "/install/",
            "/install-2/",
            "/guides-2/",
            "/guides/",
            "/guides/"
        ]
    );
    // The same on a re-run.
    let out = site.import(&dir, &[]).await;
    assert!(stdout(&out).contains("unchanged 6"), "{}", text(&out));
    let p = pages(&site).await;
    assert_eq!(p["install-2"].title, "Plain install");
    assert_eq!(
        hrefs(&p["home"].body),
        [
            "/install/",
            "/install-2/",
            "/guides-2/",
            "/guides/",
            "/guides/"
        ]
    );
}

/// Review Focus 4: an image above the folder, a symlinked file and an absolute image
/// path are reported and never read; the other pages import; exit 2.
async fn outside_dir_is_never_read(site: Site) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("site");
    write(root.path(), "secret.png", &png());
    write(root.path(), "outside.md", b"# Outside\n\nSecret text.\n");
    let secret = root.path().join("secret.png").canonicalize().unwrap();
    write(&dir, "ok.md", b"# Ok\n");
    write(
        &dir,
        "guides/a.md",
        format!(
            "# A\n\n![s](../../secret.png)\n\n![abs]({})\n",
            secret.display()
        )
        .as_bytes(),
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path().join("outside.md"), dir.join("link.md")).unwrap();
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let o = stdout(&out);
    assert!(o.contains("created 3, updated 0"), "{o}");
    assert!(o.contains("outside  guides/a.md  ../../secret.png "), "{o}");
    assert!(
        o.contains(&format!("outside  guides/a.md  {} ", secret.display())),
        "{o}"
    );
    #[cfg(unix)]
    assert!(o.contains("outside  link.md  "), "{o}");
    assert_eq!(media_count(&site).await, 0);
    let p = pages(&site).await;
    let slugs: Vec<&str> = {
        let mut s: Vec<&str> = p.keys().map(String::as_str).collect();
        s.sort_unstable();
        s
    };
    assert_eq!(slugs, ["a", "guides", "ok"]);
    assert!(
        !p.values()
            .any(|i| i.body.to_string().contains("Secret text"))
    );
}

async fn bad_front_matter_skips_the_file_exit_2(site: Site) {
    let (_root, dir) = tree();
    write(&dir, "bad.md", b"---\ntitle: Bad\n---\n# Bad\n");
    write(&dir, "worse.md", b"+++\nweight = 1\n+++\n# Worse\n");
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let o = stdout(&out);
    assert!(o.contains("created 7, "), "{o}");
    assert!(
        o.contains("bad-front-matter  bad.md  line 1: skipped, "),
        "{o}"
    );
    assert!(
        o.contains("bad-front-matter  worse.md  line 2: skipped, "),
        "{o}"
    );
    let p = pages(&site).await;
    assert!(!p.contains_key("bad") && !p.contains_key("worse"));
}

/// What a dry run must leave as it was: content, options and the media files.
async fn snapshot(site: &Site) -> String {
    let store = site.store().await;
    let r = store.read();
    let content = r.content().count(&ContentQuery::default()).await.unwrap();
    let options: Vec<(String, Value)> = r
        .options()
        .list_prefix("", "", 200)
        .await
        .unwrap()
        .into_iter()
        .map(|o| (o.key, o.value))
        .collect();
    store.close().await;
    let files = count_files(&site.data().join("media"));
    format!("content {content} files {files}\noptions {options:?}")
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .map(|e| {
            let path = e.unwrap().path();
            if path.is_dir() { count_files(&path) } else { 1 }
        })
        .sum()
}

async fn dry_run_writes_nothing(site: Site) {
    let (_root, dir) = tree();
    let before = snapshot(&site).await;
    let out = site.import(&dir, &["--dry-run", "--front-page"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let o = stdout(&out);
    assert!(o.contains("dry run: nothing was written"), "{o}");
    assert!(o.contains("created 7, updated 0, unchanged 0"), "{o}");
    assert!(o.contains("media 1"), "{o}");
    assert!(o.contains("front page: index.md"), "{o}");
    assert!(!o.contains("variants"), "{o}");
    assert_eq!(snapshot(&site).await, before);
    // The staging folder is gone.
    let left: Vec<String> = std::fs::read_dir(site.data().join("import"))
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    // A real run afterwards creates everything.
    let out = site.import(&dir, &[]).await;
    assert!(stdout(&out).contains("created 7, "), "{}", text(&out));
}

/// Another import running with a fresh heartbeat: exit 3, nothing written.
async fn busy_exit_3(site: Site) {
    let store = site.store().await;
    let user = store
        .read()
        .users()
        .get_by_login("importer")
        .await
        .unwrap()
        .unwrap();
    let now = Timestamp::now();
    store
        .read()
        .imports()
        .create_run(&ImportRun {
            id: ImportRunId::new(),
            source: "http://elsewhere.test".into(),
            token: [7; 16],
            state: ImportRunState::Running,
            started_by: user.id,
            started_at: now,
            heartbeat_at: now,
            finished_at: None,
            stats: json!({}),
            dry_run: false,
            job_id: None,
            step: None,
            report_hash: None,
        })
        .await
        .unwrap();
    store.close().await;
    drop(store);
    let (_root, dir) = tree();
    let out = site.import(&dir, &[]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(pages(&site).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_codes_documented() {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.args(["import", "markdown", "--help"]);
    let out = run(cmd).await;
    assert!(out.status.success(), "{}", text(&out));
    let o = stdout(&out);
    for code in [
        "0 done",
        "2 done with skipped files",
        "3 when another import is running",
        "1 on any error",
    ] {
        assert!(o.contains(code), "{code}: {o}");
    }
    for flag in [
        "--data",
        "--as",
        "--type",
        "--front-page",
        "--dry-run",
        "--source",
    ] {
        assert!(o.contains(flag), "{flag}: {o}");
    }
}

/// `--front-page` with another type is refused before anything runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn front_page_needs_type_page() {
    let site = Site::sqlite().await;
    let (_root, dir) = tree();
    let out = site.import(&dir, &["--front-page", "--type", "post"]).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--front-page needs --type page"),
        "{}",
        text(&out)
    );
}
