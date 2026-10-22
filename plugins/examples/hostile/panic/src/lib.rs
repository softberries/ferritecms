//! Hostile: panics on every call.
use ferrite_sdk::{Ctx, Head, Result};

fn head(_head: Head, _ctx: &Ctx) -> Result<Head> {
    panic!("boom")
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
