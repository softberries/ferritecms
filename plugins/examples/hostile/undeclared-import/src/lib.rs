//! Hostile: calls the http interface without declaring the capability.
use ferrite_sdk::{Ctx, Head, HttpRequest, Result};

fn head(head: Head, ctx: &Ctx) -> Result<Head> {
    let req = HttpRequest {
        method: "GET".into(),
        url: "https://example.com/".into(),
        headers: Vec::new(),
        body: None,
    };
    let _ = ctx.http().fetch(&req);
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
