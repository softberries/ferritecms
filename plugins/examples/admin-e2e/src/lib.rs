//! The admin E2E fixture (M2b-3 Task 10): adds `<meta name="admin-e2e">` with the
//! `greeting` setting to every page, so its `render.head` hook has calls to count.
use ferrite_sdk::bindings::ferrite::plugin::settings;
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    let greeting = settings::get("greeting").unwrap_or_default();
    head.meta("admin-e2e", greeting.trim_matches('"'));
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
