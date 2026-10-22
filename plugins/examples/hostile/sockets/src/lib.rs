//! Hostile: opens a TCP socket (wasi:sockets) from inside a filter.
use ferrite_sdk::{Ctx, Head, Result};

fn head(head: Head, _ctx: &Ctx) -> Result<Head> {
    let _ = std::net::TcpStream::connect("127.0.0.1:1");
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
