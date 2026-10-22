//! Hostile: returns 3 MiB of HTML for any body: fits in the 32 MiB memory cap, but is
//! above the host's 2 MiB output ceiling.
use ferrite_sdk::{Ctx, Result};

fn body(_html: String, _ctx: &Ctx) -> Result<String> {
    Ok(core::hint::black_box("x".repeat(3 << 20)))
}

ferrite_sdk::export_plugin! {
    filters: { "content.body" => body },
}
