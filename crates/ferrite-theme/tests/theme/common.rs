use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use ferrite_core::{
    DepKeys, Listing, MediaPublic, MediaVariant, PageItem, PublicItem, TermPublic, UserPublic,
    comments::CommentThread,
};
use ferrite_domain::{ContentId, TermId, Timestamp, UserId};
use ferrite_theme::{
    Bridged, HelperBridge, ImageRef, MenuNode, MenuTree, RenderContext, RenderOutput, Renderer,
    SiteInfo, Theme, ThemeError, ThemeSource,
};

pub const MANIFEST: &str = "[theme]\nname = \"t\"\nversion = \"0.1.0\"\n";

/// Writes `files` (path, contents) into a fresh dir; adds a manifest and an index
/// template unless given.
pub fn write_theme(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut all: Vec<(&str, &str)> = files.to_vec();
    if !files.iter().any(|(p, _)| *p == "theme.toml") {
        all.push(("theme.toml", MANIFEST));
    }
    if !files.iter().any(|(p, _)| *p == "templates/index.html") {
        all.push(("templates/index.html", "index"));
    }
    for (path, body) in all {
        write(dir.path(), path, body.as_bytes());
    }
    dir
}

pub fn write(root: &Path, path: &str, body: &[u8]) {
    let p = root.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

pub fn load(files: &[(&str, &str)]) -> Result<Theme, ThemeError> {
    let dir = write_theme(files);
    Theme::load(&ThemeSource::Dir(dir.path().to_path_buf()))
}

pub fn theme(files: &[(&str, &str)]) -> Theme {
    load(files).unwrap()
}

pub fn site() -> SiteInfo {
    SiteInfo {
        title: "My <Site>".into(),
        url: "https://example.com".into(),
        description: "desc".into(),
        locale: "en".into(),
        settings: Default::default(),
    }
}

pub fn ctx() -> RenderContext {
    RenderContext {
        site: site(),
        status: 200,
        ..RenderContext::default()
    }
}

pub fn item(kind: &str, title: &str) -> PageItem {
    let id = ContentId::new();
    PageItem {
        item: PublicItem {
            id,
            kind: kind.into(),
            slug: "hello".into(),
            title: title.into(),
            excerpt: "An excerpt".into(),
            published_at: Some(Timestamp::from_micros(1_709_634_030_000_000).unwrap()),
            updated_at: Timestamp::from_micros(1_709_634_030_000_000).unwrap(),
            menu_order: 0,
            parent_id: None,
            fields: serde_json::json!({"color": "blue"}),
        },
        url: "/hello/".into(),
        html: "<p>Body <em>html</em></p>".into(),
        author: Some(UserPublic {
            id: UserId::new(),
            login: "ana".into(),
            display_name: "Ana".into(),
        }),
        terms: vec![TermPublic {
            id: TermId::new(),
            taxonomy: "category".into(),
            slug: "news".into(),
            name: "News".into(),
            url: "/category/news/".into(),
        }],
        featured: Some(MediaPublic {
            id: ContentId::new(),
            url: "/_media/h/a.jpg".into(),
            alt: "Alt".into(),
            width: Some(1600),
            height: Some(900),
            variants: vec![MediaVariant {
                name: "card".into(),
                url: "/_media/h2/a-card.jpg".into(),
                width: Some(640),
                height: Some(360),
            }],
        }),
    }
}

pub fn listing(items: Vec<PageItem>, page: u32, total: u64) -> Listing {
    Listing {
        items,
        page,
        per_page: 2,
        total,
    }
}

/// A bridge that counts calls and returns canned data.
#[derive(Default)]
pub struct FakeBridge {
    pub calls: AtomicU32,
    pub related: Vec<PageItem>,
    pub menu: Option<MenuTree>,
    pub queries: Mutex<Vec<(String, Option<String>, u32)>>,
    pub related_args: Mutex<Vec<(ContentId, u32)>>,
    pub image: Option<ImageRef>,
    /// Keys reported with every related/query/menu result.
    pub deps: DepKeys,
    /// What `comments()` returns, and the ids it was asked for.
    pub comments: Option<CommentThread>,
    pub comment_args: Mutex<Vec<ContentId>>,
}

impl HelperBridge for FakeBridge {
    fn related(&self, item: ContentId, n: u32) -> Bridged<Vec<PageItem>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.related_args.lock().unwrap().push((item, n));
        (self.related.clone(), self.deps.clone())
    }
    fn query(&self, kind: &str, term: Option<&str>, limit: u32) -> Bridged<Vec<PageItem>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.queries
            .lock()
            .unwrap()
            .push((kind.into(), term.map(Into::into), limit));
        (vec![item(kind, "Queried")], self.deps.clone())
    }
    fn menu(&self, _: &str) -> Bridged<Option<MenuTree>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.menu.clone(), self.deps.clone())
    }
    fn image(&self, _: ContentId, _: &str) -> Option<ImageRef> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.image.clone()
    }
    fn comments(&self, item: ContentId) -> Bridged<Option<CommentThread>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.comment_args.lock().unwrap().push(item);
        (self.comments.clone(), self.deps.clone())
    }
}

pub fn menu_tree() -> MenuTree {
    MenuTree {
        items: vec![MenuNode {
            label: "Home & more".into(),
            url: "/".into(),
            children: vec![MenuNode {
                label: "About".into(),
                url: "/about/".into(),
                children: vec![],
            }],
        }],
    }
}

pub fn render(
    theme: &Theme,
    template: &str,
    ctx: RenderContext,
) -> Result<RenderOutput, ThemeError> {
    Renderer::new().render(theme, template, ctx, Arc::new(ferrite_theme::NoBridge))
}

pub fn render_str(src: &str, ctx: RenderContext) -> Result<RenderOutput, ThemeError> {
    let t = theme(&[("templates/t.html", src)]);
    render(&t, "t.html", ctx)
}

/// The fixture theme in `tests/fixtures/test-minimal`.
pub fn fixture() -> Theme {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-minimal");
    Theme::load(&ThemeSource::Dir(dir)).unwrap()
}

pub fn keys(keys: &[String]) -> DepKeys {
    let mut d = DepKeys::default();
    for k in keys {
        d.insert(k.clone());
    }
    d
}
