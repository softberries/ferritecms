//! Hostile: fails with a guest error on every call.
use ferrite_sdk::{Ctx, Head, PluginError, Result};

fn head(_head: Head, _ctx: &Ctx) -> Result<Head> {
    Err(PluginError::Other("nope".into()))
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
