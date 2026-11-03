//! MCP resources (design 12 §5.5): `ferrite://site` (the overview as Markdown) and
//! `ferrite://guide/writing` (the site's writing conventions: Markdown, images and the
//! active theme's image sizes, blocks Markdown cannot express, the title rule, SEO
//! fields). Both are read as the caller (the overview shows only what they may see).
//! An unknown URI is the protocol's resource-not-found error.

use ferrite_core::{
    blocks::BLOCK_FENCE,
    content::{
        Robots,
        seo::{MAX_SEO_CANONICAL, MAX_SEO_DESCRIPTION, MAX_SEO_TITLE},
    },
    settings::site_title_or_configured,
};
use rmcp::{
    ErrorData,
    model::{ReadResourceResult, Resource, ResourceContents, ResourceTemplate},
};
use uuid::Uuid;

use crate::api::problem::Problem;

use super::{Caller, FerriteMcp, results, tools::site::Overview};

/// The overview of the site and the caller.
pub const SITE: &str = "ferrite://site";
/// The writing conventions.
pub const WRITING_GUIDE: &str = "ferrite://guide/writing";
/// The prefix of an item's URI: `ferrite://content/{type}/{id}`.
pub const CONTENT_PREFIX: &str = "ferrite://content/";
/// The template of an item's URI.
pub const CONTENT_TEMPLATE: &str = "ferrite://content/{type}/{id}";
const MARKDOWN: &str = "text/markdown";

/// The resources `resources/list` names.
pub fn list() -> Vec<Resource> {
    vec![
        Resource::new(SITE, "site")
            .with_title("Site overview")
            .with_description(
                "The site, its content types, taxonomies and menus, and your role and \
                 scopes, as Markdown (the site_overview tool as text).",
            )
            .with_mime_type(MARKDOWN),
        Resource::new(WRITING_GUIDE, "writing-guide")
            .with_title("Writing guide")
            .with_description(
                "How to write content for this site: Markdown, images and the theme's \
                 image sizes, embeds and other blocks, titles and SEO fields.",
            )
            .with_mime_type(MARKDOWN),
    ]
}

/// The resource templates `resources/templates/list` names.
pub fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(CONTENT_TEMPLATE, "content-item")
            .with_title("Content item")
            .with_description(
                "One post, page or other content item as Markdown with a TOML front \
                 matter block between +++ lines (id, type, title, slug, status, version, \
                 urls, dates) and the body. {type} is the content type name, {id} the \
                 item's id. Drafts and other items are readable as the tools allow.",
            )
            .with_mime_type(MARKDOWN),
    ]
}

/// `resources/read` of `uri` as `caller`.
pub async fn read(
    server: &FerriteMcp,
    caller: &Caller,
    uri: &str,
) -> Result<ReadResourceResult, ErrorData> {
    let text = match uri {
        SITE => match server.overview(caller).await {
            Ok(o) => site_markdown(&o),
            Err(e) => {
                let r = results::from_core(e);
                let detail = r
                    .content
                    .first()
                    .and_then(|c| c.as_text())
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                return Err(ErrorData::internal_error(detail, None));
            }
        },
        WRITING_GUIDE => writing_guide(server).await,
        _ if uri.starts_with(CONTENT_PREFIX) => content_item(server, caller, uri).await?,
        _ => {
            return Err(ErrorData::resource_not_found(
                format!("no resource {uri:?}"),
                None,
            ));
        }
    };
    Ok(ReadResourceResult::new(vec![
        ResourceContents::text(text, uri).with_mime_type(MARKDOWN),
    ]))
}

/// The overview as Markdown.
pub fn site_markdown(o: &Overview) -> String {
    let mut out = format!(
        "# {}\n\n{} · FerriteCMS {} · theme `{}` · admin: {} · MCP: {}\n\n",
        o.site.title, o.site.url, o.site.version, o.site.theme, o.site.admin_url, o.site.mcp_url
    );
    let c = &o.caller;
    out.push_str(&format!(
        "## You\n\n- Login: {} ({})\n- Role: {}\n- Connected via: {} ({})\n- Scopes: {}\n",
        c.login,
        c.display_name,
        c.role,
        c.connection,
        c.client,
        if c.scopes.is_empty() {
            "none".to_owned()
        } else {
            c.scopes.join(", ")
        }
    ));
    if let Some(types) = &o.content_types {
        out.push_str(
            "\n## Content types\n\n| Type | Label | Published | Draft | Pending | Scheduled | Private | Trash |\n|---|---|---|---|---|---|---|---|\n",
        );
        for t in types {
            let n = t.counts;
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} | {} | {} |\n",
                t.name, t.label, n.published, n.draft, n.pending, n.scheduled, n.private, n.trash
            ));
        }
    }
    if let Some(taxonomies) = &o.taxonomies {
        out.push_str("\n## Taxonomies\n\n");
        for t in taxonomies {
            out.push_str(&format!(
                "- `{}` ({}{}): {}\n",
                t.name,
                t.label,
                if t.hierarchical { ", hierarchical" } else { "" },
                t.content_types.join(", ")
            ));
        }
    }
    if let Some(menus) = &o.menus {
        out.push_str("\n## Menus\n\n");
        if menus.is_empty() {
            out.push_str("None yet.\n");
        }
        for m in menus {
            out.push_str(&format!("- `{}` ({})\n", m.name, m.label));
        }
    }
    if !o.unavailable.is_empty() {
        out.push_str("\n## Not shown\n\n");
        for u in &o.unavailable {
            out.push_str(&format!("- {}: {}\n", u.section, u.reason));
        }
    }
    out
}

/// The writing guide, with the active theme's image sizes.
async fn writing_guide(server: &FerriteMcp) -> String {
    let api = &server.api;
    let title = site_title_or_configured(api.store.as_ref(), &api.site_title).await;
    let theme = api.site.theme.load_full();
    let mut sizes = String::new();
    for (name, size) in &theme.manifest().images {
        let height = size
            .height
            .map_or_else(|| "auto".to_owned(), |h| h.to_string());
        sizes.push_str(&format!(
            "- `{name}`: {} × {height}{}\n",
            size.width,
            if size.crop { ", cropped" } else { "" }
        ));
    }
    if sizes.is_empty() {
        sizes.push_str("- The theme names no sizes: images are shown as uploaded.\n");
    }
    let robots = [
        Robots::NoIndex,
        Robots::NoFollow,
        Robots::NoArchive,
        Robots::NoSnippet,
        Robots::NoImageIndex,
    ]
    .map(Robots::as_str)
    .join("`, `");
    format!(
        "# Writing for {title}\n\n\
         Content is written and read as Markdown (`body_markdown`): CommonMark with GitHub \
         tables and strikethrough. The site stores it as blocks and renders it with the \
         active theme, `{theme_name}`.\n\n\
         ## Markdown that maps to blocks\n\n\
         - Headings `##` to `######` (the page title is the H1; see below)\n\
         - Paragraphs with **bold**, *italic*, `code`, ~~strikethrough~~ and [links](https://example.com)\n\
         - Bulleted and numbered lists, nested\n\
         - Quotes (`>`)\n\
         - Fenced code blocks with a language (```` ```rust ````)\n\
         - Horizontal rules (`---`)\n\
         - Tables (header row, no merged cells)\n\n\
         ## Images\n\n\
         Write `![alt text](ferrite:media/<id> \"caption\")`, where `<id>` is a media item's \
         id (media_list, media_upload); the caption is optional. Always give alt text that \
         describes the image. The theme makes these sizes of every image:\n\n\
         {sizes}\n\
         ## Everything else\n\n\
         Embeds (YouTube, Vimeo), raw HTML and tables with merged cells come back as a \
         fenced `{BLOCK_FENCE}` block holding the block's JSON. Keep those fences exactly \
         as they are when you edit the text around them; to add an embed, write such a \
         fence, e.g.:\n\n\
         ````\n```{BLOCK_FENCE}\n{{\"type\": \"embed\", \"attrs\": {{\"provider\": \"youtube\", \"url\": \"https://www.youtube.com/watch?v=<video id>\"}}}}\n```\n````\n\n\
         ## The title\n\n\
         Put the title in `title`, not in the body. A body that starts with `# Heading` \
         and has no `title` takes it as the title; one equal to the title is dropped; a \
         different one is refused (move it to `title` or make it `##`).\n\n\
         ## SEO\n\n\
         Each item has an excerpt (derived from the body when not given) and SEO fields \
         under `seo`:\n\n\
         - `seo.title`: the whole `<title>` of the page, at most {MAX_SEO_TITLE} characters \
         (default: the item's title and the site title)\n\
         - `seo.description`: the meta description, at most {MAX_SEO_DESCRIPTION} \
         characters (default: the excerpt)\n\
         - `seo.canonical`: an absolute http(s) URL of at most {MAX_SEO_CANONICAL} \
         characters, when the page is a copy of another\n\
         - `seo.robots`: any of `{robots}`\n\n\
         ## Accessibility\n\n\
         - Every image needs alt text that says what it shows; not a file name and not \
         \"image\". An image that only decorates is marked decorative in the admin editor.\n\
         - Headings go in order: `##` after the title, then `###`; never skip a level, \
         never leave one empty.\n\
         - Link text says where the link leads (\"the pricing page\"), never \"click here\" \
         or a bare URL.\n\
         - Tables have a header row.\n\n\
         Call content_check on an item (or on a draft before you write it) and fix what it \
         finds before content_publish: when the type's publish gate is on, publishing is \
         refused while accessibility errors remain.\n\n\
         ## Publishing\n\n\
         Changes are live when a tool returns. Write drafts, show the user a preview \
         (content_preview_link) and publish only when they ask.\n",
        theme_name = theme.name(),
    )
}

/// `ferrite://content/{type}/{id}` as Markdown. Anything that does not name an item the
/// caller may read is the protocol's resource-not-found error.
async fn content_item(
    server: &FerriteMcp,
    caller: &Caller,
    uri: &str,
) -> Result<String, ErrorData> {
    let not_found = || ErrorData::resource_not_found(format!("no resource {uri:?}"), None);
    let rest = uri.strip_prefix(CONTENT_PREFIX).ok_or_else(not_found)?;
    let (kind, id) = rest.split_once('/').ok_or_else(not_found)?;
    let id = Uuid::parse_str(id).map_err(|_| not_found())?;
    server
        .item_markdown(caller, kind, id)
        .await
        .map_err(|p| problem_error(&p, uri))
}

/// A problem as the protocol error of a resource read: a missing or unreadable item is
/// not found, a refusal is an invalid request that names what is missing.
fn problem_error(p: &Problem, uri: &str) -> ErrorData {
    let doc = p.document();
    match doc.status {
        404 => ErrorData::resource_not_found(format!("no resource {uri:?}"), None),
        s if s < 500 => ErrorData::invalid_params(format!("{}: {}", doc.title, doc.detail), None),
        _ => ErrorData::internal_error(format!("{}: {}", doc.title, doc.detail), None),
    }
}
