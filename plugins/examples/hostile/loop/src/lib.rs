//! Hostile: never returns. Stopped by fuel exhaustion under default limits.
use ferrite_sdk::{Ctx, Head, Result};

fn head(_head: Head, _ctx: &Ctx) -> Result<Head> {
    loop {
        core::hint::black_box(());
    }
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
