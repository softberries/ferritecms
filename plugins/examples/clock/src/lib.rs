//! Test fixture for volatile output: reads the wall clock (`wasi:clocks/wall-clock`)
//! and emits `<meta name="clock" content="<unix seconds>">`.
use std::time::{SystemTime, UNIX_EPOCH};

use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    head.meta("clock", &secs.to_string());
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
