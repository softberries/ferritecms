//! The `business` profile's content (M4-3b P61): a made-up garden design studio with a
//! static front page, three child pages for the front page's feature cards, About,
//! Contact, three news posts and four generated images.

use ferrite_core::ContentDraft;
use ferrite_domain::ContentId;
use serde_json::{Value, json};

use super::demo::{paragraph, text};

pub(super) const SITE_TITLE: &str = "Larkspur Studio";
pub(super) const SITE_DESCRIPTION: &str =
    "Garden design, planting and aftercare for small city spaces.";

/// The news category of the posts: `(taxonomy, slug, name)`.
pub(super) const NEWS: (&str, &str, &str) = ("category", "news", "News");

/// A generated image: title, slug, alt text, file name, size and gradient colours.
pub(super) struct DemoImage {
    pub title: &'static str,
    pub slug: &'static str,
    pub alt: &'static str,
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub from: [f32; 3],
    pub to: [f32; 3],
}

/// The front page's hero image first, then one card image per child page.
pub(super) const IMAGES: [DemoImage; 4] = [
    DemoImage {
        title: "Courtyard gradient",
        slug: "larkspur-hero",
        alt: "A deep teal to violet gradient",
        name: "larkspur-hero.png",
        width: 1600,
        height: 900,
        from: [15.0, 118.0, 110.0],
        to: [76.0, 29.0, 149.0],
    },
    DemoImage {
        title: "Garden design gradient",
        slug: "larkspur-garden-design",
        alt: "A green to teal gradient",
        name: "larkspur-garden-design.png",
        width: 1280,
        height: 800,
        from: [101.0, 163.0, 13.0],
        to: [13.0, 148.0, 136.0],
    },
    DemoImage {
        title: "Consulting gradient",
        slug: "larkspur-consulting",
        alt: "An amber to rose gradient",
        name: "larkspur-consulting.png",
        width: 1280,
        height: 800,
        from: [245.0, 158.0, 11.0],
        to: [225.0, 29.0, 72.0],
    },
    DemoImage {
        title: "Aftercare gradient",
        slug: "larkspur-aftercare",
        alt: "A sky blue to indigo gradient",
        name: "larkspur-aftercare.png",
        width: 1280,
        height: 800,
        from: [14.0, 165.0, 233.0],
        to: [67.0, 56.0, 202.0],
    },
];

/// A page of the profile: title, slug, excerpt and body paragraphs.
pub(super) struct DemoPage {
    pub title: &'static str,
    pub slug: &'static str,
    pub excerpt: &'static str,
    pub body: &'static [&'static str],
}

/// The static front page: slug `home` (so the children live under `/home/`), with a
/// headline as its title, since the hero shows it (ruling R-b15).
pub(super) const HOME: DemoPage = DemoPage {
    title: "Gardens that grow with you",
    slug: "home",
    excerpt: "We design, plant and look after small gardens, courtyards and roof terraces across the city.",
    body: &[
        "Larkspur Studio is four garden designers. We take on a handful of projects each season, so every garden gets our full attention from the first sketch to the first flowers.",
    ],
};

/// The front page's children, in `menu_order` 1, 2, 3.
pub(super) const CHILDREN: [DemoPage; 3] = [
    DemoPage {
        title: "Garden design",
        slug: "garden-design",
        excerpt: "Full garden design, from the first site survey to planting day.",
        body: &[
            "We measure the space, listen to how you want to use it and draw a plan you can walk through before anything is dug.",
            "Our own crew builds and plants it, usually within six weeks of the plan being agreed.",
        ],
    },
    DemoPage {
        title: "Consulting",
        slug: "consulting",
        excerpt: "A two-hour visit with a designer and a written plan you can follow yourself.",
        body: &[
            "For gardeners who want to do the work themselves: we walk the garden with you, then send a planting list and a sketch within a week.",
        ],
    },
    DemoPage {
        title: "Aftercare",
        slug: "aftercare",
        excerpt: "Seasonal care visits that keep a new garden growing the way it was planned.",
        body: &[
            "Four visits a year for pruning, feeding and replacing anything that did not settle in. Every garden we build gets the first year free.",
        ],
    },
];

pub(super) const ABOUT: DemoPage = DemoPage {
    title: "About",
    slug: "about",
    excerpt: "A small studio for small spaces.",
    body: &[
        "We started Larkspur Studio in 2019 after years of designing large estates, because the gardens we liked best were the ones you could cross in ten steps.",
        "Everything on this site is demo content written by the FerriteCMS seeder.",
    ],
};

pub(super) const CONTACT: DemoPage = DemoPage {
    title: "Contact",
    slug: "contact",
    excerpt: "Tell us about your space.",
    body: &[
        "Write a few lines about your garden and what you would like from it. We answer within two working days.",
        "Studio hours: Monday to Friday, 9:00 to 17:00. 12 Orchard Lane (a made-up address).",
    ],
};

/// The news posts, newest first: title, slug and lead paragraph.
pub(super) const POSTS: [(&str, &str, &str); 3] = [
    (
        "Spring consultation dates are open",
        "spring-consultation-dates",
        "We have twelve consultation slots in March and April. Book early if your garden needs planting before summer.",
    ),
    (
        "Planting for shade in north-facing courtyards",
        "planting-for-shade",
        "Ferns, hellebores and Japanese forest grass do well where the sun never reaches. Here is what we plant and why.",
    ),
    (
        "What a roof terrace can carry",
        "what-a-roof-terrace-can-carry",
        "Wet soil is heavy. Before we plan a roof garden we ask a structural engineer, and so should you.",
    ),
];

/// A page draft: the excerpt, then the body paragraphs.
pub(super) fn page(
    p: &DemoPage,
    parent: Option<ContentId>,
    menu_order: i32,
    featured: Option<ContentId>,
) -> ContentDraft {
    let fields = featured.map_or_else(
        || json!({}),
        |id| json!({ "featured_image": id.to_string() }),
    );
    ContentDraft {
        kind: "page".into(),
        title: p.title.into(),
        slug: Some(p.slug.into()),
        parent_id: parent,
        excerpt: Some(p.excerpt.into()),
        body: json!({ "type": "doc", "content": p.body.iter().map(|t| paragraph(t)).collect::<Vec<_>>() }),
        fields,
        menu_order,
        terms: Vec::new(),
        seo: None,
    }
}

/// A news post body: the lead, a heading and a closing line.
pub(super) fn post_body(lead: &str) -> Value {
    json!({
        "type": "doc",
        "content": [
            paragraph(lead),
            { "type": "heading", "attrs": { "level": 2 }, "content": [text("Get in touch")] },
            { "type": "paragraph", "content": [
                text("Questions about your own garden? "),
                { "type": "text", "text": "Write to us", "marks": [{ "type": "link", "attrs": { "href": "/contact/" } }] },
                text("."),
            ]},
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_are_documents() {
        for p in [&HOME, &ABOUT, &CONTACT].into_iter().chain(CHILDREN.iter()) {
            let draft = page(p, None, 0, None);
            assert!(ferrite_core::blocks::is_document(&draft.body), "{}", p.slug);
            assert!(
                !p.excerpt.is_empty(),
                "{}: cards and the hero show it",
                p.slug
            );
        }
        for (_, slug, lead) in POSTS {
            assert!(
                ferrite_core::blocks::is_document(&post_body(lead)),
                "{slug}"
            );
        }
    }

    #[test]
    fn images_are_pngs_of_the_stated_size() {
        for i in &IMAGES {
            let png = super::super::demo::gradient_png_of(i.width, i.height, i.from, i.to).unwrap();
            let img = image::load_from_memory(&png).unwrap();
            assert_eq!(
                (img.width(), img.height()),
                (i.width, i.height),
                "{}",
                i.slug
            );
        }
    }
}
