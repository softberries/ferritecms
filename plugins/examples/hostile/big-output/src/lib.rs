//! Hostile: returns 100 MiB of HTML for any body.
use ferrite_sdk::{Ctx, Result};

fn body(_html: String, _ctx: &Ctx) -> Result<String> {
    Ok(core::hint::black_box("x".repeat(100 << 20)))
}

ferrite_sdk::export_plugin! {
    filters: { "content.body" => body },
}
