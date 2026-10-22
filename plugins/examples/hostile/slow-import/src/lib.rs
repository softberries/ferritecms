//! Hostile (with a slow host): spends its call inside a host import, where epochs do
//! not interrupt; only the hard deadline stops it.
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    if ctx.current_post().is_some() {
        head.meta("slow-import", "returned");
    }
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
