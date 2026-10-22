//! Hostile: never returns. Its test raises fuel so only the epoch deadline stops it.
use ferrite_sdk::{Ctx, Head, Result};

fn head(_head: Head, _ctx: &Ctx) -> Result<Head> {
    loop {
        core::hint::black_box(());
    }
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
