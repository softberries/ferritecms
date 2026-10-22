//! Test fixture for output sanitising: appends a script, an inline handler, a
//! `javascript:` link and a plain link to every body, and a marker to every excerpt.
//! Installed with `render.html = "sanitized"` the host strips the active parts; tests
//! also install it with a `raw` manifest, where everything passes through.
use ferrite_sdk::{Ctx, Result};

const INJECTED: &str = "<script>alert(1)</script>\
<img src=\"/x.png\" onerror=\"alert(2)\">\
<a href=\"javascript:alert(3)\">js</a>\
<a href=\"https://example.org/\">ok</a>";

fn body(html: String, _ctx: &Ctx) -> Result<String> {
    Ok(format!("{html}{INJECTED}"))
}

fn excerpt(text: String, _ctx: &Ctx) -> Result<String> {
    Ok(format!("{text} [filtered]"))
}

ferrite_sdk::export_plugin! {
    filters: { "content.body" => body, "content.excerpt" => excerpt },
}
