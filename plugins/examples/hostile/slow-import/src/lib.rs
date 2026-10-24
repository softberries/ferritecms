//! Hostile (with a slow host): spends its call inside a host import, where epochs do
//! not interrupt; only the hard deadline stops it. The `post.saved` action reads its
//! subject the same way (F34: the hot-swap and concurrency tests of actions).
use ferrite_sdk::bindings::ferrite::plugin::content;
use ferrite_sdk::{ActionEvent, Ctx, Head, Result};

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    if ctx.current_post().is_some() {
        head.meta("slow-import", "returned");
    }
    Ok(head)
}

fn saved(ev: ActionEvent, _ctx: &Ctx) -> Result<()> {
    let id = ev.subject.unwrap_or_default();
    match content::get_post(&id) {
        Some(_) => Ok(()),
        None => Err(ferrite_sdk::PluginError::Other(format!("no post {id}"))),
    }
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
    actions: { "post.saved" => saved },
}
