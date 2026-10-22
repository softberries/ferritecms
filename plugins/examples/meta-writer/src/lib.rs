//! Test fixture for ruling R20 (no writes on the render path): tries `meta.set` on a
//! granted key from `render.head` (twice, reporting each result as a meta) and from the
//! `post.saved` action. The host refuses the render-path writes.
use ferrite_sdk::bindings::ferrite::plugin::meta;
use ferrite_sdk::{ActionEvent, Ctx, Head, Result};

const KEY: &str = "seo.seen";

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    let post = ctx.request().post.clone().unwrap_or_default();
    for name in ["meta-set-1", "meta-set-2"] {
        let result = meta::set(&post, KEY, "\"render\"");
        head.meta(name, result.as_ref().map_or_else(|e| e.as_str(), |()| "ok"));
    }
    Ok(head)
}

fn saved(ev: ActionEvent, _ctx: &Ctx) -> Result<()> {
    let post = ev.subject.unwrap_or_default();
    meta::set(&post, KEY, "\"action\"").map_err(ferrite_sdk::PluginError::Other)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
    actions: { "post.saved" => saved },
}
