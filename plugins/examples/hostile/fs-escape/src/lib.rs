//! Hostile: tries to read the host's /etc/passwd. No preopens exist, so it must fail.
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    let seen = match std::fs::read_to_string("/etc/passwd") {
        Ok(contents) => format!("read: {contents}"),
        Err(e) => format!("error: {e}"),
    };
    head.meta("fs-escape", &seen);
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
