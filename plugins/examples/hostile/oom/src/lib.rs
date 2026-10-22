//! Hostile: allocates 1 MiB blocks until the memory limit stops it.
use ferrite_sdk::{Ctx, Head, Result};

fn head(_head: Head, _ctx: &Ctx) -> Result<Head> {
    let mut hoard: Vec<Vec<u8>> = Vec::new();
    loop {
        hoard.push(core::hint::black_box(vec![1u8; 1024 * 1024]));
    }
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
