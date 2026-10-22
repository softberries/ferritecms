//! Hostile: floods stdout (10 MiB) and stderr (1 MiB), then returns its input unchanged.
use std::io::Write;

use ferrite_sdk::{Ctx, Head, Result};

fn flood(mut out: impl Write, mebibytes: usize) {
    let line = [b'x'; 1023];
    for _ in 0..mebibytes * 1024 {
        // Ignore write errors: the point is to keep writing whatever the host does.
        let _ = out.write_all(&line);
        let _ = out.write_all(b"\n");
    }
}

fn head(head: Head, _ctx: &Ctx) -> Result<Head> {
    flood(std::io::stdout(), 10);
    flood(std::io::stderr(), 1);
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
