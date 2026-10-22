//! Hostile: its linear memory *starts* at 40 MiB (see build.rs), above the 32 MiB cap,
//! so instantiation itself must fail. Stands in for a 40 MiB data segment without a
//! 40 MB binary.
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    head.meta("fat-init", "reached");
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
