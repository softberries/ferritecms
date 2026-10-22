//! Hostile: fails with a 1 MiB plugin-error message full of newlines and ANSI escapes,
//! trying to flood and forge the host log.
use ferrite_sdk::{Ctx, Head, PluginError, Result};

fn head(_head: Head, _ctx: &Ctx) -> Result<Head> {
    // 16 bytes per line, 65_536 lines = 1 MiB.
    let line = "FORGED\n\x1b[31mred\n";
    Err(PluginError::Other(line.repeat((1 << 20) / line.len())))
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
