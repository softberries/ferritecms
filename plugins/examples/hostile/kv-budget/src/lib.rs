//! Hostile: calls `kv.set` 150 times; the host allows 100 operations per call.
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    let kv = ctx.kv();
    let ok = (0..150)
        .filter(|i| kv.set(&format!("k{i}"), b"v").is_ok())
        .count();
    head.meta("kv-ok", &ok.to_string());
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
