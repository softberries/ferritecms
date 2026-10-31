//! The demo content itself: texts, block bodies and the sample image.

use std::io::Cursor;

use ferrite_core::ContentDraft;
use ferrite_domain::ContentId;
use image::{ImageBuffer, ImageFormat, Rgb};
use serde_json::{Value, json};

pub(super) const SITE_TITLE: &str = "FerriteCMS Demo";
pub(super) const SITE_DESCRIPTION: &str =
    "A demo site: one Rust binary, sandboxed WebAssembly plugins, themes without code.";
pub(super) const IMAGE_TITLE: &str = "Sample gradient";
pub(super) const IMAGE_ALT: &str = "An orange to violet gradient";
pub(super) const IMAGE_NAME: &str = "sample-gradient.png";
const IMAGE_WIDTH: u32 = 1280;
const IMAGE_HEIGHT: u32 = 720;

/// `(taxonomy, slug, name)`.
pub(super) const TERMS: [(&str, &str, &str); 4] = [
    ("category", "news", "News"),
    ("category", "guides", "Guides"),
    ("tag", "rust", "rust"),
    ("tag", "cms", "cms"),
];

pub(super) struct DemoPost {
    pub title: &'static str,
    pub slug: &'static str,
    pub category: &'static str,
    pub tags: &'static [&'static str],
    pub featured: bool,
    pub lead: &'static str,
}

/// Newest first.
pub(super) const POSTS: [DemoPost; 12] = [
    DemoPost {
        title: "Welcome to FerriteCMS",
        slug: "welcome-to-ferritecms",
        category: "news",
        tags: &["cms"],
        featured: true,
        lead: "This site runs on a single binary with one data folder. Everything you see was created by the demo seeder.",
    },
    DemoPost {
        title: "Writing your first plugin",
        slug: "writing-your-first-plugin",
        category: "guides",
        tags: &["rust", "cms"],
        featured: false,
        lead: "Plugins are WebAssembly components. They get only the capabilities their manifest declares.",
    },
    DemoPost {
        title: "Themes without code",
        slug: "themes-without-code",
        category: "guides",
        tags: &["cms"],
        featured: true,
        lead: "A theme is a folder of templates and assets. Templates get data and a fixed set of safe helpers.",
    },
    DemoPost {
        title: "Why WebAssembly sandboxes",
        slug: "why-webassembly-sandboxes",
        category: "news",
        tags: &["rust"],
        featured: false,
        lead: "Every hook call is bounded by memory, fuel and a deadline, so a failing plugin never breaks a page.",
    },
    DemoPost {
        title: "Importing from WordPress",
        slug: "importing-from-wordpress",
        category: "guides",
        tags: &["cms"],
        featured: false,
        lead: "Posts, pages, terms and media come over from a WXR export; PHP plugins and themes do not.",
    },
    DemoPost {
        title: "Solo or cluster",
        slug: "solo-or-cluster",
        category: "guides",
        tags: &["cms"],
        featured: true,
        lead: "SQLite and a local folder for one node, or Postgres and S3 for many: the same binary runs both.",
    },
    DemoPost {
        title: "Caching pages safely",
        slug: "caching-pages-safely",
        category: "news",
        tags: &["rust"],
        featured: false,
        lead: "Every cached page records what it depends on, and every write says what it changed.",
    },
    DemoPost {
        title: "Permalinks the WordPress way",
        slug: "permalinks-the-wordpress-way",
        category: "guides",
        tags: &["cms"],
        featured: false,
        lead: "Structures such as /%year%/%month%/%slug%/ work as you know them, with redirects when slugs change.",
    },
    DemoPost {
        title: "Media and image variants",
        slug: "media-and-image-variants",
        category: "guides",
        tags: &["cms"],
        featured: true,
        lead: "Uploads are stored by content hash, and every image gets resized variants for themes to use.",
    },
    DemoPost {
        title: "A tour of the block format",
        slug: "a-tour-of-the-block-format",
        category: "news",
        tags: &["cms"],
        featured: false,
        lead: "Post bodies are structured blocks, rendered to safe HTML on the server.",
    },
    DemoPost {
        title: "Release notes 0.1",
        slug: "release-notes-0-1",
        category: "news",
        tags: &["rust", "cms"],
        featured: false,
        lead: "The first milestone: storage, media, the plugin host, themes and the public site.",
    },
    DemoPost {
        title: "Hello world",
        slug: "hello-world",
        category: "news",
        tags: &[],
        featured: true,
        lead: "Every CMS starts with a hello world post. This is ours.",
    },
];

pub(super) fn text(t: &str) -> Value {
    json!({ "type": "text", "text": t })
}

pub(super) fn paragraph(t: &str) -> Value {
    json!({ "type": "paragraph", "content": [text(t)] })
}

/// A post body with a paragraph, a heading, a list, a code block and a quote.
pub(super) fn post_body(p: &DemoPost) -> Value {
    json!({
        "type": "doc",
        "content": [
            paragraph(p.lead),
            { "type": "heading", "attrs": { "level": 2 }, "content": [text("What you get")] },
            { "type": "bulletList", "content": [
                { "type": "listItem", "content": [paragraph("A single binary and a single data folder")] },
                { "type": "listItem", "content": [paragraph("Plugins in a capability-scoped sandbox")] },
                { "type": "listItem", "content": [paragraph("Themes that are templates, not programs")] },
            ]},
            paragraph("Try it on your own machine:"),
            { "type": "codeBlock", "attrs": { "language": "sh" }, "content": [
                text("ferrite init --data ./site --url http://localhost:8080\nferrite seed demo --data ./site\nferrite serve --data ./site")
            ]},
            { "type": "blockquote", "content": [
                paragraph("Plugins are untrusted. They get only what their manifest declares.")
            ]},
            { "type": "paragraph", "content": [
                text("Read more in "),
                { "type": "text", "text": "the about page", "marks": [{ "type": "link", "attrs": { "href": "/about/" } }] },
                text("."),
            ]},
        ]
    })
}

/// A page draft.
pub(super) fn page(
    title: &str,
    slug: &str,
    parent: Option<ContentId>,
    menu_order: i32,
) -> ContentDraft {
    let lead = match slug {
        "about" => "FerriteCMS is a WordPress-style CMS written in Rust, shipped as one binary.",
        "team" => "The team page is a child of the about page, so its address is /about/team/.",
        _ => "Get in touch: this demo site has no contact form, so this page is just text.",
    };
    ContentDraft {
        kind: "page".into(),
        title: title.into(),
        slug: Some(slug.into()),
        parent_id: parent,
        excerpt: None,
        body: json!({ "type": "doc", "content": [
            paragraph(lead),
            { "type": "heading", "attrs": { "level": 2 }, "content": [text(title)] },
            paragraph("Pages are hierarchical: their addresses follow their parents."),
        ]}),
        fields: json!({}),
        menu_order,
        terms: Vec::new(),
        seo: None,
    }
}

/// A 1280×720 PNG: a diagonal orange-to-violet gradient.
pub(super) fn gradient_png() -> anyhow::Result<Vec<u8>> {
    gradient_png_of(
        IMAGE_WIDTH,
        IMAGE_HEIGHT,
        [234.0, 88.0, 12.0],
        [109.0, 40.0, 217.0],
    )
}

/// A `width`×`height` PNG: a diagonal gradient from the RGB colour `from` (top left) to
/// `to` (bottom right).
pub(super) fn gradient_png_of(
    width: u32,
    height: u32,
    from: [f32; 3],
    to: [f32; 3],
) -> anyhow::Result<Vec<u8>> {
    let img = ImageBuffer::from_fn(width, height, |x, y| {
        let t = (x + y) as f32 / (width + height) as f32;
        let mix = |a: f32, b: f32| (a + (b - a) * t).round() as u8;
        Rgb([
            mix(from[0], to[0]),
            mix(from[1], to[1]),
            mix(from[2], to[2]),
        ])
    });
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_is_a_png_of_the_stated_size() {
        let png = gradient_png().unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (IMAGE_WIDTH, IMAGE_HEIGHT));
    }

    /// The blog profile's image is unchanged byte for byte (M4-3b P61): the SHA-256 (the
    /// artifact hash) a site seeded before `gradient_png_of` existed stored for
    /// `sample-gradient.png`, 338 103 bytes.
    #[test]
    fn gradient_bytes_are_pinned() {
        use sha2::{Digest, Sha256};
        let png = gradient_png().unwrap();
        assert_eq!(png.len(), 338_103);
        let hex: String = Sha256::digest(&png)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "e61905d85f1a6e2d40815f7ad69c4b8f1fdb0a2e9133e69c3539504541b247d0"
        );
    }

    #[test]
    fn demo_posts_are_distinct_and_use_known_terms() {
        let mut slugs: Vec<_> = POSTS.iter().map(|p| p.slug).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), POSTS.len());
        let known: Vec<_> = TERMS.iter().map(|(_, s, _)| *s).collect();
        for p in &POSTS {
            assert!(known.contains(&p.category), "{}", p.slug);
            assert!(p.tags.iter().all(|t| known.contains(t)), "{}", p.slug);
        }
        assert!(POSTS[0].featured, "the newest post shows the image");
    }

    /// Content writes refuse a body that is not a document (M2b-2 Task 1).
    #[test]
    fn demo_bodies_are_documents() {
        for p in &POSTS {
            assert!(
                ferrite_core::blocks::is_document(&post_body(p)),
                "{}",
                p.slug
            );
        }
        for slug in ["about", "team", "contact"] {
            let draft = page("T", slug, None, 0);
            assert!(ferrite_core::blocks::is_document(&draft.body), "{slug}");
        }
    }

    #[test]
    fn post_body_has_every_block_kind() {
        let html = ferrite_core::blocks::render_html(
            &post_body(&POSTS[0]),
            &ferrite_core::blocks::RenderCtx { media: &|_| None },
        );
        for tag in [
            "<p>",
            "<h2 id=",
            "<ul>",
            "<pre tabindex=\"0\"><code",
            "<blockquote>",
            "<a href",
        ] {
            assert!(html.contains(tag), "{tag}: {html}");
        }
    }
}
