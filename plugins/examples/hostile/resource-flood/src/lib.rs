//! Hostile: leaks host resources. Every `get-stdout` call makes the host push a new
//! entry into its per-store `ResourceTable`; the handles are forgotten, never dropped,
//! so without a table cap host memory grows with each iteration.
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    // Above the host cap (1_000) but below wasmtime's default store handle limit
    // (100_000), so only the host's own caps can stop it.
    for _ in 0..5_000 {
        core::mem::forget(wasip2::cli::stdout::get_stdout());
    }
    head.meta("resource-flood", "survived");
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
