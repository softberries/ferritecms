//! Test fixture for content reads and dependency recording: reads the current post,
//! the latest posts, one meta value and one option, and reports them as metas
//! (`reader-current`, `reader-list`, `reader-meta`, `reader-option`).
use ferrite_sdk::bindings::ferrite::plugin::{content, meta, options};
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    let current = ctx.current_post();
    head.meta(
        "reader-current",
        current.as_ref().map_or("-", |p| p.title.as_str()),
    );
    let list: Vec<String> = content::list_posts("post", 10, 0)
        .into_iter()
        .map(|p| p.slug)
        .collect();
    head.meta("reader-list", &list.join(","));
    if let Some(p) = &current {
        let m = meta::get(&p.id, "seo.description");
        head.meta("reader-meta", m.as_deref().unwrap_or("-"));
        head.meta("reader-author", &p.author);
    }
    let o = options::get("site.title");
    head.meta("reader-option", o.as_deref().unwrap_or("-"));
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
